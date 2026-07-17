//! `dge emit <t.sexpr>` — BACK-EMISSION: Term_p → Rust source.
//!
//! EMISSION IS A LOWERING (L1, again): the printer below is NOT trusted.
//! Its output must pass the emission gate before anyone ships it:
//!   1. re-extraction round trip: extract(emit(t)) ≡ t under
//!      BitwiseNanClass over μ' (closes emit∘extract = id through our own
//!      front door), and
//!   2. a rustc compile-and-run differential on probe envs (the emitted
//!      source compiled by the REAL compiler vs interp(t)).
//!
//! Faithfulness rules (each preserves the exact op sequence):
//!   * every shared node (used ≥2×) becomes a `let tN = …;` in topological
//!     order — the arena order IS a valid schedule, so CSE costs nothing
//!   * constants print via Rust's round-trip `{:?}` with `_f64` suffix;
//!     NaN payloads and specials go through `f64::from_bits(0x…)`
//!   * `select(c,a,b)` prints as `if a < b { … }` when c is a comparison
//!     node, else `if (c) != 0.0 { … }` — both EXACTLY the interp semantics
//!     (NaN cond ⇒ then-branch for the != form; false for ordered cmps)
//!   * comparisons used as VALUES print as `((a < b) as u8 as f64)` —
//!     literally the interpreter's own definition
//!   * arity ≤ 8 → scalar params `v0..`; larger → `v: &[f64; N]`
//!
//! The attached certificate (if any) is emitted as a doc comment: the claim,
//! the rule trace, and the env fingerprint travel WITH the code.

use harness::Certificate;
use std::fmt::Write as _;
use term::{Op, Term};

const SCALAR_PARAM_MAX: usize = 8;

pub fn emit_rust(t: &Term, fn_name: &str, cert: Option<&Certificate>) -> String {
    let arity = t.arity();
    let scalars = arity <= SCALAR_PARAM_MAX;

    // reachability + use counts from the root
    let n = t.len();
    let mut uses = vec![0u32; n];
    let mut stack = vec![t.root];
    let mut reach = vec![false; n];
    while let Some(id) = stack.pop() {
        uses[id as usize] += 1;
        if reach[id as usize] { continue; }
        reach[id as usize] = true;
        let node = t.node(id);
        let ar = node.op.arity();
        if ar >= 1 { stack.push(node.a); }
        if ar >= 2 { stack.push(node.b); }
        if ar >= 3 { stack.push(node.c); }
    }
    let owners = t.fold_owners().expect("emit: ill-formed fold binders");
    // bind shared non-trivial nodes — fold-body-owned nodes excluded
    // (they are per-iteration values, re-emitted inside their loop)
    let bound: Vec<bool> = (0..n)
        .map(|i| {
            reach[i]
                && uses[i] >= 2
                && owners[i].is_none()
                && !matches!(t.node(i as u32).op,
                    Op::Const | Op::Var | Op::Acc | Op::Elem | Op::Len)
                && (i as u32) != t.root
        })
        .collect();

    let var = |i: u32| -> String {
        if scalars { format!("v{i}") } else { format!("v[{}]", i) }
    };

    fn lit(v: f64) -> String {
        if v.is_nan() || (v.is_infinite()) {
            // specials + payload-bearing NaNs: exact bits
            format!("f64::from_bits(0x{:016x})", v.to_bits())
        } else {
            format!("{v:?}_f64") // Rust {:?} round-trips f64 exactly
        }
    }

    // expression printer; `top` avoids self-reference when printing the
    // defining expression of a bound node
    fn expr(t: &Term, id: u32, bound: &[bool], var: &dyn Fn(u32) -> String, top: bool) -> String {
        if !top && bound[id as usize] {
            return format!("t{id}");
        }
        let n = t.node(id);
        let e = |k: u32| expr(t, k, bound, var, false);
        match n.op {
            Op::Const => lit(t.consts[n.a as usize]),
            Op::Var => var(n.a),
            Op::Add => format!("({} + {})", e(n.a), e(n.b)),
            Op::Sub => format!("({} - {})", e(n.a), e(n.b)),
            Op::Mul => format!("({} * {})", e(n.a), e(n.b)),
            Op::Div => format!("({} / {})", e(n.a), e(n.b)),
            Op::Neg => format!("(-{})", e(n.a)),
            Op::Abs => format!("{}.abs()", e(n.a)),
            Op::Sqrt => format!("{}.sqrt()", e(n.a)),
            Op::Floor => format!("{}.floor()", e(n.a)),
            Op::Ceil => format!("{}.ceil()", e(n.a)),
            Op::Sin => format!("{}.sin()", e(n.a)),
            Op::Cos => format!("{}.cos()", e(n.a)),
            Op::Tan => format!("{}.tan()", e(n.a)),
            Op::Exp => format!("{}.exp()", e(n.a)),
            Op::Exp2 => format!("{}.exp2()", e(n.a)),
            Op::Ln => format!("{}.ln()", e(n.a)),
            Op::Pow => format!("{}.powf({})", e(n.a), e(n.b)),
            Op::Min => format!("{}.min({})", e(n.a), e(n.b)),
            Op::Max => format!("{}.max({})", e(n.a), e(n.b)),
            Op::Fma => format!("{}.mul_add({}, {})", e(n.a), e(n.b), e(n.c)),
            // comparison as a VALUE: the interpreter's own definition
            Op::Lt => format!("(({} < {}) as u8 as f64)", e(n.a), e(n.b)),
            Op::Gt => format!("(({} > {}) as u8 as f64)", e(n.a), e(n.b)),
            Op::Le => format!("(({} <= {}) as u8 as f64)", e(n.a), e(n.b)),
            Op::Ge => format!("(({} >= {}) as u8 as f64)", e(n.a), e(n.b)),
            Op::Eq => format!("(({} == {}) as u8 as f64)", e(n.a), e(n.b)),
            Op::Ne => format!("(({} != {}) as u8 as f64)", e(n.a), e(n.b)),
            // Σ v1.2 — the loop shape the extractor round-trips exactly.
            // v1.2 forbids nested folds, so the canonical names __acc/__i
            // cannot collide (each fold is its own block scope).
            Op::Acc => "__acc".to_string(),
            Op::Elem => format!("s{}[__i]", n.a),
            Op::Len => format!("(s{}.len() as f64)", n.a),
            Op::Fold => {
                let init = e(n.a);
                if t.seq_count() == 0 {
                    // ABI: no sequences ⇒ L = 0 ⇒ fold ≡ init
                    return format!("({init})");
                }
                let body = expr(t, n.b, bound, var, true);
                format!(
                    "({{ let mut __acc = {init};                        for __i in 0..s0.len() {{ __acc = {body}; }} __acc }})"
                )
            }
            Op::Select => {
                let cnode = t.node(n.a);
                let cond = if !bound[n.a as usize] {
                    match cnode.op {
                        Op::Lt => format!("{} < {}", e(cnode.a), e(cnode.b)),
                        Op::Gt => format!("{} > {}", e(cnode.a), e(cnode.b)),
                        Op::Le => format!("{} <= {}", e(cnode.a), e(cnode.b)),
                        Op::Ge => format!("{} >= {}", e(cnode.a), e(cnode.b)),
                        Op::Eq => format!("{} == {}", e(cnode.a), e(cnode.b)),
                        Op::Ne => format!("{} != {}", e(cnode.a), e(cnode.b)),
                        _ => format!("({}) != 0.0", e(n.a)),
                    }
                } else {
                    format!("({}) != 0.0", e(n.a))
                };
                format!("(if {cond} {{ {} }} else {{ {} }})", e(n.b), e(n.c))
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "// AUTO-GENERATED by dge emit — do not hand-edit.");
    let _ = writeln!(out, "// Re-run the pipeline instead; hand edits VOID the certificate.");
    match cert {
        Some(c) => {
            let _ = writeln!(out, "/// CERTIFIED: {}", c.claim());
            if !c.rule_trace.is_empty() {
                let _ = writeln!(out, "/// rules applied: {}", c.rule_trace.join(", "));
            }
            let _ = writeln!(out, "/// env: {} fma={} avx={} libm={}",
                c.env.target_triple, c.env.fma, c.env.avx, c.env.libm);
        }
        None => {
            let _ = writeln!(out, "/// UNCERTIFIED emission — gate before use.");
        }
    }
    let seq_params: Vec<String> =
        (0..t.seq_count()).map(|k| format!("s{k}: &[f64]")).collect();
    let _ = writeln!(out, "#[allow(unused_parens, clippy::all)]");
    if scalars {
        let mut params: Vec<String> =
            (0..arity as u32).map(|i| format!("v{i}: f64")).collect();
        params.extend(seq_params);
        let _ = writeln!(out, "pub fn {fn_name}({}) -> f64 {{", params.join(", "));
    } else {
        let extra = if seq_params.is_empty() { String::new() }
            else { format!(", {}", seq_params.join(", ")) };
        let _ = writeln!(out, "pub fn {fn_name}(v: &[f64; {arity}]{extra}) -> f64 {{");
    }
    let v = |i: u32| var(i);
    for i in 0..n {
        if bound[i] {
            let _ = writeln!(out, "    let t{i} = {};", expr(t, i as u32, &bound, &v, true));
        }
    }
    let _ = writeln!(out, "    {}", expr(t, t.root, &bound, &v, true));
    let _ = writeln!(out, "}}");
    out
}

pub fn run(args: &[String]) {
    let Some(file) = args.first() else {
        eprintln!("usage: dge emit <t.sexpr> [--name <fn>] [--out <file.rs>]");
        return;
    };
    let name = args.iter().position(|a| a == "--name")
        .and_then(|i| args.get(i + 1)).cloned()
        .unwrap_or_else(|| "emitted_fn".into());
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {file}: {e}"); return; }
    };
    let t = match term::sexpr::parse(src.trim()) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse: {e:?}"); return; }
    };
    let code = emit_rust(&t, &name, None);
    print!("{code}");
    if let Some(i) = args.iter().position(|a| a == "--out") {
        if let Some(out) = args.get(i + 1) {
            std::fs::write(out, &code).ok();
            eprintln!("-> {out}");
        }
    }
    eprintln!("NOTE: emission is a lowering — run the emission gate before shipping.");
}
