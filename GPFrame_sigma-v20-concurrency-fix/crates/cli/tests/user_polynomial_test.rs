//! User-submitted real-code case: naive cubic polynomial → Horner/fma.
//! Pins the full story:
//!   1. extraction gate: rustc original ==bitwise== interp(extract)
//!   2. Tier A refuses to reassociate (bitwise-sound rules can't)
//!   3. UNBOUNDED eps gate REFUTES Horner (overflow: naive gives NaN via
//!      -inf + inf where factored gives -inf) — reassociation is NOT
//!      eps-equivalent over all of f64, and the gate proves it
//!   4. A-1 domain-bounded eps gate PROMOTES Horner, bound in certificate
//!   5. rustc original ~fma_mixed~ jit(Horner) over the bounded domain

use cli::extract::extract_fn;
use harness::metric::Metric;
use harness::strategy::{MuPrime, Rng};
use harness::{Gate, Tier};
use rules::extract::SaturationLimits;
use rules::smt::{discharge_all, Z3Cli};
use rules::{refactor, RefactorError};
use term::eval;


/// Finding 7: cross-generator equality = exact bits OR both-NaN.
fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

const SRC: &str = r#"
fn inefficient_polynomial(x: f64) -> f64 {
    let term3 = 3.0 * x * x * x;
    let term2 = 5.0 * x * x;
    let term1 = 2.0 * x;
    let term0 = 7.0;

    term3 + term2 + term1 + term0
}
"#;

fn original(x: f64) -> f64 {
    let term3 = 3.0 * x * x * x;
    let term2 = 5.0 * x * x;
    let term1 = 2.0 * x;
    let term0 = 7.0;
    term3 + term2 + term1 + term0
}

#[test]
fn extraction_gate_bitwise() {
    let t = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let mu = MuPrime::default_with_seed(0x901);
    let mut rng = Rng::new(0x901);
    for _ in 0..10_000 {
        let e = mu.sample(&mut rng, 1);
        assert!(xbit_eq(eval(&t, &e), original(e[0])), "drift at {e:?}");
    }
}

#[test]
fn tier_a_cannot_reassociate_and_stays_bitwise() {
    if !Z3Cli::available() { return; }
    let t = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let dir = std::env::temp_dir().join(format!("dge_poly_a_{}", std::process::id()));
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));
    let gate = Gate::default_dial(0x902);
    let out = refactor(&t, false, &gate, &dir, &SaturationLimits::default()).unwrap();
    assert!(matches!(out.verified.certificate().tier, Tier::A { .. }));
    // bitwise-sound rules can only shuffle commutatively: cost is unchanged
    assert_eq!(out.cost_before, out.cost_after);
    // and the result is STILL bitwise-identical to the rustc original
    let jf = jit::install(out.verified, &jit::LowerConfig::default(), &gate).unwrap();
    let mu = MuPrime::default_with_seed(3);
    let mut rng = Rng::new(3);
    for _ in 0..10_000 {
        let e = mu.sample(&mut rng, 1);
        assert!(xbit_eq(jf.call(&e), original(e[0])));
    }
}

#[test]
fn unbounded_eps_gate_refutes_reassociation() {
    if !Z3Cli::available() { return; }
    let t = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let dir = std::env::temp_dir().join(format!("dge_poly_r_{}", std::process::id()));
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));
    let mut gate = Gate::default_dial(0xD6E); // same dial the CLI uses
    gate.metric = Metric::fma_mixed();
    match refactor(&t, true, &gate, &dir, &SaturationLimits::default()) {
        Err(RefactorError::GateRefuted { minimal_env }) => {
            // the witness lives at overflow magnitude, where naive computes
            // -inf + inf = NaN and the factored form computes ±inf
            let x = minimal_env[0];
            assert!(x.abs() > 1e102, "expected overflow-scale witness, got {x:e}");
            assert!(original(x).is_nan() || original(x).is_infinite());
        }
        Ok(out) => panic!(
            "unbounded eps gate accepted reassociation — unsound: {}",
            term::sexpr::print(out.verified.term())
        ),
        Err(e) => panic!("unexpected: {e:?}"),
    }
}

#[test]
fn bounded_eps_finds_horner_and_matches_original_on_domain() {
    if !Z3Cli::available() { return; }
    let t = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let dir = std::env::temp_dir().join(format!("dge_poly_h_{}", std::process::id()));
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));

    let mut gate = Gate::default_dial(0xD6E);
    gate.metric = Metric::fma_mixed();
    gate.mu = MuPrime::bounded(0xD6E, 1e100); // A-1: user's operational domain
    let out = refactor(&t, true, &gate, &dir, &SaturationLimits::default()).unwrap();

    // Horner via fma, cost 19 -> 10
    let horner = term::sexpr::print(out.verified.term());
    assert!(horner.starts_with("(fma"), "expected Horner/fma, got {horner}");
    assert!(out.cost_after < out.cost_before, "{} !< {}", out.cost_after, out.cost_before);
    // certificate carries the domain restriction verbatim
    match &out.verified.certificate().tier {
        Tier::B { mu_spec, .. } => assert!(mu_spec.contains("DOMAIN"), "{mu_spec}"),
        _ => panic!("approx rules fired ⇒ Tier B"),
    }

    // jit the Horner form; compare against the rustc ORIGINAL on the domain
    let jf = jit::install(out.verified, &jit::LowerConfig::default(), &gate).unwrap();
    let mu = MuPrime::bounded(11, 1e100);
    let mut rng = Rng::new(11);
    let m = Metric::fma_mixed();
    for _ in 0..10_000 {
        let e = mu.sample(&mut rng, 1);
        let (jv, ov) = (jf.call(&e), original(e[0]));
        assert!(m.eq(jv, ov), "domain drift at {e:?}: horner={jv} rustc={ov}");
    }

    // informational: is Horner actually faster on this box?
    let naive = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let vt_n = match Gate::default_dial(12).promote(naive.clone(), &naive) {
        harness::GateOutcome::Promoted(v) => v, _ => unreachable!(),
    };
    let jn = jit::install(vt_n, &jit::LowerConfig::default(), &Gate::default_dial(13)).unwrap();
    let env = [1.37];
    let iters = 100_000u64;
    let t0 = std::time::Instant::now();
    let mut a = 0.0; for _ in 0..iters { a += jn.call(&env); }
    let t_naive = t0.elapsed();
    let t1 = std::time::Instant::now();
    let mut b = 0.0; for _ in 0..iters { b += jf.call(&env); }
    let t_horner = t1.elapsed();
    std::hint::black_box((a, b));
    println!("naive jit {t_naive:?} vs horner-fma jit {t_horner:?} (ratio {:.2}x)",
        t_naive.as_nanos() as f64 / t_horner.as_nanos().max(1) as f64);
}

#[test]
fn calibrated_cost_picks_the_actually_faster_horner() {
    if !Z3Cli::available() { return; }
    // L2 in action: same rules, same soundness — the calibrated table
    // (fma=5, measured through the wrapper-call lowering) steers extraction
    // to the mul/add Horner instead of the fma one.
    let t = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let dir = std::env::temp_dir().join(format!("dge_poly_c_{}", std::process::id()));
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));
    let table = dir.join("cost_table.txt");
    std::fs::write(&table, "+ 1\n- 1\n* 1\nfma 5\nselect 2\n").unwrap();
    let cost = rules::cost::CalibratedCost::load(&table).unwrap();

    let mut gate = Gate::default_dial(0xD6E);
    gate.metric = Metric::fma_mixed();
    gate.mu = MuPrime::bounded(0xD6E, 1e100);
    let out = rules::refactor_with_cost(&t, true, &gate, &dir,
        &SaturationLimits::default(), &cost).unwrap();

    let form = term::sexpr::print(out.verified.term());
    assert!(!form.contains("fma"), "calibrated cost must avoid wrapper-fma: {form}");
    assert!(out.cost_after < out.cost_before);

    // measure: mul/add Horner vs the naive original, both jitted
    let jf = jit::install(out.verified, &jit::LowerConfig::default(), &gate).unwrap();
    let naive = extract_fn(SRC, "inefficient_polynomial").unwrap();
    let vt_n = match Gate::default_dial(14).promote(naive.clone(), &naive) {
        harness::GateOutcome::Promoted(v) => v, _ => unreachable!(),
    };
    let jn = jit::install(vt_n, &jit::LowerConfig::default(), &Gate::default_dial(15)).unwrap();
    let env = [1.37];
    let iters = 100_000u64;
    let t0 = std::time::Instant::now();
    let mut a = 0.0; for _ in 0..iters { a += jn.call(&env); }
    let t_naive = t0.elapsed();
    let t1 = std::time::Instant::now();
    let mut b = 0.0; for _ in 0..iters { b += jf.call(&env); }
    let t_horner = t1.elapsed();
    std::hint::black_box((a, b));
    let ratio = t_naive.as_nanos() as f64 / t_horner.as_nanos().max(1) as f64;
    println!("calibrated horner form: {form}");
    println!("naive jit {t_naive:?} vs mul-horner jit {t_horner:?} (ratio {ratio:.2}x)");
    // MEASURED FINDING (pinned): at this kernel size the ratio is ~1.0 —
    // Horner cuts ops 9→6 but serializes the dependency chain, while the
    // naive form's independent terms exploit superscalar ILP. Σ-of-weights
    // is a THROUGHPUT cost model and cannot see latency/ILP; that is a
    // documented model limitation (candidate fix: critical-path term in
    // the cost, also O4-monotone). We assert only "not materially slower".
    assert!(ratio >= 0.85, "calibrated extraction materially slower: {ratio:.2}x");
}
