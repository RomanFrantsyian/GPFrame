//! EMISSION GATE — emission is a lowering, therefore untrusted:
//!   half 1: extract(emit(t)) ≡ t under BitwiseNanClass over μ'
//!           (closes emit∘extract = id through our own front door)
//!   half 2: rustc compiles the emitted source and the resulting BINARY
//!           agrees with interp(t) on probe envs (the real compiler is
//!           the judge of what our printed Rust means)

use cli::emit::emit_rust;
use cli::extract::extract_fn;
use harness::strategy::{MuPrime, Rng};
use term::eval;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// half 1 for an arbitrary term
fn round_trip(t: &term::Term, tag: &str) {
    let code = emit_rust(t, "rt_fn", None);
    let t2 = extract_fn(&code, "rt_fn")
        .unwrap_or_else(|e| panic!("{tag}: re-extraction failed: {e:?}\n--- emitted ---\n{code}"));
    assert_eq!(t2.arity(), t.arity(), "{tag}: arity drift");
    let arity = t.arity().max(1);
    let mu = MuPrime::default_with_seed(0xE317);
    let mut rng = Rng::new(0xE317);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, arity);
        let (a, b) = (eval(t, &e), eval(&t2, &e));
        assert!(xbit_eq(a, b),
            "{tag}: round-trip drift at sample {i}, {e:?}: {a} vs {b}\n{code}");
    }
}

#[test]
fn round_trip_polynomial_horner_fma() {
    // fma + shared subterms → exercises mul_add printing and CSE lets
    let t = term::sexpr::parse(
        "(fma (var 0) (fma (var 0) (fma 3.0 (var 0) 5.0) 2.0) 7.0)").unwrap();
    round_trip(&t, "horner");
}

#[test]
fn round_trip_easer_branch() {
    // real-code shape: select over a comparison, heavy sharing
    let src = std::fs::read_to_string("tests/real_code_test.rs").unwrap();
    let start = src.find("const EASER_CUBIC_SRC").unwrap();
    let _ = start; // (source embedded in the sibling test; re-extract here)
    let easer = r##"
fn ease_in_out(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / (d / 2.0);
    if t < 1.0 { c / 2.0 * (t * t * t) + b }
    else { let t = t - 2.0; c / 2.0 * (t * t * t + 2.0) + b }
}"##;
    let t = extract_fn(easer, "ease_in_out").unwrap();
    round_trip(&t, "ease_in_out");
}

#[test]
fn round_trip_imperative_kernels() {
    let src = r#"
fn dot4(a: [f64; 4], b: [f64; 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 { s += a[i] * b[i]; }
    s
}
fn ema8(signal: &[f64; 8], alpha: f64) -> f64 {
    let mut acc = signal[0];
    for i in 1..8 { acc = alpha * signal[i] + (1.0 - alpha) * acc; }
    acc
}
fn clamped_sum(v: &[f64; 5], cap: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..=4 { s += v[i]; if s > cap { s = cap; } }
    s
}"#;
    for name in ["dot4", "ema8", "clamped_sum"] {
        let t = extract_fn(src, name).unwrap();
        round_trip(&t, name); // ema8 arity 9 → array-param emission path
    }
}

#[test]
fn round_trip_specials_and_comparison_values() {
    // NaN-payload constant, -0.0, comparison used as a VALUE (not a cond)
    let t = term::sexpr::parse(
        "(+ (* (lt (var 0) NaN) -0.0) (select (ge (var 0) 2.0) inf (var 0)))").unwrap();
    round_trip(&t, "specials");
}

#[test]
fn emission_gate_half2_rustc_compile_and_run() {
    // the REAL compiler judges the emitted source
    let src = r#"
fn ease_in_out(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / (d / 2.0);
    if t < 1.0 { c / 2.0 * (t * t * t) + b }
    else { let t = t - 2.0; c / 2.0 * (t * t * t + 2.0) + b }
}"#;
    let t = extract_fn(src, "ease_in_out").unwrap();
    let code = emit_rust(&t, "emitted", None);

    // probe envs: boundaries + a few finite points, printed as bits
    let probes: [[f64; 4]; 7] = [
        [0.5, 1.0, 2.0, 3.0],
        [f64::NAN, 1.0, 2.0, 3.0],
        [-0.0, 0.0, 1.0, -1.0],
        [f64::INFINITY, 2.0, -3.0, 4.0],
        [1e-310, 5.0, 6.0, 7.0],
        [-7.25, 0.125, 9.5, -2.0],
        [1e300, -1e300, 1e-300, 2.0],
    ];
    let mut main_src = String::from(&code);
    main_src.push_str("fn main() {\n    let probes: [[f64; 4]; 7] = [\n");
    for p in &probes {
        main_src.push_str(&format!(
            "        [f64::from_bits(0x{:016x}), f64::from_bits(0x{:016x}), \
             f64::from_bits(0x{:016x}), f64::from_bits(0x{:016x})],\n",
            p[0].to_bits(), p[1].to_bits(), p[2].to_bits(), p[3].to_bits()));
    }
    main_src.push_str(
        "    ];\n    for p in probes { \
         println!(\"{:016x}\", emitted(p[0], p[1], p[2], p[3]).to_bits()); }\n}\n");

    let dir = std::env::temp_dir().join(format!("dge_emit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("emitted.rs");
    let bin = dir.join("emitted_bin");
    std::fs::write(&rs, &main_src).unwrap();
    let ok = std::process::Command::new("rustc")
        .args([rs.to_str().unwrap(), "-O", "-o", bin.to_str().unwrap()])
        .status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "rustc rejected the emitted source:\n{main_src}");

    let out = std::process::Command::new(&bin).output().unwrap();
    let lines: Vec<u64> = String::from_utf8_lossy(&out.stdout).lines()
        .map(|l| u64::from_str_radix(l.trim(), 16).unwrap()).collect();
    assert_eq!(lines.len(), probes.len());
    for (p, &got_bits) in probes.iter().zip(&lines) {
        let want = eval(&t, p);
        let got = f64::from_bits(got_bits);
        assert!(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()),
            "rustc-compiled emission drift at {p:?}: binary={got} interp={want}");
    }
}
