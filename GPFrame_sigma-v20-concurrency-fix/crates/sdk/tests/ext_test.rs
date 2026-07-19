//! Σ-ext gates: pluggable operator semantics WITHOUT kernel edits.
//!
//! The load-bearing claims, each pinned here:
//! 1. A plugin op integrates end to end (register → term → gate →
//!    certified emission carrying the semantics tag).
//! 2. Registration grants zero authority: a lying op is refuted with a
//!    counterexample like any lying door.
//! 3. Determinism is ENFORCED: a nondeterministic op refutes ITSELF.
//! 4. Unregistered ops get an honest Refused report, never a panic
//!    through the SDK boundary.
//! 5. The JIT trampoline path is bit-identical to the interpreter
//!    (same registry closure on both sides, O7-differentialed).
//! 6. Terms with ext ops stay plain data: sexpr round-trips without the
//!    registry; conflicting re-registration refuses.

use harness::gate::{Gate, GateOutcome};
use harness::strategy::{MuPrime, Rng};
use sdk::{register_ext_op, Engine, ExtractRequest, FrontDoor, GateReport,
          Refusal, Term};
use std::sync::Arc;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// door yielding a fixed sexpr for one fn name
struct SexprDoor(&'static str, &'static str);
impl FrontDoor for SexprDoor {
    fn name(&self) -> &str { "sexpr-door" }
    fn extract(&self, r: &ExtractRequest) -> Result<Term, Refusal> {
        if r.fn_name != self.0 {
            return Err(Refusal(format!("only knows `{}`", self.0)));
        }
        sdk::sexpr::parse(self.1).map_err(|e| Refusal(format!("{e:?}")))
    }
}

#[test]
fn plugin_relu_certifies_and_certificate_names_the_semantics() {
    register_ext_op("relu", "1.0", "spec:max(x,0)", 1,
        |a| a[0].max(0.0)).unwrap();
    let mut e = Engine::bare(0xE07);
    // door ORDER is preference: certify certifies the FIRST admitted
    // door's term — the plugin door leads, the core-Σ syn door serves as
    // the independent cross-check
    e.register_door(Arc::new(SexprDoor("relu_like", "(ext:relu (var 0))")));
    e.register_door(Arc::new(sdk::SynDoor));
    let (outs, report) = e.certify(&ExtractRequest {
        source: "pub fn relu_like(x: f64) -> f64 { x.max(0.0) }",
        fn_name: "relu_like",
    });
    assert!(outs.iter().all(|(_, r)| r.is_ok()));
    match report {
        Some(GateReport::Promoted { n, emitted, .. }) => {
            assert_eq!(n, 10_000);
            // the certificate SAYS which plugin semantics the claim stands under
            assert!(emitted.contains("MODULO extension semantics"), "{emitted}");
            assert!(emitted.contains("relu@1.0#spec:max(x,0)"), "{emitted}");
            assert!(emitted.contains("relu("), "emits the plugin's own symbol");
        }
        other => panic!("agreeing semantics must promote: {other:?}"),
    }
}

#[test]
fn lying_ext_op_is_refuted_with_a_counterexample() {
    // claims to be sqrt; is not, off-boundary
    register_ext_op("sqrtish", "0.1", "lie", 1,
        |a| a[0].sqrt() + 1e-9).unwrap();
    let e = Engine::bare(0xBAD2);
    let cand = sdk::sexpr::parse("(ext:sqrtish (var 0))").unwrap();
    let refr = sdk::sexpr::parse("(sqrt (var 0))").unwrap();
    match e.gate("sqrtish", cand, &refr) {
        GateReport::Refuted(w) => {
            assert!(!xbit_eq(w.candidate_val, w.reference_val));
        }
        other => panic!("a lying op must be refuted: {other:?}"),
    }
}

#[test]
fn nondeterministic_ext_op_refutes_itself() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TICK: AtomicU64 = AtomicU64::new(0);
    register_ext_op("noisy", "0.1", "hidden-state", 1, |a| {
        a[0] + (TICK.fetch_add(1, Ordering::Relaxed) % 2) as f64
    }).unwrap();
    let e = Engine::bare(0x4015);
    let t = sdk::sexpr::parse("(ext:noisy (var 0))").unwrap();
    match e.gate("noisy", t.clone(), &t) {
        GateReport::Refuted(_) => {} // run 1 vs run 2 disagreed — correct
        other => panic!("hidden state must trip the determinism gate: {other:?}"),
    }
}

#[test]
fn unregistered_ext_op_is_refused_not_panicked() {
    let e = Engine::bare(1);
    let t = sdk::sexpr::parse("(ext:ghost (var 0))").unwrap();
    match e.gate("g", t.clone(), &t) {
        GateReport::Refused(m) => {
            assert!(m.contains("ghost") && m.contains("not registered"), "{m}");
        }
        other => panic!("unregistered must be an honest Refused: {other:?}"),
    }
}

#[test]
fn jit_trampoline_is_bit_identical_to_interp() {
    register_ext_op("gauss", "1.0", "spec:exp(-x*x)", 1,
        |a| (-a[0] * a[0]).exp()).unwrap();
    // ext op composed with core ops, through the REAL spine:
    // promote (identity) → install → O7 already differentialed; spot-check more
    let t = sdk::sexpr::parse(
        "(* 0.5 (ext:gauss (+ (var 0) (var 1))))").unwrap();
    let vt = match Gate::default_dial(0xE71).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(w) => unreachable!("identity refuted: {w:?}"),
    };
    let gate = Gate::default_dial(0xE72);
    let jf = jit::install(vt, &jit::LowerConfig::default(), &gate)
        .unwrap_or_else(|e| panic!("ext term must JIT via trampoline: {e:?}"));
    let mu = MuPrime::default_with_seed(0xE73);
    let mut rng = Rng::new(0xE73);
    for _ in 0..2_000u32 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 2, 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (a, b) = (jf.interp_seq(&env, &sl), jf.call_seq(&env, &sl));
        assert!(xbit_eq(a, b), "trampoline drift: {a:?} vs {b:?} at {env:?}");
    }
}

#[test]
fn ext_terms_are_plain_data_and_registration_is_principled() {
    // sexpr round trip needs NO registry (`unseen` is never registered)
    let src = "(ext:unseen (+ (var 0) (ext:unseen2 (var 1) 2.0)))";
    let t = sdk::sexpr::parse(src).unwrap();
    assert_eq!(sdk::sexpr::print(&t), src);
    assert!(t.has_ext());

    // idempotent re-register: fine; conflicting: refuses with the tag
    register_ext_op("stable", "1.0", "fp", 1, |a| a[0]).unwrap();
    register_ext_op("stable", "1.0", "fp", 1, |a| a[0]).unwrap();
    let err = register_ext_op("stable", "2.0", "other", 1, |a| a[0])
        .unwrap_err();
    assert!(err.contains("conflicting semantics"), "{err}");

    // arity discipline
    let err = register_ext_op("nope", "1.0", "fp", 3, |a| a[0]).unwrap_err();
    assert!(err.contains("arity 3"), "{err}");
}

#[test]
fn fold_bodies_may_use_ext_ops() {
    // soft-plus-ish accumulation: plugin semantics INSIDE a fold, gated
    // against a core-Σ equivalent — the fold machinery is ext-transparent
    register_ext_op("halfsq", "1.0", "spec:x*x*0.5", 1,
        |a| a[0] * a[0] * 0.5).unwrap();
    let e = Engine::bare(0xF01D);
    let cand = sdk::sexpr::parse(
        "(fold 0.0 (+ acc (ext:halfsq (elem 0))))").unwrap();
    let refr = sdk::sexpr::parse(
        "(fold 0.0 (+ acc (* (* (elem 0) (elem 0)) 0.5)))").unwrap();
    match e.gate("halfsq_sum", cand, &refr) {
        GateReport::Promoted { emitted, .. } => {
            assert!(emitted.contains("halfsq@1.0"), "{emitted}");
        }
        other => panic!("equivalent fold semantics must promote: {other:?}"),
    }
}
