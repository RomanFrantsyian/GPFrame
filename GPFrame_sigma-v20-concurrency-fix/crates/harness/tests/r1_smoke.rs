//! R1 smoke: the gate promotes true equivalents, refutes fakes with a
//! T3-minimal counterexample, and surfaces (n, alpha, delta_min).
use harness::{Gate, GateOutcome, Tier};
use term::{Op, TermBuilder};

fn x_plus_x() -> term::Term {
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let x2 = b.var(0);
    let r = b.binary(Op::Add, x, x2);
    b.finish(r)
}

fn two_x() -> term::Term {
    let mut b = TermBuilder::new();
    let two = b.constant(2.0);
    let x = b.var(0);
    let r = b.binary(Op::Mul, two, x);
    b.finish(r)
}

fn three_x() -> term::Term {
    let mut b = TermBuilder::new();
    let three = b.constant(3.0);
    let x = b.var(0);
    let r = b.binary(Op::Mul, three, x);
    b.finish(r)
}

#[test]
fn promotes_true_equivalence_with_quantified_cert() {
    let gate = Gate::default_dial(42);
    match gate.promote(two_x(), &x_plus_x()) {
        GateOutcome::Promoted(vt) => {
            let cert = vt.certificate();
            match &cert.tier {
                Tier::B { n, alpha, delta_min, .. } => {
                    assert_eq!(*n, 10_000);
                    assert!((delta_min - (1.0f64 / alpha).ln() / 1e4).abs() < 1e-12);
                }
                _ => panic!("expected Tier B"),
            }
            assert!(cert.claim().contains("confidence"));
        }
        GateOutcome::Refuted(ce) => panic!("false refutation: {ce:?}"),
    }
}

#[test]
fn refutes_inequivalence_with_minimal_ce() {
    let gate = Gate::default_dial(42);
    match gate.promote(three_x(), &x_plus_x()) {
        GateOutcome::Promoted(_) => panic!("false accept — soundness bug"),
        GateOutcome::Refuted(ce) => {
            // T3: shrinker should land on a very simple witness (rank-wise
            // nothing simpler than 1.0 distinguishes 3x from 2x; 0 agrees).
            assert_eq!(ce.minimal_env, vec![1.0]);
        }
    }
}

#[test]
fn derived_dial_matches_t4() {
    // pick (alpha, delta) => n >= ln(1/alpha)/delta
    let g = Gate::for_target(1e-3, 6.9e-4, 7);
    assert!(g.n >= 10_000 && g.n <= 10_100);
}
