//! SDK-side optimization plugins.
//!
//! These are *features built entirely on the Suggester hook* — no core crate
//! is touched. Each plugin proposes candidate rewrites of a term; the sealed
//! Gate arbitrates every one of them against the ORIGINAL over 10^4 μ′
//! samples, and the cost function drops any that don't actually get cheaper.
//! So a plugin needs no trust: a wrong rewrite is refuted, a pointless one is
//! discarded. This is the whole reason the Suggester boundary exists — domain
//! and heuristic rewrite knowledge becomes usable without ever being believed.
//!
//! Everything here is deliberately BIT-EXACT (it preserves the IEEE-754
//! result, not just the real-number value), because the Gate is bitwise:
//!
//!   * constant folding reuses the real interpreter, so a folded constant is
//!     exactly what the runtime would have computed — never a re-derivation;
//!   * the algebraic identities (`x*1`, `1*x`, `x/1`, `neg(neg x)`) are the
//!     ones that hold for every f64 including −0.0, NaN, and ∞ (notably NOT
//!     `x+0.0`, which is −0.0-unsafe — the Gate would refute it, and we don't
//!     bother proposing it);
//!   * strength reduction only fires on exact powers of two, where `x / 2^k`
//!     and `x * 2^-k` are the same correctly-rounded scaling.
//!
//! Terms containing external ops (`Ext1`/`Ext2`) are left untouched: their
//! value can be caller-defined and non-foldable, and reconstructing their
//! payload indices isn't worth it here. That's a missed optimization, never a
//! wrong one.

use crate::Suggester;
use term::{Op, Term, TermBuilder};
use term::NodeId;

const MANTISSA_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;

fn has_ext(t: &Term) -> bool {
    t.nodes.iter().any(|n| matches!(n.op, Op::Ext1 | Op::Ext2))
}

/// The constant value of the subtree at `id`, if it is one — a bare `Const`.
fn const_at(t: &Term, id: NodeId) -> Option<f64> {
    let n = t.node(id);
    (n.op == Op::Const).then(|| t.consts[n.a as usize])
}

/// Evaluate `op` on already-known constant operands using the REAL
/// interpreter (a throwaway `op(Const..)` term), so the folded result is
/// bit-identical to what the compiled kernel would produce. This is what
/// makes folding sound for every op — Fma's single rounding, Pow via libm,
/// Rnd32's f32 round-trip — without re-implementing any of them.
fn fold_op(op: Op, args: &[f64]) -> f64 {
    let mut b = TermBuilder::new();
    let cs: Vec<NodeId> = args.iter().map(|&v| b.constant(v)).collect();
    let root = match op.arity() {
        1 => b.unary(op, cs[0]),
        2 => b.binary(op, cs[0], cs[1]),
        3 => b.ternary(op, cs[0], cs[1], cs[2]),
        _ => unreachable!("fold_op on an arity-0 op"),
    };
    term::eval_with_seqs(&b.finish(root), &[], &[])
}

/// Per-node "is this subtree a compile-time constant, and if so what value".
/// One forward pass is a valid post-order (children precede parents), so a
/// node is constant iff all its operands are. `Var`/`Acc`/`Elem`/`Len`/`Fold`
/// (and `Ext`) are never constant; anything built only from constants is.
fn const_values(t: &Term) -> Vec<Option<f64>> {
    let mut v: Vec<Option<f64>> = vec![None; t.nodes.len()];
    for id in 0..t.nodes.len() {
        let n = &t.nodes[id];
        v[id] = match n.op {
            Op::Const => Some(t.consts[n.a as usize]),
            Op::Var | Op::Acc | Op::Elem | Op::Len | Op::Fold | Op::Ext1 | Op::Ext2 => None,
            _ => {
                let kids = [n.a, n.b, n.c];
                let k = n.op.arity() as usize;
                let mut args = Vec::with_capacity(k);
                for &child in kids.iter().take(k) {
                    match v[child as usize] {
                        Some(x) => args.push(x),
                        None => break,
                    }
                }
                (args.len() == k).then(|| fold_op(n.op, &args))
            }
        };
    }
    v
}

/// Rebuild the subtree at `id` into `b`, applying constant folding and the
/// bit-exact identities. Only shrinking rewrites ever fire, so the result is
/// never larger than the input.
fn rebuild(src: &Term, id: NodeId, cv: &[Option<f64>], b: &mut TermBuilder) -> NodeId {
    // maximal constant subtree ⇒ collapse to a single constant
    if let Some(val) = cv[id as usize] {
        return b.constant(val);
    }
    let n = *src.node(id);
    match n.op {
        Op::Const => b.constant(src.consts[n.a as usize]),
        Op::Var => b.var(n.a),
        Op::Acc => b.acc(),
        Op::Elem => b.elem(n.a),
        Op::Len => b.len_of(n.a),
        Op::Neg => {
            // neg(neg x) → x  (exact for −0.0/NaN/∞)
            let child = src.node(n.a);
            if child.op == Op::Neg {
                return rebuild(src, child.a, cv, b);
            }
            let a = rebuild(src, n.a, cv, b);
            b.unary(Op::Neg, a)
        }
        Op::Mul => {
            // x*1 → x  and  1*x → x   (mult by 1.0 is the exact identity)
            if cv[n.b as usize] == Some(1.0) {
                return rebuild(src, n.a, cv, b);
            }
            if cv[n.a as usize] == Some(1.0) {
                return rebuild(src, n.b, cv, b);
            }
            let a = rebuild(src, n.a, cv, b);
            let c = rebuild(src, n.b, cv, b);
            b.binary(Op::Mul, a, c)
        }
        Op::Div => {
            // x/1 → x   (div by 1.0 is the exact identity)
            if cv[n.b as usize] == Some(1.0) {
                return rebuild(src, n.a, cv, b);
            }
            let a = rebuild(src, n.a, cv, b);
            let c = rebuild(src, n.b, cv, b);
            b.binary(Op::Div, a, c)
        }
        Op::Fold => {
            let init = rebuild(src, n.a, cv, b);
            let body = rebuild(src, n.b, cv, b);
            b.fold(init, body)
        }
        op if op.arity() == 1 => {
            let a = rebuild(src, n.a, cv, b);
            b.unary(op, a)
        }
        op if op.arity() == 2 => {
            let a = rebuild(src, n.a, cv, b);
            let c = rebuild(src, n.b, cv, b);
            b.binary(op, a, c)
        }
        op if op.arity() == 3 => {
            let a = rebuild(src, n.a, cv, b);
            let c = rebuild(src, n.b, cv, b);
            let d = rebuild(src, n.c, cv, b);
            b.ternary(op, a, c, d)
        }
        _ => unreachable!("Ext handled by has_ext guard"),
    }
}

/// Constant folding + bit-exact identity elimination, iterated to a fixed
/// point. Proposes the simplified term only when it is strictly smaller.
pub struct Peephole {
    max_passes: usize,
}

impl Default for Peephole {
    fn default() -> Self {
        Peephole { max_passes: 16 }
    }
}

impl Suggester for Peephole {
    fn name(&self) -> &str {
        "peephole"
    }
    fn suggest(&self, t: &Term) -> Vec<Term> {
        if has_ext(t) {
            return vec![];
        }
        let mut cur = t.clone();
        for _ in 0..self.max_passes {
            let cv = const_values(&cur);
            let mut b = TermBuilder::new();
            let root = rebuild(&cur, cur.root, &cv, &mut b);
            let next = b.finish(root);
            // every rewrite is non-growing, so equal size ⇒ fixed point
            if next.nodes.len() >= cur.nodes.len() {
                break;
            }
            cur = next;
        }
        if cur.nodes.len() < t.nodes.len() {
            vec![cur]
        } else {
            vec![]
        }
    }
}

/// True iff `c` is a normal, positive power of two whose reciprocal is also a
/// normal power of two — the exact range where `x / c == x * (1/c)` bitwise.
fn exact_pow2_recip(c: f64) -> Option<f64> {
    if !c.is_finite() || c <= 0.0 || (c.to_bits() & MANTISSA_MASK) != 0 {
        return None;
    }
    let r = 1.0 / c;
    (r.is_finite() && r != 0.0 && (r.to_bits() & MANTISSA_MASK) == 0 && r * c == 1.0).then_some(r)
}

/// Strength reduction: replace division by an exact power of two with
/// multiplication by its (exact) reciprocal. Bit-exact, and a win on any cost
/// model that prices `Div` above `Mul` (the default cost prices them equally,
/// so under it this is correctly a no-op — see the cost-hook test).
pub struct StrengthReduce;

fn reduce(src: &Term, id: NodeId, changed: &mut bool, b: &mut TermBuilder) -> NodeId {
    let n = *src.node(id);
    match n.op {
        Op::Const => b.constant(src.consts[n.a as usize]),
        Op::Var => b.var(n.a),
        Op::Acc => b.acc(),
        Op::Elem => b.elem(n.a),
        Op::Len => b.len_of(n.a),
        Op::Div => {
            if let Some(r) = const_at(src, n.b).and_then(exact_pow2_recip) {
                *changed = true;
                let a = reduce(src, n.a, changed, b);
                let rc = b.constant(r);
                return b.binary(Op::Mul, a, rc);
            }
            let a = reduce(src, n.a, changed, b);
            let c = reduce(src, n.b, changed, b);
            b.binary(Op::Div, a, c)
        }
        Op::Fold => {
            let init = reduce(src, n.a, changed, b);
            let body = reduce(src, n.b, changed, b);
            b.fold(init, body)
        }
        op if op.arity() == 1 => {
            let a = reduce(src, n.a, changed, b);
            b.unary(op, a)
        }
        op if op.arity() == 2 => {
            let a = reduce(src, n.a, changed, b);
            let c = reduce(src, n.b, changed, b);
            b.binary(op, a, c)
        }
        op if op.arity() == 3 => {
            let a = reduce(src, n.a, changed, b);
            let c = reduce(src, n.b, changed, b);
            let d = reduce(src, n.c, changed, b);
            b.ternary(op, a, c, d)
        }
        _ => unreachable!("Ext handled by has_ext guard"),
    }
}

impl Suggester for StrengthReduce {
    fn name(&self) -> &str {
        "strength-reduce-pow2"
    }
    fn suggest(&self, t: &Term) -> Vec<Term> {
        if has_ext(t) {
            return vec![];
        }
        let mut changed = false;
        let mut b = TermBuilder::new();
        let root = reduce(t, t.root, &mut changed, &mut b);
        if changed {
            vec![b.finish(root)]
        } else {
            vec![]
        }
    }
}
