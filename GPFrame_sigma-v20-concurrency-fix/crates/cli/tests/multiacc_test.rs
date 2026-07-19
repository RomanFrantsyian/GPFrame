//! Σ v1.5 multi-accumulator folds (FISSION) — gated at every layer.
//!
//! Design: N f64 accumulator phis / N mutated scalars become N SIBLING
//! Σ Folds over the same 0..L iteration space. Σ itself is UNCHANGED —
//! fold_owners, the interpreter, the JIT loop codegen, and the emitter
//! already accept sibling folds; only the two front doors refused. The
//! fission soundness precondition (no accumulator's update slice reads a
//! co-accumulator) is checked by both recognizers; coupled recurrences
//! (Welford-style) refuse with the roadmap vocabulary.
//!
//! Gates here: the MEASURED variance shape (LLVM sinks s*s into the LCSSA
//! tail and merges s² — arms mix raw next-values and tail values), a
//! no-tail two-phi merge, three accumulators, min/max, shared-scalar
//! invariants (the node-duplication discipline), cross-door bitwise
//! agreement, the full pipeline closure through the syn door, coupling
//! refusals at BOTH doors, and the O7 JIT door.

use cli::extract::extract_fn;
use cli::lift::{lift_ll, rustc_emit_ir};
use harness::strategy::{MuPrime, Rng};
use term::{eval_with_seqs, sexpr, Op};

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn emit(tag: &str, src: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("dge_ma_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join(format!("{name}.rs"));
    std::fs::write(&rs, src).unwrap();
    rustc_emit_ir(&rs, name).expect("rustc IR emission")
}

fn fold_count(t: &term::Term) -> usize {
    t.nodes.iter().filter(|n| n.op == Op::Fold).count()
}

// ---------------------------------------------------------------- gates --

/// The shape that MEASURED the feature: rustc -O1 sinks `s*s` into the
/// LCSSA tail, so the merge's two f64 phis carry {q, s²} — one raw
/// next-value arm and one tail-value arm. 10⁴ μ′ vs the in-binary original,
/// L = 0 exercised (variance of the empty set: NaN on both sides — the
/// mathematically honest answer).
fn orig_variance(xs: &[f64]) -> f64 {
    let mut s = 0.0;
    let mut q = 0.0;
    for i in 0..xs.len() {
        s += xs[i];
        q += xs[i] * xs[i];
    }
    let n = xs.len() as f64;
    (q - s * s / n) / n
}

#[test]
fn variance_gate_through_the_ir_door() {
    let ir = emit("var", r#"
pub fn variance(xs: &[f64]) -> f64 {
    let mut s = 0.0;
    let mut q = 0.0;
    for i in 0..xs.len() { s += xs[i]; q += xs[i] * xs[i]; }
    let n = xs.len() as f64;
    (q - s * s / n) / n
}
"#, "variance");
    let t = lift_ll(&ir, "variance").unwrap_or_else(|e| panic!("lift variance: {e}"));
    assert_eq!(fold_count(&t), 2, "fission must produce two sibling folds");

    let mu = MuPrime::default_with_seed(0x1A50);
    let mut rng = Rng::new(0x1A50);
    let mut zero_seen = false;
    for i in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        zero_seen |= sq[0].is_empty();
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (lv, ov) = (eval_with_seqs(&t, &e, &sl), orig_variance(&sq[0]));
        assert!(xbit_eq(lv, ov), "variance drift at {i}, {sq:?}: {lv} vs {ov}");
    }
    assert!(zero_seen, "L = 0 (the NaN case) must be exercised");
}

/// No-tail merge shape: sum and product combined AFTER the merge block, so
/// the exit carries two raw next-value phis whose entry arms must bit-match
/// their inits (0.0 and 1.0) — the strict arm check of the fission merge.
/// Cross-door: syn's fission and the IR door's fission must agree bitwise,
/// and both must agree with the rustc-compiled original.
fn orig_sum_prod(a: &[f64]) -> f64 {
    let mut s = 0.0;
    let mut p = 1.0;
    for i in 0..a.len() {
        s += a[i];
        p = p * a[i];
    }
    s + p
}

#[test]
fn sum_and_product_cross_door() {
    const SRC: &str = r#"
pub fn sum_prod(a: &[f64]) -> f64 {
    let mut s = 0.0;
    let mut p = 1.0;
    for i in 0..a.len() {
        s += a[i];
        p = p * a[i];
    }
    s + p
}
"#;
    let ts = extract_fn(SRC, "sum_prod").expect("syn door fissions");
    let ir = emit("sp", SRC, "sum_prod");
    let ti = lift_ll(&ir, "sum_prod").unwrap_or_else(|e| panic!("IR door fissions: {e}"));
    assert_eq!(fold_count(&ts), 2);
    assert_eq!(fold_count(&ti), 2);

    let mu = MuPrime::default_with_seed(0x1A51);
    let mut rng = Rng::new(0x1A51);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (sv, iv, ov) = (eval_with_seqs(&ts, &e, &sl),
                            eval_with_seqs(&ti, &e, &sl),
                            orig_sum_prod(&sq[0]));
        assert!(xbit_eq(sv, iv), "cross-door drift at {sq:?}: {sv} vs {iv}");
        assert!(xbit_eq(iv, ov), "vs original at {sq:?}: {iv} vs {ov}");
    }
}

/// Three accumulators (first three raw moments) — fission is N-way, not a
/// two-accumulator special case.
fn orig_moments(a: &[f64], w: f64) -> f64 {
    let mut m1 = 0.0;
    let mut m2 = 0.0;
    let mut m3 = 0.0;
    for i in 0..a.len() {
        m1 += a[i] * w;
        m2 += a[i] * a[i] * w;
        m3 += a[i] * a[i] * a[i] * w;
    }
    m1 + 2.0 * m2 + 3.0 * m3
}

#[test]
fn three_accumulators_with_a_shared_scalar() {
    // the SHARED SCALAR `w` is the node-duplication discipline's test: read
    // by every body, it must be materialized per-fold (never shared between
    // sibling bodies), on BOTH doors.
    const SRC: &str = r#"
pub fn moments(a: &[f64], w: f64) -> f64 {
    let mut m1 = 0.0;
    let mut m2 = 0.0;
    let mut m3 = 0.0;
    for i in 0..a.len() {
        m1 += a[i] * w;
        m2 += a[i] * a[i] * w;
        m3 += a[i] * a[i] * a[i] * w;
    }
    m1 + 2.0 * m2 + 3.0 * m3
}
"#;
    let ts = extract_fn(SRC, "moments").expect("syn door fissions 3-way");
    let ir = emit("mom", SRC, "moments");
    let ti = lift_ll(&ir, "moments").unwrap_or_else(|e| panic!("IR door fissions 3-way: {e}"));
    assert_eq!(fold_count(&ts), 3);
    assert_eq!(fold_count(&ti), 3);

    let mu = MuPrime::default_with_seed(0x1A52);
    let mut rng = Rng::new(0x1A52);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (sv, iv, ov) = (eval_with_seqs(&ts, &e, &sl),
                            eval_with_seqs(&ti, &e, &sl),
                            orig_moments(&sq[0], e[0]));
        assert!(xbit_eq(sv, iv), "cross-door drift at w={} {sq:?}: {sv} vs {iv}", e[0]);
        assert!(xbit_eq(iv, ov), "vs original at w={} {sq:?}: {iv} vs {ov}", e[0]);
    }
}

/// min + max in one pass (data range). NaN/±0 boundaries matter here:
/// f64::min/max drop a NaN operand in favor of the other — μ′ exercises
/// exactly those.
fn orig_range(v: &[f64], lo0: f64, hi0: f64) -> f64 {
    let mut lo = lo0;
    let mut hi = hi0;
    for i in 0..v.len() {
        lo = lo.min(v[i]);
        hi = hi.max(v[i]);
    }
    hi - lo
}

#[test]
fn min_max_range_cross_door() {
    const SRC: &str = r#"
pub fn range(v: &[f64], lo0: f64, hi0: f64) -> f64 {
    let mut lo = lo0;
    let mut hi = hi0;
    for i in 0..v.len() {
        lo = lo.min(v[i]);
        hi = hi.max(v[i]);
    }
    hi - lo
}
"#;
    let ts = extract_fn(SRC, "range").expect("syn door fissions min/max");
    let ir = emit("rng", SRC, "range");
    let ti = lift_ll(&ir, "range").unwrap_or_else(|e| panic!("IR door fissions min/max: {e}"));
    assert_eq!(fold_count(&ts), 2);
    assert_eq!(fold_count(&ti), 2);

    let mu = MuPrime::default_with_seed(0x1A53);
    let mut rng = Rng::new(0x1A53);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 2, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (sv, iv, ov) = (eval_with_seqs(&ts, &e, &sl),
                            eval_with_seqs(&ti, &e, &sl),
                            orig_range(&sq[0], e[0], e[1]));
        assert!(xbit_eq(sv, iv), "cross-door drift at {e:?} {sq:?}: {sv} vs {iv}");
        assert!(xbit_eq(iv, ov), "vs original at {e:?} {sq:?}: {iv} vs {ov}");
    }
}

// ------------------------------------------------------------- refusals --

/// Coupled recurrences (one accumulator's update reads another) have no
/// fission — BOTH doors refuse with the same roadmap vocabulary. The shape
/// is Welford's running mean/M2, the canonical coupled pair.
#[test]
fn coupled_recurrence_refuses_at_both_doors() {
    const SRC: &str = r#"
pub fn welfordish(a: &[f64]) -> f64 {
    let mut m = 0.0;
    let mut q = 0.0;
    for i in 0..a.len() {
        q += (a[i] - m) * a[i];
        m += a[i];
    }
    q
}
"#;
    match extract_fn(SRC, "welfordish") {
        Err(cli::extract::ExtractError::Unsupported(m)) =>
            assert!(m.contains("co-accumulator") && m.contains("fission"), "{m}"),
        other => panic!("syn door must refuse coupling: {other:?}"),
    }
    let ir = emit("wf", SRC, "welfordish");
    match lift_ll(&ir, "welfordish") {
        Err(cli::lift::LiftError::Refused(m)) =>
            assert!(m.contains("co-accumulator") && m.contains("fission"), "{m}"),
        other => panic!("IR door must refuse coupling: {other:?}"),
    }
}

// ------------------------------------------------- pipeline closure + O7 --

/// The closure: lift variance through the pipeline, emit Rust containing TWO
/// sibling fold blocks, and have the SYN door re-extract the emission — the
/// doors certify each other, now over fissioned terms. Independent third
/// differential vs the in-binary original.
#[test]
fn fission_pipeline_closure_variance() {
    use cli::pipeline::{certify, Door, PipelineOpts};
    use rules::smt::{discharge_all, Z3Cli};
    let art = std::env::temp_dir().join(format!("dge_ma_art_{}", std::process::id()));
    if !Z3Cli::available() {
        eprintln!("z3 not installed; skipping");
        return;
    }
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&art));

    let dir = std::env::temp_dir().join(format!("dge_ma_pl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("variance.rs");
    std::fs::write(&rs, r#"
pub fn variance(xs: &[f64]) -> f64 {
    let mut s = 0.0;
    let mut q = 0.0;
    for i in 0..xs.len() { s += xs[i]; q += xs[i] * xs[i]; }
    let n = xs.len() as f64;
    (q - s * s / n) / n
}
"#).unwrap();

    let opts = PipelineOpts { lift: true, artifacts: art, ..Default::default() };
    let c = certify(rs.to_str().unwrap(), "variance", &opts).expect("pipeline");
    assert_eq!(c.door, Door::Lift);
    assert!(c.code.matches("for __i in").count() >= 2,
        "emission must contain two sibling fold loops:\n{}", c.code);

    let t = extract_fn(&c.code, "variance_dge").expect("syn re-reads sibling folds");
    let mu = MuPrime::default_with_seed(0x1A54);
    let mut rng = Rng::new(0x1A54);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(xbit_eq(eval_with_seqs(&t, &e, &sl), orig_variance(&sq[0])),
            "certified variance drifts at {sq:?}");
    }
}

/// O7 discipline: install a hand-built two-fold term through the JIT door
/// (its internal 10⁴-sample differential must pass — sequential loop codegen
/// for sibling folds), then spot-check the seq API incl. L = 0.
#[test]
fn fission_jit_door_differential() {
    use harness::{Gate, GateOutcome};
    // sum ⊕ sum-of-squares, combined outside the folds
    let t = sexpr::parse(
        "(- (fold 0.0 (+ acc (* (elem 0) (elem 0)))) (fold 0.0 (+ acc (elem 0))))",
    ).unwrap();
    assert_eq!(fold_count(&t), 2);
    let vt = match Gate::default_dial(0x1A55).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(w) => unreachable!("identity gate refuted: {w:?}"),
    };
    let gate = Gate::default_dial(0x1A56);
    let jf = jit::install(vt, &jit::LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("O7 install must pass for sibling folds: {e:?}"));
    let mu = MuPrime::default_with_seed(0x1A57);
    let mut rng = Rng::new(0x1A57);
    for _ in 0..2_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, 1, 1);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (iv, jv) = (jf.interp_seq(&e, &sl), jf.call_seq(&e, &sl));
        assert!(xbit_eq(iv, jv), "jit/interp fission drift at {sq:?}: {iv} vs {jv}");
    }
}
