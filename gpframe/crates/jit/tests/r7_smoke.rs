//! R7: the O7 door end-to-end — bitwise agreement across every op category,
//! and the differential gate catching a REAL compiler-semantics mismatch
//! (cranelift fmin propagates NaN; Rust f64::min returns the other operand).
use harness::{Gate, GateOutcome};
use jit::{install, InstallError, LowerConfig};
use term::sexpr::parse;

/// Promote a term through the R1 gate against itself (identity reference) —
/// the legitimate way to mint a VerifiedTerm for lowering tests.
fn verified(src: &str, seed: u64) -> harness::VerifiedTerm {
    let t = parse(src).unwrap();
    match Gate::default_dial(seed).promote(t.clone(), &t) {
        GateOutcome::Promoted(vt) => vt,
        GateOutcome::Refuted(_) => unreachable!("identity gate cannot refute"),
    }
}

#[test]
fn o7_bitwise_across_all_op_categories() {
    // arithmetic + select + fma + transcendental + min/max in one term
    let src = "(select (min (var 0) (max (var 1) 0.5)) \
                 (fma (sin (var 0)) (exp (var 1)) (sqrt (abs (var 0)))) \
                 (/ (+ (var 0) 1.5) (- (var 1) (floor (var 0)))))";
    let vt = verified(src, 41);
    let gate = Gate::default_dial(42);
    let jf = install(vt, &LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("install failed: {e:?}"));

    // O7 already ran n=10^4 bitwise checks inside install; spot-check the
    // public API surface including boundary values.
    for env in [
        [0.0, 0.0], [-0.0, 1.0], [f64::NAN, 2.0], [f64::INFINITY, -1.0],
        [1.5e-308, 3.0], [-7.25, 0.125], [1e300, -1e300],
    ] {
        let (a, b) = (jf.call(&env), jf.interp(&env));
        assert!(a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
            "jit/interp divergence at {env:?}: {a} vs {b}");
    }
    let (metric, n) = jf.o7_evidence();
    assert_eq!(n, 10_000);
    assert!(matches!(metric, harness::metric::Metric::BitwiseNanClass));
}

#[test]
fn o7_catches_naive_fmin_compiler_semantics() {
    // (min (var 0) (var 1)) lowered via CLIF fmin: NaN-PROPAGATING, while
    // Rust f64::min returns the other operand. O7 must refute at a NaN env.
    let vt = verified("(min (var 0) (var 1))", 7);
    let gate = Gate::default_dial(8);
    let cfg = LowerConfig { naive_min_max: true, ..Default::default() };
    match install(vt, &cfg, &gate) {
        Err(InstallError::DifferentialMismatch { minimal_env, jit_val, interp_val, .. }) => {
            assert!(minimal_env.iter().any(|x| x.is_nan()),
                "witness should involve NaN, got {minimal_env:?}");
            assert!(jit_val.is_nan() != interp_val.is_nan()
                    || jit_val.to_bits() != interp_val.to_bits());
        }
        Ok(_) => panic!("O7 HOLE: naive fmin installed despite NaN semantics mismatch"),
        Err(e) => panic!("wrong failure mode: {e:?}"),
    }
    // and the correct lowering (wrapper) installs fine
    let vt2 = verified("(min (var 0) (var 1))", 7);
    install(vt2, &LowerConfig::default(), &gate).expect("wrapper min must pass O7");
}

#[test]
fn jit_speed_smoke_informational() {
    // NOT a pass/fail on the ≥5x target (that's R7 calibration on real
    // kernels); just proves the compiled path runs and reports the ratio.
    // deep pure-arithmetic Horner chain — representative of the hot-kernel
    // shape; no extern wrapper calls on the path.
    let mut src = String::from("(var 0)");
    for k in 0..24 {
        src = format!("(+ (* {src} (var 0)) {}.5)", k);
    }
    let vt = verified(&src, 9);
    let gate = Gate::default_dial(10);
    let jf = install(vt, &LowerConfig::default(), &gate).unwrap();
    let env = [1.234];
    let iters = 200_000u64;

    let t0 = std::time::Instant::now();
    let mut acc = 0.0;
    for _ in 0..iters { acc += jf.call(&env); }
    let jit_t = t0.elapsed();

    let t1 = std::time::Instant::now();
    let mut acc2 = 0.0;
    for _ in 0..iters { acc2 += jf.interp(&env); }
    let interp_t = t1.elapsed();

    assert_eq!(acc.to_bits(), acc2.to_bits());
    println!("jit {jit_t:?} vs interp {interp_t:?}  (ratio {:.1}x)",
        interp_t.as_nanos() as f64 / jit_t.as_nanos().max(1) as f64);
}

#[test]
fn hot_dispatch_installs_after_threshold_and_pins_on_failure() {
    use jit::hot::{HotDispatch, HotPolicy};
    let gate = Gate::default_dial(3);
    let pol = HotPolicy { install_threshold: 10 };

    // healthy term: crosses the threshold and gets jitted
    let vt = verified("(+ (* (var 0) (var 0)) 1.0)", 30);
    let mut d = HotDispatch::new(vt, HotPolicy { install_threshold: 10 }, LowerConfig::default(), gate.clone());
    for _ in 0..20 { assert_eq!(d.call(&[2.0]), 5.0); }
    assert!(d.is_jitted(), "should install after threshold");

    // poisoned lowering: O7 fails once, dispatcher pins to interp forever
    let vt2 = verified("(min (var 0) (var 1))", 31);
    let cfg = LowerConfig { naive_min_max: true, ..Default::default() };
    let mut d2 = HotDispatch::new(vt2, pol, cfg, gate);
    for _ in 0..30 {
        let v = d2.call(&[f64::NAN, 4.0]);
        assert_eq!(v, 4.0, "interp semantics must hold throughout"); // Rust min(NaN,4)=4
    }
    assert!(!d2.is_jitted(), "failed O7 must pin to the interp fallback");
}
