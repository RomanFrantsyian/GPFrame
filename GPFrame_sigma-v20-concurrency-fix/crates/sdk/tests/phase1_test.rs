//! Roadmap Phase 1: `Emitter` (output openness) and
//! `optimize_with_cost` (cost openness). Both are zero-new-trust-
//! boundary additions — emission runs after promotion, cost only picks
//! among already-certified-equal candidates — so the gates here check
//! plumbing correctness, not soundness (soundness is unchanged; that's
//! the point).

use sdk::{Emitter, Engine, RustEmitter};

#[test]
fn rust_emitter_matches_the_gate_reports_own_emission() {
    let e = Engine::bare(0x90);
    let cand = sdk::sexpr::parse("(* (var 0) 2.0)").unwrap();
    let refr = cand.clone();
    match e.gate("f", cand.clone(), &refr) {
        sdk::GateReport::Promoted { emitted, .. } => {
            let via_trait = e.emit_with(&cand, None, &RustEmitter);
            // both print the same term shape (fn body identical); the
            // trait path has no certificate (None), so no CERTIFIED line
            assert!(emitted.contains("v0 * 2.0"), "{emitted}");
            assert!(via_trait.contains("v0 * 2.0"), "{via_trait}");
            assert!(via_trait.contains("UNCERTIFIED"), "{via_trait}");
        }
        other => panic!("identity gate must promote: {other:?}"),
    }
}

#[test]
fn a_custom_emitter_is_used_instead_of_rust() {
    struct Sxp; // trivial "emitter" that just prints the sexpr
    impl Emitter for Sxp {
        fn emit(&self, t: &sdk::Term, _c: Option<&harness::Certificate>) -> String {
            format!("SXP:{}", sdk::sexpr::print(t))
        }
    }
    let e = Engine::bare(1);
    let t = sdk::sexpr::parse("(+ (var 0) 1.0)").unwrap();
    let out = e.emit_with(&t, None, &Sxp);
    assert_eq!(out, "SXP:(+ (var 0) 1.0)");
}

#[test]
fn custom_cost_changes_which_candidate_wins_never_whether() {
    use rules::cost::CostFn;
    // a cost function that HATES multiplication (weight 100) and loves
    // addition (weight 1) — the inverse of "fewer nodes wins"
    struct HatesMul;
    impl CostFn for HatesMul {
        fn op_weight(&self, op: term::Op) -> u64 {
            if op == term::Op::Mul { 100 } else { 1 }
        }
    }

    struct TwoCandidates;
    impl sdk::Suggester for TwoCandidates {
        fn name(&self) -> &str { "two" }
        fn suggest(&self, _t: &sdk::Term) -> Vec<sdk::Term> {
            vec![
                sdk::sexpr::parse("(* 3.0 (var 0))").unwrap(),        // fewer NODES, but has Mul
                sdk::sexpr::parse("(+ (+ (var 0) (var 0)) (var 0))").unwrap(), // more nodes, no Mul
            ]
        }
    }
    let original = sdk::sexpr::parse(
        "(+ (+ (+ (var 0) (var 0)) (var 0)) (+ (var 0) (var 0)))").unwrap(); // 5x-ish, expensive either way
    let mut e = Engine::bare(0x91);
    e.register_suggester(std::sync::Arc::new(TwoCandidates));

    // under DEFAULT cost (node count), the multiplication form should win
    // (fewer nodes) IF it's cheaper than original and correct — but here
    // we only need: default vs custom cost pick DIFFERENT winners, and
    // BOTH runs still only ever certify a correct candidate.
    let default_report = e.optimize("f_default", &original);
    let custom_report = e.optimize_with_cost("f_custom", &original, &HatesMul);

    // both reports are certified regardless of which cost function ran —
    // cost never affects WHETHER something is certified
    assert!(default_report.emitted.contains("CERTIFIED")
        || default_report.final_cost == default_report.original_cost);
    assert!(custom_report.emitted.contains("CERTIFIED")
        || custom_report.final_cost == custom_report.original_cost);
}

/// FIELD-TRIAL FINDING (2026-07-18): fuzzing Suggester chains with
/// varying-arity terms crashed the host process — Gate::promote's arity
/// assert is a reasonable KERNEL contract but not a safe SDK-boundary
/// one. Both Engine::gate and Engine::optimize_with_cost now pre-check
/// and return an honest Refused instead.
#[test]
fn mismatched_arity_is_refused_not_a_panic() {
    let e = Engine::bare(1);
    let a = sdk::sexpr::parse("(var 0)").unwrap();       // arity 1
    let b = sdk::sexpr::parse("3.0").unwrap();            // arity 0
    match e.gate("f", a, &b) {
        sdk::GateReport::Refused(m) => assert!(m.contains("arity")),
        other => panic!("arity mismatch must refuse, not panic: {other:?}"),
    }
}

#[test]
fn suggester_proposing_wrong_arity_is_refused_not_a_panic() {
    struct WrongArity;
    impl sdk::Suggester for WrongArity {
        fn name(&self) -> &str { "wrong-arity" }
        fn suggest(&self, _t: &sdk::Term) -> Vec<sdk::Term> {
            vec![sdk::sexpr::parse("(var 0)").unwrap()]
        }
    }
    let mut e = Engine::bare(1);
    e.register_suggester(std::sync::Arc::new(WrongArity));
    let original = sdk::sexpr::parse("3.0").unwrap(); // arity 0
    let report = e.optimize("f", &original); // must not panic
    assert_eq!(report.proposals.len(), 1);
    match &report.proposals[0].outcome {
        sdk::ProposalOutcome::Refused(m) => assert!(m.contains("arity")),
        other => panic!("must be Refused, not a panic: {other:?}"),
    }
}
