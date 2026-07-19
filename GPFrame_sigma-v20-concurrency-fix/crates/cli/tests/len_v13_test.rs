//! Σ v1.3 `Len(k)` — the field trial's #1 priced extension, gated at every
//! layer it touches: interp semantics (incl. L = 0 and the traced-eval
//! convention), sexpr round trip, the IR-door gate on the exact function
//! that priced it (quadratic-mean shape), the FULL pipeline closure (lift →
//! refactor → emit `(s.len() as f64)` → syn re-extract), and the JIT door.

use cli::lift::{lift_ll, rustc_emit_ir};
use cli::pipeline::{certify, Door, PipelineOpts};
use harness::strategy::{MuPrime, Rng};
use rules::smt::{discharge_all, Z3Cli};
use term::{eval_with_seqs, sexpr};

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[test]
fn len_semantics_and_sexpr_round_trip() {
    // sqrt(fold(0, acc + elem^2) / len) — the quadratic mean, hand-built
    let t = sexpr::parse(
        "(sqrt (/ (fold 0.0 (+ acc (* (elem 0) (elem 0)))) (len 0)))").unwrap();
    assert_eq!(t.seq_count(), 1, "Len alone must claim its sequence slot");
    let s = vec![3.0, 4.0];
    assert_eq!(eval_with_seqs(&t, &[], &[&s]),
        ((9.0f64 + 16.0) / 2.0).sqrt());
    // L = 0: fold ≡ init (0.0), len = 0.0 → 0/0 = NaN → sqrt(NaN) = NaN.
    // That IS the mathematical answer: an empty set has no quadratic mean.
    assert!(eval_with_seqs(&t, &[], &[&[][..]]).is_nan());
    // round trip
    assert_eq!(sexpr::print(&sexpr::parse(&sexpr::print(&t)).unwrap()),
               sexpr::print(&t));
}

/// A term that is ONLY a length (no Elem) still requires the sequence.
#[test]
fn len_without_elem_still_counts_sequences() {
    let t = sexpr::parse("(* 2.0 (len 0))").unwrap();
    assert_eq!(t.seq_count(), 1);
    assert_eq!(eval_with_seqs(&t, &[], &[&[1.0, 2.0, 3.0][..]]), 6.0);
}

/// IR-door gate on the shape the field trial measured: rustc's -O1 IR for a
/// quadratic mean has `uitofp` of the length param in its LCSSA tail — the
/// exact line that refused before Σ v1.3.
fn orig_qmean(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i] * s[i];
    }
    (acc / s.len() as f64).sqrt()
}

#[test]
fn len_gate_quadratic_mean_through_the_ir_door() {
    let dir = std::env::temp_dir().join(format!("dge_len_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("qmean.rs");
    std::fs::write(&rs, r#"
pub fn qmean(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i] * s[i];
    }
    (acc / s.len() as f64).sqrt()
}
"#).unwrap();
    let ir = rustc_emit_ir(&rs, "qmean").expect("emit");
    let t = lift_ll(&ir, "qmean").unwrap_or_else(|e| panic!("lift qmean: {e}"));
    assert!(t.has_fold());
    assert!(sexpr::print(&t).contains("(len 0)"), "{}", sexpr::print(&t));

    let mu = MuPrime::default_with_seed(0x1E11);
    let mut rng = Rng::new(0x1E11);
    let mut zero_seen = false;
    for i in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        zero_seen |= sq[0].is_empty();
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (lv, ov) = (eval_with_seqs(&t, &e, &sl), orig_qmean(&sq[0]));
        assert!(xbit_eq(lv, ov), "qmean drift at {i}, {sq:?}: {lv} vs {ov}");
    }
    assert!(zero_seen, "L = 0 (the NaN case) must be exercised");
}

/// The closure: lift a MEAN, refactor under the gate, emit Rust containing
/// `(s0.len() as f64)`, and have the SYN door re-extract it in the emission
/// gate — the new Cast/MethodCall path in the extractor is what this pins.
#[test]
fn len_full_pipeline_mean_to_certified_rust() {
    let art = std::env::temp_dir().join(format!("dge_len_art_{}", std::process::id()));
    if !Z3Cli::available() {
        eprintln!("z3 not installed; skipping");
        return;
    }
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&art));

    let dir = std::env::temp_dir().join(format!("dge_len_pl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("mean.rs");
    std::fs::write(&rs, r#"
pub fn mean(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i];
    }
    acc / s.len() as f64
}
"#).unwrap();

    let opts = PipelineOpts { lift: true, artifacts: art, ..Default::default() };
    let c = certify(rs.to_str().unwrap(), "mean", &opts).expect("pipeline");
    assert_eq!(c.door, Door::Lift);
    assert!(c.code.contains(".len() as f64"), "emitted Len form missing:\n{}", c.code);

    // independent third differential vs the in-binary original
    let orig = |s: &[f64]| -> f64 {
        let mut a = 0.0;
        for i in 0..s.len() { a += s[i]; }
        a / s.len() as f64
    };
    let t = cli::extract::extract_fn(&c.code, "mean_dge").expect("syn re-reads Len");
    let mu = MuPrime::default_with_seed(0x1E12);
    let mut rng = Rng::new(0x1E12);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(xbit_eq(eval_with_seqs(&t, &e, &sl), orig(&sq[0])),
            "certified mean drifts at {sq:?}");
    }
}

/// O7 discipline for the new op: install through the JIT door (which runs
/// its own 10^4-sample differential internally and REFUSES on mismatch),
/// then spot-check the seq API against the interpreter — including L = 0,
/// where mean = 0/0 = NaN on both sides.
#[test]
fn len_jit_door_differential() {
    use harness::{Gate, GateOutcome};
    let t = sexpr::parse(
        "(/ (fold 0.0 (+ acc (elem 0))) (len 0))").unwrap(); // mean
    let vt = match Gate::default_dial(0x1E13).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(w) => unreachable!("identity gate refuted: {w:?}"),
    };
    let gate = Gate::default_dial(0x1E14);
    let jf = jit::install(vt, &jit::LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("O7 install must pass for Len: {e:?}"));
    let mu = MuPrime::default_with_seed(0x1E15);
    let mut rng = Rng::new(0x1E15);
    for _ in 0..2_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (iv, jv) = (jf.interp_seq(&e, &sl), jf.call_seq(&e, &sl));
        assert!(xbit_eq(iv, jv), "jit/interp Len drift at {sq:?}: {iv} vs {jv}");
    }
}
