//! [O7] The SECOND typestate door: VerifiedTerm → JitFn, ONLY through the
//! differential gate. Same pattern as harness::Gate::promote.
//!
//! O7: ∀ e ∈ V_gate: jit(T)(e) bitwise== interp(T)(e), with the metric
//! relaxing to Metric::fma_mixed() iff fma_contraction (SPEC CORRECTION:
//! not "≤1 ULP" — see lower.rs) — flag AND metric enter the certificate.
//!
//! L1 discipline: a mismatch here is a COMPILER-SEMANTICS FINDING, not a
//! term bug — the term was already verified. The interp remains permanent
//! fallback and oracle.

use crate::lower::{lower, Compiled, LowerConfig, LowerError};
use harness::metric::Metric;
use harness::strategy::Rng;
use harness::{shrink, Gate, VerifiedTerm};

/// A jitted function. NO public constructor — `install` is the only door.
/// Carries its VerifiedTerm: interp stays available as fallback and oracle.
pub struct JitFn {
    compiled: Compiled,
    source: VerifiedTerm,
    /// metric the O7 gate used (recorded evidence)
    o7_metric: Metric,
    o7_samples: u64,
}

impl JitFn {
    pub fn call(&self, args: &[f64]) -> f64 {
        assert!(
            args.len() >= self.source.term().arity(),
            "env narrower than term arity"
        );
        // SAFETY: length checked above; RawFn reads at most arity() f64s.
        unsafe { (self.compiled.raw)(args.as_ptr()) }
    }

    /// Permanent oracle/fallback path.
    pub fn interp(&self, args: &[f64]) -> f64 {
        term::eval(self.source.term(), args)
    }

    pub fn source(&self) -> &VerifiedTerm { &self.source }
    pub fn o7_evidence(&self) -> (Metric, u64) { (self.o7_metric, self.o7_samples) }
}

#[derive(Debug)]
pub enum InstallError {
    Lowering(String),
    /// O7 refutation: compiler bug or env drift — with the shrunk witness.
    DifferentialMismatch {
        minimal_env: Vec<f64>,
        jit_val: f64,
        interp_val: f64,
    },
}

/// O7 differential gate + install. `gate` supplies μ' and n (V_gate).
pub fn install(vt: VerifiedTerm, cfg: &LowerConfig, gate: &Gate) -> Result<JitFn, InstallError> {
    let compiled = match lower(vt.term(), cfg) {
        Ok(c) => c,
        Err(LowerError::Backend(s)) => return Err(InstallError::Lowering(s)),
    };

    // Metric selection (v2.1 §1, corrected at R2): bitwise unless contraction.
    let metric = if cfg.fma_contraction { Metric::fma_mixed() } else { Metric::Bitwise };

    let term = vt.term();
    let arity = term.arity().max(1);
    let mut rng = Rng::new(gate.mu.seed ^ 0x07_07_07);
    let jit_eval = |env: &[f64]| -> f64 {
        debug_assert!(env.len() >= arity);
        unsafe { (compiled.raw)(env.as_ptr()) }
    };

    for _ in 0..gate.n {
        let e = gate.mu.sample(&mut rng, arity);
        let jv = jit_eval(&e);
        let iv = term::eval(term, &e);
        if !metric.eq(jv, iv) {
            // T3 shrink to a minimal witness of the compiler mismatch.
            let mut fails = |env: &[f64]| !metric.eq(jit_eval(env), term::eval(term, env));
            let minimal = shrink::shrink(e, &mut fails);
            let (jm, im) = (jit_eval(&minimal), term::eval(term, &minimal));
            return Err(InstallError::DifferentialMismatch {
                minimal_env: minimal,
                jit_val: jm,
                interp_val: im,
            });
        }
    }

    Ok(JitFn {
        compiled,
        source: vt,
        o7_metric: metric,
        o7_samples: gate.n,
    })
}
