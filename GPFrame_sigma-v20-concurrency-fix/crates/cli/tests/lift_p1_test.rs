//! v3-Exp P1 — the LLVM IR front door, gate-arbitrated.
//!
//! The chain under test (the P1 KPI from RFC-REVIEW-ir-lifting-v3):
//!
//!   rustc(source, -O1) ──emit=llvm-ir──▶ IR text ──lift──▶ Term_p
//!   rustc(source)      ──in this test binary──▶ the GROUND-TRUTH original
//!
//!   interp(lift(IR)) ==BitwiseNanClass== original, over 10⁴ μ′ samples
//!   (log-uniform magnitudes + NaN/±0/Inf/subnormal boundaries).
//!
//! The lifter is UNTRUSTED (L1): these gates are the only reason to believe
//! its recognition. Refusal tests pin the P1 contract's edges — br/phi,
//! memory ops, unknown calls, fast-math flags — each refused WITH the reason
//! and the phase that will admit it.

use cli::lift::{lift_ll, rustc_emit_ir, LiftError};
use harness::strategy::{MuPrime, Rng};
use term::eval;

/// Finding 7 discipline: cross-generator equality = exact bits OR both-NaN.
fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Write source to a temp .rs, emit IR at -O1, lift `name`, and run the
/// extraction gate against the in-binary rustc-compiled original.
fn lift_and_gate(tag: &str, src: &str, name: &str, arity: usize, orig: &dyn Fn(&[f64]) -> f64)
    -> term::Term
{
    let dir = std::env::temp_dir().join(format!("dge_lift_p1_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join(format!("{tag}.rs"));
    std::fs::write(&rs, src).unwrap();
    let ir = rustc_emit_ir(&rs, name).expect("rustc --emit=llvm-ir");
    let t = lift_ll(&ir, name).unwrap_or_else(|e| panic!("lift {name}: {e}"));
    assert_eq!(t.arity(), arity, "{name}: lifted arity");

    let mu = MuPrime::default_with_seed(0x11F7);
    let mut rng = Rng::new(0x11F7);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, arity);
        let (lv, ov) = (eval(&t, &e), orig(&e));
        assert!(xbit_eq(lv, ov),
            "{name}: LIFT DRIFT at sample {i}, env {e:?}: interp={lv} rustc={ov}");
    }
    t
}

// ---------------------------------------------------------------- gates ----

/// Straight-line arithmetic: the naive cubic from the user case study,
/// now entering through the IR door instead of the syn door.
fn orig_poly(x: f64) -> f64 {
    let term3 = 3.0 * x * x * x;
    let term2 = 5.0 * x * x;
    let term1 = 2.0 * x;
    term3 + term2 + term1 + 7.0
}

#[test]
fn p1_gate_straight_line_polynomial() {
    let src = r#"
pub fn poly(x: f64) -> f64 {
    let term3 = 3.0 * x * x * x;
    let term2 = 5.0 * x * x;
    let term1 = 2.0 * x;
    term3 + term2 + term1 + 7.0
}
"#;
    let t = lift_and_gate("poly", src, "poly", 1, &|e| orig_poly(e[0]));
    // and the point of the mission: the IR door and the syn door recover
    // the SAME math (identical interpretation over μ′ — not necessarily
    // identical trees, so compare behavior, not structure)
    let t2 = cli::extract::extract_fn(src, "poly").unwrap();
    let mu = MuPrime::default_with_seed(0xD00A);
    let mut rng = Rng::new(0xD00A);
    for _ in 0..10_000u32 {
        let e = mu.sample(&mut rng, 1);
        assert!(xbit_eq(eval(&t, &e), eval(&t2, &e)), "two front doors disagree at {e:?}");
    }
}

/// Syntax the syn extractor REFUSES — tuples, `match`, method sugar — but
/// whose instruction-level meaning is straight-line f64 math (SROA dissolves
/// the aggregates, `match` becomes fcmp+select, mul_add becomes @llvm.fma):
/// the mission statement's core claim, in a test.
fn orig_tuple_math(a: f64, b: f64, c: f64) -> f64 {
    let p = (a * b, b + c);
    let q = (p.0.mul_add(p.1, c), p.1);
    match q.1 > 0.0 {
        true => q.0,
        false => q.0 * q.1,
    }
}

#[test]
fn p1_gate_syntax_costume_lifts_where_syn_refuses() {
    let src = r#"
pub fn tuple_math(a: f64, b: f64, c: f64) -> f64 {
    let p = (a * b, b + c);
    let q = (p.0.mul_add(p.1, c), p.1);
    match q.1 > 0.0 {
        true => q.0,
        false => q.0 * q.1,
    }
}
"#;
    // the syn door refuses this costume…
    assert!(cli::extract::extract_fn(src, "tuple_math").is_err(),
        "premise broken: syn extractor now handles tuples/match — update this test");
    // …the IR door reads what the CPU is told, and the gate certifies it
    lift_and_gate("tuple", src, "tuple_math", 3,
        &|e| orig_tuple_math(e[0], e[1], e[2]));
}

/// The mission statement's central example, MEASURED: an iterator chain over
/// a fixed window — syntax the syn extractor refuses outright — is fully
/// inlined + unrolled by -O1 into straight-line fmul/fadd. The loop was a
/// costume; the instruction-level meaning is a 3-term dot product, and the
/// gate certifies P1 recovered exactly it. (The module also contains the
/// closure/fold symbols, whose mangled names EMBED `9iter_dot3` — the
/// exact-symbol-first matcher is what this test forced into existence.)
fn orig_iter_dot3(a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> f64 {
    [a0, a1, a2].iter().zip([b0, b1, b2].iter()).map(|(x, y)| x * y).sum()
}

#[test]
fn p1_gate_iterator_syntax_lifts_where_syn_refuses() {
    let src = r#"
pub fn iter_dot3(a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> f64 {
    [a0, a1, a2].iter().zip([b0, b1, b2].iter()).map(|(x, y)| x * y).sum()
}
"#;
    assert!(cli::extract::extract_fn(src, "iter_dot3").is_err(),
        "premise broken: syn extractor now handles iterator chains — update this test");
    lift_and_gate("iterdot", src, "iter_dot3", 6,
        &|e| orig_iter_dot3(e[0], e[1], e[2], e[3], e[4], e[5]));
}

/// Real-rustc P2 refusal: a runtime-bound `while` cannot unroll — its -O1 IR
/// is a genuine CFG with phi nodes, and P1 refuses it BY PHASE NAME instead
/// of guessing. (The handwritten-IR refusal tests pin the parser; this one
/// pins the claim against IR rustc actually emits.)
#[test]
fn p1_refuses_runtime_loop_from_real_rustc_ir() {
    let dir = std::env::temp_dir().join(format!("dge_lift_p1_{}_while", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("halve.rs");
    std::fs::write(&rs, r#"
pub fn halve_down(mut x: f64, limit: f64) -> f64 {
    while x > limit {
        x = x * 0.5;
    }
    x
}
"#).unwrap();
    let ir = rustc_emit_ir(&rs, "halve_down").expect("rustc --emit=llvm-ir");
    match lift_ll(&ir, "halve_down") {
        Err(LiftError::Refused(m)) => assert!(m.contains("P2"),
            "refusal must name the admitting phase: {m}"),
        other => panic!("expected P2 refusal for a runtime loop, got {other:?}"),
    }
}

/// Transcendentals through the libm call map (O8: interp and the original
/// link the SAME libm in this process — bitwise agreement is the claim).
fn orig_wave(x: f64, y: f64) -> f64 {
    (x.sin() * y.cos() + (x * y).exp().sqrt()).abs()
}

#[test]
fn p1_gate_libm_call_map() {
    let src = r#"
pub fn wave(x: f64, y: f64) -> f64 {
    (x.sin() * y.cos() + (x * y).exp().sqrt()).abs()
}
"#;
    lift_and_gate("wave", src, "wave", 2, &|e| orig_wave(e[0], e[1]));
}

/// min/max lower to llvm.minnum/maxnum — the SAME Rust semantics O7 pinned
/// when it refuted CLIF fmin's NaN propagation. NaN inputs are in μ′.
fn orig_clamp(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}

#[test]
fn p1_gate_minnum_maxnum_nan_semantics() {
    let src = r#"
pub fn clamp3(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}
"#;
    lift_and_gate("clamp", src, "clamp3", 3, &|e| orig_clamp(e[0], e[1], e[2]));
}

/// fcmp + select from clang-shaped IR (verbatim clang -O1 output shape:
/// dso_local, noundef, local_unnamed_addr, hex float constants) — one
/// lifter, two languages. Semantics: x < y ? x*k : y (k = 0.5 as hex bits).
#[test]
fn p1_gate_clang_shaped_select() {
    const CLANG_IR: &str = r#"
; ModuleID = 'sel.c'
source_filename = "sel.c"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

; Function Attrs: mustprogress nofree norecurse nosync nounwind willreturn memory(none) uwtable
define dso_local double @pick_scaled(double noundef %0, double noundef %1) local_unnamed_addr #0 {
  %3 = fcmp olt double %0, %1
  %4 = fmul double %0, 0x3FE0000000000000
  %5 = select i1 %3, double %4, double %1
  ret double %5
}

attributes #0 = { mustprogress nofree norecurse nosync nounwind willreturn }
"#;
    let t = lift_ll(CLANG_IR, "pick_scaled").expect("lift clang-shaped IR");
    assert_eq!(t.arity(), 2);
    let orig = |x: f64, y: f64| if x < y { x * 0.5 } else { y };
    let mu = MuPrime::default_with_seed(0xC1A6);
    let mut rng = Rng::new(0xC1A6);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, 2);
        let (lv, ov) = (eval(&t, &e), orig(e[0], e[1]));
        assert!(xbit_eq(lv, ov), "clang select drift at {i}, {e:?}: {lv} vs {ov}");
    }
}

// ------------------------------------------------------------- refusals ----

fn ir_with_body(body: &str) -> String {
    format!("define double @f(double %0, double %1) {{\nstart:\n{body}\n}}\n")
}

#[test]
fn p1_refuses_multi_exit_branching_with_roadmap() {
    // Since trial item 2 (diamond recovery), single-exit branch trees LIFT;
    // this multi-exit shape (each arm returns) is the remaining refusal,
    // named as such. A merged-return version of the same math gates in
    // diamond_test.rs.
    let ir = ir_with_body(
        "  %2 = fcmp olt double %0, %1\n  br i1 %2, label %a, label %b\n\
         a:\n  ret double %0\nb:\n  ret double %1");
    match lift_ll(&ir, "f") {
        Err(LiftError::Refused(m)) => assert!(
            m.contains("multi-exit") && m.contains("roadmap"),
            "refusal must name the shape and the plan: {m}"),
        other => panic!("expected multi-exit refusal, got {other:?}"),
    }
}

#[test]
fn p1_refuses_memory_ops_with_p3_roadmap() {
    let ir = ir_with_body(
        "  %p = alloca double\n  store double %0, ptr %p\n  ret double %1");
    match lift_ll(&ir, "f") {
        Err(LiftError::Refused(m)) => assert!(m.contains("P3"), "must name P3: {m}"),
        other => panic!("expected P3 refusal, got {other:?}"),
    }
}

#[test]
fn p1_refuses_calls_outside_the_libm_map() {
    let ir = ir_with_body(
        "  %2 = call double @totally_legit_math(double %0)\n  ret double %2");
    match lift_ll(&ir, "f") {
        Err(LiftError::Refused(m)) => assert!(m.contains("libm map"), "{m}"),
        other => panic!("expected closed-map refusal, got {other:?}"),
    }
}

#[test]
fn p1_refuses_fast_math_flags() {
    let ir = ir_with_body("  %2 = fadd fast double %0, %1\n  ret double %2");
    match lift_ll(&ir, "f") {
        Err(LiftError::Refused(m)) => assert!(m.contains("IEEE"), "{m}"),
        other => panic!("expected fmf refusal, got {other:?}"),
    }
}

#[test]
fn p1_refuses_unsupported_fcmp_predicates() {
    // `one` (ordered-not-equal, NaN => false) is NOT Rust's `!=` (which is
    // une, NaN => true) — since Σ v1.4 admitted oeq/une, `one` is the pin
    // for the predicates that remain outside: no Rust operator means them.
    let ir = ir_with_body("  %2 = fcmp one double %0, %1\n  %3 = select i1 %2, \
                           double %0, double %1\n  ret double %3");
    match lift_ll(&ir, "f") {
        Err(LiftError::Refused(m)) => assert!(m.contains("one"), "{m}"),
        other => panic!("expected predicate refusal, got {other:?}"),
    }
}

/// Hex-bit constants must round-trip exactly (LLVM prints NaN/inf/most
/// values this way) — a wrong-bits parse would be caught by any gate, but
/// pin it directly with a signaling-pattern payload and −0.0.
#[test]
fn p1_hex_float_constants_are_bit_exact() {
    let ir = "define double @c(double %0) {\n\
              start:\n  %1 = fadd double %0, 0x8000000000000000\n  ret double %1\n}\n";
    let t = lift_ll(ir, "c").unwrap();
    assert_eq!(t.consts.len(), 1);
    assert_eq!(t.consts[0].to_bits(), (-0.0f64).to_bits(), "must parse as -0.0 exactly");
    // and the −0.0 trap from Finding 1 stays visible through the IR door:
    // x + (−0.0) ≡ x bitwise (unlike x + 0.0 at x = −0.0)
    assert_eq!(eval(&t, &[-0.0]).to_bits(), (-0.0f64).to_bits());
}
