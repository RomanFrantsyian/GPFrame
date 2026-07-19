//! `sdk::plugins` — optimization features built purely on the Suggester hook.
//!
//! These tests drive the plugins through the REAL `Engine::optimize` path, so
//! every accepted rewrite has passed the sealed Gate (10^4 μ′ samples) and the
//! emitted result carries a certificate. On top of that they re-check, on
//! their own, that each rewrite is bitwise-equal to the original — the plugins
//! claim to be bit-exact, and that claim is verified independently of the Gate.

use sdk::plugins::{Peephole, StrengthReduce};
use sdk::{Engine, ProposalOutcome, Suggester};
use std::sync::Arc;
use term::{Op, Term, TermBuilder};

use harness::strategy::{MuPrime, Rng};

fn beq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Assert `cand` is bitwise-equal to `orig` over 10^4 μ′ samples.
fn assert_bit_equal(orig: &Term, cand: &Term) {
    assert_eq!(cand.arity(), orig.arity(), "arity changed");
    let mu = MuPrime::default_with_seed(0xF00D);
    let mut rng = Rng::new(0xF00D);
    for _ in 0..10_000u32 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, orig.arity().max(1), 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(
            beq(
                term::eval_with_seqs(cand, &env, &sl),
                term::eval_with_seqs(orig, &env, &sl)
            ),
            "rewrite differs from original at {env:?}"
        );
    }
}

/// `(3 * (2 + 4)) + (x * 1)` — a constant subtree and a `*1` identity.
fn folding_term() -> Term {
    let mut b = TermBuilder::new();
    let c2 = b.constant(2.0);
    let c4 = b.constant(4.0);
    let sum = b.binary(Op::Add, c2, c4);
    let c3 = b.constant(3.0);
    let prod = b.binary(Op::Mul, c3, sum);
    let x = b.var(0);
    let one = b.constant(1.0);
    let xid = b.binary(Op::Mul, x, one);
    let root = b.binary(Op::Add, prod, xid);
    b.finish(root)
}

#[test]
fn peephole_folds_and_simplifies_under_the_gate() {
    let t = folding_term();
    let before = t.nodes.len();

    // 1) the plugin in isolation: strictly smaller AND bit-exact
    let out = Peephole::default().suggest(&t);
    assert_eq!(out.len(), 1, "peephole should fire on this term");
    let simplified = &out[0];
    assert!(simplified.nodes.len() < before, "must shrink: {} -> {}", before, simplified.nodes.len());
    assert_bit_equal(&t, simplified);

    // 2) through the engine: gated, accepted, and the emission is certified
    let mut e = Engine::new(7);
    e.register_suggester(Arc::new(Peephole::default()));
    let rep = e.optimize("f", &t);
    assert!(rep.final_cost < rep.original_cost, "cost must drop: {} -> {}", rep.original_cost, rep.final_cost);
    assert!(
        rep.proposals.iter().any(|p| matches!(p.outcome, ProposalOutcome::Accepted { .. })),
        "engine should ACCEPT the peephole rewrite; proposals: {:?}",
        rep.proposals
    );
    assert!(rep.emitted.contains("CERTIFIED"), "emitted output must carry a certificate:\n{}", rep.emitted);
}

#[test]
fn peephole_reaches_a_fixed_point() {
    // neg(neg(neg(neg(x)))) → x needs several passes
    let mut b = TermBuilder::new();
    let mut cur = b.var(0);
    for _ in 0..4 {
        cur = b.unary(Op::Neg, cur);
    }
    let t = b.finish(cur);
    let out = Peephole::default().suggest(&t);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].nodes.len(), 1, "should collapse entirely to `x`");
    assert_eq!(out[0].node(out[0].root).op, Op::Var);
    assert_bit_equal(&t, &out[0]);
}

#[test]
fn peephole_proposes_nothing_when_already_minimal() {
    // x + y : nothing to fold or simplify
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let y = b.var(1);
    let root = b.binary(Op::Add, x, y);
    let t = b.finish(root);
    assert!(Peephole::default().suggest(&t).is_empty(), "no rewrite for an already-minimal term");
}

/// A cost model that prices division above multiplication (as real hardware
/// does), so the pow-2 strength reduction is a genuine improvement.
struct DivHeavy;
impl rules::cost::CostFn for DivHeavy {
    fn op_weight(&self, op: Op) -> u64 {
        if op == Op::Div {
            4
        } else {
            1
        }
    }
}

#[test]
fn strength_reduce_is_bit_exact_and_cost_gated() {
    // x / 8.0  →  x * 0.125
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let c8 = b.constant(8.0);
    let root = b.binary(Op::Div, x, c8);
    let t = b.finish(root);

    // plugin in isolation: fires, stays bit-exact, and is now a Mul
    let out = StrengthReduce.suggest(&t);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].node(out[0].root).op, Op::Mul, "div replaced by mul");
    assert_bit_equal(&t, &out[0]);

    // under the DEFAULT cost (div == mul) it is correctly NOT accepted —
    // soundness never depended on cost, only which equal term wins (L2).
    let mut e = Engine::new(3);
    e.register_suggester(Arc::new(StrengthReduce));
    let def = e.optimize("f", &t);
    assert_eq!(def.final_cost, def.original_cost, "no win under equal-priced ops");
    assert!(!def.proposals.iter().any(|p| matches!(p.outcome, ProposalOutcome::Accepted { .. })));

    // under a div-heavy cost the SAME rewrite is accepted and certified
    let heavy = e.optimize_with_cost("f", &t, &DivHeavy);
    assert!(heavy.final_cost < heavy.original_cost, "div-heavy cost should reward the reduction");
    assert!(heavy.proposals.iter().any(|p| matches!(p.outcome, ProposalOutcome::Accepted { .. })));
    assert!(heavy.emitted.contains("CERTIFIED"));
}

#[test]
fn strength_reduce_ignores_non_pow2() {
    // x / 3.0 : 1/3 is not exact, so no rewrite is proposed
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let c3 = b.constant(3.0);
    let root = b.binary(Op::Div, x, c3);
    let t = b.finish(root);
    assert!(StrengthReduce.suggest(&t).is_empty(), "must not touch non-power-of-two divisors");
}

/// SOUNDNESS PIN: the Suggester hook grants no authority. A suggester that
/// proposes a non-equivalent term is REFUTED by the Gate, never emitted.
struct Liar;
impl Suggester for Liar {
    fn name(&self) -> &str {
        "liar"
    }
    fn suggest(&self, t: &Term) -> Vec<Term> {
        // propose `x + 1` for a same-arity term: cheaper-looking, wrong
        let mut b = TermBuilder::new();
        let x = b.var(0);
        let one = b.constant(1.0);
        let root = b.binary(Op::Add, x, one);
        let _ = t;
        vec![b.finish(root)]
    }
}

#[test]
fn lying_suggester_is_refuted_not_shipped() {
    // original: x * x * x  (arity 1, cost 5). Liar proposes x + 1 (cost 3,
    // cheaper — so it REACHES the Gate instead of being dropped on cost).
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let x2 = b.var(0);
    let m1 = b.binary(Op::Mul, x, x2);
    let x3 = b.var(0);
    let root = b.binary(Op::Mul, m1, x3);
    let t = b.finish(root);

    let mut e = Engine::new(11);
    e.register_suggester(Arc::new(Liar));
    let rep = e.optimize("f", &t);
    assert!(
        rep.proposals.iter().any(|p| matches!(p.outcome, ProposalOutcome::Refuted(_))),
        "the wrong rewrite must be refuted: {:?}",
        rep.proposals
    );
    // and the emitted result is the ORIGINAL, unchanged and certified
    assert_eq!(rep.final_cost, rep.original_cost);
}
