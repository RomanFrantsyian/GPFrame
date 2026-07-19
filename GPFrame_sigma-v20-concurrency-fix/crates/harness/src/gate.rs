//! [T4 + T3 composition] The Gate — sole constructor of `VerifiedTerm`.
//!
//! Typestate spine (spec AST bottom line):
//!   Term --gate.promote--> VerifiedTerm --jit.install(O7)--> JitFn
//! `VerifiedTerm`'s field is PRIVATE and this module exposes no other way in;
//! the type system is the T8 exit-door.
//!
//! promote() pseudocode:
//!   δ_min = ln(1/α)/n                                  // T4, surfaced
//!   for k in 0..n:
//!       e = μ'.sample(rng, arity)
//!       if metric.eq(interp(cand,e), interp(refr,e)) fails:
//!           e* = shrink(e, |x| fails(x))               // T3 minimal CE
//!           return CounterExample{e*, ...}
//!   cert = Certificate{Tier::B{n,α,δ_min,μ'-spec}, env=O8 fingerprint}
//!   return VerifiedTerm{cand, cert}

use crate::cert::{Certificate, EnvFingerprint, Tier};
use crate::metric::Metric;
use crate::shrink;
use crate::strategy::{MuPrime, Rng};
use term::{eval_with_seqs, Term};

/// A term that has passed the gate. NO public constructor — `Gate::promote`
/// and (Tier A, R2) `Gate::promote_proved` are the only doors.
#[derive(Debug, Clone)]
pub struct VerifiedTerm {
    term: Term,               // private on purpose
    cert: Certificate,
}

impl VerifiedTerm {
    pub fn term(&self) -> &Term { &self.term }
    pub fn certificate(&self) -> &Certificate { &self.cert }

    /// Attach provenance (applied rule names) to the certificate. This is
    /// annotation, not evidence: it changes WHAT the trace says was done,
    /// never the claim's tier, metric, or quantification.
    pub fn annotate_rule_trace(&mut self, trace: Vec<String>) {
        self.cert.rule_trace = trace;
    }
}

/// ⊏-minimal counterexample (T3) plus the raw hit that found it.
#[derive(Debug, Clone)]
pub struct CounterExample {
    pub minimal_env: Vec<f64>,
    pub original_env: Vec<f64>,
    /// Σ v1.2: parallel sequence part of the witness (empty for scalar terms)
    pub minimal_seqs: Vec<Vec<f64>>,
    pub candidate_val: f64,
    pub reference_val: f64,
}

#[derive(Debug)]
pub enum GateOutcome {
    Promoted(VerifiedTerm),
    Refuted(Box<CounterExample>),
}

/// Gate parameters — (n, α, δ) are DERIVED DIALS, not vibes (v2.1 §2):
/// pick target (α, δ) ⇒ n ≥ ln(1/α)/δ; or pick n ⇒ δ_min = ln(1/α)/n.
#[derive(Debug, Clone)]
pub struct Gate {
    pub n: u64,
    pub alpha: f64,
    pub metric: Metric,
    pub mu: MuPrime,
}

impl Gate {
    /// Default dial: n = 10⁴, α = 10⁻³ ⇒ defects of measure ≥ 6.9e−4 caught
    /// with 99.9% confidence (T4, exactly).
    pub fn default_dial(seed: u64) -> Self {
        Gate { n: 10_000, alpha: 1e-3, metric: Metric::Semantic, mu: MuPrime::default_with_seed(seed) }
    }

    /// Derived dial: n from target (α, δ).
    pub fn for_target(alpha: f64, delta: f64, seed: u64) -> Self {
        let n = ((1.0 / alpha).ln() / delta).ceil() as u64;
        Gate { n, alpha, metric: Metric::Semantic, mu: MuPrime::default_with_seed(seed) }
    }

    pub fn delta_min(&self) -> f64 {
        (1.0 / self.alpha).ln() / self.n as f64
    }

    /// Tier-B promotion of `candidate` against `reference` (the definitional
    /// interpreter runs both — the judge never trusts a faster realization).
    /// Σ v1.2: sequence-bearing terms are judged over μ' extended with the
    /// parallel-sequence measure; the well-formedness of fold binders is
    /// checked FIRST (an ill-formed candidate is refused, not evaluated).
    pub fn promote(&self, candidate: Term, reference: &Term) -> GateOutcome {
        assert_eq!(candidate.arity(), reference.arity(), "arity mismatch at gate");
        let seq_count = candidate.seq_count().max(reference.seq_count());
        candidate.fold_owners().expect("candidate: ill-formed fold binders");
        reference.fold_owners().expect("reference: ill-formed fold binders");
        // Σ-ext pre-flight: every ext op registered (honest panic with the
        // remedy — SDK paths pre-check and refuse before reaching here),
        // and collect the certificate tags NOW so the claim names its
        // semantics.
        let has_ext = candidate.has_ext() || reference.has_ext();
        let mut ext_tags = term::ext::tags_for(&candidate.exts)
            .expect("gate: candidate uses unregistered extension ops");
        for t in term::ext::tags_for(&reference.exts)
            .expect("gate: reference uses unregistered extension ops") {
            if !ext_tags.contains(&t) { ext_tags.push(t); }
        }
        let arity = reference.arity().max(1);
        let mut rng = Rng::new(self.mu.seed);

        for _ in 0..self.n {
            let (e, sq) = self.mu.sample_with_seqs(&mut rng, arity, seq_count);
            let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
            let cv = eval_with_seqs(&candidate, &e, &sl);
            let rv = eval_with_seqs(reference, &e, &sl);
            // DETERMINISM PRE-GATE (Σ-ext): plugin semantics are enforced
            // deterministic, not assumed — double-run each sample; a
            // nondeterministic op REFUTES ITSELF (run 1 vs run 2 is the
            // counterexample; reference_val carries the first run).
            if has_ext {
                let cv2 = eval_with_seqs(&candidate, &e, &sl);
                if !self.metric.eq(cv, cv2) {
                    return GateOutcome::Refuted(Box::new(CounterExample {
                        minimal_env: e.clone(),
                        original_env: e,
                        minimal_seqs: sq,
                        candidate_val: cv2,
                        reference_val: cv,
                    }));
                }
                let rv2 = eval_with_seqs(reference, &e, &sl);
                if !self.metric.eq(rv, rv2) {
                    return GateOutcome::Refuted(Box::new(CounterExample {
                        minimal_env: e.clone(),
                        original_env: e,
                        minimal_seqs: sq,
                        candidate_val: rv2,
                        reference_val: rv,
                    }));
                }
            }
            if !self.metric.eq(cv, rv) {
                // T3: descend to a ⊏-minimal counterexample.
                let mut fails = |env: &[f64], seqs: &[Vec<f64>]| {
                    let sl: Vec<&[f64]> = seqs.iter().map(|v| v.as_slice()).collect();
                    !self.metric.eq(
                        eval_with_seqs(&candidate, env, &sl),
                        eval_with_seqs(reference, env, &sl),
                    )
                };
                let (menv, mseqs) = shrink::shrink_seq(e.clone(), sq, &mut fails);
                let msl: Vec<&[f64]> = mseqs.iter().map(|v| v.as_slice()).collect();
                let mcv = eval_with_seqs(&candidate, &menv, &msl);
                let mrv = eval_with_seqs(reference, &menv, &msl);
                return GateOutcome::Refuted(Box::new(CounterExample {
                    minimal_env: menv,
                    original_env: e,
                    minimal_seqs: mseqs,
                    candidate_val: mcv,
                    reference_val: mrv,
                }));
            }
        }

        GateOutcome::Promoted(VerifiedTerm {
            term: candidate,
            cert: Certificate {
                tier: Tier::B {
                    n: self.n,
                    alpha: self.alpha,
                    delta_min: self.delta_min(),
                    mu_spec: {
                        let mut s = self.mu.spec_string();
                        if seq_count > 0 {
                            s.push_str(crate::strategy::MuPrime::seq_spec_clause());
                        }
                        s
                    },
                },
                rule_trace: vec![],
                env: EnvFingerprint::capture(),
                fma_contraction: false,
                ext_semantics: ext_tags,
            },
        })
    }

    /// Tier-A door (used by `rules` at R2): the caller supplies the finished
    /// Tier-A certificate (rule trace + stored SMT artifacts, O1/O2).
    /// Kept here so `VerifiedTerm` construction stays inside this module —
    /// rules provides evidence; the judge mints the credential.
    pub fn promote_proved(candidate: Term, cert: Certificate) -> VerifiedTerm {
        assert!(matches!(cert.tier, Tier::A { .. }), "promote_proved requires Tier A evidence");
        // R2 TODO: verify artifact hashes against the on-disk O1 store
        // before minting; unverifiable artifact ⇒ panic, not downgrade.
        VerifiedTerm { term: candidate, cert }
    }
}
