//! Trial item 2 — diamond-CFG Select recovery, gate-arbitrated.
//!
//! An `if/else` whose arms both bind lets lowers at -O1 to a branch diamond;
//! all 10 of easer's P2+ refusals in FIELD-TRIAL №1 were this one shape.
//! The recognizer turns each phi at a 2-pred acyclic merge into
//! Select(cond, vTrue, vFalse). Σ's eager evaluation of both arms is sound
//! because the alphabet is total — pinned here by gating over full μ′
//! (NaN/±0/Inf/subnormal), where the unpicked arm routinely evaluates to
//! NaN/Inf and must simply drop.

use cli::lift::{lift_ll, rustc_emit_ir, LiftError};
use harness::strategy::{MuPrime, Rng};
use term::eval;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn emit(tag: &str, src: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("dge_dia_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join(format!("{tag}.rs"));
    std::fs::write(&rs, src).unwrap();
    rustc_emit_ir(&rs, name).expect("rustc --emit=llvm-ir")
}

/// The exact easer shape that priced this item, gated 10⁴ μ′ against the
/// in-binary original — including NaN t, where `fcmp olt` (NaN ⇒ false ⇒
/// else-arm) must match Σ Lt's 0.0 ⇒ else routing bitwise.
fn orig_ease_in_out(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / (d / 2.0);
    if t < 1.0 {
        c / 2.0 * t * t + b
    } else {
        let t = t - 1.0;
        -c / 2.0 * (t * (t - 2.0) - 1.0) + b
    }
}

#[test]
fn diamond_gate_ease_in_out() {
    let src = r#"
pub fn ease_in_out(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / (d / 2.0);
    if t < 1.0 {
        c / 2.0 * t * t + b
    } else {
        let t = t - 1.0;
        -c / 2.0 * (t * (t - 2.0) - 1.0) + b
    }
}
"#;
    let ir = emit("ease", src, "ease_in_out");
    let t = lift_ll(&ir, "ease_in_out").unwrap_or_else(|e| panic!("lift: {e}"));
    assert_eq!(t.arity(), 4);

    let mu = MuPrime::default_with_seed(0xD1A0);
    let mut rng = Rng::new(0xD1A0);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, 4);
        let (lv, ov) = (eval(&t, &e), orig_ease_in_out(e[0], e[1], e[2], e[3]));
        assert!(xbit_eq(lv, ov), "diamond drift at {i}, {e:?}: {lv} vs {ov}");
    }

    // cross-door: the syn extractor ALSO admits this fn (it reads the
    // source if/else) — two independent lowerings of branching code must
    // agree bitwise. The IR term has ~25 nodes, the syn term ~39; the claim
    // is behavior, not structure.
    let t2 = cli::extract::extract_fn(src, "ease_in_out").expect("syn door");
    let mut rng = Rng::new(0xD1A1);
    for _ in 0..10_000u32 {
        let e = mu.sample(&mut rng, 4);
        assert!(xbit_eq(eval(&t, &e), eval(&t2, &e)),
            "front doors disagree on the diamond at {e:?}");
    }
}

/// TRIANGLE (one pred IS the decider) from handwritten IR — pinned
/// deterministically since rustc often selects this shape away.
/// Semantics: x > 0 ? sqrt(x)*2+1 : x.
#[test]
fn triangle_gate_handwritten() {
    const IR: &str = r#"
define double @tri(double %x) {
start:
  %c = fcmp ogt double %x, 0.000000e+00
  br i1 %c, label %then, label %merge
then:
  %s = call double @llvm.sqrt.f64(double %x)
  %s2 = fmul double %s, 2.000000e+00
  %s3 = fadd double %s2, 1.000000e+00
  br label %merge
merge:
  %r = phi double [ %s3, %then ], [ %x, %start ]
  ret double %r
}
"#;
    let t = lift_ll(IR, "tri").expect("lift triangle");
    let orig = |x: f64| if x > 0.0 { x.sqrt() * 2.0 + 1.0 } else { x };
    let mu = MuPrime::default_with_seed(0xD1A2);
    let mut rng = Rng::new(0xD1A2);
    for _ in 0..10_000u32 {
        let e = mu.sample(&mut rng, 1);
        let (lv, ov) = (eval(&t, &e), orig(e[0]));
        assert!(xbit_eq(lv, ov), "triangle drift at {e:?}: {lv} vs {ov}");
    }
}

/// Nested diamonds (an if/else INSIDE an arm) — inner merge resolves before
/// the outer one in topo order, so this composes for free. Gated.
fn orig_nested(x: f64, y: f64) -> f64 {
    if x < y {
        let a = x * 2.0;
        if a < 1.0 { let b = a + y; b * b } else { a - y }
    } else {
        let c = y * 3.0;
        c + x
    }
}

#[test]
fn nested_diamond_gate() {
    let src = r#"
pub fn nested(x: f64, y: f64) -> f64 {
    if x < y {
        let a = x * 2.0;
        if a < 1.0 { let b = a + y; b * b } else { a - y }
    } else {
        let c = y * 3.0;
        c + x
    }
}
"#;
    let ir = emit("nested", src, "nested");
    match lift_ll(&ir, "nested") {
        Ok(t) => {
            let mu = MuPrime::default_with_seed(0xD1A3);
            let mut rng = Rng::new(0xD1A3);
            for _ in 0..10_000u32 {
                let e = mu.sample(&mut rng, 2);
                let (lv, ov) = (eval(&t, &e), orig_nested(e[0], e[1]));
                assert!(xbit_eq(lv, ov), "nested drift at {e:?}: {lv} vs {ov}");
            }
        }
        // rustc may emit a shape (shared arm, cross-block edge) outside the
        // v1 grammar — then the refusal must carry the roadmap vocabulary
        Err(LiftError::Refused(m)) => assert!(m.contains("roadmap"), "{m}"),
        Err(e) => panic!("unexpected: {e:?}"),
    }
}

/// Refusal pin: an INTEGER branch condition has no Σ reading.
#[test]
fn diamond_refuses_integer_condition() {
    const IR: &str = r#"
define double @f(double %x, i64 %n) {
start:
  %c = icmp eq i64 %n, 0
  br i1 %c, label %a, label %bb
a:
  %v = fmul double %x, 2.000000e+00
  br label %m
bb:
  %w = fmul double %x, 3.000000e+00
  br label %m
m:
  %r = phi double [ %v, %a ], [ %w, %bb ]
  ret double %r
}
"#;
    match lift_ll(IR, "f") {
        Err(LiftError::Refused(m)) => assert!(
            // refused at the params (bare i64) or at the condition — either
            // names the same fact: integer data has no Σ reading
            m.contains("Σ reading") || m.contains("Sigma") || m.contains("roadmap"),
            "{m}"),
        other => panic!("expected refusal, got {other:?}"),
    }
}

/// An else-if chain collapses to ONE 3-way phi in LLVM; the branch-tree
/// resolver unfolds it back into nested Selects. Gated against the source
/// semantics: x<0 -> 1, x<1 -> 2, else 3 (NaN takes every else: 3).
#[test]
fn nway_merge_unfolds_to_nested_selects() {
    const IR: &str = r#"
define double @f(double %x) {
start:
  %c1 = fcmp olt double %x, 0.000000e+00
  br i1 %c1, label %m, label %k
k:
  %c2 = fcmp olt double %x, 1.000000e+00
  br i1 %c2, label %m, label %j
j:
  br label %m
m:
  %r = phi double [ 1.000000e+00, %start ], [ 2.000000e+00, %k ], [ 3.000000e+00, %j ]
  ret double %r
}
"#;
    let t = lift_ll(IR, "f").expect("3-way merge lifts via the branch tree");
    let orig = |x: f64| if x < 0.0 { 1.0 } else if x < 1.0 { 2.0 } else { 3.0 };
    let mu = MuPrime::default_with_seed(0xD1A4);
    let mut rng = Rng::new(0xD1A4);
    for _ in 0..10_000u32 {
        let e = mu.sample(&mut rng, 1);
        assert!(xbit_eq(eval(&t, &e), orig(e[0])), "else-if drift at {e:?}");
    }
}
