//! Σ v1.2 end-to-end: dynamic-length kernels through EVERY door.
//! rustc original ⇒ extraction gate ⇒ Tier-B gate ⇒ O7 loop-codegen ⇒
//! emission round trip — over random lengths INCLUDING 0 and 1.

use cli::emit::emit_rust;
use cli::extract::extract_fn;
use harness::strategy::{MuPrime, Rng};
use harness::{Gate, GateOutcome};
use term::{eval_with_seqs, Term};

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

const SRC: &str = r#"
/// dynamic dot product — the canonical fold
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len() {
        s += a[i] * b[i];
    }
    s
}

/// L2 norm squared with a scalar scale — mixed scalar/seq inputs
fn scaled_norm2(v: &[f64], scale: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..v.len() {
        s += (scale * v[i]) * (scale * v[i]);
    }
    s
}

/// clamped running sum — conditional accumulator update INSIDE the fold
fn clamped_dyn_sum(v: &[f64], cap: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..v.len() {
        s += v[i];
        if s > cap {
            s = cap;
        }
    }
    s
}
"#;

fn orig_dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len() { s += a[i] * b[i]; }
    s
}
fn orig_scaled_norm2(v: &[f64], scale: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..v.len() { s += (scale * v[i]) * (scale * v[i]); }
    s
}
fn orig_clamped(v: &[f64], cap: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..v.len() { s += v[i]; if s > cap { s = cap; } }
    s
}

fn extraction_gate(t: &Term, tag: &str, orig: impl Fn(&[f64], &[&[f64]]) -> f64) {
    let arity = t.arity();
    let k = t.seq_count();
    let mu = MuPrime::default_with_seed(0xF01D);
    let mut rng = Rng::new(0xF01D);
    for i in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, arity.max(1), k);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (iv, ov) = (eval_with_seqs(t, &e, &sl), orig(&e, &sl));
        assert!(xbit_eq(iv, ov),
            "{tag}: drift at sample {i}, scalars {e:?}, seqs {sq:?}: {iv} vs {ov}");
    }
}

#[test]
fn extraction_gate_dynamic_dot() {
    let t = extract_fn(SRC, "dot").unwrap();
    assert!(t.has_fold());
    assert_eq!(t.seq_count(), 2);
    extraction_gate(&t, "dot", |_, s| orig_dot(s[0], s[1]));
}

#[test]
fn extraction_gate_scaled_norm() {
    let t = extract_fn(SRC, "scaled_norm2").unwrap();
    assert_eq!((t.seq_count(), t.arity()), (1, 1));
    extraction_gate(&t, "norm", |e, s| orig_scaled_norm2(s[0], e[0]));
}

#[test]
fn extraction_gate_conditional_fold() {
    let t = extract_fn(SRC, "clamped_dyn_sum").unwrap();
    extraction_gate(&t, "clamp", |e, s| orig_clamped(s[0], e[0]));
}

#[test]
fn gate_judges_fold_terms() {
    // 2·dot(a,b) as fold(0, acc + 2ab)  vs  2 * fold(0, acc + ab):
    // rounding order differs ⇒ Tier-B gate under fma_mixed decides.
    // Simpler exact pair: dot(a,b) vs dot with commuted product — bitwise.
    let t1 = term::sexpr::parse("(fold 0.0 (+ acc (* (elem 0) (elem 1))))").unwrap();
    let t2 = term::sexpr::parse("(fold 0.0 (+ acc (* (elem 1) (elem 0))))").unwrap();
    match Gate::default_dial(0x5E).promote(t1.clone(), &t2) {
        GateOutcome::Promoted(vt) => {
            let claim = vt.certificate().claim();
            assert!(claim.contains("seqs"), "seq measure must be in the claim: {claim}");
        }
        GateOutcome::Refuted(ce) => panic!("commuted dot refuted: {ce:?}"),
    }
    // and a WRONG candidate is refuted with a SHORT sequence witness
    let bad = term::sexpr::parse("(fold 0.0 (+ acc (+ (elem 0) (elem 1))))").unwrap();
    match Gate::default_dial(6).promote(bad, &t1) {
        GateOutcome::Refuted(ce) => {
            let len = ce.minimal_seqs.first().map(|s| s.len()).unwrap_or(0);
            assert!(len <= 2, "shrink should find a short witness, got len {len}");
        }
        GateOutcome::Promoted(_) => panic!("sum accepted as dot"),
    }
}

#[test]
fn o7_loop_codegen_bitwise_and_fast() {
    let t = extract_fn(SRC, "dot").unwrap();
    let vt = match Gate::default_dial(0xA1).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(_) => unreachable!(),
    };
    let gate = Gate::default_dial(0xA2);
    let jf = jit::install(vt, &jit::LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("O7 failed on fold codegen: {e:?}"));
    // O7 already ran 10^4 seq-sampled differentials inside install;
    // spot-check edges through the public API
    let empty: [f64; 0] = [];
    assert_eq!(jf.call_seq(&[], &[&empty, &empty]), 0.0, "L=0 => init");
    let a = [1.0, 2.0, 3.0]; let b = [4.0, 5.0, 6.0];
    assert_eq!(jf.call_seq(&[], &[&a, &b]), 32.0);
    let nan_a = [f64::NAN]; let one_b = [1.0];
    assert!(xbit_eq(jf.call_seq(&[], &[&nan_a, &one_b]),
                    jf.interp_seq(&[], &[&nan_a, &one_b])));

    // THE ≥5× TARGET: len-4096 dot, jit loop vs interpreter
    let big_a: Vec<f64> = (0..4096).map(|i| (i as f64).sin()).collect();
    let big_b: Vec<f64> = (0..4096).map(|i| (i as f64).cos()).collect();
    let iters = 2_000u64;
    let t0 = std::time::Instant::now();
    let mut x = 0.0;
    for _ in 0..iters { x += jf.call_seq(&[], &[&big_a, &big_b]); }
    let jit_t = t0.elapsed();
    let t1 = std::time::Instant::now();
    let mut y = 0.0;
    for _ in 0..iters { y += jf.interp_seq(&[], &[&big_a, &big_b]); }
    let interp_t = t1.elapsed();
    assert!(xbit_eq(x, y));
    let ratio = interp_t.as_nanos() as f64 / jit_t.as_nanos().max(1) as f64;
    println!("fold jit {jit_t:?} vs interp {interp_t:?} (ratio {ratio:.1}x)");
    assert!(ratio >= 5.0, "loop codegen should clear the 5x target: {ratio:.1}x");
}

#[test]
fn emission_round_trip_folds() {
    for name in ["dot", "scaled_norm2", "clamped_dyn_sum"] {
        let t = extract_fn(SRC, name).unwrap();
        let code = emit_rust(&t, "rt_fold", None);
        let t2 = extract_fn(&code, "rt_fold")
            .unwrap_or_else(|e| panic!("{name}: re-extract: {e:?}\n{code}"));
        let mu = MuPrime::default_with_seed(0xE2);
        let mut rng = Rng::new(0xE2);
        for _ in 0..5_000u32 {
            let (e, sq) = mu.sample_with_seqs(&mut rng, t.arity().max(1), t.seq_count());
            let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
            let (a, b) = (eval_with_seqs(&t, &e, &sl), eval_with_seqs(&t2, &e, &sl));
            assert!(xbit_eq(a, b), "{name}: round-trip drift\n{code}");
        }
    }
}

#[test]
fn honest_refusals_v12() {
    // v1.5 flipped the old multi-accumulator pin: INDEPENDENT accumulators
    // now fission into sibling folds and ADMIT (gated in multiacc_test.rs);
    // the honest refusal moved to COUPLED recurrences, whose update reads a
    // co-accumulator and therefore has no fission.
    let multi = "fn f(a: &[f64]) -> f64 { let mut s = 0.0; let mut p = 1.0; \
                 for i in 0..a.len() { s += a[i]; p = p * a[i]; } s + p }";
    let t = extract_fn(multi, "f").expect("independent accumulators fission (v1.5)");
    assert!(t.has_fold(), "fission must produce folds");
    let coupled = "fn w(a: &[f64]) -> f64 { let mut m = 0.0; let mut q = 0.0; \
                   for i in 0..a.len() { q += (a[i] - m) * a[i]; m = m + a[i]; } q }";
    match extract_fn(coupled, "w") {
        Err(cli::extract::ExtractError::Unsupported(m)) =>
            assert!(m.contains("co-accumulator") && m.contains("fission"), "{m}"),
        other => panic!("coupled recurrence must be refused: {other:?}"),
    }
    let offset = "fn g(a: &[f64]) -> f64 { let mut s = 0.0; \
                  for i in 1..a.len() { s += a[i]; } s }";
    match extract_fn(offset, "g") {
        Err(cli::extract::ExtractError::Unsupported(m)) =>
            assert!(m.contains("start at 0"), "{m}"),
        other => panic!("offset range must be refused: {other:?}"),
    }
}
