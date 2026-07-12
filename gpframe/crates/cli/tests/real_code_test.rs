//! FIRST REAL-CODE RUN. Target: Cubic easing from easer 0.3.0 (MIT,
//! https://crates.io/crates/easer) — real published code, embedded verbatim
//! below (generic source for the extractor; monomorphized F=f64 originals
//! compiled by rustc as the ground-truth oracles).
//!
//! The chain under test:
//!   rustc-compiled original
//!     ==bitwise==  interp(extract(source))          [extraction gate]
//!     ==bitwise==  cranelift-jit(refactor(term))    [Tier A + O7]
//! over mu' (log-uniform magnitudes + NaN/±0/Inf/subnormal boundaries).

use cli::extract::extract_fn;
use harness::strategy::{MuPrime, Rng};
use harness::{Gate, GateOutcome, Tier};
use rules::extract::SaturationLimits;
use rules::smt::{discharge_all, Z3Cli};
use term::eval;

/// easer 0.3.0 src/functions/cubic.rs (MIT) — verbatim body, generic form.

/// Finding 7: cross-generator equality = exact bits OR both-NaN.
fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

const EASER_CUBIC_SRC: &str = r#"
impl<F: Float> Easing<F> for Cubic {
    fn ease_in(t: F, b: F, c: F, d: F) -> F {
        let t = t / d;
        c * (t * t * t) + b
    }

    fn ease_out(t: F, b: F, c: F, d: F) -> F {
        let t = t / d - f(1.0);
        c * ((t * t * t) + f(1.0)) + b
    }

    fn ease_in_out(t: F, b: F, c: F, d: F) -> F {
        let t = t / (d / f(2.0));
        if t < f(1.0) {
            c / f(2.0) * (t * t * t) + b
        }
        else {
            let t = t - f(2.0);
            c / f(2.0) * (t * t * t + f(2.0)) + b
        }
    }
}
"#;

// Monomorphized originals (F = f64, f = identity) — compiled by rustc;
// these are the GROUND TRUTH the extraction must match.
fn orig_ease_in(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / d;
    c * (t * t * t) + b
}
fn orig_ease_out(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / d - 1.0;
    c * ((t * t * t) + 1.0) + b
}
fn orig_ease_in_out(t: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = t / (d / 2.0);
    if t < 1.0 {
        c / 2.0 * (t * t * t) + b
    } else {
        let t = t - 2.0;
        c / 2.0 * (t * t * t + 2.0) + b
    }
}

fn extraction_gate(fn_name: &str, orig: fn(f64, f64, f64, f64) -> f64) {
    let t = extract_fn(EASER_CUBIC_SRC, fn_name)
        .unwrap_or_else(|e| panic!("extract {fn_name}: {e:?}"));
    assert_eq!(t.arity(), 4);
    let mu = MuPrime::default_with_seed(0xEA5E);
    let mut rng = Rng::new(0xEA5E);
    for i in 0..10_000u32 {
        let e = mu.sample(&mut rng, 4);
        let iv = eval(&t, &e);
        let ov = orig(e[0], e[1], e[2], e[3]);
        assert!(xbit_eq(iv, ov),
            "{fn_name}: extraction drift at sample {i}, env {e:?}: interp={iv} rustc={ov}");
    }
}

#[test]
fn extraction_gate_ease_in() { extraction_gate("ease_in", orig_ease_in); }

#[test]
fn extraction_gate_ease_out() { extraction_gate("ease_out", orig_ease_out); }

#[test]
fn extraction_gate_ease_in_out_with_branch() {
    // exercises the comparison encoding (t < 1.0, const RHS ⇒ exact) and
    // let-shadowing across branch scopes
    extraction_gate("ease_in_out", orig_ease_in_out);
}

#[test]
fn real_code_full_chain_rustc_to_jit_bitwise() {
    if !Z3Cli::available() { eprintln!("z3 not installed; skipping"); return; }
    let t = extract_fn(EASER_CUBIC_SRC, "ease_in_out").unwrap();

    // Tier-A refactor over the discharged rule set
    let dir = std::env::temp_dir().join(format!("dge_real_{}", std::process::id()));
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&dir));
    let gate = Gate::default_dial(0xC0DE);
    let out = rules::refactor(&t, false, &gate, &dir, &SaturationLimits::default())
        .expect("refactor real code");
    assert!(matches!(out.verified.certificate().tier, Tier::A { .. }));
    assert!(out.cost_after <= out.cost_before);

    // O7 install of the refactored term
    let jf = jit::install(out.verified, &jit::LowerConfig::default(), &gate)
        .expect("O7 must pass on real code");

    // The headline: rustc(original) ==bitwise== cranelift-jit(extracted+refactored)
    let mu = MuPrime::default_with_seed(7);
    let mut rng = Rng::new(7);
    for _ in 0..10_000u32 {
        let e = mu.sample(&mut rng, 4);
        assert!(xbit_eq(jf.call(&e), orig_ease_in_out(e[0], e[1], e[2], e[3])),
            "rustc vs jit divergence at {e:?}");
    }
    // sanity that the identity gate would also promote it (Tier B evidence)
    match Gate::default_dial(9).promote(t.clone(), &t) {
        GateOutcome::Promoted(_) => {}
        GateOutcome::Refuted(_) => unreachable!(),
    }
}
