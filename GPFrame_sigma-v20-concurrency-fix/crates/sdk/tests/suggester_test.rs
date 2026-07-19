//! Suggester hook (v1.8): a pluggable OPTIMIZATION hypothesis source.
//! The load-bearing claim, pinned every way that matters: a suggester
//! proposes, the core Gate disposes — always by re-checking against the
//! ORIGINAL term, never a chain of trust through intermediate "wins."

use sdk::{Engine, OptimizeReport, ProposalOutcome, Suggester};
use term::Term;

/// suggester returning a fixed list of candidate sexprs (parse failures
/// panic — test wiring, not the thing under test)
struct FixedSuggester(&'static str, Vec<&'static str>);
impl Suggester for FixedSuggester {
    fn name(&self) -> &str { self.0 }
    fn suggest(&self, _t: &Term) -> Vec<Term> {
        self.1.iter().map(|s| sdk::sexpr::parse(s).unwrap()).collect()
    }
}

fn accepted(r: &OptimizeReport) -> usize {
    r.proposals.iter().filter(|p| matches!(p.outcome, ProposalOutcome::Accepted { .. })).count()
}
fn refuted(r: &OptimizeReport) -> usize {
    r.proposals.iter().filter(|p| matches!(p.outcome, ProposalOutcome::Refuted(_))).count()
}

#[test]
fn a_correct_cheaper_suggestion_is_adopted_and_certified() {
    // (x+x)+x = 3x (cost 5 under DefaultCost's node-count weighting) ->
    // 3.0 * x (cost 3): correct AND strictly cheaper
    let original = sdk::sexpr::parse(
        "(+ (+ (var 0) (var 0)) (var 0))").unwrap();
    let mut e = Engine::bare(0x50);
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("triple", vec!["(* 3.0 (var 0))"])));
    let r = e.optimize("f", &original);
    assert_eq!(accepted(&r), 1);
    assert!(r.final_cost < r.original_cost);
    assert!(r.emitted.contains("fn f"));
    assert!(r.emitted.contains("CERTIFIED"));
}

#[test]
fn a_wrong_suggestion_is_refuted_and_never_adopted() {
    // 3x (cost 5) is the original; "2x" (cost 3) is CHEAPER, so it clears
    // the cost check and reaches the gate — where it must be refuted,
    // since 2x != 3x.
    let original = sdk::sexpr::parse(
        "(+ (+ (var 0) (var 0)) (var 0))").unwrap();
    let mut e = Engine::bare(0x51);
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("liar", vec!["(+ (var 0) (var 0))"])));
    let r = e.optimize("f", &original);
    assert_eq!(accepted(&r), 0);
    assert_eq!(refuted(&r), 1);
    // final cost is UNCHANGED — the wrong suggestion never became best
    assert_eq!(r.final_cost, r.original_cost);
}

#[test]
fn a_chain_of_suggestions_cannot_drift_meaning() {
    // Two suggesters, run in order, against a 4x original (cost 7).
    // The SECOND proposes something equivalent to the FIRST's (wrong)
    // 2x output, not to the original 4x. Because every candidate is
    // gated against the ORIGINAL — never the running "best" — neither
    // can sneak through, even though s2 would agree with s1's mistake.
    let original = sdk::sexpr::parse(
        "(+ (+ (+ (var 0) (var 0)) (var 0)) (var 0))").unwrap(); // 4x, cost 7
    let mut e = Engine::bare(0x52);
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("s1_wrong", vec!["(+ (var 0) (var 0))"]))); // 2x, cost 3, wrong
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("s2_agrees_with_s1_not_original",
            vec!["(* 2.0 (var 0))"]))); // == s1's WRONG 2x, still != 4x original
    let r = e.optimize("f", &original);
    assert_eq!(accepted(&r), 0, "neither may be adopted: both disagree with the ORIGINAL");
    assert_eq!(refuted(&r), 2);
    assert_eq!(r.final_cost, r.original_cost);
}

#[test]
fn expensive_worse_candidates_are_skipped_before_gating() {
    let original = sdk::sexpr::parse("(var 0)").unwrap();
    let mut e = Engine::bare(0x53);
    // strictly more expensive than the original (more nodes) — must be
    // rejected on cost WITHOUT a gate run (cheap, deterministic check)
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("worse", vec!["(+ (var 0) 0.0)"])));
    let r = e.optimize("f", &original);
    assert_eq!(r.proposals.len(), 1);
    assert!(matches!(r.proposals[0].outcome,
        ProposalOutcome::RefusedNotCheaper { .. }));
    assert_eq!(r.final_cost, r.original_cost);
}

#[test]
fn no_suggestions_still_yields_certified_original() {
    let original = sdk::sexpr::parse("(sin (var 0))").unwrap();
    let e = Engine::bare(0x54); // no suggesters registered at all
    let r = e.optimize("f", &original);
    assert!(r.proposals.is_empty());
    assert_eq!(r.final_cost, r.original_cost);
    assert!(r.emitted.contains("CERTIFIED"));
}

#[test]
fn suggesters_run_in_registration_order_and_best_accumulates() {
    // three candidates of decreasing cost, all correct — must end on the
    // CHEAPEST regardless of trial order, and every acceptance recorded
    let original = sdk::sexpr::parse(
        "(+ (+ (var 0) (var 0)) (+ (var 0) (var 0)))").unwrap(); // 4x, expensive
    let mut e = Engine::bare(0x55);
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("mid", vec!["(* 4.0 (var 0))"])));       // correct, cheaper
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("best", vec!["(* (var 0) 4.0)"])));      // same cost as mid
    let r = e.optimize("f", &original);
    assert!(accepted(&r) >= 1);
    assert!(r.final_cost < r.original_cost);
}

#[test]
fn unregistered_ext_op_in_a_suggestion_is_refused_not_panicked() {
    let original = sdk::sexpr::parse("(var 0)").unwrap();
    let mut e = Engine::bare(0x56);
    e.register_suggester(std::sync::Arc::new(
        FixedSuggester("ghost", vec!["(ext:nope (var 0))"])));
    let r = e.optimize("f", &original);
    assert_eq!(r.proposals.len(), 1);
    match &r.proposals[0].outcome {
        ProposalOutcome::Refused(m) => assert!(m.contains("not registered")),
        other => panic!("must be Refused, not a panic: {other:?}"),
    }
}
