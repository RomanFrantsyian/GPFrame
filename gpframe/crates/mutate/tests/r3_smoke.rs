//! R3 smoke: mutants are generated, differ from the original, and a decent
//! sample set kills the non-equivalent ones (MS machinery end-to-end).
use mutate::ops::all_mutants;
use term::{eval, sexpr::parse};

#[test]
fn mutants_enumerate_and_die() {
    let p = parse("(+ (* 2.0 (var 0)) 3.0)").unwrap(); // 2x + 3
    let mutants = all_mutants(&p);
    assert!(mutants.len() >= 10, "expected a healthy mutant set, got {}", mutants.len());

    // A small distinguishing sample set (plays the role of suite T).
    let suite: Vec<Vec<f64>> = vec![vec![0.0], vec![1.0], vec![-2.0], vec![10.5]];
    let killed = mutants.iter().filter(|m| {
        suite.iter().any(|e| {
            let a = eval(&m.term, e);
            let b = eval(&p, e);
            a.to_bits() != b.to_bits()
        })
    }).count();
    // Every first-order mutant of 2x+3 is non-equivalent and detectable on
    // this suite (no equivalent mutants exist in this catalogue for it).
    assert_eq!(killed, mutants.len(), "all mutants of 2x+3 should die on the suite");
}

#[test]
fn mutation_score_excludes_equivalent_mutants_via_smt() {
    use mutate::score::mutation_score;
    use rules::smt::Z3Cli;
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }

    // p = select(1.0, x, 5.0): the else-branch constant is DEAD code.
    // ConstSet mutants on the guard (1.0 -> 2.0/-1.0) keep it truthy —
    // EQUIVALENT mutants that only the SMT filter can exclude.
    let p = parse("(select 1.0 (var 0) 5.0)").unwrap();
    let suite: Vec<Vec<f64>> = vec![vec![0.0], vec![1.0], vec![-3.5], vec![7.0]];
    let dir = std::env::temp_dir().join(format!("dge_eqf_{}", std::process::id()));
    let mut smt = Z3Cli::new(&dir);
    let rep = mutation_score(&p, &suite, &mut smt);

    assert!(rep.equivalent_excluded >= 2,
        "guard-preserving ConstSet mutants must be SMT-excluded, got {}",
        rep.equivalent_excluded);
    assert!(rep.confirmed_non_equivalent > 0);
    assert!(rep.ms() > 0.9, "suite should kill the live mutants: {}", rep.render());
    println!("{}", rep.render());
}

#[test]
fn sat_model_parses_to_concrete_witness() {
    use rules::smt::{SmtBackend, SmtVerdict, Z3Cli};
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    let a = parse("(+ (var 0) 1.0)").unwrap();
    let b = parse("(+ (var 0) 2.0)").unwrap();
    let dir = std::env::temp_dir().join(format!("dge_model_{}", std::process::id()));
    match Z3Cli::new(&dir).check_term_inequiv(&a, &b) {
        SmtVerdict::SatRefuted { model } => {
            assert!(!model.is_empty(), "model must contain the witness env");
            // the witness actually distinguishes the terms in OUR interpreter
            let (va, vb) = (eval(&a, &model), eval(&b, &model));
            assert_ne!(va.to_bits(), vb.to_bits(),
                "parsed witness {model:?} fails to distinguish: {va} vs {vb}");
        }
        _ => panic!("x+1 vs x+2 must be SAT"),
    }
}

#[test]
fn pin_emitter_renders_suite() {
    use harness::CounterExample;
    let ce = CounterExample {
        minimal_env: vec![0.0, 1.0],
        minimal_seqs: vec![],
        original_env: vec![123.456, -9.0],
        candidate_val: 1.0,
        reference_val: 2.0,
    };
    let src = mutate::pin::emit_pinned_suite("my_kernel", "out.is_finite()", &[ce]);
    assert!(src.contains("fn pinned_ce_0()"));
    assert!(src.contains("vec![0.0, 1.0]"));
    assert!(src.contains("proptest!"));
    assert!(src.contains("my_kernel"));
}
