//! Mutation operator set M — local syntactic edits, endofunctions on Term.
//! [D5, R3] Implemented via rebuild: every mutant is a fresh tree built
//! through `TermBuilder` (topological invariant preserved by construction).
//!
//! Catalogue:
//!   OpSwap    : + ↔ −, * ↔ /, min ↔ max, sin ↔ cos, ...
//!   ConstSet  : c → c+1, c−1, −c, 0.0, 1.0
//!   ChildSwap : (op a b) → (op b a) on NON-commutative binaries (−, /, pow)
//!   NegateNode: root → (neg root)

use term::{Op, Term, TermBuilder};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mutation {
    OpSwap { at: u32, to: Op },
    ConstSet { at: u32, value: f64 },
    ChildSwap { at: u32 },
    NegateNode { at: u32 },
}

pub struct Mutant {
    pub mutation: Mutation,
    pub term: Term,
}

fn op_swaps(op: Op) -> &'static [Op] {
    use Op::*;
    match op {
        Add => &[Sub, Mul],
        Sub => &[Add, Div],
        Mul => &[Div, Add],
        Div => &[Mul, Sub],
        Min => &[Max],
        Max => &[Min],
        Sin => &[Cos, Tan],
        Cos => &[Sin],
        Exp => &[Ln],
        Ln => &[Exp],
        Neg => &[Abs],
        Abs => &[Neg],
        Floor => &[Ceil],
        Ceil => &[Floor],
        _ => &[],
    }
}

const NON_COMMUTATIVE: &[Op] = &[Op::Sub, Op::Div, Op::Pow];

/// Rebuild `t` applying `m`. Pure tree reconstruction from the root.
pub fn apply(t: &Term, m: Mutation) -> Term {
    fn go(t: &Term, id: u32, m: Mutation, b: &mut TermBuilder) -> u32 {
        match m {
            Mutation::ConstSet { at, value } if at == id => return b.constant(value),
            Mutation::NegateNode { at } if at == id => {
                let inner = rebuild_node(t, id, m, b);
                return b.unary(Op::Neg, inner);
            }
            _ => {}
        }
        rebuild_node(t, id, m, b)
    }

    fn rebuild_node(t: &Term, id: u32, m: Mutation, b: &mut TermBuilder) -> u32 {
        let n = *t.node(id);
        let effective_op = match m {
            Mutation::OpSwap { at, to } if at == id => to,
            _ => n.op,
        };
        let swap_kids = matches!(m, Mutation::ChildSwap { at } if at == id);
        match n.op {
            Op::Const => b.constant(t.consts[n.a as usize]),
            Op::Var => b.var(n.a),
            Op::Acc => b.acc(),
            Op::Elem => b.elem(n.a),
            Op::Fold => {
                let init = go(t, n.a, m, b);
                let body = go(t, n.b, m, b);
                b.fold(init, body)
            }
            // Σ-ext: payload slots are NAME-table indices, not children —
            // the generic arm below would read them as child ids and
            // corrupt the term. Rebuild by name (op_swaps returns no swaps
            // for ext ops, so effective_op == n.op here).
            Op::Ext1 => {
                let a = go(t, n.a, m, b);
                b.ext1(&t.exts[n.b as usize], a)
            }
            Op::Ext2 => {
                let a = go(t, n.a, m, b);
                let k2 = go(t, n.b, m, b);
                let (a, k2) = if swap_kids { (k2, a) } else { (a, k2) };
                b.ext2(&t.exts[n.c as usize], a, k2)
            }
            _ => {
                let ar = n.op.arity();
                debug_assert_eq!(ar, effective_op.arity(), "OpSwap must preserve arity");
                let a = go(t, n.a, m, b);
                if ar == 1 { return b.unary(effective_op, a); }
                let k2 = go(t, n.b, m, b);
                let (a, k2) = if swap_kids { (k2, a) } else { (a, k2) };
                if ar == 2 { return b.binary(effective_op, a, k2); }
                let k3 = go(t, n.c, m, b);
                b.ternary(effective_op, a, k2, k3)
            }
        }
    }

    let mut b = TermBuilder::new();
    let root = go(t, t.root, m, &mut b);
    b.finish(root)
}

/// Enumerate all first-order mutants of `p`.
pub fn all_mutants(p: &Term) -> Vec<Mutant> {
    let mut out: Vec<Mutant> = Vec::new();
    let mut push = |m: Mutation| {
        let t = apply(p, m);
        if !t.structurally_eq(p) {
            out.push(Mutant { mutation: m, term: t });
        }
    };

    for (i, n) in p.nodes.iter().enumerate() {
        let id = i as u32;
        match n.op {
            Op::Const => {
                let c = p.consts[n.a as usize];
                for v in [c + 1.0, c - 1.0, -c, 0.0, 1.0] {
                    if v.to_bits() != c.to_bits() {
                        push(Mutation::ConstSet { at: id, value: v });
                    }
                }
            }
            Op::Var => {}
            op => {
                for &to in op_swaps(op) {
                    push(Mutation::OpSwap { at: id, to });
                }
                if NON_COMMUTATIVE.contains(&op) {
                    push(Mutation::ChildSwap { at: id });
                }
            }
        }
        if id == p.root {
            push(Mutation::NegateNode { at: id });
        }
    }
    out
}
