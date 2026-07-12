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
    /// Scalar-term entry (seq_count == 0).
    pub fn call(&self, args: &[f64]) -> f64 {
        assert!(
            args.len() >= self.source.term().arity(),
            "env narrower than term arity"
        );
        assert_eq!(self.source.term().seq_count(), 0,
            "sequence-bearing term: use call_seq");
        // SAFETY: length checked; no sequence loads occur (seq_count == 0).
        unsafe { (self.compiled.raw)(args.as_ptr(), std::ptr::null(), 0) }
    }

    /// Σ v1.2 entry: parallel same-length sequences.
    pub fn call_seq(&self, args: &[f64], seqs: &[&[f64]]) -> f64 {
        let t = self.source.term();
        assert!(args.len() >= t.arity(), "env narrower than term arity");
        assert!(seqs.len() >= t.seq_count(), "need {} sequences", t.seq_count());
        let len = seqs.first().map(|s| s.len()).unwrap_or(0);
        assert!(seqs.iter().all(|s| s.len() == len), "sequences must share length");
        let ptrs: Vec<*const f64> = seqs.iter().map(|s| s.as_ptr()).collect();
        // SAFETY: pointer table covers seq_count(); every slice has `len`
        // elements; the compiled loop reads indices < len only.
        unsafe { (self.compiled.raw)(args.as_ptr(), ptrs.as_ptr(), len as i64) }
    }

    /// Permanent oracle/fallback path.
    pub fn interp(&self, args: &[f64]) -> f64 {
        term::eval(self.source.term(), args)
    }

    pub fn interp_seq(&self, args: &[f64], seqs: &[&[f64]]) -> f64 {
        term::eval_with_seqs(self.source.term(), args, seqs)
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
        minimal_seqs: Vec<Vec<f64>>,
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

    // Metric selection (v2.1 §1, corrected at R2; NaN-class per Finding 7:
    // cranelift and rustc are different NaN generators, so cross-generator
    // "bitwise" means bitwise-modulo-NaN-payload).
    let metric = if cfg.fma_contraction { Metric::fma_mixed() } else { Metric::BitwiseNanClass };

    let term = vt.term();
    let arity = term.arity().max(1);
    let seq_count = term.seq_count();
    term.fold_owners().map_err(InstallError::Lowering)?;
    let mut rng = Rng::new(gate.mu.seed ^ 0x07_07_07);
    let jit_eval = |env: &[f64], seqs: &[Vec<f64>]| -> f64 {
        debug_assert!(env.len() >= arity);
        let ptrs: Vec<*const f64> = seqs.iter().map(|s| s.as_ptr()).collect();
        let len = seqs.first().map(|s| s.len()).unwrap_or(0);
        unsafe { (compiled.raw)(env.as_ptr(), ptrs.as_ptr(), len as i64) }
    };
    let interp_eval = |env: &[f64], seqs: &[Vec<f64>]| -> f64 {
        let sl: Vec<&[f64]> = seqs.iter().map(|v| v.as_slice()).collect();
        term::eval_with_seqs(term, env, &sl)
    };

    for _ in 0..gate.n {
        let (e, sq) = gate.mu.sample_with_seqs(&mut rng, arity, seq_count);
        let jv = jit_eval(&e, &sq);
        let iv = interp_eval(&e, &sq);
        if !metric.eq(jv, iv) {
            // T3 shrink to a minimal witness of the compiler mismatch.
            let mut fails = |env: &[f64], seqs: &[Vec<f64>]|
                !metric.eq(jit_eval(env, seqs), interp_eval(env, seqs));
            let (menv, mseqs) = shrink::shrink_seq(e, sq, &mut fails);
            let (jm, im) = (jit_eval(&menv, &mseqs), interp_eval(&menv, &mseqs));
            return Err(InstallError::DifferentialMismatch {
                minimal_env: menv,
                minimal_seqs: mseqs,
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
