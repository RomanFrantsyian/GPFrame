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
    /// v1.5 fission: a CO-accumulator while translating another
    /// accumulator's fold body. Its update statements are skipped (they
    /// belong to the sibling fold); READING it refuses — a coupled
    /// recurrence has no fission.
    Foreign,
    /// receiver flattening: a non-f64 struct field — refuses on READ with
    /// its type name (a method that never reads it still extracts).
    NonF64Field(String),
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
    /// Σ v1.6 f32 mode: the source function is f32-typed. Every ROUNDING
    /// op's result is wrapped in Rnd32 — by double-rounding innocuousness
    /// (f64 p=53 ≥ 2·24+2) that is BIT-IDENTICAL to native f32 for
    /// +,-,*,/,sqrt. Exact ops (neg/abs/min/max/floor/ceil, comparisons)
    /// need no wrap: exact results of f32-representable operands are
    /// f32-representable. Transcendentals are NOT innocuous and refuse.
    f32_mode: bool,
}

impl Cx<'_> {
    /// wrap in Rnd32 iff extracting an f32 function
    fn r32(&mut self, id: u32) -> u32 {
        if self.f32_mode { self.b.unary(Op::Rnd32, id) } else { id }
    }

    // ---------------------------------------------------------- values --
    fn expr(&mut self, e: &syn::Expr, sc: &mut Scopes) -> Result<u32, ExtractError> {
        use syn::Expr as E;
        let unsup = |s: String| ExtractError::Unsupported(s);
        match e {
            // receiver field read: `self.avg` — resolves through the
            // flattened bindings extract_fn installed (f64 field → Var;
            // non-f64 field → honest refusal naming the type)
            E::Field(f) => {
                let is_self = matches!(&*f.base,
                    E::Path(p) if p.path.is_ident("self"));
                if !is_self {
                    return Err(unsup(
                        "field access on a non-receiver value -- struct \
                         dataflow beyond `self` flattening is P3 territory".into()));
                }
                let syn::Member::Named(m) = &f.member else {
                    return Err(unsup("tuple-struct receiver field".into()));
                };
                match find_binding(sc, &format!("self.{m}")) {
                    Some(Binding::Node(id)) => Ok(*id),
                    Some(Binding::NonF64Field(t)) => Err(unsup(format!(
                        "reads non-f64 receiver field `{m}: {t}` -- Sigma is \
                         f64-only; receiver flattening covers f64 fields \
                         (non-f64 state: the audit's effort class)"))),
                    _ => Err(unsup(format!("unknown receiver field `{m}`"))),
                }
            }
            E::Lit(l) => match &l.lit {
                syn::Lit::Float(f) => {
                    let v = f.base10_parse::<f64>().map_err(|e| unsup(e.to_string()))?;
                    // f32 source literal: the compiler rounds it at parse
                    // time — the Σ constant is that rounded value
                    let v = if self.f32_mode { (v as f32) as f64 } else { v };
                    Ok(self.b.constant(v))
                }
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
                    Some(Binding::Foreign) => Err(unsup(format!(
                        "fold body reads co-accumulator `{name}` -- coupled \
                         recurrences (Welford-style) have no fission; \
                         cross-accumulator folds are on the roadmap"))),
                    // receiver fields are keyed `self.<field>` — a bare path
                    // can't reach one; kept for exhaustiveness
                    Some(Binding::NonF64Field(t)) => Err(unsup(format!(
                        "reads non-f64 receiver field `{name}: {t}` -- Sigma \
                         is f64-only"))),
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
            E::Cast(c) => {
                // TYPE-AWARE casts (v1.6). The old handler was transparent,
                // which silently read `(x as f32) as f64` as the identity —
                // dropping the rounding. Now:
                //   (e as f32) as f64  → Rnd32(e)      (the emitted form too)
                //   e as f64           → transparent    (widening/int lift)
                //   e as f32 bare      → Rnd32 in f32 mode; refuses in f64
                //                        mode (an f32-typed intermediate
                //                        makes later ops f32 ops the syntax
                //                        walk cannot see)
                //   e as <int>         → refuses (truncation changes value)
                let target = |t: &syn::Type| -> Option<String> {
                    if let syn::Type::Path(p) = t {
                        p.path.get_ident().map(|i| i.to_string())
                    } else { None }
                };
                match target(&c.ty).as_deref() {
                    Some("f64") => {
                        // peel parens: `(e as f32) as f64` parses the inner
                        // as Paren(Cast(..))
                        let mut inner_e: &syn::Expr = &c.expr;
                        while let E::Paren(p) = inner_e { inner_e = &p.expr; }
                        if let E::Cast(inner) = inner_e {
                            if target(&inner.ty).as_deref() == Some("f32") {
                                let a = self.expr(&inner.expr, sc)?;
                                return Ok(self.b.unary(Op::Rnd32, a));
                            }
                        }
                        self.expr(&c.expr, sc)
                    }
                    Some("f32") => {
                        if self.f32_mode {
                            let a = self.expr(&c.expr, sc)?;
                            Ok(self.b.unary(Op::Rnd32, a))
                        } else {
                            Err(unsup(
                                "bare `as f32` in an f64 function -- the \
                                 f32-typed intermediate makes downstream ops \
                                 f32 ops invisible to syntax; only the \
                                 widened round-trip `(e as f32) as f64` has \
                                 a Sigma reading (Rnd32)".into()))
                        }
                    }
                    Some(t) if ["i8","i16","i32","i64","i128","u8","u16",
                                "u32","u64","u128","isize","usize"]
                        .contains(&t) => {
                        // exemption: `(a < b) as u8` is the emitter's own
                        // comparison-as-value form — bool→int is EXACT 0/1,
                        // not a truncation
                        let mut inner_e: &syn::Expr = &c.expr;
                        while let E::Paren(p) = inner_e { inner_e = &p.expr; }
                        let is_cmp = matches!(inner_e, E::Binary(b) if matches!(
                            b.op, syn::BinOp::Lt(_) | syn::BinOp::Gt(_)
                                | syn::BinOp::Le(_) | syn::BinOp::Ge(_)
                                | syn::BinOp::Eq(_) | syn::BinOp::Ne(_)));
                        if is_cmp { return self.expr(inner_e, sc); }
                        Err(unsup(format!(
                            "`as {t}` truncates -- integer casts change the \
                             value, no Sigma reading")))
                    }
                    _ => self.expr(&c.expr, sc), // T::from-style generics
                }
            }
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
                let r = self.b.binary(op, a, c);
                Ok(self.r32(r)) // +,-,*,/ round in f32 — innocuous double rounding
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
                    if self.f32_mode {
                        return match op {
                            // sqrt IS innocuous (IEEE correctly-rounded)
                            Op::Sqrt => { let v = self.b.unary(Op::Sqrt, recv);
                                          Ok(self.r32(v)) }
                            // exact ops: no rounding to model
                            Op::Abs | Op::Floor | Op::Ceil =>
                                Ok(self.b.unary(*op, recv)),
                            _ => Err(unsup(format!(
                                "f32 `.{name}()` -- libm {name}f is not \
                                 round64({name}): transcendentals have no \
                                 innocuous double rounding, no Sigma reading"))),
                        };
                    }
                    return Ok(self.b.unary(*op, recv));
                }
                match name.as_str() {
                    "powf" | "powi" | "min" | "max" => {
                        if self.f32_mode && (name == "powf" || name == "powi") {
                            return Err(unsup(
                                "f32 `.powf()`/`.powi()` -- powf32 is not \
                                 round64(pow): no innocuous double rounding, \
                                 no Sigma reading".into()));
                        }
                        let arg = self.expr(&m.args[0], sc)?;
                        let op = match name.as_str() {
                            "min" => Op::Min, "max" => Op::Max, _ => Op::Pow,
                        };
                        Ok(self.b.binary(op, recv, arg)) // min/max: exact
                    }
                    "mul_add" => {
                        if self.f32_mode {
                            return Err(unsup(
                                "f32 `.mul_add()` -- fmaf rounds ONCE at 24 \
                                 bits; Rnd32(f64 fma) double-rounds a fused \
                                 result, which is NOT innocuous".into()));
                        }
                        let a1 = self.expr(&m.args[0], sc)?;
                        let a2 = self.expr(&m.args[1], sc)?;
                        Ok(self.b.ternary(Op::Fma, recv, a1, a2))
                    }
                    "recip" => {
                        let one = self.b.constant(1.0);
                        let v = self.b.binary(Op::Div, one, recv);
                        Ok(self.r32(v))
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
                    // `Float::sqrt(x)` / `f64::abs(x)` — the SAME Σ op as
                    // `.sqrt()`, call syntax only (MEASURED on average
                    // 0.16.0's `error()`, which spells it num_traits-style)
                    if c.args.len() == 1 {
                        if let Some((_, op)) = METHODS1.iter().find(|(n, _)| *n == name) {
                            let a = self.expr(&c.args[0], sc)?;
                            if self.f32_mode {
                                return match op {
                                    Op::Sqrt => { let v = self.b.unary(Op::Sqrt, a);
                                                  Ok(self.r32(v)) }
                                    Op::Abs | Op::Floor | Op::Ceil =>
                                        Ok(self.b.unary(*op, a)),
                                    _ => Err(unsup(format!(
                                        "f32 `{name}` -- no innocuous double \
                                         rounding for transcendentals"))),
                                };
                            }
                            return Ok(self.b.unary(*op, a));
                        }
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
                if matches!(find_binding(sc, &name), Some(Binding::Foreign)) {
                    return Ok(None); // co-accumulator update: fissioned into the sibling fold
                }
                let v = self.expr(&a.right, sc)?;
                if !rebind(sc, &name, Binding::Node(v)) {
                    return Err(ExtractError::Unsupported(format!("assign to unbound `{name}`")));
                }
                Ok(None)
            }
            // compound assignment
            E::Binary(b) if is_compound(&b.op) => {
                let name = expr_ident(&b.left)?;
                if matches!(find_binding(sc, &name), Some(Binding::Foreign)) {
                    return Ok(None); // co-accumulator update: fissioned into the sibling fold
                }
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
                let v = self.r32(v);
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
        if tgts.is_empty() {
            return Err(ExtractError::Unsupported(
                "fold body assigns nothing (side-effect loops are outside Σ)".into()));
        }
        // v1.5 MULTI-ACCUMULATOR (fission): each mutated scalar becomes its
        // OWN Σ Fold over the same iteration space — Σ itself is unchanged
        // (v1.2's fold_owners, the interpreter, the JIT, and the emitter all
        // already accept sibling folds). Soundness precondition per
        // accumulator: its body translation must not READ a co-accumulator
        // (Binding::Foreign refuses) — coupled recurrences have no fission.
        //
        // capture every init BEFORE any fold rebinding
        let mut inits = Vec::with_capacity(tgts.len());
        for name in &tgts {
            match find_binding(sc, name) {
                Some(Binding::Node(id)) => inits.push(*id),
                _ => return Err(ExtractError::Unsupported(format!(
                    "accumulator `{name}` must be a scalar bound before the loop"))),
            }
        }
        for (k, acc_name) in tgts.iter().enumerate() {
            // translate the body with acc ↦ Acc, co-accumulators ↦ Foreign,
            // loop var ↦ LoopIdx; shared intermediates re-translate fresh per
            // pass (tree-ification: sharing is space, not semantics)
            let acc_leaf = self.b.acc();
            // shadow every visible outer scalar with a DUPLICATE subtree:
            // a loop-invariant read from two sibling bodies must not share
            // arena nodes (v1.2 ownership; duplication is the same
            // tree-ification rule as copy_subtree). Invariants stay pure by
            // construction — they were bound before the loop.
            let mut dups: Vec<(String, u32)> = Vec::new();
            if tgts.len() > 1 {
                let mut visible: HashMap<String, u32> = HashMap::new();
                for fr in sc.iter() {
                    for (n, bd) in fr {
                        if let Binding::Node(id) = bd {
                            visible.insert(n.clone(), *id);
                        }
                    }
                }
                for (n, id) in visible {
                    // never dup a target (overwritten by Acc/Foreign below)
                    // and never dup a fold-valued binding: reading one inside
                    // a body is genuine NESTING, which must reach the v1.2
                    // validator's refusal, not be duplicated around
                    if !tgts.contains(&n) && !self.b.subtree_has_fold(id) {
                        dups.push((n, self.b.dup_subtree(id)));
                    }
                }
            }
            sc.push(HashMap::new());
            let frame = sc.last_mut().unwrap();
            for (n, id) in dups {
                frame.insert(n, Binding::Node(id));
            }
            frame.insert(acc_name.clone(), Binding::Node(acc_leaf));
            for other in &tgts {
                if other != acc_name {
                    frame.insert(other.clone(), Binding::Foreign);
                }
            }
            if let Some(v) = loop_var {
                frame.insert(v.clone(), Binding::LoopIdx);
            }
            self.exec_stmts(&f.body, sc)?;
            let body_root = match find_binding(sc, acc_name) {
                Some(Binding::Node(id)) => *id,
                _ => unreachable!(),
            };
            sc.pop();
            if body_root == acc_leaf {
                return Err(ExtractError::Unsupported(
                    "fold body never updates the accumulator".into()));
            }

            let fold_id = self.b.fold(inits[k], body_root);
            if !rebind(sc, acc_name, Binding::Node(fold_id)) {
                unreachable!("accumulator binding vanished");
            }
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
        -> Option<(&'a syn::Signature, &'a syn::Block, Option<String>)>
    {
        for item in items {
            match item {
                syn::Item::Fn(f) if f.sig.ident == name =>
                    return Some((&f.sig, &f.block, None)),
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        if let Some(hit) = find(inner, name) { return Some(hit); }
                    }
                }
                syn::Item::Impl(i) => {
                    let ty = match &*i.self_ty {
                        syn::Type::Path(p) => p.path.segments.last()
                            .map(|s| s.ident.to_string()),
                        _ => None,
                    };
                    for ii in &i.items {
                        if let syn::ImplItem::Fn(f) = ii {
                            if f.sig.ident == name {
                                return Some((&f.sig, &f.block, ty));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Named fields of `struct ty_name`, in DECLARATION order (that order IS
    /// the receiver's Var slot assignment — deterministic arity & meaning).
    fn struct_fields(items: &[syn::Item], ty_name: &str)
        -> Option<Vec<(String, String)>>
    {
        for item in items {
            match item {
                syn::Item::Struct(s) if s.ident == ty_name => {
                    let syn::Fields::Named(named) = &s.fields else { return None };
                    return Some(named.named.iter().map(|f| {
                        use quote::ToTokens;
                        (f.ident.as_ref().unwrap().to_string(),
                         f.ty.to_token_stream().to_string().replace(' ', ""))
                    }).collect());
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

    let (sig, block, self_ty) = find(&ast.items, fn_name)
        .ok_or_else(|| ExtractError::FnNotFound(fn_name.into()))?;

    // Σ is f64-only. A CONCRETE non-f64 numeric type (f32, i32, …) must
    // refuse: extracting f32 arithmetic as f64 changes the rounding of every
    // op (MEASURED on simple-easing 1.0.1 — an all-f32 crate the extractor
    // silently read as f64). Generic params (`T: Float`) stay admitted:
    // the extracted term IS the f64 instantiation — the monomorphize-then-
    // extract reading the audit prescribes, and the cross-door gate arbitrates.
    fn concrete_non_f64(ty: &syn::Type) -> Option<&'static str> {
        // NOTE v1.6: f32 is NOT in this list any more — f32 functions get
        // real Σ semantics via Rnd32 (round-at-every-op; see f32 mode).
        const BAD: &[&str] = &["i8", "i16", "i32", "i64", "i128",
            "u8", "u16", "u32", "u64", "u128", "isize", "usize", "bool", "char"];
        if let syn::Type::Path(p) = ty {
            if let Some(id) = p.path.get_ident() {
                return BAD.iter().find(|b| id == *b).copied();
            }
        }
        None
    }
    fn is_ident(ty: &syn::Type, name: &str) -> bool {
        matches!(ty, syn::Type::Path(p) if p.path.is_ident(name))
    }
    // f32 MODE DETECTION: an all-f32 float signature enters f32 mode.
    // Mixed f32/f64 float types refuse — the syntax walk cannot type
    // mixed-precision dataflow.
    let mut saw32 = false;
    let mut saw64 = false;
    let mut note = |ty: &syn::Type| {
        if is_ident(ty, "f32") { saw32 = true; }
        if is_ident(ty, "f64") { saw64 = true; }
    };
    if let syn::ReturnType::Type(_, ty) = &sig.output { note(ty); }
    for arg in &sig.inputs {
        if let syn::FnArg::Typed(t) = arg { note(&t.ty); }
    }
    let f32_mode = saw32;
    if saw32 && saw64 {
        return Err(ExtractError::Unsupported(
            "mixed f32/f64 signature -- precision of each op is invisible \
             to the syntax walk; single-precision signatures only".into()));
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        if let Some(t) = concrete_non_f64(ty) {
            return Err(ExtractError::Unsupported(format!(
                "return type `{t}` -- Sigma is f64-only; extracting `{t}` \
                 arithmetic as f64 changes rounding at every op ({t} lifting: \
                 roadmap)")));
        }
    }

    let mut b = TermBuilder::new();
    let mut scopes: Scopes = vec![HashMap::new()];
    let mut slot: u32 = 0;
    let mut seq_slot: u32 = 0;
    for arg in &sig.inputs {
        match arg {
            syn::FnArg::Typed(t) => {
                let name = pat_ident(&t.pat)?;
                if let Some(bad) = concrete_non_f64(&t.ty) {
                    return Err(ExtractError::Unsupported(format!(
                        "param `{name}: {bad}` -- Sigma is f64-only; \
                         extracting `{bad}` arithmetic as f64 changes rounding \
                         at every op ({bad} lifting: roadmap)")));
                }
                if is_unsized_slice(&t.ty) {
                    if f32_mode {
                        return Err(ExtractError::Unsupported(
                            "`&[f32]` sequence param -- f32 sequences are on \
                             the roadmap (Sigma seqs are f64)".into()));
                    }
                    // Σ v1.2: dynamic sequence — becomes a fold input
                    let k = seq_slot;
                    seq_slot += 1;
                    scopes[0].insert(name, Binding::Seq(k));
                    continue;
                }
                if let Some(n) = array_len(&t.ty) {
                    if f32_mode {
                        return Err(ExtractError::Unsupported(
                            "f32 array param -- f32 arrays are on the \
                             roadmap".into()));
                    }
                    let ids: Vec<u32> = (0..n).map(|j| b.var(slot + j as u32)).collect();
                    slot += n as u32;
                    scopes[0].insert(name, Binding::Array(ids));
                } else {
                    let id = b.var(slot);
                    slot += 1;
                    // f32 mode: the param Var is wrapped in Rnd32 AT BINDING,
                    // so the term is TOTAL over raw f64 μ′ — the claim
                    // quantifies over all f64 envs with rounding inside the
                    // term: term(e) == widen(native_f32(round32(e))).
                    let id = if f32_mode { b.unary(Op::Rnd32, id) } else { id };
                    scopes[0].insert(name, Binding::Node(id));
                }
            }
            syn::FnArg::Receiver(r) => {
                // METHOD RECEIVERS (field trial №2's 82% bucket, item 1):
                // an immutable `&self`/`self` receiver of a struct DEFINED IN
                // THIS FILE flattens — each f64 field becomes a Var in field
                // DECLARATION order (deterministic slot meaning). Non-f64
                // fields refuse ON READ (a body that never touches them still
                // extracts). Mutable receivers are effectful and refuse.
                if r.mutability.is_some() {
                    return Err(ExtractError::Unsupported(
                        "mutable method receiver -- `&mut self` mutates \
                         receiver state (the audit's effort/P3 class); \
                         Sigma reads `&self` getters and free fns".into()));
                }
                let ty_name = self_ty.clone().ok_or_else(|| ExtractError::Unsupported(
                    "method on an unnameable self type".into()))?;
                let fields = struct_fields(&ast.items, &ty_name)
                    .ok_or_else(|| ExtractError::Unsupported(format!(
                        "receiver struct `{ty_name}` has no named-field \
                         definition in this file -- field layout unknown, \
                         cannot flatten `self`")))?;
                for (fname, fty) in fields {
                    if fty == "f64" {
                        let id = b.var(slot);
                        slot += 1;
                        scopes[0].insert(format!("self.{fname}"), Binding::Node(id));
                    } else {
                        scopes[0].insert(format!("self.{fname}"),
                            Binding::NonF64Field(fty));
                    }
                }
            }
        }
    }

    let root = Cx { b: &mut b, f32_mode }.block_value(block, &mut scopes)?;
    let t = b.finish(root);
    // the Σ v1.2 binding discipline is a free internal validator (the lift
    // door runs the same check): if translation wired a body value outside
    // its fold or shared one between sibling folds, refuse honestly here
    // instead of panicking downstream in the gate.
    t.fold_owners().map_err(|e| ExtractError::Unsupported(format!(
        "a body value escapes its fold ({e}) -- fission requires \
         accumulator-disjoint bodies")))?;
    Ok(t)
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
