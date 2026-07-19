//! gentest + calib library-path tests.
use rules::smt::Z3Cli;

#[test]
fn gentest_grows_mutation_adequate_suite() {
    let p = term::sexpr::parse("(+ (* 2.0 (var 0)) 3.0)").unwrap();
    let mut z3;
    let smt: Option<&mut dyn rules::smt::SmtBackend> = if Z3Cli::available() {
        let dir = std::env::temp_dir().join(format!("dge_gt_{}", std::process::id()));
        z3 = Z3Cli::new(dir);
        Some(&mut z3)
    } else { None };
    let rep = cli::gentest::generate(&p, "lin", 0xde, 5_000, smt);
    assert!(rep.ms() >= 0.99, "suite must reach mutation adequacy: MS={}", rep.ms());
    assert!(!rep.suite.is_empty());
    // shrunk envs should be simple (T3): every pinned env is rank-small
    for (env, _) in &rep.suite {
        assert!(harness::shrink::rank(env) < 1 << 41, "unshrunk env {env:?}");
    }
    assert!(rep.emitted.contains("golden_0"));
    assert!(rep.emitted.contains("to_bits()"));
}

#[test]
fn calibrated_cost_loads_and_orders_sanely() {
    // measure a tiny table via the real path? too slow for a unit test —
    // instead test the loader contract on a handwritten file.
    let dir = std::env::temp_dir().join(format!("dge_ct_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cost_table.txt");
    std::fs::write(&path, "# env: test\n+ 1\nsin 8\npow 19\nbogus notanumber\nexp 0\n").unwrap();
    let c = rules::cost::CalibratedCost::load(&path).unwrap();
    use rules::cost::CostFn;
    use term::Op;
    assert_eq!(c.op_weight(Op::Add), 1);
    assert_eq!(c.op_weight(Op::Sin), 8);
    assert_eq!(c.op_weight(Op::Pow), 19);
    assert_eq!(c.op_weight(Op::Exp), 1, "w=0 must clamp to 1 (O4: w >= 1)");
    assert_eq!(c.op_weight(Op::Cos), 32, "missing ops fall back to default");
}

#[test]
fn env_fingerprint_is_stable_and_behavioral() {
    use harness::EnvFingerprint;
    let a = EnvFingerprint::capture();
    let b = EnvFingerprint::capture();
    assert!(a.matches(&b), "same process must fingerprint identically");
    assert!(a.libm.starts_with("behavioral:"), "libm pin must be behavioral: {}", a.libm);
    // certificate staleness works off the fingerprint
    let mut c = a.clone();
    c.libm = "behavioral:0000000000000000".into();
    assert!(!a.matches(&c));
}
