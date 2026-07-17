//! `dge extract <file.rs> <fn_name>` — the FRONT DOOR: Rust fn → Term_p.
//!
//! v2 coverage (the audit's EXTRACT class, now including the imperative
//! patterns that are pure computations in disguise):
//!   * f64 scalar params AND fixed-size array params `[f64; N]` / `&[f64; N]`
//!     (each element becomes a var slot; indexing by compile-time ints)
//!   * `let` bindings with shadowing
//!   * `let mut` + assignment / compound assignment — SSA REBINDING:
//!     mutation of a local that never escapes is pure dataflow (O5 intact)
//!   * `for i in LO..HI` / `LO..=HI` with literal bounds — UNROLLING
//!     (cap 1024 iterations; the loop var is a compile-time int usable as
//!     an index or, via `as f64`, as a value)
//!   * statement-level `if` with assignments in branches — PHI-MERGE:
//!     every binding that differs across branches becomes select(cond,·,·)
//!   * expression `if/else`, Σ math methods, comparisons (< > <= >=)
//!
//! Σ v1.2: `&[f64]` params are SEQUENCE inputs, and
//! `for i in 0..s.len() { <single-accumulator body> }` becomes fold(init,
//! body) — bounded by the runtime length, so Term_p stays total. Contract:
//! one accumulator per loop, indexing only `s[i]`, all sequences one length.
//!
//! Deliberately NOT covered (with the reason in the error message):
//!   * `while` / `loop` — no runtime bound object exists for them
//!   * iterator adapters, offset/windowed indexing, multi-accumulator
//!     folds, non-zero range starts — each refused with "roadmap"
//!
//! EXTRACTION IS A LOWERING (L1 in reverse): the translation is NOT trusted.
//! Its output must pass the extraction gate — a bitwise differential of
//! interp(term) against the rustc-compiled original over μ'.
//!
//! Comparisons are FIRST-CLASS Σ ops (v1.1: lt/gt/le/ge, 1.0/0.0-valued,
//! Rust semantics — false on NaN, ±0 equal). The encoding caveats of the
//! v1 extractor are gone; the extraction gate remains the arbiter of the
//! translation as a whole.

use std::collections::HashMap;
use term::{Op, Term, TermBuilder};

const UNROLL_CAP: i64 = 1024;

#[derive(Debug)]
pub enum ExtractError {
    Parse(String),
    FnNotFound(String),
    Unsupported(String),
}

#[derive(Clone)]
enum Binding {
    Node(u32),
    Int(i64),
    Array(Vec<u32>), // pre-created var node ids (fixed-size params)
    /// Σ v1.2: dynamic sequence param — index k into the term's seq slots.
    Seq(u32),
    /// the fold loop variable: valid ONLY as `s[__i]` inside the fold body.
    LoopIdx,
}

type Scopes = Vec<HashMap<String, Binding>>;

fn find_binding<'a>(scopes: &'a Scopes, name: &str) -> Option<&'a Binding> {
    scopes.iter().rev().find_map(|s| s.get(name))
}

/// rebind in the innermost frame that already holds `name` (assignment
/// semantics — mutation escapes the loop/branch frame to the declaration).
fn rebind(scopes: &mut Scopes, name: &str, b: Binding) -> bool {
    for frame in scopes.iter_mut().rev() {
        if frame.contains_key(name) {
            frame.insert(name.to_string(), b);
            return true;
        }
    }
    false
}

const METHODS1: &[(&str, Op)] = &[
    ("sin", Op::Sin), ("cos", Op::Cos), ("tan", Op::Tan),
    ("exp", Op::Exp), ("exp2", Op::Exp2), ("ln", Op::Ln), ("sqrt", Op::Sqrt),
    ("abs", Op::Abs), ("floor", Op::Floor), ("ceil", Op::Ceil),
];

struct Cx<'a> {
    b: &'a mut TermBuilder,
}

impl Cx<'_> {
    // ---------------------------------------------------------- values --
    fn expr(&mut self, e: &syn::Expr, sc: &mut Scopes) -> Result<u32, ExtractError> {
        use syn::Expr as E;
        let unsup = |s: String| ExtractError::Unsupported(s);
        match e {
            E::Lit(l) => match &l.lit {
                syn::Lit::Float(f) => Ok(self.b.constant(
                    f.base10_parse::<f64>().map_err(|e| unsup(e.to_string()))?)),
                syn::Lit::Int(i) => Ok(self.b.constant(
                    i.base10_parse::<i64>().map_err(|e| unsup(e.to_string()))? as f64)),
                _ => Err(unsup("non-numeric literal".into())),
            },
            E::Path(p) => {
                let name = p.path.get_ident().map(|i| i.to_string())
                    .ok_or_else(|| unsup("qualified path".into()))?;
                match find_binding(sc, &name) {
                    Some(Binding::Node(id)) => Ok(*id),
                    Some(Binding::Int(k)) => Ok(self.b.constant(*k as f64)),
                    Some(Binding::Array(_)) =>
                        Err(unsup(format!("array `{name}` used as a scalar"))),
                    Some(Binding::Seq(_)) =>
                        Err(unsup(format!("sequence `{name}` used as a scalar"))),
                    Some(Binding::LoopIdx) =>
                        Err(unsup("fold loop var used outside `s[__i]` indexing".into())),
                    None => Err(unsup(format!("unbound `{name}`"))),
                }
            }
            E::Index(ix) => {
                let name = match &*ix.expr {
                    E::Path(p) => p.path.get_ident().map(|i| i.to_string()),
                    _ => None,
                }.ok_or_else(|| unsup("indexing a non-identifier".into()))?;
                // Σ v1.2: `s[loop_var]` inside a fold body → Elem(k)
                // (checked BEFORE const-index resolution: the loop var is
                // not a compile-time int)
                if let Some(Binding::Seq(k)) = find_binding(sc, &name).cloned() {
                    return match &*ix.index {
                        E::Path(p) if p.path.get_ident()
                            .is_some_and(|id| matches!(
                                find_binding(sc, &id.to_string()),
                                Some(Binding::LoopIdx))) =>
                            Ok(self.b.elem(k)),
                        _ => Err(unsup(format!(
                            "sequence `{name}` indexed by a non-loop expression \
                             (windowed/offset access: roadmap)"))),
                    };
                }
                let idx = self.const_int(&ix.index, sc)?;
                match find_binding(sc, &name) {
                    Some(Binding::Array(ids)) => {
                        let i = usize::try_from(idx)
                            .ok().filter(|i| *i < ids.len())
                            .ok_or_else(|| unsup(format!(
                                "index {idx} out of bounds for `{name}` (len {})", ids.len())))?;
                        Ok(ids[i])
                    }
                    _ => Err(unsup(format!("`{name}` is not an array param"))),
                }
            }
            E::Paren(p) => self.expr(&p.expr, sc),
            E::Group(g) => self.expr(&g.expr, sc),
            E::Cast(c) => self.expr(&c.expr, sc), // Int bindings already lift to consts
            E::Unary(u) => match u.op {
                syn::UnOp::Neg(_) => {
                    let a = self.expr(&u.expr, sc)?;
                    Ok(self.b.unary(Op::Neg, a))
                }
                _ => Err(unsup("unary op".into())),
            },
            E::Binary(bin) => {
                use syn::BinOp::*;
                let op = match bin.op {
                    Add(_) => Op::Add, Sub(_) => Op::Sub,
                    Mul(_) => Op::Mul, Div(_) => Op::Div,
                    Lt(_) | Gt(_) | Le(_) | Ge(_) | Eq(_) =>
                        return self.comparison(bin, sc),
                    // emitter round-trip special case FIRST: `c != 0.0` IS
                    // select's raw condition semantics — translate to c
                    // itself (behaviorally Ne(c, 0.0) would be identical,
                    // but the raw form keeps emit∘extract structurally
                    // idempotent instead of growing a Ne layer per cycle).
                    // Every other `!=` is Σ v1.4 Ne (Rust's une exactly).
                    Ne(_) => {
                        if let E::Lit(l) = &*bin.right {
                            if matches!(&l.lit, syn::Lit::Float(f) if f.base10_parse::<f64>().map(|v| v == 0.0).unwrap_or(false)) {
                                return self.expr(&bin.left, sc);
                            }
                        }
                        return self.comparison(bin, sc);
                    }
                    _ => return Err(unsup("unsupported binary op".into())),
                };
                let a = self.expr(&bin.left, sc)?;
                let c = self.expr(&bin.right, sc)?;
                Ok(self.b.binary(op, a, c))
            }
            E::MethodCall(m) => {
                let name = m.method.to_string();
                // Σ v1.3: `s.len()` on a sequence param is the Len terminal
                // (used as `s.len() as f64`; the Cast layer passes through).
                if name == "len" && m.args.is_empty() {
                    if let E::Path(p) = &*m.receiver {
                        if let Some(id) = p.path.get_ident() {
                            if let Some(Binding::Seq(k)) =
                                find_binding(sc, &id.to_string()).cloned() {
                                return Ok(self.b.len_of(k));
                            }
                        }
                    }
                }
                let recv = self.expr(&m.receiver, sc)?;
                if let Some((_, op)) = METHODS1.iter().find(|(n, _)| *n == name) {
                    return Ok(self.b.unary(*op, recv));
                }
                match name.as_str() {
                    "powf" | "powi" | "min" | "max" => {
                        let arg = self.expr(&m.args[0], sc)?;
                        let op = match name.as_str() {
                            "min" => Op::Min, "max" => Op::Max, _ => Op::Pow,
                        };
                        Ok(self.b.binary(op, recv, arg))
                    }
                    "mul_add" => {
                        let a1 = self.expr(&m.args[0], sc)?;
                        let a2 = self.expr(&m.args[1], sc)?;
                        Ok(self.b.ternary(Op::Fma, recv, a1, a2))
                    }
                    "recip" => {
                        let one = self.b.constant(1.0);
                        Ok(self.b.binary(Op::Div, one, recv))
                    }
                    _ => Err(unsup(format!("method .{name}() outside Σ"))),
                }
            }
            E::Call(c) => {
                if let E::Path(p) = &*c.func {
                    let name = p.path.segments.last()
                        .map(|s| s.ident.to_string()).unwrap_or_default();
                    if c.args.len() == 1 && (name == "f" || name == "from" || name == "cast") {
                        return self.expr(&c.args[0], sc);
                    }
                    // emitter round-trip: exact-bits constants
                    if name == "from_bits" && c.args.len() == 1 {
                        if let E::Lit(l) = &c.args[0] {
                            if let syn::Lit::Int(i) = &l.lit {
                                let bits: u64 = i.base10_parse()
                                    .map_err(|e| unsup(e.to_string()))?;
                                return Ok(self.b.constant(f64::from_bits(bits)));
                            }
                        }
                        return Err(unsup("from_bits with non-literal".into()));
                    }
                    return Err(unsup(format!("call `{name}` outside Σ")));
                }
                Err(unsup("indirect call".into()))
            }
            E::If(i) => {
                let cond = self.expr(&i.cond, sc)?;
                let then_v = self.block_value(&i.then_branch, sc)?;
                let else_v = match &i.else_branch {
                    Some((_, eb)) => self.expr(eb, sc)?,
                    None => return Err(unsup("value `if` without else".into())),
                };
                Ok(self.b.ternary(Op::Select, cond, then_v, else_v))
            }
            E::Block(b) => self.block_value(&b.block, sc),
            E::Return(r) => match &r.expr {
                Some(e) => self.expr(e, sc),
                None => Err(unsup("bare return".into())),
            },
            E::While(_) | E::Loop(_) => Err(unsup(
                "unbounded loop: Term_p is total — use `for LO..HI` with literal \
                 bounds (unrolled); a fold operator in Σ is the roadmap item".into())),
            E::ForLoop(_) => Err(unsup("`for` used as a value expression".into())),
            other => Err(unsup(format!("expression form {:?}", std::mem::discriminant(other)))),
        }
    }

    /// compile-time integer (loop bounds, indices)
    fn const_int(&mut self, e: &syn::Expr, sc: &Scopes) -> Result<i64, ExtractError> {
        use syn::Expr as E;
        match e {
            E::Lit(l) => match &l.lit {
                syn::Lit::Int(i) => i.base10_parse::<i64>()
                    .map_err(|e| ExtractError::Unsupported(e.to_string())),
                _ => Err(ExtractError::Unsupported("non-int index".into())),
            },
            E::Path(p) => {
                let name = p.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
                match find_binding(sc, &name) {
                    Some(Binding::Int(k)) => Ok(*k),
                    Some(Binding::LoopIdx) => Err(ExtractError::Unsupported(
                        "fold loop var in a constant-index position".into())),
                    _ => Err(ExtractError::Unsupported(format!(
                        "index `{name}` is not a compile-time int (dynamic \
                         indexing needs the fold roadmap item)"))),
                }
            }
            E::Binary(b) => {
                let l = self.const_int(&b.left, sc)?;
                let r = self.const_int(&b.right, sc)?;
                use syn::BinOp::*;
                Ok(match b.op {
                    Add(_) => l + r, Sub(_) => l - r, Mul(_) => l * r,
                    _ => return Err(ExtractError::Unsupported("index arithmetic op".into())),
                })
            }
            E::Paren(p) => self.const_int(&p.expr, sc),
            _ => Err(ExtractError::Unsupported("non-constant index expression".into())),
        }
    }

    fn comparison(&mut self, bin: &syn::ExprBinary, sc: &mut Scopes) -> Result<u32, ExtractError> {
        use syn::BinOp::*;
        let a = self.expr(&bin.left, sc)?;
        let b = self.expr(&bin.right, sc)?;
        // Σ v1.1: first-class ordered comparisons — exact Rust semantics
        // (false on NaN, ±0 equal). No encodings, no caveats.
        let op = match bin.op {
            Lt(_) => Op::Lt, Gt(_) => Op::Gt, Le(_) => Op::Le, Ge(_) => Op::Ge,
            // Σ v1.4: Rust `==`/`!=` exactly (oeq/une — NaN ⇒ false/true)
            Eq(_) => Op::Eq, Ne(_) => Op::Ne,
            _ => unreachable!(),
        };
        Ok(self.b.binary(op, a, b))
    }

    // ------------------------------------------------------ statements --
    /// execute statements for their bindings/mutations; return the trailing
    /// value expression's node if the block has one.
    fn exec_stmts(&mut self, blk: &syn::Block, sc: &mut Scopes) -> Result<Option<u32>, ExtractError> {
        let mut last: Option<u32> = None;
        for stmt in &blk.stmts {
            last = None;
            match stmt {
                syn::Stmt::Local(l) => {
                    let name = pat_ident(&l.pat)?;
                    let init = l.init.as_ref()
                        .ok_or(ExtractError::Unsupported("let without init".into()))?;
                    let v = self.expr(&init.expr, sc)?;
                    sc.last_mut().unwrap().insert(name, Binding::Node(v));
                }
                syn::Stmt::Expr(e, _) => last = self.stmt_expr(e, sc)?,
                _ => return Err(ExtractError::Unsupported("statement form".into())),
            }
        }
        Ok(last)
    }

    fn stmt_expr(&mut self, e: &syn::Expr, sc: &mut Scopes) -> Result<Option<u32>, ExtractError> {
        use syn::Expr as E;
        match e {
            // plain assignment: SSA rebind at the declaring frame
            E::Assign(a) => {
                let name = expr_ident(&a.left)?;
                let v = self.expr(&a.right, sc)?;
                if !rebind(sc, &name, Binding::Node(v)) {
                    return Err(ExtractError::Unsupported(format!("assign to unbound `{name}`")));
                }
                Ok(None)
            }
            // compound assignment
            E::Binary(b) if is_compound(&b.op) => {
                let name = expr_ident(&b.left)?;
                let cur = match find_binding(sc, &name) {
                    Some(Binding::Node(id)) => *id,
                    _ => return Err(ExtractError::Unsupported(format!("`{name}` not a scalar"))),
                };
                let rhs = self.expr(&b.right, sc)?;
                use syn::BinOp::*;
                let op = match b.op {
                    AddAssign(_) => Op::Add, SubAssign(_) => Op::Sub,
                    MulAssign(_) => Op::Mul, DivAssign(_) => Op::Div,
                    _ => return Err(ExtractError::Unsupported("compound op".into())),
                };
                let v = self.b.binary(op, cur, rhs);
                rebind(sc, &name, Binding::Node(v));
                Ok(None)
            }
            // bounded for-loop: UNROLL
            E::ForLoop(f) => {
                let var = match &*f.pat {
                    syn::Pat::Ident(pi) => Some(pi.ident.to_string()),
                    syn::Pat::Wild(_) => None,
                    _ => return Err(ExtractError::Unsupported("loop pattern".into())),
                };
                let E::Range(r) = &*f.expr else {
                    return Err(ExtractError::Unsupported(
                        "loop over a non-range (iterator adapters: roadmap)".into()));
                };
                // Σ v1.2: `for i in 0..s.len() { <single-accumulator body> }`
                if let Some(end) = r.end.as_deref() {
                    if let E::MethodCall(mc) = end {
                        if mc.method == "len" {
                            let start_ok = matches!(r.start.as_deref(),
                                Some(E::Lit(l)) if matches!(&l.lit,
                                    syn::Lit::Int(i) if i.base10_parse::<i64>().map(|v| v == 0).unwrap_or(false)));
                            if !start_ok {
                                return Err(ExtractError::Unsupported(
                                    "fold range must start at 0 (offset folds: roadmap)".into()));
                            }
                            let seq_name = match &*mc.receiver {
                                E::Path(p) => p.path.get_ident().map(|i| i.to_string()),
                                _ => None,
                            }.ok_or(ExtractError::Unsupported("len() on non-param".into()))?;
                            if !matches!(find_binding(sc, &seq_name), Some(Binding::Seq(_))) {
                                return Err(ExtractError::Unsupported(format!(
                                    "`{seq_name}.len()` bound requires a `&[f64]` param")));
                            }
                            return self.dynamic_fold(f, &var, sc);
                        }
                    }
                }
                let lo = self.const_int(
                    r.start.as_deref().ok_or(ExtractError::Unsupported("open range".into()))?, sc)?;
                let hi_raw = self.const_int(
                    r.end.as_deref().ok_or(ExtractError::Unsupported("open range".into()))?, sc)?;
                let hi = match r.limits {
                    syn::RangeLimits::HalfOpen(_) => hi_raw,
                    syn::RangeLimits::Closed(_) => hi_raw + 1,
                };
                if hi - lo > UNROLL_CAP {
                    return Err(ExtractError::Unsupported(format!(
                        "unroll of {} iterations exceeds cap {UNROLL_CAP}", hi - lo)));
                }
                for k in lo..hi {
                    sc.push(HashMap::new());
                    if let Some(v) = &var {
                        sc.last_mut().unwrap().insert(v.clone(), Binding::Int(k));
                    }
                    self.exec_stmts(&f.body, sc)?;
                    sc.pop();
                }
                Ok(None)
            }
            // statement-if with mutations: PHI-MERGE via Select
            E::If(i) if is_stmt_if(i) => {
                let cond = self.expr(&i.cond, sc)?;
                let mut then_sc = sc.clone();
                then_sc.push(HashMap::new());
                self.exec_stmts(&i.then_branch, &mut then_sc)?;
                then_sc.pop();
                let mut else_sc = sc.clone();
                if let Some((_, eb)) = &i.else_branch {
                    match &**eb {
                        E::Block(b) => {
                            else_sc.push(HashMap::new());
                            self.exec_stmts(&b.block, &mut else_sc)?;
                            else_sc.pop();
                        }
                        other => { self.stmt_expr(other, &mut else_sc)?; }
                    }
                }
                // merge: any scalar binding that diverged becomes a select
                for fi in 0..sc.len() {
                    let names: Vec<String> = sc[fi].keys().cloned().collect();
                    for name in names {
                        let (t, e2) = match (then_sc[fi].get(&name), else_sc[fi].get(&name)) {
                            (Some(Binding::Node(t)), Some(Binding::Node(e2))) => (*t, *e2),
                            _ => continue,
                        };
                        if t != e2 {
                            let merged = self.b.ternary(Op::Select, cond, t, e2);
                            sc[fi].insert(name, Binding::Node(merged));
                        }
                    }
                }
                Ok(None)
            }
            // anything else: evaluate as a value (trailing expression)
            other => Ok(Some(self.expr(other, sc)?)),
        }
    }

    /// Build a Fold from `for i in 0..s.len() { … }`.
    /// v1.2 contract (each refusal states it):
    ///   * the body mutates exactly ONE pre-existing scalar binding — the
    ///     accumulator (multi-accumulator folds: roadmap);
    ///   * every sequence access inside is `seq[i]` with the loop variable;
    ///   * ALL sequences share one runtime length (the emitted/extracted
    ///     claim is over equal-length inputs — recorded in certificates).
    fn dynamic_fold(
        &mut self,
        f: &syn::ExprForLoop,
        loop_var: &Option<String>,
        sc: &mut Scopes,
    ) -> Result<Option<u32>, ExtractError> {
        // which pre-existing scalar does the body assign?
        fn targets(stmts: &[syn::Stmt], out: &mut Vec<String>) {
            for st in stmts {
                if let syn::Stmt::Expr(e, _) = st {
                    match e {
                        syn::Expr::Assign(a) => {
                            if let Ok(n) = expr_ident(&a.left) { out.push(n); }
                        }
                        syn::Expr::Binary(b) if is_compound(&b.op) => {
                            if let Ok(n) = expr_ident(&b.left) { out.push(n); }
                        }
                        syn::Expr::If(i) => {
                            targets(&i.then_branch.stmts, out);
                            if let Some((_, eb)) = &i.else_branch {
                                if let syn::Expr::Block(bb) = &**eb {
                                    targets(&bb.block.stmts, out);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut tgts = Vec::new();
        targets(&f.body.stmts, &mut tgts);
        tgts.sort();
        tgts.dedup();
        let acc_name = match tgts.as_slice() {
            [one] => one.clone(),
            [] => return Err(ExtractError::Unsupported(
                "fold body assigns nothing (side-effect loops are outside Σ)".into())),
            many => return Err(ExtractError::Unsupported(format!(
                "multi-accumulator fold over {many:?}: roadmap"))),
        };
        let init = match find_binding(sc, &acc_name) {
            Some(Binding::Node(id)) => *id,
            _ => return Err(ExtractError::Unsupported(format!(
                "accumulator `{acc_name}` must be a scalar bound before the loop"))),
        };

        // translate the body once with acc ↦ Acc, loop var ↦ LoopIdx
        let acc_leaf = self.b.acc();
        sc.push(HashMap::new());
        sc.last_mut().unwrap().insert(acc_name.clone(), Binding::Node(acc_leaf));
        if let Some(v) = loop_var {
            sc.last_mut().unwrap().insert(v.clone(), Binding::LoopIdx);
        }
        self.exec_stmts(&f.body, sc)?;
        let body_root = match find_binding(sc, &acc_name) {
            Some(Binding::Node(id)) => *id,
            _ => unreachable!(),
        };
        sc.pop();
        if body_root == acc_leaf {
            return Err(ExtractError::Unsupported(
                "fold body never updates the accumulator".into()));
        }

        let fold_id = self.b.fold(init, body_root);
        if !rebind(sc, &acc_name, Binding::Node(fold_id)) {
            unreachable!("accumulator binding vanished");
        }
        Ok(None)
    }

    fn block_value(&mut self, blk: &syn::Block, sc: &mut Scopes) -> Result<u32, ExtractError> {
        sc.push(HashMap::new());
        let last = self.exec_stmts(blk, sc)?;
        sc.pop();
        last.ok_or(ExtractError::Unsupported("block yields no value".into()))
    }
}

fn is_compound(op: &syn::BinOp) -> bool {
    use syn::BinOp::*;
    matches!(op, AddAssign(_) | SubAssign(_) | MulAssign(_) | DivAssign(_))
}

fn is_stmt_if(i: &syn::ExprIf) -> bool {
    // an `if` whose branches end in statements (assignments) rather than a
    // trailing value — heuristic: then-branch has no trailing value expr
    !matches!(i.then_branch.stmts.last(),
        Some(syn::Stmt::Expr(e, None)) if !matches!(e,
            syn::Expr::Assign(_) | syn::Expr::ForLoop(_) | syn::Expr::If(_)))
}

fn pat_ident(p: &syn::Pat) -> Result<String, ExtractError> {
    match p {
        syn::Pat::Ident(pi) => Ok(pi.ident.to_string()),
        syn::Pat::Type(pt) => pat_ident(&pt.pat),
        _ => Err(ExtractError::Unsupported("pattern binding".into())),
    }
}

fn expr_ident(e: &syn::Expr) -> Result<String, ExtractError> {
    match e {
        syn::Expr::Path(p) => p.path.get_ident().map(|i| i.to_string())
            .ok_or(ExtractError::Unsupported("qualified lvalue".into())),
        _ => Err(ExtractError::Unsupported("non-identifier lvalue".into())),
    }
}

/// array length from a param type, if it is a fixed-size f64 array
fn array_len(ty: &syn::Type) -> Option<usize> {
    match ty {
        syn::Type::Reference(r) => array_len(&r.elem),
        syn::Type::Array(a) => {
            if let syn::Expr::Lit(l) = &a.len {
                if let syn::Lit::Int(i) = &l.lit {
                    return i.base10_parse::<usize>().ok();
                }
            }
            None
        }
        _ => None,
    }
}

fn is_unsized_slice(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Reference(r) => is_unsized_slice(&r.elem),
        syn::Type::Slice(_) => true,
        _ => false,
    }
}

/// Extract `fn_name` from Rust source. Searches free fns and impl methods.
pub fn extract_fn(src: &str, fn_name: &str) -> Result<Term, ExtractError> {
    let ast = syn::parse_file(src).map_err(|e| ExtractError::Parse(e.to_string()))?;

    fn find<'a>(items: &'a [syn::Item], name: &str)
        -> Option<(&'a syn::Signature, &'a syn::Block)>
    {
        for item in items {
            match item {
                syn::Item::Fn(f) if f.sig.ident == name => return Some((&f.sig, &f.block)),
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        if let Some(hit) = find(inner, name) { return Some(hit); }
                    }
                }
                syn::Item::Impl(i) => {
                    for ii in &i.items {
                        if let syn::ImplItem::Fn(f) = ii {
                            if f.sig.ident == name { return Some((&f.sig, &f.block)); }
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    let (sig, block) = find(&ast.items, fn_name)
        .ok_or_else(|| ExtractError::FnNotFound(fn_name.into()))?;

    let mut b = TermBuilder::new();
    let mut scopes: Scopes = vec![HashMap::new()];
    let mut slot: u32 = 0;
    let mut seq_slot: u32 = 0;
    for arg in &sig.inputs {
        match arg {
            syn::FnArg::Typed(t) => {
                let name = pat_ident(&t.pat)?;
                if is_unsized_slice(&t.ty) {
                    // Σ v1.2: dynamic sequence — becomes a fold input
                    let k = seq_slot;
                    seq_slot += 1;
                    scopes[0].insert(name, Binding::Seq(k));
                    continue;
                }
                if let Some(n) = array_len(&t.ty) {
                    let ids: Vec<u32> = (0..n).map(|j| b.var(slot + j as u32)).collect();
                    slot += n as u32;
                    scopes[0].insert(name, Binding::Array(ids));
                } else {
                    let id = b.var(slot);
                    slot += 1;
                    scopes[0].insert(name, Binding::Node(id));
                }
            }
            syn::FnArg::Receiver(_) => return Err(ExtractError::Unsupported(
                "method receiver (flatten self first)".into())),
        }
    }

    let root = Cx { b: &mut b }.block_value(block, &mut scopes)?;
    Ok(b.finish(root))
}

pub fn run(args: &[String]) {
    let (Some(file), Some(name)) = (args.first(), args.get(1)) else {
        eprintln!("usage: dge extract <file.rs> <fn_name> [--out <t.sexpr>]");
        return;
    };
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {file}: {e}"); return; }
    };
    match extract_fn(&src, name) {
        Ok(t) => {
            let sexpr = term::sexpr::print(&t);
            println!("{sexpr}");
            if let Some(i) = args.iter().position(|a| a == "--out") {
                if let Some(out) = args.get(i + 1) {
                    std::fs::write(out, format!("{sexpr}\n")).ok();
                    eprintln!("({} nodes, arity {}) -> {out}", t.len(), t.arity());
                }
            }
            eprintln!("NOTE: extraction is a lowering — gate this term against \
                       the compiled original before trusting it.");
        }
        Err(e) => eprintln!("extraction failed: {e:?}"),
    }
}
