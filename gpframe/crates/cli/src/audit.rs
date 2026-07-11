//! Pre-R0 gate (§9): compute s = fraction of the codebase extractable to
//! Term_p. Build iff s ≥ ~0.2–0.3; below that, the tool has no workload.
//!
//! O5 is a SYNTACTIC check, so the audit is a syntactic classifier:
//!
//!   EXTRACTABLE      — expression-level pure numeric code over Σ:
//!                      fn(numeric args) -> numeric, arithmetic + Σ math
//!                      methods, if/else (→ Select), let-bindings, calls to
//!                      other EXTRACTABLE fns in the audit set (inline-able).
//!   WITH_EFFORT      — pure numeric but needs manual/semi-auto extraction:
//!                      loops, iterators, match, local mut accumulators,
//!                      recursion, casts, integer sorts (exact sort pending
//!                      in Σ), closures, %. This bucket is exactly the
//!                      post-R7 "semi-automatic extraction" workload (§8).
//!   NOT_EXTRACTABLE  — outside the perimeter by construction: IO, unsafe,
//!                      &mut params, non-numeric types, unknown calls,
//!                      macros with effects, async.
//!
//! s_strict counts EXTRACTABLE only; s_loose adds WITH_EFFORT. The §9
//! verdict uses s_strict (conservative: the engine as built at R7 can only
//! consume the strict slice). Weights = LOC of each fn.
//!
//! Epistemic note: this measures the SYNTACTIC criterion. A fn can be
//! syntactically clean yet semantically unsuitable (e.g. depends on global
//! rounding-mode changes elsewhere) — rare, caught later by O8. The audit
//! never overrides per-term gating; it only sizes the workload.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Extractable,
    WithEffort,
    NotExtractable,
}

#[derive(Debug)]
pub struct FnReport {
    pub file: PathBuf,
    pub name: String,
    pub loc: usize,
    pub class: Class,
    pub reasons: Vec<String>,
}

#[derive(Debug, Default)]
pub struct AuditReport {
    pub fns: Vec<FnReport>,
    pub skipped_files: Vec<(PathBuf, String)>,
}

impl AuditReport {
    fn weight(&self, pred: impl Fn(Class) -> bool) -> usize {
        self.fns.iter().filter(|f| pred(f.class)).map(|f| f.loc).sum()
    }

    pub fn total_loc(&self) -> usize {
        self.weight(|_| true)
    }

    /// s over the strict slice — the §9 decision number.
    pub fn s_strict(&self) -> f64 {
        let t = self.total_loc();
        if t == 0 { return 0.0; }
        self.weight(|c| c == Class::Extractable) as f64 / t as f64
    }

    /// s including the with-effort slice — sizes the post-R7 extraction win.
    pub fn s_loose(&self) -> f64 {
        let t = self.total_loc();
        if t == 0 { return 0.0; }
        self.weight(|c| c != Class::NotExtractable) as f64 / t as f64
    }

    /// §9 verdict. INFERRED threshold band 0.2–0.3: below 0.2 no-build,
    /// above 0.3 build, in between judgment call (report says so).
    pub fn verdict(&self) -> &'static str {
        let s = self.s_strict();
        if s >= 0.3 {
            "BUILD: s >= 0.3 — kick off R0 under the §5 map"
        } else if s >= 0.2 {
            "BORDERLINE: 0.2 <= s < 0.3 — judgment call; weigh domain trajectory"
        } else {
            "DO NOT BUILD: s < 0.2 — no workload; revisit if domain shifts toward numerics"
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "== pre-R0 audit (§9 gate) ==");
        for f in &self.fns {
            let tag = match f.class {
                Class::Extractable => "EXTRACT ",
                Class::WithEffort => "EFFORT  ",
                Class::NotExtractable => "OUTSIDE ",
            };
            let _ = writeln!(
                out, "{tag} {:>5} loc  {}::{}{}",
                f.loc,
                f.file.display(),
                f.name,
                if f.reasons.is_empty() { String::new() }
                else { format!("  [{}]", f.reasons.join("; ")) }
            );
        }
        for (p, e) in &self.skipped_files {
            let _ = writeln!(out, "SKIPPED  {}: {}", p.display(), e);
        }
        let _ = writeln!(out, "--");
        let _ = writeln!(out, "total audited: {} loc across {} fns", self.total_loc(), self.fns.len());
        let _ = writeln!(out, "s_strict = {:.3}   (Term_p today)", self.s_strict());
        let _ = writeln!(out, "s_loose  = {:.3}   (+ with-effort; post-R7 extraction workload)", self.s_loose());
        let _ = writeln!(out, "verdict: {}", self.verdict());
        out
    }
}

// ---------------------------------------------------------------- types --

fn numeric_ident(id: &str) -> Option<Numeric> {
    match id {
        "f64" | "f32" => Some(Numeric::Float),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
        | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Some(Numeric::Int),
        "bool" => Some(Numeric::Bool),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Numeric { Float, Int, Bool }

/// Result flags of a type check.
#[derive(Default, Clone, Copy)]
struct TypeFlags { int: bool, generic: bool }

/// Is `ty` a Term_p-embeddable value type (given the fn's generic params)?
fn check_type(ty: &syn::Type, generics: &std::collections::HashSet<String>) -> Result<TypeFlags, String> {
    match ty {
        syn::Type::Path(p) => {
            let id = p.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            if generics.contains(&id) {
                // generic numeric-assumed param: monomorphize-then-extract
                return Ok(TypeFlags { int: false, generic: true });
            }
            match numeric_ident(&id) {
                Some(Numeric::Int) => Ok(TypeFlags { int: true, generic: false }),
                Some(_) => Ok(TypeFlags::default()),
                None => Err(format!("non-numeric type `{id}`")),
            }
        }
        syn::Type::Reference(r) => {
            if r.mutability.is_some() {
                return Err("&mut parameter (mutation escaping scope)".into());
            }
            check_type(&r.elem, generics)
        }
        syn::Type::Slice(s) => check_type(&s.elem, generics),
        syn::Type::Array(a) => check_type(&a.elem, generics),
        syn::Type::Tuple(t) => {
            let mut fl = TypeFlags::default();
            for e in &t.elems {
                let f = check_type(e, generics)?;
                fl.int |= f.int;
                fl.generic |= f.generic;
            }
            Ok(fl)
        }
        syn::Type::Paren(p) => check_type(&p.elem, generics),
        other => Err(format!("unsupported type shape `{}`", type_name(other))),
    }
}

fn type_name(t: &syn::Type) -> &'static str {
    match t {
        syn::Type::ImplTrait(_) => "impl Trait",
        syn::Type::TraitObject(_) => "dyn Trait",
        syn::Type::Ptr(_) => "raw pointer",
        syn::Type::BareFn(_) => "fn pointer",
        syn::Type::Never(_) => "!",
        _ => "complex",
    }
}

// -------------------------------------------------------------- visitor --

/// Σ-expressible f64 methods (strict slice).
const MATH_METHODS: &[&str] = &[
    "sin", "cos", "tan", "exp", "ln", "sqrt", "abs", "powf", "powi",
    "mul_add", "floor", "ceil", "min", "max", "recip", "clamp", "copysign",
    "neg",
];

#[derive(Default)]
struct FnScan {
    self_name: String,
    generics: std::collections::HashSet<String>,
    hard: Vec<String>,
    effort: Vec<String>,
    /// unresolved calls to same-audit-set fns, resolved by fixed point
    calls: Vec<String>,
}

impl FnScan {
    fn hard(&mut self, r: impl Into<String>) {
        let r = r.into();
        if !self.hard.contains(&r) { self.hard.push(r); }
    }
    fn effort(&mut self, r: impl Into<String>) {
        let r = r.into();
        if !self.effort.contains(&r) { self.effort.push(r); }
    }
}

impl<'ast> Visit<'ast> for FnScan {
    /// Catches macros in ALL positions (statement, expression, item).
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        let name = m.path.segments.last()
            .map(|s| s.ident.to_string()).unwrap_or_default();
        match name.as_str() {
            // pure panics = ⊥; Term_p is total ⇒ effort (guard removal)
            "assert" | "assert_eq" | "assert_ne" | "debug_assert"
            | "debug_assert_eq" | "panic" | "unreachable" =>
                self.effort(format!("panic path `{name}!` (Term_p is total)")),
            _ => self.hard(format!("macro `{name}!`")),
        }
        syn::visit::visit_macro(self, m);
    }

    fn visit_expr(&mut self, e: &'ast syn::Expr) {
        use syn::Expr::*;
        match e {
            Unsafe(_) => self.hard("unsafe block"),
            Async(_) | Await(_) => self.hard("async"),
            Call(c) => {
                if let syn::Expr::Path(p) = &*c.func {
                    let segs: Vec<String> =
                        p.path.segments.iter().map(|s| s.ident.to_string()).collect();
                    let name = segs.last().cloned().unwrap_or_default();
                    let qualifier = if segs.len() >= 2 { Some(segs[0].as_str()) } else { None };
                    if name == self.self_name {
                        self.effort("recursion (Σ has no fixpoint)");
                    } else if let Some(q) = qualifier {
                        // F::from / f64::from / Self::helper — numeric assoc
                        // calls are const/sort lifts: monomorphize-then-extract
                        if self.generics.contains(q) || numeric_ident(q).is_some() || q == "Self" {
                            self.effort(format!("assoc call `{q}::{name}` (const/sort lift)"));
                        } else {
                            self.hard(format!("call `{}` outside Σ", segs.join("::")));
                        }
                    } else {
                        self.calls.push(name);
                    }
                } else {
                    self.hard("indirect call");
                }
            }
            MethodCall(m) => {
                let name = m.method.to_string();
                if !MATH_METHODS.contains(&name.as_str()) {
                    match name.as_str() {
                        "iter" | "map" | "fold" | "sum" | "product" | "zip"
                        | "enumerate" | "len" | "get" | "windows" | "chunks" =>
                            self.effort(format!("iterator method `.{name}()`")),
                        // panic paths = ⊥; Term_p is total ⇒ guard removal
                        "unwrap" | "expect" =>
                            self.effort(format!("panic path `.{name}()` (Term_p is total)")),
                        _ => self.hard(format!("method `.{name}()` outside Σ")),
                    }
                }
            }
            Loop(_) | While(_) | ForLoop(_) => self.effort("loop (needs unroll/recurrence extraction)"),
            Match(_) => self.effort("match (needs Select lowering)"),
            Closure(_) => self.effort("closure"),
            Cast(_) => self.effort("cast (sort change)"),
            Assign(_) => self.effort("local mutation (accumulator pattern)"),
            Binary(b) => {
                use syn::BinOp::*;
                match b.op {
                    AddAssign(_) | SubAssign(_) | MulAssign(_) | DivAssign(_)
                    | RemAssign(_) | BitAndAssign(_) | BitOrAssign(_)
                    | BitXorAssign(_) | ShlAssign(_) | ShrAssign(_) =>
                        self.effort("local mutation (compound assign)"),
                    Rem(_) => self.effort("% (needs floor-div lowering)"),
                    BitAnd(_) | BitOr(_) | BitXor(_) | Shl(_) | Shr(_) =>
                        self.effort("bit op (exact sort pending)"),
                    _ => {}
                }
            }
            Field(f) => {
                if matches!(&*f.base, syn::Expr::Path(p) if p.path.is_ident("self")) {
                    self.effort("self field access (flatten struct)");
                }
            }
            Reference(r) if r.mutability.is_some() => self.hard("&mut borrow"),
            _ => {}
        }
        syn::visit::visit_expr(self, e);
    }
}

// ----------------------------------------------------------------- scan --

struct RawFn {
    file: PathBuf,
    name: String,
    loc: usize,
    hard: Vec<String>,
    effort: Vec<String>,
    calls: Vec<String>,
}

fn scan_fn(
    file: &Path,
    sig: &syn::Signature,
    block: &syn::Block,
    span_loc: usize,
    outer_generics: &std::collections::HashSet<String>,
) -> RawFn {
    let name = sig.ident.to_string();
    let mut hard = Vec::new();
    let mut effort = Vec::new();

    let mut generics: std::collections::HashSet<String> = sig.generics.params.iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(t) => Some(t.ident.to_string()),
            _ => None,
        })
        .collect();
    generics.extend(outer_generics.iter().cloned()); // impl<F: Float> scope
    if sig.asyncness.is_some() { hard.push("async fn".into()); }
    if sig.unsafety.is_some() { hard.push("unsafe fn".into()); }

    let mut uses_int = false;
    let mut uses_generic = false;
    for arg in &sig.inputs {
        match arg {
            syn::FnArg::Receiver(r) => {
                // audit v2: receiver methods are extraction candidates via
                // struct flattening (self.x -> extra args) — effort, unless
                // the receiver is mutable (mutation escaping scope — hard).
                if r.mutability.is_some() && r.reference.is_some() {
                    hard.push("&mut self (mutation escaping scope)".into());
                } else {
                    effort.push("method receiver (flatten self to args)".into());
                }
            }
            syn::FnArg::Typed(t) => match check_type(&t.ty, &generics) {
                Ok(fl) => { uses_int |= fl.int; uses_generic |= fl.generic; }
                Err(e) => hard.push(format!("param: {e}")),
            },
        }
    }
    match &sig.output {
        syn::ReturnType::Default => hard.push("returns () — no value to certify".into()),
        syn::ReturnType::Type(_, ty) => match check_type(ty, &generics) {
            Ok(fl) => { uses_int |= fl.int; uses_generic |= fl.generic; }
            Err(e) => hard.push(format!("return: {e}")),
        },
    }
    if uses_int && hard.is_empty() {
        effort.push("integer sort (exact sorts pending in Σ)".to_string());
    }
    if uses_generic && hard.is_empty() {
        effort.push("generic numeric param (monomorphize first)".to_string());
    }

    let mut scan = FnScan { self_name: name.clone(), generics, ..Default::default() };
    scan.visit_block(block);
    hard.extend(scan.hard);
    effort.extend(scan.effort);

    RawFn { file: file.to_path_buf(), name, loc: span_loc, hard, effort, calls: scan.calls }
}

fn loc_of(span: proc_macro2::Span) -> usize {
    span.end().line.saturating_sub(span.start().line) + 1
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg")
                && a.to_token_stream().to_string().contains("test"))
    })
}

/// Recursively collect fns from items: free fns, inline `mod` contents, and
/// impl-block methods. Test code (`#[test]` fns, `#[cfg(test)]` mods) is NOT
/// workload — §9 measures production code only.
fn collect_fns(file: &Path, items: &[syn::Item], raws: &mut Vec<RawFn>) {
    let no_generics = std::collections::HashSet::new();
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                if is_cfg_test(&f.attrs) { continue; }
                raws.push(scan_fn(file, &f.sig, &f.block, loc_of(f.span()), &no_generics));
            }
            syn::Item::Mod(m) => {
                if is_cfg_test(&m.attrs) { continue; }
                if let Some((_, inner)) = &m.content {
                    collect_fns(file, inner, raws);
                }
            }
            syn::Item::Impl(i) => {
                // generics declared on the impl block scope its methods
                let impl_generics: std::collections::HashSet<String> = i.generics.params.iter()
                    .filter_map(|p| match p {
                        syn::GenericParam::Type(t) => Some(t.ident.to_string()),
                        _ => None,
                    })
                    .collect();
                for ii in &i.items {
                    if let syn::ImplItem::Fn(f) = ii {
                        if is_cfg_test(&f.attrs) { continue; }
                        raws.push(scan_fn(file, &f.sig, &f.block, loc_of(f.span()), &impl_generics));
                    }
                }
            }
            _ => {}
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !matches!(name.as_str(), "target" | ".git" | "node_modules" | "tests" | "benches") {
                walk(&p, out);
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Audit a directory tree of Rust sources.
pub fn audit_dir(dir: &Path) -> AuditReport {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();

    let mut raws: Vec<RawFn> = Vec::new();
    let mut report = AuditReport::default();

    for file in files {
        let src = match fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => { report.skipped_files.push((file, e.to_string())); continue; }
        };
        let ast = match syn::parse_file(&src) {
            Ok(a) => a,
            Err(e) => { report.skipped_files.push((file, format!("parse: {e}"))); continue; }
        };
        collect_fns(&file, &ast.items, &mut raws);
    }

    // Fixed point over local calls: a call to an EXTRACTABLE audited fn is
    // inline-able (allowed); to a WITH_EFFORT fn — effort; to anything not
    // in the audit set or NOT — hard.
    let names: HashMap<String, usize> =
        raws.iter().enumerate().map(|(i, r)| (r.name.clone(), i)).collect();
    let mut class: Vec<Class> = raws
        .iter()
        .map(|r| if !r.hard.is_empty() { Class::NotExtractable }
             else if !r.effort.is_empty() { Class::WithEffort }
             else { Class::Extractable })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..raws.len() {
            if class[i] == Class::NotExtractable { continue; }
            for callee in &raws[i].calls {
                let demote_to = match names.get(callee) {
                    Some(&j) => match class[j] {
                        Class::Extractable => continue,
                        Class::WithEffort => Class::WithEffort,
                        Class::NotExtractable => Class::NotExtractable,
                    },
                    None => Class::NotExtractable, // unknown call = outside Σ
                };
                if (demote_to == Class::NotExtractable && class[i] != Class::NotExtractable)
                    || (demote_to == Class::WithEffort && class[i] == Class::Extractable)
                {
                    class[i] = demote_to;
                    changed = true;
                }
            }
        }
        if !changed { break; }
    }

    for (i, mut r) in raws.into_iter().enumerate() {
        let mut reasons = std::mem::take(&mut r.hard);
        reasons.extend(r.effort);
        for callee in &r.calls {
            match names.get(callee).map(|&j| class[j]) {
                Some(Class::Extractable) => {}
                Some(Class::WithEffort) =>
                    reasons.push(format!("calls with-effort fn `{callee}`")),
                Some(Class::NotExtractable) =>
                    reasons.push(format!("calls non-extractable fn `{callee}`")),
                None => reasons.push(format!("calls unknown fn `{callee}`")),
            }
        }
        report.fns.push(FnReport {
            file: r.file, name: r.name, loc: r.loc, class: class[i], reasons,
        });
    }
    report
}

pub fn run(args: &[String]) {
    let Some(dir) = args.first() else {
        eprintln!("usage: dge audit <src-dir>");
        return;
    };
    let report = audit_dir(Path::new(dir));
    print!("{}", report.render());
}
