//! FINDING 7 (pinned): NaN payloads are not portable observables.
//! Identical source arithmetic — ((((0+p0)+p1)+p2)+p3) — evaluated by our
//! interpreter vs the rustc-compiled twin in the SAME binary yields NaN with
//! DIFFERENT bit patterns (x86 runtime "real indefinite" 0xfff8… vs LLVM's
//! canonical 0x7ff8…), because IEEE-754 leaves NaN payload propagation
//! unspecified and compilers fold/canonicalize differently. Cross-generator
//! equality is therefore defined as bitwise-modulo-NaN-class
//! (Metric::BitwiseNanClass), with ±0 signs still exact.
#[test]
fn finding7_nan_payloads_are_not_portable() {
    let src = r#"
fn dot4(a: [f64; 4], b: [f64; 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 { s += a[i] * b[i]; }
    s
}"#;
    let t = cli::extract::extract_fn(src, "dot4").unwrap();
    let e = [5e-324f64, -1.3458159950839428e118, -9.764524728021949e281, 1.2961917694998514e53,
             -3.1336361400143475e-11, 1.7976931348623157e308, -4.091541809787174e49, f64::NAN];
    fn orig(a: [f64; 4], b: [f64; 4]) -> f64 {
        let mut s = 0.0;
        for i in 0..4 { s += a[i] * b[i]; }
        s
    }
    let iv = term::eval(&t, &e);
    let ov = orig([e[0], e[1], e[2], e[3]], [e[4], e[5], e[6], e[7]]);
    assert!(iv.is_nan() && ov.is_nan(), "both sides are NaN — class equal");
    // NOTE: iv.to_bits() may or may not equal ov.to_bits() depending on
    // compiler version and codegen choices — that variability IS the finding.
    // The metric that survives it:
    let m = harness::metric::Metric::BitwiseNanClass;
    assert!(m.eq(iv, ov));
    // and ±0 signs remain exact under the same metric:
    assert!(!m.eq(0.0, -0.0), "zero signs are specified and must stay exact");
}
