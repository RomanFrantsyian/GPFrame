//! Σ v1.4 — `Eq`, `Ne`, `Exp2`: the ops FIELD-TRIAL №1's fcmp-oeq bucket
//! actually priced (a bucketing bug had labeled them "fast-math"; the raw
//! refusals were easer's `t == 0.0` guards and `powf(2, x)`→llvm.exp2).
//!
//! The semantic heart: Rust's `==`/`!=` are the ASYMMETRIC IEEE pair
//! oeq/une — `NaN == NaN` is false but `NaN != NaN` is true, and
//! `-0.0 == 0.0` is true. Every layer (interp, emit, syn, lift, JIT) must
//! agree on that pair bitwise, pinned here.

use cli::lift::{lift_ll, rustc_emit_ir};
use cli::pipeline::{certify, Door, PipelineOpts};
use harness::strategy::{MuPrime, Rng};
use harness::{Gate, GateOutcome};
use rules::smt::{discharge_all, Z3Cli};
use term::{eval, sexpr};

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[test]
fn eq_ne_semantics_pins() {
    let eq = sexpr::parse("(eq (var 0) (var 1))").unwrap();
    let ne = sexpr::parse("(ne (var 0) (var 1))").unwrap();
    let nan = f64::NAN;
    // the asymmetric pair
    assert_eq!(eval(&eq, &[nan, nan]), 0.0, "NaN == NaN must be false (oeq)");
    assert_eq!(eval(&ne, &[nan, nan]), 1.0, "NaN != NaN must be true (une)");
    assert_eq!(eval(&eq, &[nan, 1.0]), 0.0);
    assert_eq!(eval(&ne, &[nan, 1.0]), 1.0);
    // signed zeros compare equal
    assert_eq!(eval(&eq, &[-0.0, 0.0]), 1.0, "-0.0 == +0.0 (IEEE)");
    assert_eq!(eval(&ne, &[-0.0, 0.0]), 0.0);
    assert_eq!(eval(&eq, &[2.0, 2.0]), 1.0);
    // sexpr round trips
    for t in [&eq, &ne] {
        assert_eq!(sexpr::print(&sexpr::parse(&sexpr::print(t)).unwrap()),
                   sexpr::print(t));
    }
    // exp2 = libm exp2 (O8: interp calls the same f64::exp2)
    let e2 = sexpr::parse("(exp2 (var 0))").unwrap();
    for x in [0.0, 1.0, 10.5, -3.25, f64::NAN, f64::INFINITY] {
        assert!(xbit_eq(eval(&e2, &[x]), x.exp2()));
    }
}

/// The REAL easer expo ease_in body (its `t == 0.0` guard was the priced
/// refusal; its `2f64.powf(…)` becomes llvm.exp2). Gated 10⁴ μ′, where NaN
/// t must take the guard's else (oeq: NaN ⇒ false) bitwise like rustc.
fn orig_expo_ease_in(t: f64, b: f64, c: f64, d: f64) -> f64 {
    if t == 0.0 {
        b
    } else {
        c * 2f64.powf(10.0 * (t / d - 1.0)) + b
    }
}

#[test]
fn v14_gate_expo_ease_in_through_the_ir_door() {
    let dir = std::env::temp_dir().join(format!("dge_v14_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("expo.rs");
    std::fs::write(&rs, r#"
pub fn expo_in(t: f64, b: f64, c: f64, d: f64) -> f64 {
    if t == 0.0 {
        b
    } else {
        c * 2f64.powf(10.0 * (t / d - 1.0)) + b
    }
}
"#).unwrap();
    let ir = rustc_emit_ir(&rs, "expo_in").expect("emit");
    let t = lift_ll(&ir, "expo_in").unwrap_or_else(|e| panic!("lift expo: {e}"));
    let s = sexpr::print(&t);
    assert!(s.contains("(eq ") && s.contains("(exp2 "), "{s}");

    let mu = MuPrime::default_with_seed(0xE014);
    let mut rng = Rng::new(0xE014);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, 4);
        let (lv, ov) = (eval(&t, &e), orig_expo_ease_in(e[0], e[1], e[2], e[3]));
        assert!(xbit_eq(lv, ov), "expo drift at {i}, {e:?}: {lv} vs {ov}");
    }
}

/// The REAL easer elastic ease_in shape: a CHAIN of equality guards with
/// early returns (rustc merges them into one n-way phi — the branch-tree
/// resolver from item 2 unfolds it) + sin + exp2 + π constants.
fn orig_elastic_in(t: f64, b: f64, c: f64, d: f64) -> f64 {
    use std::f64::consts::PI;
    if t == 0.0 {
        return b;
    }
    let t = t / d;
    if t == 1.0 {
        return b + c;
    }
    let p = d * 0.3;
    let s = p / 4.0;
    let t = t - 1.0;
    let post = c * 2f64.powf(10.0 * t);
    -(post * ((t * d - s) * (2.0 * PI) / p).sin()) + b
}

#[test]
fn v14_gate_elastic_guard_chain() {
    let dir = std::env::temp_dir().join(format!("dge_v14e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("elastic.rs");
    std::fs::write(&rs, r#"
pub fn elastic_in(t: f64, b: f64, c: f64, d: f64) -> f64 {
    use std::f64::consts::PI;
    if t == 0.0 {
        return b;
    }
    let t = t / d;
    if t == 1.0 {
        return b + c;
    }
    let p = d * 0.3;
    let s = p / 4.0;
    let t = t - 1.0;
    let post = c * 2f64.powf(10.0 * t);
    -(post * ((t * d - s) * (2.0 * PI) / p).sin()) + b
}
"#).unwrap();
    let ir = rustc_emit_ir(&rs, "elastic_in").expect("emit");
    let t = lift_ll(&ir, "elastic_in").unwrap_or_else(|e| panic!("lift elastic: {e}"));
    let mu = MuPrime::default_with_seed(0xE1A5);
    let mut rng = Rng::new(0xE1A5);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, 4);
        let (lv, ov) = (eval(&t, &e), orig_elastic_in(e[0], e[1], e[2], e[3]));
        assert!(xbit_eq(lv, ov), "elastic drift at {i}, {e:?}: {lv} vs {ov}");
    }
}

/// O7: the JIT door's Equal/NotEqual CLIF codes must be the exact
/// asymmetric pair — install's internal differential plus NaN spot checks.
#[test]
fn v14_jit_door_eq_ne_exp2() {
    let t = sexpr::parse(
        "(select (eq (var 0) 0.0) (var 1) (* (var 1) (exp2 (ne (var 0) (var 0)))))")
        .unwrap();
    let vt = match Gate::default_dial(0xE014).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(w) => unreachable!("identity refuted: {w:?}"),
    };
    let gate = Gate::default_dial(0xE015);
    let jf = jit::install(vt, &jit::LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("O7 install must pass for Eq/Ne/Exp2: {e:?}"));
    for env in [[f64::NAN, 3.0], [-0.0, 5.0], [0.0, 7.0], [2.0, 11.0]] {
        assert!(xbit_eq(jf.call(&env), jf.interp(&env)), "at {env:?}");
    }
}

/// Pipeline closure: the emitted `==`/`exp2` forms must survive the syn
/// door's re-extraction in the emission gate.
#[test]
fn v14_pipeline_closure() {
    let art = std::env::temp_dir().join(format!("dge_v14_art_{}", std::process::id()));
    if !Z3Cli::available() {
        eprintln!("z3 not installed; skipping");
        return;
    }
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&art));
    let dir = std::env::temp_dir().join(format!("dge_v14_pl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("g.rs");
    std::fs::write(&rs, r#"
pub fn gated_scale(x: f64, y: f64) -> f64 {
    if x == 0.0 { y } else { y * 2f64.powf(x) }
}
"#).unwrap();
    let opts = PipelineOpts { lift: true, artifacts: art, ..Default::default() };
    let c = certify(rs.to_str().unwrap(), "gated_scale", &opts).expect("pipeline");
    assert_eq!(c.door, Door::Lift);
    assert!(c.code.contains("==") && c.code.contains("exp2"),
        "emitted v1.4 forms missing:\n{}", c.code);
}
