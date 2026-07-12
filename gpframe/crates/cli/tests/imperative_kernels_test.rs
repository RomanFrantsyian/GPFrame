//! Extractor v2: imperative production kernels — loops, accumulators,
//! arrays, conditional updates — extracted and gated bitwise against the
//! rustc-compiled originals over mu' (NaN/±0/Inf/subnormals included).

use cli::extract::{extract_fn, ExtractError};
use harness::strategy::{MuPrime, Rng};
use term::eval;


/// Finding 7: cross-generator equality = exact bits OR both-NaN.
fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

const SRC: &str = r#"
/// dot product over fixed-size vectors — the canonical accumulator loop
fn dot4(a: [f64; 4], b: [f64; 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 {
        s += a[i] * b[i];
    }
    s
}

/// exponential moving average — accumulator carried across iterations,
/// array indexing offset by the loop variable
fn ema8(signal: &[f64; 8], alpha: f64) -> f64 {
    let mut acc = signal[0];
    for i in 1..8 {
        acc = alpha * signal[i] + (1.0 - alpha) * acc;
    }
    acc
}

/// Newton-Raphson inverse sqrt refinement — fixed iteration count,
/// wildcard loop variable, self-referential update
fn newton_invsqrt4(x: f64) -> f64 {
    let mut y = 1.0;
    for _ in 0..4 {
        y = y * (1.5 - 0.5 * x * y * y);
    }
    y
}

/// clamped running sum — statement-if with assignment in the branch
/// (phi-merge), compound assignment, inclusive range
fn clamped_sum(v: &[f64; 5], cap: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..=4 {
        s += v[i];
        if s > cap {
            s = cap;
        }
    }
    s
}
"#;

fn orig_dot4(a: [f64; 4], b: [f64; 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 { s += a[i] * b[i]; }
    s
}
fn orig_ema8(signal: &[f64; 8], alpha: f64) -> f64 {
    let mut acc = signal[0];
    for i in 1..8 { acc = alpha * signal[i] + (1.0 - alpha) * acc; }
    acc
}
fn orig_newton_invsqrt4(x: f64) -> f64 {
    let mut y = 1.0;
    for _ in 0..4 { y = y * (1.5 - 0.5 * x * y * y); }
    y
}
fn orig_clamped_sum(v: &[f64; 5], cap: f64) -> f64 {
    let mut s = 0.0;
    for i in 0..=4 {
        s += v[i];
        if s > cap { s = cap; }
    }
    s
}

fn gate(name: &str, arity: usize, orig: impl Fn(&[f64]) -> f64) {
    let t = extract_fn(SRC, name).unwrap_or_else(|e| panic!("extract {name}: {e:?}"));
    assert_eq!(t.arity(), arity, "{name} arity");
    let mu = MuPrime::default_with_seed(0x1009);
    let mut rng = Rng::new(0x1009);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, arity);
        let (iv, ov) = (eval(&t, &e), orig(&e));
        assert!(xbit_eq(iv, ov),
            "{name}: drift at sample {i}, env {e:?}: interp={iv} rustc={ov}");
    }
}

#[test]
fn dot_product_unrolled_bitwise() {
    gate("dot4", 8, |e| orig_dot4(
        [e[0], e[1], e[2], e[3]], [e[4], e[5], e[6], e[7]]));
}

#[test]
fn ema_filter_bitwise() {
    gate("ema8", 9, |e| {
        let sig: [f64; 8] = e[..8].try_into().unwrap();
        orig_ema8(&sig, e[8])
    });
}

#[test]
fn newton_iteration_bitwise() {
    gate("newton_invsqrt4", 1, |e| orig_newton_invsqrt4(e[0]));
}

#[test]
fn conditional_clamp_phi_merge_bitwise() {
    // exercises statement-if + assignment: s' = select(s > cap, cap, s)
    gate("clamped_sum", 6, |e| {
        let v: [f64; 5] = e[..5].try_into().unwrap();
        orig_clamped_sum(&v, e[5])
    });
}

#[test]
fn honest_refusals_carry_reasons() {
    let while_src = "fn f(x: f64) -> f64 { let mut y = x; while y > 1.0 { y = y / 2.0; } y }";
    match extract_fn(while_src, "f") {
        Err(ExtractError::Unsupported(msg)) => {
            assert!(msg.contains("total"), "reason should cite totality: {msg}");
        }
        other => panic!("while must be refused, got {other:?}"),
    }
    // Σ v1.2: `&[f64]` is a SEQUENCE param now — the refusal boundary moved
    // to non-loop indexing of a sequence (windowed access: roadmap)
    let slice_src = "fn g(a: &[f64]) -> f64 { a[0] }";
    match extract_fn(slice_src, "g") {
        Err(ExtractError::Unsupported(msg)) => {
            assert!(msg.contains("non-loop"), "{msg}");
        }
        other => panic!("constant-indexed sequence must be refused, got {other:?}"),
    }
}

#[test]
fn finding3_reconfirmed_fma_fusion_unsafe_for_ema_even_bounded() {
    use harness::{Gate, metric::Metric, strategy::MuPrime};
    use rules::{refactor, RefactorError};
    use rules::extract::SaturationLimits;
    use rules::smt::{discharge_all, Z3Cli};
    if !Z3Cli::available() { return; }
    let t = extract_fn(SRC, "ema8").unwrap();
    let dir = std::env::temp_dir().join(format!("dge_ema_{}", std::process::id()));
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));
    let mut gate = Gate::default_dial(0xE0A);
    gate.metric = Metric::fma_mixed();
    gate.mu = MuPrime::bounded(0xE0A, 1e30); // domain bound does NOT help:
    match refactor(&t, true, &gate, &dir, &SaturationLimits::default()) {
        Err(RefactorError::GateRefuted { .. }) => {} // cancellation is scale-free
        Ok(out) => panic!("fma-fused EMA accepted — cancellation drift shipped: {}",
            term::sexpr::print(out.verified.term())),
        Err(e) => panic!("unexpected: {e:?}"),
    }
}
