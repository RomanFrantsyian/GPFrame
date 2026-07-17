//! [[.]] — the DEFINITIONAL semantics. TRUSTED BASE (v2.1 §7). Keep small.
//!
//! Everything else in the workspace — egg extraction, GP candidates, the JIT,
//! the extractor, the emitter — is *refuted against this file*, never against
//! each other.
//!
//! Σ v1.2 adds `fold(init, body)` over K parallel same-length sequences:
//!   let L = shared length of `seqs` (0 sequences ⇒ L = 0)
//!   acc := [[init]]                                  (outer environment)
//!   for i in 0..L:
//!       acc := [[body]] with Acc ↦ acc, Elem(k) ↦ seqs[k][i]
//!   result := acc                                    (L = 0 ⇒ init)
//! Body-OWNED nodes (Term::fold_owners) are re-evaluated per iteration;
//! loop-invariant shared nodes are read from the outer pass — hoisting is
//! sound by purity (O5) and is the semantics, not an optimization.
//!
//! Divergence/panic (⊥ of D1) cannot arise from Σ itself: every op is total
//! on f64 and every fold is bounded by its runtime L. Panics below are
//! caller bugs (env too narrow, seqs of unequal length, ill-formed binders),
//! outside D1's semantic domain.

use crate::ast::Term;
use crate::sig::Op;

/// Scalar environment: one f64 per free `Var` index.
pub type Env = [f64];

/// Non-leaf, non-fold op application. One screen; the whole of Σ's
/// computational content lives here.
#[inline]
fn apply(op: Op, x: f64, y: f64, z: f64) -> f64 {
    match op {
        Op::Neg => -x,
        Op::Abs => x.abs(),
        Op::Sqrt => x.sqrt(),
        Op::Floor => x.floor(),
        Op::Ceil => x.ceil(),
        Op::Sin => x.sin(),
        Op::Cos => x.cos(),
        Op::Tan => x.tan(),
        Op::Exp => x.exp(),
        Op::Exp2 => x.exp2(),
        Op::Ln => x.ln(),
        Op::Add => x + y,
        Op::Sub => x - y,
        Op::Mul => x * y,
        Op::Div => x / y,
        Op::Min => x.min(y),
        Op::Max => x.max(y),
        Op::Pow => x.powf(y),
        // Σ v1.1 comparisons — semantics ARE Rust's operators (NaN ⇒ 0.0)
        Op::Lt => (x < y) as u8 as f64,
        Op::Eq => (x == y) as u8 as f64,
        Op::Ne => (x != y) as u8 as f64,
        Op::Gt => (x > y) as u8 as f64,
        Op::Le => (x <= y) as u8 as f64,
        Op::Ge => (x >= y) as u8 as f64,
        Op::Fma => x.mul_add(y, z),
        Op::Select => if x != 0.0 { y } else { z },
        Op::Const | Op::Var | Op::Acc | Op::Elem | Op::Len | Op::Fold => {
            unreachable!("leaves and fold are handled by the evaluator")
        }
    }
}

/// Evaluate a sequence-free term. Panics on env narrower than `t.arity()`
/// or if the term contains fold/seq constructs (use `eval_with_seqs`).
pub fn eval(t: &Term, env: &Env) -> f64 {
    if t.has_fold() || t.seq_count() > 0 {
        return eval_with_seqs(t, env, &[]);
    }
    values(t, env)[t.root as usize]
}

/// Σ v1.2 entry: evaluate with K parallel sequences (all the same length).
pub fn eval_with_seqs(t: &Term, env: &Env, seqs: &[&[f64]]) -> f64 {
    values_seq(t, env, seqs)[t.root as usize]
}

fn values(t: &Term, env: &Env) -> Vec<f64> {
    let mut val: Vec<f64> = Vec::with_capacity(t.nodes.len());
    for n in &t.nodes {
        let v = match n.op {
            Op::Const => t.consts[n.a as usize],
            Op::Var => env[n.a as usize],
            op => {
                let ar = op.arity();
                let x = if ar >= 1 { val[n.a as usize] } else { 0.0 };
                let y = if ar >= 2 { val[n.b as usize] } else { 0.0 };
                let z = if ar >= 3 { val[n.c as usize] } else { 0.0 };
                apply(op, x, y, z)
            }
        };
        val.push(v);
    }
    val
}

fn values_seq(t: &Term, env: &Env, seqs: &[&[f64]]) -> Vec<f64> {
    if !t.has_fold() && t.seq_count() == 0 {
        return values(t, env);
    }
    assert!(t.seq_count() <= seqs.len(), "term needs {} sequences", t.seq_count());
    let len = seqs.first().map(|s| s.len()).unwrap_or(0);
    assert!(seqs.iter().all(|s| s.len() == len), "parallel sequences must share length");

    let owner = t.fold_owners().expect("ill-formed fold binding structure");
    // owned node indices per fold, in (already topological) index order
    let mut owned: Vec<Vec<usize>> = vec![Vec::new(); t.len()];
    for (i, o) in owner.iter().enumerate() {
        if let Some(f) = o {
            owned[*f as usize].push(i);
        }
    }

    let n = t.len();
    let mut val = vec![0.0f64; n];
    for i in 0..n {
        if owner[i].is_some() {
            continue; // body node: evaluated per-iteration below
        }
        let node = &t.nodes[i];
        val[i] = match node.op {
            Op::Const => t.consts[node.a as usize],
            Op::Var => env[node.a as usize],
            // graceful 0.0 when seqs are absent = the eval_traced L=0
            // convention (init path only); with seqs present it is exact
            Op::Len => seqs.get(node.a as usize).map_or(0.0, |s| s.len() as f64),
            Op::Acc | Op::Elem => unreachable!("binder outside a body (validated)"),
            Op::Fold => {
                let mut acc = val[node.a as usize];
                for it in 0..len {
                    for &j in &owned[i] {
                        let bn = &t.nodes[j];
                        val[j] = match bn.op {
                            Op::Const => t.consts[bn.a as usize],
                            Op::Var => env[bn.a as usize],
                            Op::Acc => acc,
                            Op::Elem => seqs[bn.a as usize][it],
                            Op::Len =>
                                seqs.get(bn.a as usize).map_or(0.0, |s| s.len() as f64),
                            op => {
                                let ar = op.arity();
                                let x = if ar >= 1 { val[bn.a as usize] } else { 0.0 };
                                let y = if ar >= 2 { val[bn.b as usize] } else { 0.0 };
                                let z = if ar >= 3 { val[bn.c as usize] } else { 0.0 };
                                apply(op, x, y, z)
                            }
                        };
                    }
                    acc = val[node.b as usize];
                }
                acc
            }
            op => {
                let ar = op.arity();
                let x = if ar >= 1 { val[node.a as usize] } else { 0.0 };
                let y = if ar >= 2 { val[node.b as usize] } else { 0.0 };
                let z = if ar >= 3 { val[node.c as usize] } else { 0.0 };
                apply(op, x, y, z)
            }
        };
    }
    val
}

/// [R4 hook] Eval + coverage: `used[j]` = node j's value was CONSUMED on the
/// demand path to the root (Select prunes the untaken branch). For fold
/// terms (evaluated with zero sequences here): the init path is covered,
/// the body is covered only if it executed — with L = 0 it did not.
pub fn eval_traced(t: &Term, env: &Env) -> (f64, Vec<bool>) {
    let val = values_seq(t, env, &[]);
    let mut used = vec![false; t.nodes.len()];
    let mut stack = vec![t.root];
    while let Some(id) = stack.pop() {
        if used[id as usize] {
            continue;
        }
        used[id as usize] = true;
        let n = t.node(id);
        match n.op {
            Op::Const | Op::Var | Op::Acc | Op::Elem | Op::Len => {}
            Op::Select => {
                stack.push(n.a); // condition always demanded
                stack.push(if val[n.a as usize] != 0.0 { n.b } else { n.c });
            }
            Op::Fold => stack.push(n.a), // L = 0 here: init only
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
