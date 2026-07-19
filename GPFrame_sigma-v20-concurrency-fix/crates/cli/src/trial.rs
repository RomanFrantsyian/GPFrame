//! `dge trial <src-dir>` — the FIELD TRIAL (roadmap item 1).
//!
//! Purpose: generate the data that prices the roadmap. For every function
//! the audit can see, try BOTH front doors and bucket every refusal by the
//! phase that would admit it. The P3 go/no-go question ("is side-effect
//! slicing worth 5-10x the P1+P2 effort?") is answered by the size of the
//! P3 bucket on real code — measured, not guessed.
//!
//! What is and is not claimed:
//!   * When BOTH doors admit a function, the trial runs a CROSS-DOOR GATE:
//!     syn-term vs lifted-term, BitwiseNanClass over μ′ (scalars and
//!     sequences). Two independent untrusted lowerings agreeing bitwise is
//!     a real check, and it needs no FFI to the compiled original.
//!   * When ONE door admits, the term is reported "admitted (ungated in
//!     trial)" — the full extraction gate needs a callable original, which
//!     a trial over arbitrary third-party code cannot conjure. No
//!     equivalence claim is made for these.
//!   * The IR door needs the FILE to compile standalone (`rustc` on one
//!     file). Files with crate-internal imports fail honestly as
//!     `build-failed`; generic functions produce no IR at all
//!     (monomorphization finding, RFC review doc) and land in
//!     `no-ir-symbol`. Both buckets are findings, not noise: they measure
//!     how much real code the IR door can even SEE.

use crate::audit::{audit_dir, Class};
use crate::extract::extract_fn;
use crate::lift::{lift_ll, rustc_emit_ir};
use harness::strategy::{MuPrime, Rng};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoorOutcome {
    Admitted { nodes: usize },
    Refused(String), // bucketed reason
}

#[derive(Debug)]
pub struct FnTrial {
    pub file: std::path::PathBuf,
    pub name: String,
    pub audit_class: Class,
    pub syn: DoorOutcome,
    pub ir: DoorOutcome,
    /// Some(Ok) both doors agree bitwise over μ′; Some(Err) = DISAGREEMENT
    /// (a bug in one lowering — gold); None = fewer than two admissions.
    pub cross_gate: Option<Result<(), String>>,
}

#[derive(Debug, Default)]
pub struct TrialReport {
    pub fns: Vec<FnTrial>,
}

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Collapse a refusal string into a stable histogram bucket. The buckets are
/// the roadmap's own vocabulary — the P2/P3 words in refusal messages exist
/// precisely so this function can count them.
pub fn bucket(reason: &str) -> &'static str {
    let r = reason;
    if r.contains("E0277") { return "generic bounds not f64-instantiable (shim)"; }
    if r.contains("E0425") || r.contains("E0433") || r.contains("cannot find") {
        return "not in the crate's public API (shim)";
    }
    if r.contains("rustc failed (shim)") { return "shim-build-failed"; }
    // BEFORE the generic signature bucket, which would swallow it (the
    // Addendum-3 classifier-bug class): trial №2's dominant sub-population.
    if r.contains("method takes self") {
        return "method receiver (no IR shim; syn door audits &self readers)";
    }
    if r.contains("f32 signature") {
        return "f32 signature (no IR shim; syn door reads f32 via Rnd32)";
    }
    if r.contains("mutable method receiver") {
        return "&mut self (effectful method -- audit's effort/P3 class)";
    }
    if r.contains("non-f64 receiver field") {
        return "reads non-f64 receiver state (Sigma is f64-only)";
    }
    if r.contains("unsupported signature (shim)") { return "unsupported signature (shim)"; }
    if r.contains("rustc failed") { return "build-failed (not standalone-compilable)"; }
    if r.contains("no `define") || r.contains("not found in source") {
        return "no-ir-symbol (generic or not codegen'd)";
    }
    // BEFORE the phase checks: the refusal carries the `P2 fold recognition:`
    // prefix from `refuse()`, and its text mentions Sigma — either substring
    // would swallow it below (the Addendum-3 bug class, pre-empted).
    if r.contains("panic/assert") || r.contains("`unreachable`") {
        return "panic path (partial fn -- totality effort, audit class)";
    }
    if r.contains("P3") { return "P3 (memory / side effects)"; }
    if r.contains("P2") { return "P2+ (loop shape beyond canonical fold)"; }
    if r.contains("libm map") { return "call outside the closed libm map"; }
    // NB: an earlier version also matched "IEEE" here, which swallowed the
    // fcmp-predicate refusals ("Rust/IEEE ordered comparisons") and reported
    // easer's `t == 0.0` guards as fast-math — a measured bucketing bug.
    if r.contains("fast-math") { return "fast-math flags"; }
    if r.contains("Σ v1.1") || r.contains("no Σ symbol") || r.contains("Sigma") {
        return "op outside Σ";
    }
    if r.contains("non-double") || r.contains("pure-f64") { return "non-f64 types"; }
    "other (see raw reasons)"
}

fn try_syn(src: &str, name: &str) -> DoorOutcome {
    match extract_fn(src, name) {
        Ok(t) => DoorOutcome::Admitted { nodes: t.len() },
        Err(e) => DoorOutcome::Refused(format!("{e:?}")),
    }
}

fn try_ir(file: &Path, name: &str) -> (DoorOutcome, Option<term::Term>) {
    let ir = match rustc_emit_ir(file, name) {
        Ok(ir) => ir,
        Err(e) => return (DoorOutcome::Refused(e), None),
    };
    match lift_ll(&ir, name) {
        Ok(t) => { let n = t.len(); (DoorOutcome::Admitted { nodes: n }, Some(t)) }
        Err(e) => (DoorOutcome::Refused(e.to_string()), None),
    }
}

/// Cross-door gate: 2·10³ μ′ samples (trial budget), BitwiseNanClass.
fn cross_gate(a: &term::Term, b: &term::Term) -> Result<(), String> {
    if a.arity() != b.arity() || a.seq_count() != b.seq_count() {
        return Err(format!(
            "shape mismatch: syn (arity {}, {} seqs) vs ir (arity {}, {} seqs)",
            a.arity(), a.seq_count(), b.arity(), b.seq_count()));
    }
    let mu = MuPrime::default_with_seed(0x7121);
    let mut rng = Rng::new(0x7121);
    for _ in 0..2_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, a.arity().max(1), a.seq_count());
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (x, y) = (term::eval_with_seqs(a, &e, &sl), term::eval_with_seqs(b, &e, &sl));
        if !xbit_eq(x, y) {
            return Err(format!("DOORS DISAGREE at {e:?} {sq:?}: syn={x} ir={y}"));
        }
    }
    Ok(())
}

pub fn trial_dir(dir: &Path) -> TrialReport {
    let audit = audit_dir(dir);
    let mut rep = TrialReport::default();
    for f in &audit.fns {
        let src = std::fs::read_to_string(&f.file).unwrap_or_default();
        let syn = try_syn(&src, &f.name);
        let syn_term = if matches!(syn, DoorOutcome::Admitted { .. }) {
            extract_fn(&src, &f.name).ok()
        } else { None };
        let (ir, ir_term) = try_ir(&f.file, &f.name);
        let cross = match (&syn_term, &ir_term) {
            (Some(a), Some(b)) => Some(cross_gate(a, b)),
            _ => None,
        };
        rep.fns.push(FnTrial {
            file: f.file.clone(), name: f.name.clone(), audit_class: f.class,
            syn, ir, cross_gate: cross,
        });
    }
    rep
}

impl TrialReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let n = self.fns.len();
        let syn_ok = self.fns.iter()
            .filter(|f| matches!(f.syn, DoorOutcome::Admitted { .. })).count();
        let ir_ok = self.fns.iter()
            .filter(|f| matches!(f.ir, DoorOutcome::Admitted { .. })).count();
        let ir_only = self.fns.iter().filter(|f|
            matches!(f.ir, DoorOutcome::Admitted { .. })
            && matches!(f.syn, DoorOutcome::Refused(_))).count();
        let both = self.fns.iter().filter(|f| f.cross_gate.is_some()).count();
        let agree = self.fns.iter()
            .filter(|f| matches!(f.cross_gate, Some(Ok(())))).count();

        out.push_str(&format!(
            "FIELD TRIAL: {n} fns audited\n\
             \x20 syn door admits : {syn_ok}\n\
             \x20 IR  door admits : {ir_ok}  ({ir_only} of them REFUSED by syn — the IR door's added coverage)\n\
             \x20 cross-door gate : {agree}/{both} agree bitwise over 2e3 mu' samples\n"));
        for f in self.fns.iter().filter(|f| matches!(f.cross_gate, Some(Err(_)))) {
            if let Some(Err(e)) = &f.cross_gate {
                out.push_str(&format!("  !! {}: {}\n", f.name, e));
            }
        }

        let mut hist: BTreeMap<&'static str, usize> = BTreeMap::new();
        for f in &self.fns {
            if let DoorOutcome::Refused(r) = &f.ir {
                *hist.entry(bucket(r)).or_default() += 1;
            }
        }
        out.push_str("\nIR-door refusal histogram (the roadmap pricing data):\n");
        let mut rows: Vec<_> = hist.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for (b, c) in rows {
            out.push_str(&format!("  {c:4}  {b}\n"));
        }

        out.push_str("\nper-fn detail:\n");
        for f in &self.fns {
            let d = |o: &DoorOutcome| match o {
                DoorOutcome::Admitted { nodes } => format!("OK({nodes}n)"),
                DoorOutcome::Refused(r) => format!("refused[{}]", bucket(r)),
            };
            let g = match &f.cross_gate {
                Some(Ok(())) => " gate=AGREE", Some(Err(_)) => " gate=DISAGREE!", None => "",
            };
            out.push_str(&format!("  {:30} syn={} ir={}{}\n",
                f.name, d(&f.syn), d(&f.ir), g));
        }

        // raw refusal reasons: EVERY `other` (that bucket is unpriced until
        // its raws are read — Addendum 3's classifier-bug lesson), plus one
        // sample per named bucket so a misclassification is visible on sight.
        out.push_str("\nraw IR refusal reasons (all `other`, one sample per named bucket):\n");
        let mut sampled: BTreeMap<&'static str, ()> = BTreeMap::new();
        for f in &self.fns {
            if let DoorOutcome::Refused(r) = &f.ir {
                let b = bucket(r);
                if b.starts_with("other") || sampled.insert(b, ()).is_none() {
                    out.push_str(&format!("  {:30} [{}] {}\n", f.name, b, r));
                }
            }
        }

        // same discipline for the syn door — with receiver flattening the
        // syn column carries real audit data (methods refuse for their
        // REAL reason: &mut self, non-f64 field reads, …), and an unread
        // `other` bucket is unpriced data.
        out.push_str("\nraw syn refusal reasons (all `other`, one sample per named bucket):\n");
        let mut ssampled: BTreeMap<&'static str, ()> = BTreeMap::new();
        for f in &self.fns {
            if let DoorOutcome::Refused(r) = &f.syn {
                let b = bucket(r);
                if b.starts_with("other") || ssampled.insert(b, ()).is_none() {
                    out.push_str(&format!("  {:30} [{}] {}\n", f.name, b, r));
                }
            }
        }
        out
    }
}

pub fn run(args: &[String]) {
    let Some(dir) = args.first() else {
        eprintln!("usage: dge trial <src-dir> [--out report.txt]");
        return;
    };
    let d = Path::new(dir);
    let rep = match trial_crate(d) {
        Ok(r) => r,
        Err(_) => trial_dir(d), // plain directory of standalone files
    };
    let text = rep.render();
    match args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)) {
        Some(p) => { std::fs::write(p, &text).ok(); eprintln!("-> {p}"); }
        None => print!("{text}"),
    }
}

// ============================================================ crate mode ====
// Real crates are not standalone-compilable files (MEASURED: the per-file
// mode scored ZERO IR admissions across the whole first corpus — module
// imports break single-file rustc, and generic fns emit no IR at all). The
// fix is an INSTANTIATION SHIM: for each candidate fn, generate a tiny crate
// that path-depends on the target and re-exports one #[no_mangle] wrapper
// calling it with f64/&[f64] arguments. Monomorphization then materializes
// the generic body INSIDE the shim's codegen unit, small fns inline into the
// wrapper at -O1, and the IR door reads the result. The shim is build
// tooling on the untrusted side: whatever it does wrong, the cross-door gate
// or extraction gate is what would catch it.

/// Supported wrapper parameter kinds. Anything else → honest skip bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PKind { Scalar, Slice, OptNone }

#[derive(Debug)]
struct Candidate {
    name: String,
    /// `Some("Quad")` for inherent/trait impl methods without a receiver.
    self_ty: Option<String>,
    params: Vec<PKind>,
}

/// syn-walk one file for shim-able signatures: free fns and receiver-less
/// impl methods whose params are all f64 / single-generic scalars / slices
/// of the same, returning the same. Unsupported shapes are reported as
/// reasons so they land in the histogram, not on the floor.
fn candidates_in(src: &str) -> (Vec<Candidate>, Vec<(String, String)>) {
    let mut ok = Vec::new();
    let mut skipped = Vec::new();
    let Ok(ast) = syn::parse_file(src) else {
        return (ok, vec![("<file>".into(), "syn parse failed".into())]);
    };
    let mut visit = |name: String, self_ty: Option<String>, sig: &syn::Signature| {
        match classify_sig(sig) {
            Ok(params) => ok.push(Candidate { name, self_ty, params }),
            Err(r) => skipped.push((name, r)),
        }
    };
    for item in &ast.items {
        match item {
            syn::Item::Fn(f) => visit(f.sig.ident.to_string(), None, &f.sig),
            syn::Item::Impl(im) => {
                let ty = match &*im.self_ty {
                    syn::Type::Path(p) => p.path.segments.last()
                        .map(|s| s.ident.to_string()),
                    _ => None,
                };
                for it in &im.items {
                    if let syn::ImplItem::Fn(m) = it {
                        visit(m.sig.ident.to_string(), ty.clone(), &m.sig);
                    }
                }
            }
            _ => {}
        }
    }
    (ok, skipped)
}

fn classify_sig(sig: &syn::Signature) -> Result<Vec<PKind>, String> {
    let scalarish = |t: &syn::Type| -> bool {
        matches!(t, syn::Type::Path(p) if p.path.segments.len() == 1 && {
            let id = p.path.segments[0].ident.to_string();
            id == "f64" || (id.len() <= 2 && id.chars().all(|c| c.is_ascii_uppercase()))
        })
    };
    // v1.6: f32 signatures have REAL syn-door semantics (Rnd32); the IR
    // door has no float-op parser yet — name that precisely instead of
    // "unsupported param type"
    let quote_all = |sig: &syn::Signature| {
        use quote::ToTokens;
        sig.to_token_stream().to_string()
    };
    if quote_all(sig).contains("f32") {
        return Err("f32 signature (no IR shim -- float-op parsing is \
                    roadmap; the syn door reads f32 via Rnd32)".into());
    }
    let mut out = Vec::new();
    for a in &sig.inputs {
        match a {
            syn::FnArg::Receiver(_) => return Err("method takes self".into()),
            syn::FnArg::Typed(t) => match &*t.ty {
                ty if scalarish(ty) => out.push(PKind::Scalar),
                syn::Type::Reference(r) => match &*r.elem {
                    syn::Type::Slice(s) if scalarish(&s.elem) => out.push(PKind::Slice),
                    _ => return Err(format!("unsupported param type: {}",
                        quote_ty(&t.ty))),
                },
                syn::Type::Path(p) if p.path.segments.last()
                    .map(|sg| sg.ident == "Option").unwrap_or(false) =>
                    out.push(PKind::OptNone),
                _ => return Err(format!("unsupported param type: {}", quote_ty(&t.ty))),
            },
        }
    }
    match &sig.output {
        syn::ReturnType::Type(_, t) if scalarish(t) => {}
        _ => return Err("return type is not f64/generic-scalar".into()),
    }
    if !out.iter().any(|p| matches!(p, PKind::Scalar | PKind::Slice)) {
        return Err("no f64/slice inputs the trial can drive".into());
    }
    Ok(out)
}

fn quote_ty(t: &syn::Type) -> String {
    use quote::ToTokens;
    t.to_token_stream().to_string()
}

/// Crate-level facts the shims need, read once.
struct TargetCrate {
    root: std::path::PathBuf,
    pkg: String,      // package name with dashes → underscores for `use`
    pub_mods: Vec<String>,
}

fn read_target(root: &Path) -> Result<TargetCrate, String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| e.to_string())?;
    let pkg = toml.lines()
        .skip_while(|l| l.trim() != "[package]").skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .find_map(|l| l.split_once('=').and_then(|(k, v)|
            (k.trim() == "name").then(|| v.trim().trim_matches('"').replace('-', "_"))))
        .ok_or("no [package] name")?;
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap_or_default();
    let pub_mods = lib.lines()
        .filter_map(|l| l.trim().strip_prefix("pub mod "))
        .map(|m| m.chars().take_while(|c| c.is_alphanumeric() || *c == '_')
              .collect::<String>())
        .filter(|m| !m.is_empty())
        .collect();
    Ok(TargetCrate { root: root.to_path_buf(), pkg, pub_mods })
}

/// One shim = one crate = one wrapper. Returns the IR text of the shim.
fn build_shim(t: &TargetCrate, c: &Candidate, work: &Path, idx: usize)
    -> Result<String, String>
{
    let shim = work.join(format!("shim_{idx}_{}", c.name));
    std::fs::create_dir_all(shim.join("src")).map_err(|e| e.to_string())?;
    std::fs::write(shim.join("Cargo.toml"), format!(
        "[package]\nname = \"shim_{idx}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\
         [lib]\npath = \"src/lib.rs\"\n\
         [dependencies]\n{} = {{ path = {:?} }}\n\
         [profile.dev]\nopt-level = 1\ndebug = 0\n\
         [workspace]\n",
        t.pkg, t.root)).map_err(|e| e.to_string())?;

    let mut sig = Vec::new();
    let mut args = Vec::new();
    for (i, p) in c.params.iter().enumerate() {
        match p {
            PKind::Scalar => { sig.push(format!("p{i}: f64")); args.push(format!("p{i}")); }
            PKind::Slice => { sig.push(format!("p{i}: &[f64]")); args.push(format!("p{i}")); }
            PKind::OptNone => args.push("None".into()), // not a wrapper param
        }
    }
    let globs: String = std::iter::once(format!("    use {}::*;\n", t.pkg))
        .chain(t.pub_mods.iter().map(|m| format!("    #[allow(unused_imports)] use {}::{m}::*;\n", t.pkg)))
        .collect();
    let call = match &c.self_ty {
        Some(ty) => format!("{ty}::{}({})", c.name, args.join(", ")),
        None => format!("{}({})", c.name, args.join(", ")),
    };
    std::fs::write(shim.join("src/lib.rs"), format!(
        "#[no_mangle]\npub fn dge_trial({}) -> f64 {{\n{globs}    {call}\n}}\n",
        sig.join(", "))).map_err(|e| e.to_string())?;

    let target_dir = work.join("shared-target"); // deps compiled once, reused
    let out = std::process::Command::new("cargo")
        .args(["rustc", "--quiet", "--", "--emit=llvm-ir",
               "-C", "llvm-args=--unroll-runtime=false"])
        .current_dir(&shim)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output().map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let gist = err.lines().find(|l| l.contains("error")).unwrap_or("").trim().to_string();
        return Err(format!("rustc failed (shim): {gist}"));
    }
    // newest matching .ll in deps
    let deps = target_dir.join("debug/deps");
    let mut lls: Vec<_> = std::fs::read_dir(&deps).map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with(&format!("shim_{idx}-")) && n.ends_with(".ll")
        })
        .collect();
    lls.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let ll = lls.pop().ok_or("no .ll emitted")?;
    std::fs::read_to_string(ll.path()).map_err(|e| e.to_string())
}

/// Try the wrapper symbol; if it refused on an opaque call, the instantiation
/// was codegen'd but not inlined — lift the monomorphized define directly.
fn lift_from_shim(ir: &str, fn_name: &str) -> Result<term::Term, String> {
    match lift_ll(ir, "dge_trial") {
        Ok(t) => Ok(t),
        Err(e1) => {
            let s1 = e1.to_string();
            if s1.contains("libm map") {
                lift_ll(ir, fn_name).map_err(|e2| format!(
                    "{s1}; direct instantiation also refused: {e2}"))
            } else { Err(s1) }
        }
    }
}

/// Field trial over a CRATE (dir containing Cargo.toml, or its src/).
pub fn trial_crate(root: &Path) -> Result<TrialReport, String> {
    let root = if root.join("Cargo.toml").exists() { root.to_path_buf() }
        else if root.parent().map(|p| p.join("Cargo.toml").exists()) == Some(true) {
            root.parent().unwrap().to_path_buf()
        } else { return Err("no Cargo.toml here or in parent".into()); };
    let t = read_target(&root)?;
    let src = root.join("src");
    let work = crate::lift::unique_tmp_dir(&format!("dge_trial_{}", t.pkg));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let audit = audit_dir(&src);
    let mut rep = TrialReport::default();
    let mut idx = 0usize;
    for f in &audit.fns {
        let filesrc = std::fs::read_to_string(&f.file).unwrap_or_default();
        let syn_out = try_syn(&filesrc, &f.name);
        let syn_term = extract_fn(&filesrc, &f.name).ok();

        let (cands, skipped) = candidates_in(&filesrc);
        let (ir, ir_term) = if let Some(c) = cands.iter().find(|c| c.name == f.name) {
            idx += 1;
            match build_shim(&t, c, &work, idx) {
                Ok(irtext) => match lift_from_shim(&irtext, &f.name) {
                    Ok(tm) => { let n = tm.len();
                        (DoorOutcome::Admitted { nodes: n }, Some(tm)) }
                    Err(e) => (DoorOutcome::Refused(e), None),
                },
                Err(e) => (DoorOutcome::Refused(e), None),
            }
        } else {
            let why = skipped.iter().find(|(n, _)| *n == f.name)
                .map(|(_, r)| format!("unsupported signature (shim): {r}"))
                .unwrap_or_else(|| "unsupported signature (shim): not found by syn walk".into());
            (DoorOutcome::Refused(why), None)
        };
        let cross = match (&syn_term, &ir_term) {
            (Some(a), Some(b)) => Some(cross_gate(a, b)),
            _ => None,
        };
        rep.fns.push(FnTrial {
            file: f.file.clone(), name: f.name.clone(), audit_class: f.class,
            syn: syn_out, ir, cross_gate: cross,
        });
    }
    Ok(rep)
}
