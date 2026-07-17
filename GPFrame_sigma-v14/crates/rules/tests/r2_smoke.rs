//! R2 end-to-end: O1 discharge via Z3, the −0.0 trap caught as SAT,
//! Tier-A refactor with certificate, eps-mode Tier-B routing,
//! and the entry-condition (no rule ships unproved) enforced.
use harness::metric::Metric;
use harness::{Gate, Tier};
use rules::extract::SaturationLimits;
use rules::smt::{discharge_all, SmtBackend, SmtVerdict, Z3Cli};
use rules::{refactor, RefactorError};
use term::sexpr::parse;

fn artifact_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("dge_o1_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn o1_discharge_seed_rules_unsat() {
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    let dir = artifact_dir("seed");
    let mut z3 = Z3Cli::new(&dir);
    let (proved, rejected, unknown) = discharge_all(&rules::r_dec::table(), &mut z3);
    assert!(rejected.is_empty(), "sound seed rules refuted: {rejected:?}");
    assert!(unknown.is_empty(), "seed rules must be decidable: {unknown:?}");
    assert_eq!(proved.len(), rules::r_dec::table().len());
    // artifacts on disk, verifiable at load time
    for name in &proved {
        assert!(rules::smt::artifact_ok(&dir, name), "artifact missing for {name}");
    }
}

#[test]
fn o1_catches_the_minus_zero_trap() {
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    // The classic: x + 0.0 -> x is UNSOUND over f64 (x = −0.0 ⇒ +0.0 ≠ −0.0).
    // The whole point of O1 is that this NEVER reaches the rule table.
    let mut z3 = Z3Cli::new(artifact_dir("trap"));
    match z3.check_rule_inequiv("add-zero-TRAP", "(+ ?a 0.0)", "?a") {
        SmtVerdict::SatRefuted { .. } => {} // counterexample exists: −0.0
        SmtVerdict::UnsatProved { .. } => panic!("SOUNDNESS HOLE: add-zero proved"),
        SmtVerdict::Unknown => panic!("should be decidable"),
    }
    // And the sound variant IS provable: x - 0.0 -> x (−0 − +0 = −0 ✓).
    match z3.check_rule_inequiv("sub-zero", "(- ?a 0.0)", "?a") {
        SmtVerdict::UnsatProved { .. } => {}
        _ => panic!("sub-zero should be provable"),
    }
}

#[test]
fn transcendentals_route_to_unknown() {
    let mut z3 = Z3Cli::new(artifact_dir("t2"));
    // T2: no decidable theory ⇒ encoder refuses ⇒ Unknown (Tier B / triage).
    match z3.check_rule_inequiv("sin-sq", "(* (sin ?a) (sin ?a))", "?a") {
        SmtVerdict::Unknown => {}
        _ => panic!("transcendental rule must be Unknown"),
    }
}

#[test]
fn refactor_tier_a_with_certificate() {
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    let dir = artifact_dir("tier_a");
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));

    // (neg (neg (* (var 0) 1.0)))  ——→  (var 0)
    let p = parse("(neg (neg (* (var 0) 1.0)))").unwrap();
    let gate = Gate::default_dial(11);
    let out = refactor(&p, false, &gate, &dir, &SaturationLimits::default()).unwrap();

    assert_eq!(term::sexpr::print(out.verified.term()), "(var 0)");
    assert!(out.cost_after < out.cost_before, "O4: cost must not rise");
    match &out.verified.certificate().tier {
        Tier::A { smt_artifacts } => assert!(!smt_artifacts.is_empty()),
        _ => panic!("Dec-only refactor must be Tier A"),
    }
    assert!(out.verified.certificate().claim().contains("PROVED"));
    assert!(out.rule_trace.iter().any(|r| r == "neg-neg"));
    assert!(out.rule_trace.iter().any(|r| r == "mul-one"));

    // Defense in depth: the gate agrees with the proof.
    match gate.promote(out.verified.term().clone(), &p) {
        harness::GateOutcome::Promoted(_) => {}
        harness::GateOutcome::Refuted(ce) =>
            panic!("Tier A result refuted at {:?} — rule soundness bug!", ce.minimal_env),
    }
}

#[test]
fn refactor_eps_mode_routes_tier_b() {
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    let dir = artifact_dir("tier_b");
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));

    // (+ (* (var 0) (var 1)) 2.0) — fma-contract~ fires in eps mode; the
    // result differs by rounding ⇒ Tier B gate MANDATORY, mixed-eps metric
    // (see gate_refutes_the_one_ulp_fma_claim for why NOT one_ulp).
    let p = parse("(+ (* (var 0) (var 1)) 2.0)").unwrap();
    let mut gate = Gate::default_dial(13);
    gate.metric = Metric::fma_mixed();
    let out = refactor(&p, true, &gate, &dir, &SaturationLimits::default()).unwrap();

    assert!(term::sexpr::print(out.verified.term()).starts_with("(fma"),
        "expected fma extraction, got {}", term::sexpr::print(out.verified.term()));
    match &out.verified.certificate().tier {
        Tier::B { n, delta_min, .. } => { assert!(*n > 0 && *delta_min > 0.0); }
        Tier::A { .. } => panic!("approx rule fired ⇒ Tier B is mandatory"),
    }
}

#[test]
fn entry_condition_no_rule_ships_unproved() {
    // Empty artifact dir ⇒ refactor must REFUSE, not silently proceed.
    let dir = artifact_dir("empty");
    let p = parse("(* (var 0) 1.0)").unwrap();
    let gate = Gate::default_dial(17);
    match refactor(&p, false, &gate, &dir, &SaturationLimits::default()) {
        Err(RefactorError::UndischargedRule(_)) => {}
        Err(other) => panic!("expected UndischargedRule, got {other:?}"),
        Ok(_) => panic!("refactor proceeded with undischarged rules — entry condition hole"),
    }
}


#[test]
fn gate_refutes_the_one_ulp_fma_claim() {
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    // PINNED SPEC CORRECTION: v2.1 §1 claims fma contraction is "≤1 ULP".
    // The gate found a cancellation counterexample (a*b ≈ -c makes the
    // plain form's mul-rounding an O(1) RELATIVE error in the result).
    // This test keeps the refutation permanent.
    let dir = artifact_dir("ulp_claim");
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));
    let p = parse("(+ (* (var 0) (var 1)) 2.0)").unwrap();
    let mut gate = Gate::default_dial(13);
    gate.metric = Metric::one_ulp();
    match refactor(&p, true, &gate, &dir, &SaturationLimits::default()) {
        Err(RefactorError::GateRefuted { .. }) => {} // correct: claim is false
        Ok(_) => panic!("one_ulp accepted fma-contraction — the unsound claim resurfaced"),
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}
