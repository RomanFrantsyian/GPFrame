//! v3-Exp end-to-end: Rust in → CERTIFIED Rust out, THROUGH THE IR DOOR.
//!
//! The mission's consequence, executed: source whose SYNTAX the syn
//! extractor refuses enters via `--lift` (or the automatic fallback),
//! is refactored under the gate, and leaves as emitted Rust carrying its
//! certificate — with the emission round-trip closure running through the
//! SYN door, so the two front doors certify each other on every output.

use cli::pipeline::{certify, Door, PipelineOpts};
use harness::strategy::{MuPrime, Rng};
use rules::smt::{discharge_all, Z3Cli};

/// O1-discharge the Dec rule table into `dir` (refactor's entry condition:
/// no artifact => refusal). Returns false when z3 is absent -- callers skip,
/// same pattern as real_code_test.
fn discharged(dir: &std::path::Path) -> bool {
    if !Z3Cli::available() {
        eprintln!("z3 not installed; skipping");
        return false;
    }
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(dir));
    true
}

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn write_src(tag: &str, src: &str) -> String {
    let dir = std::env::temp_dir().join(format!("dge_e2e_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join(format!("{tag}.rs"));
    std::fs::write(&rs, src).unwrap();
    rs.to_string_lossy().into_owned()
}

/// Iterator-chain syntax (syn door refuses) → straight-line lift →
/// Tier-A-eligible term → certified Rust. The syn door then RE-EXTRACTS the
/// emitted code in the emission gate: full circle across both doors.
#[test]
fn e2e_iterator_chain_to_certified_rust_via_fallback() {
    let src = r#"
pub fn iter_dot3(a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> f64 {
    [a0, a1, a2].iter().zip([b0, b1, b2].iter()).map(|(x, y)| x * y).sum()
}
"#;
    // premise: the syn door alone cannot do this
    assert!(cli::extract::extract_fn(src, "iter_dot3").is_err());

    let file = write_src("iterdot", src);
    let dir = std::env::temp_dir().join(format!("dge_e2e_art_{}", std::process::id()));
    if !discharged(&dir) { return; }
    // NO --lift: exercise the automatic syn→IR fallback
    let opts = PipelineOpts { artifacts: dir, ..Default::default() };
    let c = certify(&file, "iter_dot3", &opts).expect("pipeline through the IR door");
    assert_eq!(c.door, Door::Lift, "must have gone through the IR door");
    assert!(c.code.contains("pub fn iter_dot3_dge"), "emitted fn missing:\n{}", c.code);
    assert!(c.code.contains("/// CERTIFIED"), "certificate comment missing:\n{}", c.code);

    // and the emitted code must behave as the original, checked here a third
    // time INDEPENDENTLY (syn-extract the emitted code, compare to the
    // in-binary rustc original over fresh mu')
    let orig = |e: &[f64]| -> f64 {
        [e[0], e[1], e[2]].iter().zip([e[3], e[4], e[5]].iter()).map(|(x, y)| x * y).sum()
    };
    let t = cli::extract::extract_fn(&c.code, "iter_dot3_dge").expect("emitted code is clean Rust");
    let mu = MuPrime::default_with_seed(0xE2E1);
    let mut rng = Rng::new(0xE2E1);
    for _ in 0..10_000u32 {
        let e = mu.sample(&mut rng, 6);
        assert!(xbit_eq(term::eval(&t, &e), orig(&e)),
            "certified output drifts from the original at {e:?}");
    }
}

/// A FOLD end-to-end: EMA (runtime-length slice + LICM preheader) → Fold
/// term → certified Rust whose emitted loop the syn Σ v1.2 extractor
/// re-reads in the emission gate.
#[test]
fn e2e_ema_fold_to_certified_rust_via_lift_flag() {
    let src = r#"
pub fn ema(s: &[f64], alpha: f64) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc = alpha * s[i] + (1.0 - alpha) * acc;
    }
    acc
}
"#;
    let file = write_src("ema", src);
    let dir = std::env::temp_dir().join(format!("dge_e2e_art2_{}", std::process::id()));
    if !discharged(&dir) { return; }
    let opts = PipelineOpts { lift: true, artifacts: dir, ..Default::default() };
    let c = certify(&file, "ema", &opts).expect("fold pipeline through the IR door");
    assert_eq!(c.door, Door::Lift);
    assert!(c.code.contains("for __i in 0..") || c.code.contains("fold"),
        "emitted code should contain the loop form:\n{}", c.code);

    // independent third check against the in-binary original, seq-aware
    let orig = |s: &[f64], alpha: f64| -> f64 {
        let mut acc = 0.0;
        for i in 0..s.len() { acc = alpha * s[i] + (1.0 - alpha) * acc; }
        acc
    };
    let t = cli::extract::extract_fn(&c.code, "ema_dge").expect("emitted fold is clean Rust");
    let mu = MuPrime::default_with_seed(0xE2E2);
    let mut rng = Rng::new(0xE2E2);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(xbit_eq(term::eval_with_seqs(&t, &e, &sl), orig(&sq[0], e[0])),
            "certified EMA drifts at {e:?} {sq:?}");
    }
}

/// Refusal honesty end-to-end: when BOTH doors refuse, the pipeline reports
/// both reasons and ships nothing.
#[test]
fn e2e_both_doors_refuse_with_both_reasons() {
    let src = r#"
pub fn stateful(s: &mut [f64]) -> f64 {
    s[0] = 1.0;
    s[0]
}
"#;
    let file = write_src("stateful", src);
    let opts = PipelineOpts::default();
    let err = certify(&file, "stateful", &opts).unwrap_err();
    assert!(err.contains("syn") && err.contains("lift"),
        "must report both doors: {err}");
}
