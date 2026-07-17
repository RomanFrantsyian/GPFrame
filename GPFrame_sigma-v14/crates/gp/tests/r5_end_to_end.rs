//! R5 end-to-end: symbolic regression finds x^2 + x from samples, and the
//! result exits ONLY through the harness gate, certificate attached (T8).
use gp::evolve::{run, EvolveParams};
use gp::fitness::FitnessParams;
use gp::pop::GpConfig;
use harness::metric::Metric;
use harness::strategy::Rng;
use harness::{Gate, GateOutcome, Tier};
use term::{eval, sexpr::parse};

#[test]
fn sr_finds_target_and_gate_promotes() {
    let target = parse("(+ (* (var 0) (var 0)) (var 0))").unwrap(); // x^2 + x

    // training samples: x in [-3, 3]
    let mut srng = Rng::new(7);
    let targets: Vec<(Vec<f64>, f64)> = (0..32)
        .map(|_| {
            let x = srng.uniform01() * 6.0 - 3.0;
            (vec![x], eval(&target, &[x]))
        })
        .collect();

    let cfg = GpConfig::default();
    let ep = EvolveParams::default();
    let fp = FitnessParams::default();
    let mut rng = Rng::new(1);
    let out = run(&cfg, &ep, &fp, &targets, &mut rng);

    assert_eq!(out.best_error, 0.0, "SR failed to converge: err={} after {} gens ({})",
        out.best_error, out.generations, term::sexpr::print(&out.best));

    // T8: search output is a HYPOTHESIS until the gate says otherwise.
    // Tolerant metric: GP may find an algebraically-equal form (e.g. x*(x+1))
    // that differs by rounding — exactly the ~_eps case.
    let mut gate = Gate::default_dial(99);
    gate.metric = Metric::Tolerant { eps_abs: 0.0, eps_rel: 0.0, max_ulp: 8 };
    match gate.promote(out.best, &target) {
        GateOutcome::Promoted(vt) => {
            match &vt.certificate().tier {
                Tier::B { n, delta_min, .. } => {
                    assert_eq!(*n, 10_000);
                    assert!(*delta_min > 0.0);
                }
                _ => panic!("expected Tier B"),
            }
            println!("promoted: {}", term::sexpr::print(vt.term()));
            println!("claim: {}", vt.certificate().claim());
        }
        GateOutcome::Refuted(ce) => {
            panic!("gate refuted the SR result at {:?}", ce.minimal_env);
        }
    }
}

#[test]
fn nelder_mead_refines_constants() {
    use gp::refine::{refine, NmParams};
    // shape: c0*x + c1 with wrong consts; targets from 2x + 3
    let t = parse("(+ (* 1.6 (var 0)) 2.2)").unwrap();
    let mut srng = Rng::new(3);
    let targets: Vec<(Vec<f64>, f64)> = (0..24)
        .map(|_| { let x = srng.uniform01() * 8.0 - 4.0; (vec![x], 2.0 * x + 3.0) })
        .collect();
    let obj = |c: &term::Term| gp::fitness::error(c, &targets);
    let (refined, final_err) = refine(&t, &obj, &NmParams::default());
    assert!(final_err < 1e-8, "NM failed to converge: err={final_err}");
    assert!((refined.consts[0] - 2.0).abs() < 1e-5, "c0={}", refined.consts[0]);
    assert!((refined.consts[1] - 3.0).abs() < 1e-5, "c1={}", refined.consts[1]);
}

#[test]
fn repair_fixes_planted_fault_and_exits_via_gate() {
    use gp::repair::{repair, RepairParams};
    use locate::{ochiai, spectrum};

    // oracle: x^2 + x        broken: x^2 - x  (single op fault)
    let oracle = parse("(+ (* (var 0) (var 0)) (var 0))").unwrap();
    let broken = parse("(- (* (var 0) (var 0)) (var 0))").unwrap();

    // localize with a small suite against the oracle-as-phi
    let tests: Vec<Vec<f64>> = vec![vec![0.0], vec![1.0], vec![-2.0], vec![3.5], vec![-0.5]];
    let phi = |env: &[f64], out: f64| out.to_bits() == eval(&oracle, env).to_bits();
    let spec = spectrum::collect(&broken, &tests, &phi);
    let ranking = ochiai::rank(&spec);

    let gate = Gate::default_dial(23);
    let fixed = repair(&broken, &ranking, &oracle, &gate, &RepairParams::default(), 5)
        .expect("repair should find the single-op fix");
    // exits ONLY via the gate: certificate is attached and quantified
    match &fixed.certificate().tier {
        Tier::B { n, .. } => assert_eq!(*n, 10_000),
        _ => panic!("repair must exit Tier B"),
    }
    // and the fix is extensionally right on fresh inputs
    for x in [-7.3, 0.0, 2.25, 41.0] {
        assert_eq!(eval(fixed.term(), &[x]).to_bits(), eval(&oracle, &[x]).to_bits());
    }
}
