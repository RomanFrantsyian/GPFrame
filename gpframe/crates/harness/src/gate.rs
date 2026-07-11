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
use term::{eval, Term};

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
}

/// ⊏-minimal counterexample (T3) plus the raw hit that found it.
#[derive(Debug, Clone)]
pub struct CounterExample {
    pub minimal_env: Vec<f64>,
    pub original_env: Vec<f64>,
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
    pub fn promote(&self, candidate: Term, reference: &Term) -> GateOutcome {
        assert_eq!(candidate.arity(), reference.arity(), "arity mismatch at gate");
        let arity = reference.arity().max(1);
        let mut rng = Rng::new(self.mu.seed);

        for _ in 0..self.n {
            let e = self.mu.sample(&mut rng, arity);
            let cv = eval(&candidate, &e);
            let rv = eval(reference, &e);
            if !self.metric.eq(cv, rv) {
                // T3: descend to a ⊏-minimal counterexample.
                let mut fails = |env: &[f64]| {
                    !self.metric.eq(eval(&candidate, env), eval(reference, env))
                };
                let minimal = shrink::shrink(e.clone(), &mut fails);
                let (mcv, mrv) = (eval(&candidate, &minimal), eval(reference, &minimal));
                return GateOutcome::Refuted(Box::new(CounterExample {
                    minimal_env: minimal,
                    original_env: e,
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
                    mu_spec: self.mu.spec_string(),
                },
                rule_trace: vec![],
                env: EnvFingerprint::capture(),
                fma_contraction: false,
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
