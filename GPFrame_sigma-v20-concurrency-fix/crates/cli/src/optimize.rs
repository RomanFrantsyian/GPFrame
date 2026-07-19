//! `dge optimize <path>` — the WHOLE-CODEBASE front door.
//!
//! One command turns the multi-step engineer loop (discharge → pipeline →
//! copy the certificate comment out by hand, once per function) into a
//! single pass over a file or a directory tree that rewrites every
//! certifiable function IN PLACE and leaves everything else exactly as it
//! was, with the honest reason printed.
//!
//! It is a pure ORCHESTRATION layer: it never proves anything itself. Every
//! rewrite still goes through `pipeline::certify` (front door → refactor
//! under the gate → emit with certificate → emission round-trip). On top of
//! that, before a single byte is written, the *rewritten source in file
//! context* is re-extracted and checked bitwise against the proven term
//! over μ′ — the same `emission_round_trip` gate the pipeline uses. So the
//! in-place edit inherits the pipeline's guarantee verbatim:
//!
//!   NOTHING is written that does not re-extract to its own certificate.
//!
//! Scope of the in-place edit: free functions AND impl methods (immutable
//! `&self`/`self` over a struct defined in the same file) whose parameters are
//! scalar `f64`/`f32`, fixed-size `&[f64; N]` / `[f64; N]` arrays, and/or
//! `&[f64]` sequences — at any arity (the emitter switches to a single
//! `&[f64; N]` param above 8, which the editor maps back onto the original
//! parameters). A function that certifies but whose shape still can't be
//! rewritten in place (`&mut self`, a receiver struct defined elsewhere, an
//! ambiguous duplicate name, a non-numeric parameter) is reported CERTIFIED
//! but left for `dge pipeline`; it is never silently skipped.
//!
//! Files are independent, so `--jobs` fans them out across threads; results
//! are reassembled in candidate order, so output is identical to a
//! sequential run regardless of the worker count.

use crate::audit::{self, Class};
use crate::pipeline::{certify, emission_round_trip, PipelineOpts};
use proc_macro2::LineColumn;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;

// ------------------------------------------------------------------ opts --

pub struct OptimizeOpts {
    /// actually write files (default true — the whole point is in-place).
    pub write: bool,
    /// keep a `<file>.bak` next to each rewritten file.
    pub backup: bool,
    /// also attempt the audit's WITH_EFFORT class, not just EXTRACT.
    pub include_effort: bool,
    pub artifacts: PathBuf,
    pub eps: bool,
    pub domain: Option<f64>,
    /// number of files to certify+rewrite in parallel (default 1). Each file
    /// is independent, so this scales cleanly across a codebase.
    pub jobs: usize,
}

impl Default for OptimizeOpts {
    fn default() -> Self {
        Self {
            write: true,
            backup: true,
            include_effort: false,
            artifacts: "artifacts/o1".into(),
            eps: false,
            domain: None,
            jobs: 1,
        }
    }
}

// --------------------------------------------------------------- outcome --

#[derive(Debug)]
pub enum Status {
    /// certified AND rewritten in place (or would be, under --dry-run).
    Rewritten { cost_before: u64, cost_after: u64, rules: Vec<String> },
    /// certified, but the function shape can't be shimmed in place yet.
    CertifiedNotInPlace(String),
    /// the pipeline honestly refused this function.
    Refused(String),
    /// not attempted / not locatable as a free function.
    Skipped(String),
}

#[derive(Debug)]
pub struct FnOutcome {
    pub file: PathBuf,
    pub name: String,
    pub status: Status,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub outcomes: Vec<FnOutcome>,
    pub files_written: Vec<PathBuf>,
    pub discharged: bool,
    pub dry_run: bool,
}

impl Summary {
    pub fn rewritten(&self) -> usize {
        self.outcomes.iter().filter(|o| matches!(o.status, Status::Rewritten { .. })).count()
    }
    pub fn refused(&self) -> usize {
        self.outcomes.iter().filter(|o| matches!(o.status, Status::Refused(_))).count()
    }
}

// -------------------------------------------------------- source locating --

/// A located function (free fn OR impl method): byte spans + the mapping
/// from emitted slots back to ORIGINAL source expressions.
struct FnLoc {
    start: usize,       // byte offset of the item start (first attr / vis / fn)
    prefix_end: usize,  // byte offset just past the body-opening `{`
    end: usize,         // byte offset just past the body-closing `}`
    /// var slot k (emitted `v{k}` / `v[k]`) → original Rust expression:
    /// `x`, `coeffs[3]`, `self.avg`, … in var-slot order. This mirrors the
    /// extractor's slot assignment exactly (receiver f64 fields first in
    /// declaration order, then params in source order; scalar = 1 slot,
    /// `[f64; N]` = N slots, `&[f64]` = a separate seq slot).
    slot_exprs: Vec<String>,
    /// seq slot j (emitted `s{j}`) → original sequence param name.
    seq_names: Vec<String>,
    /// Some(reason) if the shape can't be rewritten in place.
    unshimmable: Option<String>,
}

fn line_starts(src: &str) -> Vec<usize> {
    let mut v = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

fn lc_to_byte(src: &str, ls: &[usize], lc: LineColumn) -> usize {
    let line_start = ls[lc.line.saturating_sub(1).min(ls.len() - 1)];
    let mut byte = line_start;
    let mut col = 0usize;
    for ch in src[line_start..].chars() {
        if col == lc.column {
            break;
        }
        byte += ch.len_utf8();
        col += 1;
        if ch == '\n' {
            break;
        }
    }
    byte
}

fn is_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg") && {
                use quote::ToTokens;
                a.to_token_stream().to_string().contains("test")
            })
    })
}

/// One typed parameter's shape (the receiver is handled separately).
enum ParamKind {
    Scalar(String),
    Array(String, usize), // name, element count N
    Seq(String),
    Bad(String),
}

/// f64 array length for `[f64; N]` / `&[f64; N]` (immutable), else None.
fn f64_array_len(ty: &syn::Type) -> Option<usize> {
    let arr = match ty {
        syn::Type::Array(a) => a,
        syn::Type::Reference(r) if r.mutability.is_none() => match &*r.elem {
            syn::Type::Array(a) => a,
            _ => return None,
        },
        _ => return None,
    };
    let is_f64 = matches!(&*arr.elem,
        syn::Type::Path(p) if p.path.segments.last().map(|s| s.ident == "f64").unwrap_or(false));
    if !is_f64 {
        return None;
    }
    match &arr.len {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(n), .. }) => n.base10_parse::<usize>().ok(),
        _ => None,
    }
}

fn param_kind(pt: &syn::PatType) -> ParamKind {
    let name = match &*pt.pat {
        syn::Pat::Ident(pi) => pi.ident.to_string(),
        _ => return ParamKind::Bad("non-identifier parameter pattern".into()),
    };
    if let Some(n) = f64_array_len(&pt.ty) {
        return ParamKind::Array(name, n);
    }
    match &*pt.ty {
        syn::Type::Path(p) => {
            let id = p.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            match id.as_str() {
                "f64" | "f32" => ParamKind::Scalar(name),
                other => ParamKind::Bad(format!("non-f64 scalar `{other}`")),
            }
        }
        syn::Type::Reference(r) if r.mutability.is_none() => match &*r.elem {
            syn::Type::Slice(s) => match &*s.elem {
                syn::Type::Path(p)
                    if p.path.segments.last().map(|s| s.ident == "f64").unwrap_or(false) =>
                {
                    ParamKind::Seq(name)
                }
                _ => ParamKind::Bad("&[T] where T != f64".into()),
            },
            _ => ParamKind::Bad("reference param that is not &[f64] / &[f64; N]".into()),
        },
        syn::Type::Reference(_) => ParamKind::Bad("&mut parameter".into()),
        _ => ParamKind::Bad("non-numeric parameter shape".into()),
    }
}

/// Named f64/other fields of `struct ty_name` in DECLARATION order — that
/// order IS the receiver's Var-slot assignment (mirrors the extractor).
fn struct_fields(items: &[syn::Item], ty_name: &str) -> Option<Vec<(String, String)>> {
    use quote::ToTokens;
    for item in items {
        match item {
            syn::Item::Struct(s) if s.ident == ty_name => {
                let syn::Fields::Named(named) = &s.fields else { return None };
                return Some(
                    named
                        .named
                        .iter()
                        .map(|f| {
                            (
                                f.ident.as_ref().unwrap().to_string(),
                                f.ty.to_token_stream().to_string().replace(' ', ""),
                            )
                        })
                        .collect(),
                );
            }
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    if let Some(hit) = struct_fields(inner, ty_name) {
                        return Some(hit);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Build the slot→expression map for a signature, expanding a `&self`/`self`
/// receiver into its struct's f64 fields (declaration order) exactly the way
/// the extractor assigns slots. `root` is the whole file (for struct lookup).
fn build_slotmap(
    sig: &syn::Signature,
    self_ty: Option<&str>,
    root: &[syn::Item],
) -> (Vec<String>, Vec<String>, Option<String>) {
    let mut slot_exprs: Vec<String> = Vec::new();
    let mut seq_names: Vec<String> = Vec::new();
    let mut bad: Option<String> = None;
    let flag = |b: &mut Option<String>, name: &str| {
        if collides_generated(name) {
            b.get_or_insert(format!("param `{name}` collides with a generated identifier"));
        }
    };
    for arg in &sig.inputs {
        match arg {
            syn::FnArg::Receiver(r) => {
                if r.mutability.is_some() {
                    bad.get_or_insert("`&mut self` receiver (effectful)".into());
                    continue;
                }
                let Some(tyn) = self_ty else {
                    bad.get_or_insert("method on an unnameable self type".into());
                    continue;
                };
                let Some(fields) = struct_fields(root, tyn) else {
                    bad.get_or_insert(format!("receiver struct `{tyn}` not defined in this file"));
                    continue;
                };
                // only f64 fields consume slots (non-f64 fields bind but take
                // no slot; a body that reads one would already have refused).
                for (fname, fty) in fields {
                    if fty == "f64" {
                        slot_exprs.push(format!("self.{fname}"));
                    }
                }
            }
            syn::FnArg::Typed(pt) => match param_kind(pt) {
                ParamKind::Scalar(n) => {
                    flag(&mut bad, &n);
                    slot_exprs.push(n);
                }
                ParamKind::Array(n, len) => {
                    flag(&mut bad, &n);
                    for j in 0..len {
                        slot_exprs.push(format!("{n}[{j}]"));
                    }
                }
                ParamKind::Seq(n) => {
                    flag(&mut bad, &n);
                    seq_names.push(n);
                }
                ParamKind::Bad(r) => {
                    bad.get_or_insert(r);
                }
            },
        }
    }
    (slot_exprs, seq_names, bad)
}

fn span_start_byte(src: &str, ls: &[usize], attrs: &[syn::Attribute], vis: &syn::Visibility, sig: &syn::Signature) -> usize {
    let lc = attrs
        .first()
        .map(|a| a.span().start())
        .unwrap_or_else(|| match vis {
            syn::Visibility::Inherited => sig.fn_token.span().start(),
            v => v.span().start(),
        });
    lc_to_byte(src, ls, lc)
}

fn loc_of(
    src: &str,
    ls: &[usize],
    attrs: &[syn::Attribute],
    vis: &syn::Visibility,
    sig: &syn::Signature,
    block: &syn::Block,
    self_ty: Option<&str>,
    root: &[syn::Item],
) -> FnLoc {
    let open = block.brace_token.span.open();
    let close = block.brace_token.span.close();
    let (slot_exprs, seq_names, unshimmable) = build_slotmap(sig, self_ty, root);
    FnLoc {
        start: span_start_byte(src, ls, attrs, vis, sig),
        prefix_end: lc_to_byte(src, ls, open.end()),
        end: lc_to_byte(src, ls, close.end()),
        slot_exprs,
        seq_names,
        unshimmable,
    }
}

/// Collect free functions AND impl methods (top level + inline `mod`s),
/// skipping test code. Names appearing more than once (across free fns and
/// methods) are dropped — `certify`/`extract_fn` resolve a name to the first
/// definition, so we only rewrite names with a single unambiguous target.
fn collect_fns(src: &str, ls: &[usize], items: &[syn::Item], root: &[syn::Item], out: &mut HashMap<String, FnLoc>, dup: &mut Vec<String>) {
    let consider = |name: String, loc: FnLoc, out: &mut HashMap<String, FnLoc>, dup: &mut Vec<String>| {
        if out.remove(&name).is_some() || dup.contains(&name) {
            dup.push(name);
            return;
        }
        out.insert(name, loc);
    };
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                if is_test_attr(&f.attrs) {
                    continue;
                }
                let name = f.sig.ident.to_string();
                let loc = loc_of(src, ls, &f.attrs, &f.vis, &f.sig, &f.block, None, root);
                consider(name, loc, out, dup);
            }
            syn::Item::Impl(i) => {
                let self_ty = match &*i.self_ty {
                    syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
                    _ => None,
                };
                for ii in &i.items {
                    if let syn::ImplItem::Fn(f) = ii {
                        if is_test_attr(&f.attrs) {
                            continue;
                        }
                        let name = f.sig.ident.to_string();
                        let loc = loc_of(src, ls, &f.attrs, &f.vis, &f.sig, &f.block, self_ty.as_deref(), root);
                        consider(name, loc, out, dup);
                    }
                }
            }
            syn::Item::Mod(m) => {
                if is_test_attr(&m.attrs) {
                    continue;
                }
                if let Some((_, inner)) = &m.content {
                    collect_fns(src, ls, inner, root, out, dup);
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------- emitted-code bits --

/// The text between the emitted function's outer braces (let-bindings +
/// final expression), brace-matched so nested folds/selects are safe.
fn body_inner(code: &str) -> Option<String> {
    let open = code.find('{')?;
    let mut depth = 0i32;
    let mut end = None;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    Some(code[open + 1..end].to_string())
}

/// The `///` certificate doc lines from the emitted code, verbatim.
fn cert_doc(code: &str) -> String {
    code.lines()
        .map(|l| l.trim_start())
        .filter(|l| l.starts_with("///"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop a previously-generated preamble from the front of a prefix so
/// re-running `optimize` is idempotent. Only strips lines this tool emits;
/// human doc comments and attributes are preserved.
fn strip_generated_preamble(prefix: &str) -> &str {
    let mut idx = 0usize;
    for line in prefix.split_inclusive('\n') {
        let t = line.trim();
        let generated = t.is_empty()
            || t.starts_with("/// CERTIFIED")
            || t.starts_with("/// rules applied")
            || t.starts_with("/// MODULO")
            || t.starts_with("/// env")
            || t.starts_with("// AUTO-GENERATED")
            || t.starts_with("// Re-run")
            || t == "#[allow(unused_parens, clippy::all)]"
            || t == "#[allow(unused_variables, unused_parens, clippy::all)]";
        if generated {
            idx += line.len();
        } else {
            break;
        }
    }
    // the surviving first line may be a former continuation line still
    // carrying the item's source indent (e.g. a method's `pub fn` after we
    // strip a prior generated preamble); left-trim so reindent doesn't
    // compound it on repeat runs. No-op in the pristine case.
    prefix[idx..].trim_start()
}

// ------------------------------------------------------------- the driver --

fn ensure_discharged(art: &Path) -> Result<bool, String> {
    let ready = art.is_dir()
        && std::fs::read_dir(art).map(|mut r| r.next().is_some()).unwrap_or(false);
    if ready {
        return Ok(false);
    }
    if !rules::smt::Z3Cli::available() {
        return Err(
            "z3 not found — install z3 to prepare the proof table (apt install z3 / brew install z3)".into(),
        );
    }
    let mut z3 = rules::smt::Z3Cli::new(art.to_str().unwrap_or("artifacts/o1"));
    let (_proved, rejected, _unknown) = rules::smt::discharge_all(&rules::r_dec::table(), &mut z3);
    if !rejected.is_empty() {
        return Err(format!("discharge REJECTED rule(s): {rejected:?}"));
    }
    Ok(true)
}

/// Gather candidate (file, fn_name) pairs. Directory → audit-prefiltered to
/// the EXTRACT (and, with --all, WITH_EFFORT) class; single file → every
/// non-test free function in it.
fn candidates(path: &Path, include_effort: bool) -> BTreeMap<PathBuf, Vec<String>> {
    let mut map: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    if path.is_dir() {
        let report = audit::audit_dir(path);
        for f in report.fns {
            let take = f.class == Class::Extractable
                || (include_effort && f.class == Class::WithEffort);
            if take {
                map.entry(f.file).or_default().push(f.name);
            }
        }
    } else if let Ok(src) = std::fs::read_to_string(path) {
        if let Ok(ast) = syn::parse_file(&src) {
            let ls = line_starts(&src);
            let mut fns = HashMap::new();
            let mut dup = Vec::new();
            collect_fns(&src, &ls, &ast.items, &ast.items, &mut fns, &mut dup);
            let mut names: Vec<String> = fns.into_keys().collect();
            names.sort();
            map.insert(path.to_path_buf(), names);
        }
    }
    // de-dup names within a file, keep order
    for v in map.values_mut() {
        let mut seen = std::collections::HashSet::new();
        v.retain(|n| seen.insert(n.clone()));
    }
    map
}

/// Result of processing one file: its per-function outcomes, and whether a
/// write happened (or the write error). Kept separate so files can be
/// processed on independent threads and merged deterministically after.
struct FileWork {
    outcomes: Vec<FnOutcome>,
    written: Result<Option<PathBuf>, String>,
}

pub fn optimize(path: &Path, opts: &OptimizeOpts) -> Result<Summary, String> {
    let discharged = ensure_discharged(&opts.artifacts)?;
    let mut summary = Summary { discharged, dry_run: !opts.write, ..Default::default() };

    let popts = PipelineOpts {
        eps: opts.eps,
        domain: opts.domain,
        artifacts: opts.artifacts.clone(),
        lift: false,
        quiet: true,
    };

    let cands: Vec<(PathBuf, Vec<String>)> =
        candidates(path, opts.include_effort).into_iter().collect();

    // Each file is independent: `certify` here (lift=false, rules already
    // discharged) only reads the input file and the artifacts dir read-only
    // and is otherwise pure in-memory, and each file writes only its own
    // output + `.bak`. So files fan out across threads; results are
    // reassembled in candidate order to keep output identical to the
    // sequential run regardless of `--jobs`.
    let jobs = opts.jobs.max(1).min(cands.len().max(1));
    let mut results: Vec<(usize, FileWork)> = if jobs <= 1 {
        cands
            .iter()
            .enumerate()
            .map(|(i, (file, names))| (i, process_file(file, names, &popts, opts)))
            .collect()
    } else {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cursor = AtomicUsize::new(0);
        let cursor = &cursor;
        let cands = &cands;
        let popts = &popts;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..jobs)
                .map(|_| {
                    scope.spawn(move || {
                        let mut local = Vec::new();
                        loop {
                            let i = cursor.fetch_add(1, Ordering::Relaxed);
                            if i >= cands.len() {
                                break;
                            }
                            let (file, names) = &cands[i];
                            local.push((i, process_file(file, names, popts, opts)));
                        }
                        local
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("optimize worker thread panicked"))
                .collect()
        })
    };

    results.sort_by_key(|(i, _)| *i);
    let mut write_err: Option<String> = None;
    for (_, work) in results {
        summary.outcomes.extend(work.outcomes);
        match work.written {
            Ok(Some(p)) => summary.files_written.push(p),
            Ok(None) => {}
            Err(e) => {
                if write_err.is_none() {
                    write_err = Some(e);
                }
            }
        }
    }
    if let Some(e) = write_err {
        return Err(e);
    }

    Ok(summary)
}

/// Certify + in-place-rewrite every candidate function in one file. Pure with
/// respect to other files (reads this file and the read-only artifacts dir,
/// writes only this file and its `.bak`), so it is safe to run concurrently
/// across distinct files.
fn process_file(file: &Path, names: &[String], popts: &PipelineOpts, opts: &OptimizeOpts) -> FileWork {
    let mut outcomes: Vec<FnOutcome> = Vec::new();
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            for name in names {
                outcomes.push(FnOutcome {
                    file: file.to_path_buf(),
                    name: name.clone(),
                    status: Status::Skipped(format!("read: {e}")),
                });
            }
            return FileWork { outcomes, written: Ok(None) };
        }
    };
    let ls = line_starts(&src);
    let ast = match syn::parse_file(&src) {
        Ok(a) => a,
        Err(e) => {
            for name in names {
                outcomes.push(FnOutcome {
                    file: file.to_path_buf(),
                    name: name.clone(),
                    status: Status::Skipped(format!("parse: {e}")),
                });
            }
            return FileWork { outcomes, written: Ok(None) };
        }
    };
    let mut locs = HashMap::new();
    let mut dup = Vec::new();
    collect_fns(&src, &ls, &ast.items, &ast.items, &mut locs, &mut dup);

    // (start, end, replacement) for verified rewrites in this file
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    for name in names {
        let name = name.clone();
        let file_s = file.to_string_lossy().to_string();
        let c = match certify(&file_s, &name, popts) {
            Ok(c) => c,
            Err(e) => {
                outcomes.push(FnOutcome {
                    file: file.to_path_buf(),
                    name,
                    status: Status::Refused(first_line(&e)),
                });
                continue;
            }
        };

        let Some(loc) = locs.get(&name) else {
            outcomes.push(FnOutcome {
                file: file.to_path_buf(),
                name,
                status: Status::CertifiedNotInPlace(
                    "not a free function (impl method / ambiguous) — use `dge pipeline`".into(),
                ),
            });
            continue;
        };

        if let Some(reason) = &loc.unshimmable {
            outcomes.push(FnOutcome {
                file: file.to_path_buf(),
                name,
                status: Status::CertifiedNotInPlace(format!("{reason} — use `dge pipeline`")),
            });
            continue;
        }
        // the emitted slots must line up with the located param map
        if loc.slot_exprs.len() != c.term.arity() || loc.seq_names.len() != c.term.seq_count() {
            outcomes.push(FnOutcome {
                file: file.to_path_buf(),
                name,
                status: Status::CertifiedNotInPlace(
                    "param/arity shape mismatch — use `dge pipeline`".into(),
                ),
            });
            continue;
        }

        let (Some(inner), prefix) = (body_inner(&c.code), &src[loc.start..loc.prefix_end]) else {
            outcomes.push(FnOutcome {
                file: file.to_path_buf(),
                name,
                status: Status::CertifiedNotInPlace("could not parse emitted body".into()),
            });
            continue;
        };

        // arity > 8 ⇒ emit uses a single array param `v[k]`; else `v{k}`.
        let array_shape = c.term.arity() > 8;
        let indent = line_indent(&src, loc.start);
        let repl = build_replacement(
            &c.code,
            strip_generated_preamble(prefix),
            &loc.slot_exprs,
            &loc.seq_names,
            &inner,
            array_shape,
            indent,
        );

        // SAFETY GATE: the rewritten source, re-extracted in file context,
        // must re-prove equal to the certified term over μ′. Nothing that
        // fails this is ever recorded, let alone written.
        let candidate = format!("{}{}{}", &src[..loc.start], repl, &src[loc.end..]);
        if let Err(e) = emission_round_trip(&c.term, &candidate, &name) {
            outcomes.push(FnOutcome {
                file: file.to_path_buf(),
                name,
                status: Status::CertifiedNotInPlace(format!(
                    "in-place form failed re-extraction gate ({}) — use `dge pipeline`",
                    first_line(&e)
                )),
            });
            continue;
        }

        edits.push((loc.start, loc.end, repl));
        outcomes.push(FnOutcome {
            file: file.to_path_buf(),
            name,
            status: Status::Rewritten {
                cost_before: c.cost_before,
                cost_after: c.cost_after,
                rules: cert_rules(&c.code),
            },
        });
    }

    if !edits.is_empty() && opts.write {
        // apply bottom-to-top so earlier offsets stay valid
        edits.sort_by(|a, b| b.0.cmp(&a.0));
        let mut new_src = src.clone();
        for (start, end, repl) in &edits {
            new_src.replace_range(*start..*end, repl);
        }
        if opts.backup {
            let bak = file.with_extension(match file.extension().and_then(|e| e.to_str()) {
                Some(e) => format!("{e}.bak"),
                None => "bak".into(),
            });
            let _ = std::fs::write(&bak, &src);
        }
        return match std::fs::write(file, &new_src) {
            Ok(()) => FileWork { outcomes, written: Ok(Some(file.to_path_buf())) },
            Err(e) => FileWork { outcomes, written: Err(format!("write {}: {e}", file.display())) },
        };
    }

    FileWork { outcomes, written: Ok(None) }
}

/// The leading whitespace of the line containing byte `start`, if that run
/// is entirely blank (an item's own indentation); else "". Empty for a
/// top-level item, so top-level output is byte-for-byte unchanged.
fn line_indent(src: &str, start: usize) -> &str {
    let begin = src[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let seg = &src[begin..start];
    if !seg.is_empty() && seg.bytes().all(|b| b == b' ' || b == b'\t') {
        seg
    } else {
        ""
    }
}

/// Prefix every line after the first with `indent` (non-empty lines only).
/// The first line already follows the source indent that precedes the item.
fn reindent(s: &str, indent: &str) -> String {
    if indent.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn build_replacement(
    code: &str,
    prefix: &str,
    slot_exprs: &[String],
    seq_names: &[String],
    inner: &str,
    array_shape: bool,
    indent: &str,
) -> String {
    let mut r = String::new();
    let doc = cert_doc(code);
    if !doc.is_empty() {
        r.push_str(&doc);
        r.push('\n');
    }
    r.push_str("#[allow(unused_parens, clippy::all)]\n");
    // the copied prefix already carries the item's source indentation on any
    // continuation lines (multi-line signature, or a doc comment above `fn`);
    // strip one level so the uniform reindent below doesn't double it.
    r.push_str(&dedent_continuation(prefix, indent));
    r.push_str(remap_body(inner, slot_exprs, seq_names, array_shape).trim_end_matches([' ', '\n', '\r']));
    r.push_str("\n}");
    reindent(&r, indent)
}

/// Remove one leading `indent` from every line after the first.
fn dedent_continuation(s: &str, indent: &str) -> String {
    if indent.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(line.strip_prefix(indent).unwrap_or(line));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Rewrite the emitted body so it references the ORIGINAL source expressions
/// instead of the generated slots: `v{k}` (or `v[k]` in array shape) → the
/// slot's expression (`x`, `coeffs[3]`, `self.avg`, …), and `s{j}` → the
/// sequence param name. CSE temporaries (`tN`), `__acc`, `__i`, literals and
/// operators are untouched. The re-extraction gate still verifies the result.
fn remap_body(inner: &str, slot_exprs: &[String], seq_names: &[String], array_shape: bool) -> String {
    // seqs are `s{j}` identifiers in either shape
    let mut ident_map: HashMap<String, String> = HashMap::new();
    for (j, n) in seq_names.iter().enumerate() {
        ident_map.insert(format!("s{j}"), n.clone());
    }
    if !array_shape {
        for (k, e) in slot_exprs.iter().enumerate() {
            ident_map.insert(format!("v{k}"), e.clone());
        }
    }

    let bytes = inner.as_bytes();
    let mut out = String::with_capacity(inner.len());
    let mut i = 0usize;
    while i < inner.len() {
        let c = bytes[i] as char;
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            i += 1;
            while i < inner.len() && {
                let d = bytes[i] as char;
                d.is_ascii_alphanumeric() || d == '_'
            } {
                i += 1;
            }
            let ident = &inner[start..i];
            // array shape: `v` followed by `[ <k> ]` → slot_exprs[k]
            if array_shape && ident == "v" {
                let mut j = i;
                while j < inner.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < inner.len() && bytes[j] == b'[' {
                    j += 1;
                    let ds = j;
                    while j < inner.len() && (bytes[j] as char).is_ascii_digit() {
                        j += 1;
                    }
                    let de = j;
                    while j < inner.len() && (bytes[j] as char).is_whitespace() {
                        j += 1;
                    }
                    if de > ds && j < inner.len() && bytes[j] == b']' {
                        if let Ok(k) = inner[ds..de].parse::<usize>() {
                            if let Some(e) = slot_exprs.get(k) {
                                out.push_str(e);
                                i = j + 1; // past ']'
                                continue;
                            }
                        }
                    }
                }
                out.push_str(ident);
                continue;
            }
            match ident_map.get(ident) {
                Some(rep) => out.push_str(rep),
                None => out.push_str(ident),
            }
        } else {
            // non-identifier byte; copy one char (inner is ASCII Rust code)
            let ch_len = inner[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            out.push_str(&inner[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// A param name collides with a generated identifier if it looks like a slot
/// (`vN`/`sN`/`tN`) or a fold internal (`__acc`/`__i`) — in-place remap would
/// be ambiguous, so we decline (the pipeline still handles it).
fn collides_generated(name: &str) -> bool {
    name == "__acc"
        || name == "__i"
        || (name.len() >= 2
            && matches!(name.as_bytes()[0], b'v' | b's' | b't')
            && name[1..].bytes().all(|b| b.is_ascii_digit()))
}

fn cert_rules(code: &str) -> Vec<String> {
    for l in code.lines() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("/// rules applied:") {
            return rest.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }
    Vec::new()
}

/// Flatten a multi-line refusal/error into one readable summary line.
fn first_line(s: &str) -> String {
    let joined = s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    if joined.chars().count() > 240 {
        let mut t: String = joined.chars().take(237).collect();
        t.push_str("...");
        t
    } else {
        joined
    }
}

// ------------------------------------------------------------------- run --

pub fn run(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!(
            "usage: dge optimize <path> [--dry-run] [--no-backup] [--all]\n\
             \x20              [--jobs <n|auto>] [--artifacts <dir>] [--eps [--domain <mag>]]\n\
             \n\
             Rewrites every certifiable f64 function under <path> (a file or a\n\
             directory) IN PLACE, each with its certificate; refuses the rest\n\
             with the exact reason. --dry-run previews without writing.\n\
             --jobs runs files in parallel (auto = all cores)."
        );
        return;
    };
    let jobs = match args.iter().position(|a| a == "--jobs" || a == "-j").and_then(|i| args.get(i + 1)) {
        Some(v) if v == "auto" => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        Some(v) => v.parse::<usize>().ok().filter(|&n| n >= 1).unwrap_or(1),
        None => 1,
    };
    let opts = OptimizeOpts {
        write: !args.iter().any(|a| a == "--dry-run"),
        backup: !args.iter().any(|a| a == "--no-backup"),
        include_effort: args.iter().any(|a| a == "--all"),
        artifacts: args
            .iter()
            .position(|a| a == "--artifacts")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or("artifacts/o1")
            .into(),
        eps: args.iter().any(|a| a == "--eps"),
        domain: args
            .iter()
            .position(|a| a == "--domain")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<f64>().ok()),
        jobs,
    };

    match optimize(Path::new(path), &opts) {
        Ok(s) => print_summary(&s),
        Err(e) => eprintln!("optimize aborted: {e}"),
    }
}

fn print_summary(s: &Summary) {
    if s.discharged {
        println!("[discharge] proof table prepared (Z3)");
    }
    let mut certified_ip = 0;
    let mut cert_not_ip = 0;
    let mut refused = 0;
    let mut skipped = 0;

    for o in &s.outcomes {
        let loc = format!("{}::{}", o.file.display(), o.name);
        match &o.status {
            Status::Rewritten { cost_before, cost_after, rules } => {
                certified_ip += 1;
                let verb = if s.dry_run { "would rewrite" } else { "rewritten" };
                let rules = if rules.is_empty() { String::new() } else { format!("  [{}]", rules.join(", ")) };
                println!("  \u{2713} {loc}   cost {cost_before}\u{2192}{cost_after}{rules}   CERTIFIED, {verb} in place");
            }
            Status::CertifiedNotInPlace(why) => {
                cert_not_ip += 1;
                println!("  \u{2295} {loc}   certified but not in-place: {why}");
            }
            Status::Refused(why) => {
                refused += 1;
                println!("  \u{25ef} {loc}   refused: {why}");
            }
            Status::Skipped(why) => {
                skipped += 1;
                println!("  \u{00b7} {loc}   skipped: {why}");
            }
        }
    }

    println!("--");
    println!(
        "{certified_ip} rewritten in place, {cert_not_ip} certified-not-in-place, {refused} refused, {skipped} skipped",
    );
    if !s.dry_run && !s.files_written.is_empty() {
        println!("files changed: {}{}",
            s.files_written.len(),
            if s.files_written.iter().any(|_| true) { "  (originals saved as *.bak unless --no-backup)" } else { "" });
    } else if s.dry_run {
        println!("(dry run — no files written; drop --dry-run to apply)");
    }
    println!("every rewrite re-extracts to its own certificate — nothing uncertified was written.");
}
