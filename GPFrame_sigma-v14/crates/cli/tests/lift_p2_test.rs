//! v3-Exp P2 — loops-as-math: CFG + phi → Σ Fold, gate-arbitrated.
//!
//! The payoff phase of the mission statement: a runtime-bound loop is not
//! "unsupported syntax" — at instruction level it is what it always was, a
//! recurrence, i.e. our Fold. The recognizer targets the MEASURED canonical
//! 3-block shape rustc emits at -O1 with --unroll-runtime=false (see
//! cli/src/lift.rs, mod fold), and every recovered term must pass the
//! extraction gate: BitwiseNanClass vs the rustc-compiled in-binary original
//! over 10⁴ μ′ (env, sequences) samples — random lengths included, so the
//! L = 0 ⇒ init contract is exercised through the entry guard path.

use cli::lift::{lift_ll, rustc_emit_ir, LiftError};
use harness::strategy::{MuPrime, Rng};
use term::eval_with_seqs;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Emit IR at -O1 (driver suppresses runtime unrolling), lift, gate against
/// the in-binary original over μ′ scalars AND parallel same-length sequences.
fn lift_and_gate_fold(tag: &str, src: &str, name: &str, arity: usize, k: usize,
                      orig: &dyn Fn(&[f64], &[Vec<f64>]) -> f64) -> term::Term
{
    let dir = std::env::temp_dir().join(format!("dge_lift_p2_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join(format!("{tag}.rs"));
    std::fs::write(&rs, src).unwrap();
    let ir = rustc_emit_ir(&rs, name).expect("rustc --emit=llvm-ir");
    let t = lift_ll(&ir, name).unwrap_or_else(|e| panic!("lift {name}: {e}"));
    assert_eq!(t.arity(), arity, "{name}: lifted scalar arity");
    assert_eq!(t.seq_count(), k, "{name}: lifted sequence count");
    assert!(t.has_fold(), "{name}: no Fold recovered");

    let mu = MuPrime::default_with_seed(0xF01D);
    let mut rng = Rng::new(0xF01D);
    let mut zero_len_seen = false;
    for i in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, arity.max(1), k);
        zero_len_seen |= sq.iter().any(|s| s.is_empty());
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (lv, ov) = (eval_with_seqs(&t, &e, &sl), orig(&e, &sq));
        assert!(xbit_eq(lv, ov),
            "{name}: FOLD DRIFT at sample {i}, env {e:?} seqs {sq:?}: interp={lv} rustc={ov}");
    }
    assert!(zero_len_seen, "μ′ never exercised L = 0 — the entry-guard path is untested");
    t
}

// ---------------------------------------------------------------- gates ----

/// The plainest fold: slice sum. The loop IR carries a 0-started index phi,
/// a 1-started tracker phi, and the zero-length entry guard — all of it
/// index machinery the recognizer records but never interprets.
fn orig_sum(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i];
    }
    acc
}

#[test]
fn p2_gate_sum_slice() {
    let src = r#"
pub fn sum_slice(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i];
    }
    acc
}
"#;
    lift_and_gate_fold("sum", src, "sum_slice", 0, 1, &|_, sq| orig_sum(&sq[0]));
}

/// Conditional update inside the body (fcmp + select) with a HOISTED scalar
/// (the cap is loop-invariant — Σ v1.2's outside-node semantics).
fn orig_capped(s: &[f64], cap: f64) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i];
        if acc > cap {
            acc = cap;
        }
    }
    acc
}

#[test]
fn p2_gate_capped_sum_with_scalar() {
    let src = r#"
pub fn capped_sum(s: &[f64], cap: f64) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i];
        if acc > cap {
            acc = cap;
        }
    }
    acc
}
"#;
    lift_and_gate_fold("capped", src, "capped_sum", 1, 1,
        &|e, sq| orig_capped(&sq[0], e[0]));
}

/// Two parallel sequences through iterator syntax: zip-dot. The IR trip
/// count is umin(len_a, len_b) — with the Σ same-length contract (asserted
/// by eval_with_seqs and guaranteed by μ′) umin(L, L) = L, and the gate
/// checks exactly that reading.
fn orig_dot(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc += x * y;
    }
    acc
}

#[test]
fn p2_gate_zip_dot_two_sequences() {
    let src = r#"
pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc += x * y;
    }
    acc
}
"#;
    lift_and_gate_fold("dot", src, "dot", 0, 2,
        &|_, sq| orig_dot(&sq[0], &sq[1]));
}

/// f64 post-processing AFTER the loop (exit-block straight-line tail):
/// Euclidean norm = sqrt applied to the fold result.
fn orig_norm(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i] * s[i];
    }
    acc.sqrt()
}

#[test]
fn p2_gate_fold_with_post_processing() {
    let src = r#"
pub fn norm(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i] * s[i];
    }
    acc.sqrt()
}
"#;
    lift_and_gate_fold("norm", src, "norm", 0, 1, &|_, sq| orig_norm(&sq[0]));
}

/// The PREHEADER shape: LICM hoists the loop-invariant `1.0 - alpha` into a
/// block between guard and loop — LLVM materializing exactly Σ v1.2's
/// outside-node hoisting semantics. The recognizer lifts preheader values as
/// outside (hoisted) nodes; the gate certifies the whole reading.
fn orig_ema(s: &[f64], alpha: f64) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc = alpha * s[i] + (1.0 - alpha) * acc;
    }
    acc
}

#[test]
fn p2_gate_ema_with_licm_preheader() {
    let src = r#"
pub fn ema(s: &[f64], alpha: f64) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc = alpha * s[i] + (1.0 - alpha) * acc;
    }
    acc
}
"#;
    lift_and_gate_fold("ema", src, "ema", 1, 1,
        &|e, sq| orig_ema(&sq[0], e[0]));
}

// ------------------------------------------------------------- refusals ----

fn emit(tag: &str, src: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("dge_lift_p2_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join(format!("{tag}.rs"));
    std::fs::write(&rs, src).unwrap();
    rustc_emit_ir(&rs, name).expect("rustc --emit=llvm-ir")
}

/// An f64-data-bound loop (`while x > lim`) is a recurrence with no runtime
/// LENGTH object — Term_p stays total, so there is no Σ reading. Refused by
/// name, from real rustc IR.
#[test]
fn p2_refuses_data_bound_while_loop() {
    let ir = emit("while", r#"
pub fn halve_down(mut x: f64, limit: f64) -> f64 {
    while x > limit {
        x = x * 0.5;
    }
    x
}
"#, "halve_down");
    match lift_ll(&ir, "halve_down") {
        Err(LiftError::Refused(m)) => assert!(
            m.contains("P2") && m.contains("LENGTH"), "must explain totality: {m}"),
        other => panic!("expected refusal, got {other:?}"),
    }
}

/// Two live accumulators (mean needs sum AND count-as-float, or here two
/// running sums) — Σ v1.2 is single-accumulator; refused with the roadmap.
#[test]
fn p2_refuses_multi_accumulator() {
    let ir = emit("twoacc", r#"
pub fn sum_and_sumsq(s: &[f64]) -> f64 {
    let mut a = 0.0;
    let mut b = 0.0;
    for i in 0..s.len() {
        a += s[i];
        b += s[i] * s[i];
    }
    a + b
}
"#, "sum_and_sumsq");
    match lift_ll(&ir, "sum_and_sumsq") {
        Err(LiftError::Refused(m)) => assert!(m.contains("P2"), "{m}"),
        Ok(t) => {
            // LLVM may legally rewrite two accumulators into one live-out
            // (reassociation is bitwise-visible, so it usually won't) — if a
            // future compiler does, the GATE decides, not this pin. Then this
            // arm should be replaced by a gate check.
            panic!("two-accumulator loop unexpectedly lifted ({} nodes) — \
                    re-examine and gate it", t.len());
        }
        Err(e) => panic!("expected P2 refusal, got {e:?}"),
    }
}

/// Index-dependent body values (i as f64) have no Σ reading — fold bodies
/// are index-blind (Acc/Elem only). Refused with the reason.
#[test]
fn p2_refuses_index_dependent_body() {
    let ir = emit("idx", r#"
pub fn weighted(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i] * (i as f64);
    }
    acc
}
"#, "weighted");
    match lift_ll(&ir, "weighted") {
        Err(LiftError::Refused(m)) => assert!(m.contains("P2"), "{m}"),
        other => panic!("expected P2 refusal, got {other:?}"),
    }
}

/// Cross-door agreement on the FOLD alphabet: the syn extractor's Σ v1.2
/// path and the IR door must recover behaviorally identical folds.
#[test]
fn p2_two_front_doors_agree_on_folds() {
    let src = r#"
pub fn sum_slice(s: &[f64]) -> f64 {
    let mut acc = 0.0;
    for i in 0..s.len() {
        acc += s[i];
    }
    acc
}
"#;
    let t_syn = cli::extract::extract_fn(src, "sum_slice").expect("syn door");
    let ir = emit("agree", src, "sum_slice");
    let t_ir = lift_ll(&ir, "sum_slice").expect("ir door");
    let mu = MuPrime::default_with_seed(0xA9EE);
    let mut rng = Rng::new(0xA9EE);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (a, c) = (eval_with_seqs(&t_syn, &e, &sl), eval_with_seqs(&t_ir, &e, &sl));
        assert!(xbit_eq(a, c), "front doors disagree at {e:?} {sq:?}: {a} vs {c}");
    }
}
