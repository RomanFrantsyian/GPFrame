//! Σ v1.6: f32 lifting via Rnd32 (round-at-every-op).
//!
//! THE CLAIM (and why the gate stays bitwise): for +, -, *, /, sqrt over
//! f32-representable operands, computing in f64 and rounding to f32 is
//! BIT-IDENTICAL to native f32 — double rounding is innocuous because
//! f64's p=53 ≥ 2·24+2 (Figueroa 1995). The extracted term wraps every
//! rounding op AND every param Var in Rnd32, so it is TOTAL over raw f64
//! μ′: term(e) == widen(native_f32(round32(e))) for ALL f64 envs.
//! Transcendentals are NOT innocuous (libm sinf ≠ round64(sin)) and
//! refuse; so do f32 fma (single 24-bit rounding), mixed-precision
//! signatures, and bare `as f32` intermediates in f64 functions.

use cli::extract::{extract_fn, ExtractError};
use harness::gate::{Gate, GateOutcome};
use harness::strategy::{MuPrime, Rng};
use term::eval_with_seqs;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// native f32 mirror of gate 1 — simple-easing 1.0.1's `back_in` shape
fn back_in(t: f32) -> f32 {
    t * t * (2.70158 * t - 1.70158)
}

const BACK_IN_SRC: &str = r#"
pub fn back_in(t: f32) -> f32 {
    t * t * (2.70158 * t - 1.70158)
}
"#;

#[test]
fn f32_easing_agrees_with_native_bitwise_over_raw_f64_mu() {
    let t = extract_fn(BACK_IN_SRC, "back_in").expect("f32 fn must extract (v1.6)");
    let mu = MuPrime::default_with_seed(0xF32);
    let mut rng = Rng::new(0xF32);
    for i in 0..10_000u32 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 1, 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        // rounding lives INSIDE the term: feed the native the rounded input
        let native = back_in(env[0] as f32) as f64;
        let got = eval_with_seqs(&t, &env, &sl);
        assert!(xbit_eq(native, got),
            "f32 drift at {i}: env {:?} native {native:?} term {got:?}",
            env[0]);
    }
}

/// division + sqrt (both innocuous) + literal rounding, all in one body
fn hyp_scaled(a: f32, b: f32) -> f32 {
    ((a * a + b * b) / 1.1).sqrt()
}

#[test]
fn f32_div_sqrt_and_literal_rounding_bitwise() {
    let src = r#"
pub fn hyp_scaled(a: f32, b: f32) -> f32 {
    ((a * a + b * b) / 1.1).sqrt()
}
"#;
    let t = extract_fn(src, "hyp_scaled").expect("div+sqrt f32 must extract");
    let mu = MuPrime::default_with_seed(0xF33);
    let mut rng = Rng::new(0xF33);
    for _ in 0..10_000u32 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 2, 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let native = hyp_scaled(env[0] as f32, env[1] as f32) as f64;
        assert!(xbit_eq(native, eval_with_seqs(&t, &env, &sl)));
    }
}

/// O7: the same bits through the JIT door (fdemote/fpromote lowering)
#[test]
fn f32_term_jit_matches_interp() {
    let t = extract_fn(BACK_IN_SRC, "back_in").unwrap();
    let vt = match Gate::default_dial(0xF34).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(w) => unreachable!("identity gate refuted: {w:?}"),
    };
    let gate = Gate::default_dial(0xF39);
    let jf = jit::install(vt, &jit::LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("f32 term must JIT (fdemote path): {e:?}"));
    let mu = MuPrime::default_with_seed(0xF34);
    let mut rng = Rng::new(0xF34);
    for _ in 0..2_000u32 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 1, 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (a, b) = (jf.interp_seq(&env, &sl), jf.call_seq(&env, &sl));
        assert!(xbit_eq(a, b), "JIT/interp drift: {a:?} vs {b:?} at {env:?}");
    }
}

/// emission closure: emitted `(e as f32) as f64` maps back to Rnd32
#[test]
fn f32_emit_extract_round_trip() {
    let t = extract_fn(BACK_IN_SRC, "back_in").unwrap();
    let code = cli::emit::emit_rust(&t, "back_in_dge", None);
    let t2 = extract_fn(&code, "back_in_dge")
        .expect("emitted f32-semantics code must re-extract");
    let mu = MuPrime::default_with_seed(0xF35);
    let mut rng = Rng::new(0xF35);
    for _ in 0..10_000u32 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 1, 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(xbit_eq(eval_with_seqs(&t, &env, &sl),
                        eval_with_seqs(&t2, &env, &sl)));
    }
}

#[test]
fn non_innocuous_shapes_refuse_with_the_rounding_vocabulary() {
    let refuse = |src: &str, name: &str, needles: &[&str]| {
        match extract_fn(src, name) {
            Err(ExtractError::Unsupported(m)) => for n in needles {
                assert!(m.contains(n), "`{m}` missing `{n}`");
            },
            other => panic!("{name} must refuse: {other:?}"),
        }
    };
    // transcendental: sinf is not round64(sin)
    refuse("pub fn s(t: f32) -> f32 { t.sin() }", "s",
           &["innocuous", "sin"]);
    // f32 fma rounds once at 24 bits
    refuse("pub fn f(a: f32, b: f32, c: f32) -> f32 { a.mul_add(b, c) }", "f",
           &["mul_add", "innocuous"]);
    // powf
    refuse("pub fn p(a: f32, b: f32) -> f32 { a.powf(b) }", "p",
           &["powf", "Sigma"]);
    // mixed precision
    refuse("pub fn m(a: f32, b: f64) -> f64 { b }", "m",
           &["mixed f32/f64"]);
    // bare `as f32` inside an f64 function: invisible f32 dataflow
    refuse("pub fn g(x: f64) -> f64 { ((x as f32) * (x as f32)) as f64 }", "g",
           &["bare `as f32`"]);
    // integer casts truncate
    refuse("pub fn h(x: f64) -> f64 { (x as i64) as f64 }", "h",
           &["truncates"]);
}

/// the round-trip cast in an f64 function IS Rnd32 — and no longer the
/// identity (the pre-v1.6 transparent-cast reading was unsound)
#[test]
fn widened_round_trip_cast_is_rnd32_not_identity() {
    let t = extract_fn(
        "pub fn r(x: f64) -> f64 { (x as f32) as f64 }", "r").unwrap();
    // 1 + 2^-40 is not f32-representable: identity would return it unchanged
    let x = 1.0 + (2.0f64).powi(-40);
    let sl: Vec<&[f64]> = vec![];
    let got = eval_with_seqs(&t, &[x], &sl);
    assert!(xbit_eq(got, (x as f32) as f64));
    assert!(got != x, "cast must round, not pass through");
}
