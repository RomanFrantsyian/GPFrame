//! [[.]] — the DEFINITIONAL semantics. TRUSTED BASE (v2.1 §7). One screen.
//!
//! Everything else in the workspace — egg extraction, GP candidates, the JIT —
//! is *refuted against this function*, never against each other:
//! * O7: `jit(T)(e) bitwise== interp(T)(e)` on V_gate, per term, at install.
//! * T6: memo caches exactly this function's outputs.
//!
//! Iterative single pass (no recursion ⇒ no stack overflow on deep GP terms),
//! valid post-order by the arena topological invariant.
//!
//! Divergence/panic (⊥ of D1) cannot arise: every op below is total on f64
//! (div-by-zero → ±Inf/NaN per IEEE; that IS the semantics, not an error).

use crate::ast::Term;
use crate::sig::Op;

/// Environment: one f64 per free variable index.
pub type Env = [f64];

/// Evaluate `[[t]] env`. Panics only on env narrower than `t.arity()`
/// (a caller bug, outside D1's semantic domain).
pub fn eval(t: &Term, env: &Env) -> f64 {
    values(t, env)[t.root as usize]
}

/// [R4 hook] Eval + coverage: `used[j]` = node j's value was CONSUMED on the
/// demand path to the root (Select prunes the untaken branch). Trusted-base
/// delta reviewed alongside `eval`: values come from the same single pass;
/// only the demand marking is added.
pub fn eval_traced(t: &Term, env: &Env) -> (f64, Vec<bool>) {
    let val = values(t, env);
    let mut used = vec![false; t.nodes.len()];
    let mut stack = vec![t.root];
    while let Some(id) = stack.pop() {
        if used[id as usize] {
            continue;
        }
        used[id as usize] = true;
        let n = t.node(id);
        match n.op {
            Op::Const | Op::Var => {}
            Op::Select => {
                stack.push(n.a); // condition always demanded
                stack.push(if val[n.a as usize] != 0.0 { n.b } else { n.c });
            }
            _ => {
                let ar = n.op.arity();
                if ar >= 1 { stack.push(n.a); }
                if ar >= 2 { stack.push(n.b); }
                if ar >= 3 { stack.push(n.c); }
            }
        }
    }
    (val[t.root as usize], used)
}

fn values(t: &Term, env: &Env) -> Vec<f64> {
    let mut val: Vec<f64> = Vec::with_capacity(t.nodes.len());
    for n in &t.nodes {
        let v = match n.op {
            Op::Const => t.consts[n.a as usize],
            Op::Var => env[n.a as usize],

            Op::Neg => -val[n.a as usize],
            Op::Abs => val[n.a as usize].abs(),
            Op::Sqrt => val[n.a as usize].sqrt(),
            Op::Floor => val[n.a as usize].floor(),
            Op::Ceil => val[n.a as usize].ceil(),
            Op::Sin => val[n.a as usize].sin(),
            Op::Cos => val[n.a as usize].cos(),
            Op::Tan => val[n.a as usize].tan(),
            Op::Exp => val[n.a as usize].exp(),
            Op::Ln => val[n.a as usize].ln(),

            Op::Add => val[n.a as usize] + val[n.b as usize],
            Op::Sub => val[n.a as usize] - val[n.b as usize],
            Op::Mul => val[n.a as usize] * val[n.b as usize],
            Op::Div => val[n.a as usize] / val[n.b as usize],
            Op::Min => val[n.a as usize].min(val[n.b as usize]),
            Op::Max => val[n.a as usize].max(val[n.b as usize]),
            Op::Pow => val[n.a as usize].powf(val[n.b as usize]),

            Op::Fma => val[n.a as usize].mul_add(val[n.b as usize], val[n.c as usize]),
            Op::Select => {
                if val[n.a as usize] != 0.0 { val[n.b as usize] } else { val[n.c as usize] }
            }
        };
        val.push(v);
    }
    val
}
