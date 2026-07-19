//! crate `rules` — [D3 T1 O1 O2 O4 | R2] — refactoring. Deps: term, harness, egg.
//!
//! T1: if every rule in R is sound, extraction from the saturated e-graph
//! yields extract(P) ~ P. Whole-system correctness reduces to per-rule
//! obligations (O1/O2) — NO RULE SHIPS UNPROVED (R2 entry condition,
//! enforced at load: `smt::artifact_ok` per Dec rule).
//!
//! L2 (cost irrelevance): cost affects WHICH equal term you get, never
//! WHETHER it is equal ⇒ calibrate cost aggressively, zero soundness risk.

pub mod lang;
pub mod r_dec;
pub mod r_sem;
pub mod r_approx;
pub mod cost;
pub mod extract;
pub mod smt;

use crate::cost::{CostFn, DefaultCost};
use crate::extract::{saturate_and_extract, SaturationLimits};
use crate::lang::{from_egg, to_egg, translate_pattern, SigLang};
use egg::{Pattern, Rewrite};
use harness::{Certificate, EnvFingerprint, Gate, GateOutcome, Tier, VerifiedTerm};
use std::path::Path;
use term::Term;

/// One rewrite rule l → r with its obligation class.
pub struct Rule {
    pub name: &'static str,
    /// s-expression patterns over Σ with pattern vars ?a ?b … (human syntax;
    /// f64 literals allowed — translated to bitwise tokens for egg).
    pub lhs: &'static str,
    pub rhs: &'static str,
    pub class: RuleClass,
}

pub enum RuleClass {
    /// O1: [[l]]=[[r]] in QF_FP; UNSAT artifact stored on disk.
    Dec,
    /// O2: reviewed algebraic proof, compiled into the source (review =
    /// code review of this text). Entry condition: proof non-empty.
    Sem { proof: &'static str },
    /// Sound only under ~_eps (D2 relaxed) ⇒ Tier B gate MANDATORY after use.
    Approx { rationale: &'static str },
}

impl Rule {
    fn to_rewrite(&self) -> Rewrite<SigLang, ()> {
        let l: Pattern<SigLang> = translate_pattern(self.lhs).parse()
            .unwrap_or_else(|e| panic!("rule {}: bad lhs: {e}", self.name));
        let r: Pattern<SigLang> = translate_pattern(self.rhs).parse()
            .unwrap_or_else(|e| panic!("rule {}: bad rhs: {e}", self.name));
        Rewrite::new(self.name, l, r)
            .unwrap_or_else(|e| panic!("rule {}: {e}", self.name))
    }
}

#[derive(Debug)]
pub enum RefactorError {
    /// R2 entry condition violated: a Dec rule has no UNSAT artifact on disk.
    UndischargedRule(String),
    /// Tier-B mandatory gate refuted the extraction (approx rules + bad eps).
    GateRefuted { minimal_env: Vec<f64> },
}

pub struct RefactorOutcome {
    pub verified: VerifiedTerm,
    pub rule_trace: Vec<String>,
    pub cost_before: u64,
    pub cost_after: u64,
    pub budget_hit: bool,
}

/// Top-level refactor. Output contract: VerifiedTerm (certificate inside).
///
/// Tier routing (v2.1 §2):
///   only Dec/Sem rules applied → Tier A (proofs compose, T1)
///   any Approx rule applied    → Tier B gate MANDATORY on the result
pub fn refactor(
    p: &Term,
    eps_mode: bool,
    gate: &Gate,
    artifact_dir: &Path,
    limits: &SaturationLimits,
) -> Result<RefactorOutcome, RefactorError> {
    refactor_with_cost(p, eps_mode, gate, artifact_dir, limits, &DefaultCost)
}

/// L2: the cost function is a perf-only dial — swap in the `dge calib`
/// table via `CalibratedCost` with zero soundness impact.
pub fn refactor_with_cost(
    p: &Term,
    eps_mode: bool,
    gate: &Gate,
    artifact_dir: &Path,
    limits: &SaturationLimits,
    cost: &dyn CostFn,
) -> Result<RefactorOutcome, RefactorError> {
    // Σ-ext bypass: rules never rewrite extension semantics — the term
    // passes through UNREWRITTEN, Tier B identity-gated (which also runs
    // the determinism pre-gate on the plugin ops). Not an error: "no
    // rewrite available" is a valid refactor outcome; the certificate's
    // ext tags say under whose semantics the claim stands.
    if p.has_ext() {
        let verified = match gate.promote(p.clone(), p) {
            harness::gate::GateOutcome::Promoted(v) => v,
            harness::gate::GateOutcome::Refuted(w) => {
                return Err(RefactorError::GateRefuted {
                    minimal_env: w.minimal_env.clone(),
                });
            }
        };
        let c = cost.cost(p);
        return Ok(RefactorOutcome {
            verified,
            rule_trace: vec!["<ext-term: rewriting skipped>".into()],
            cost_before: c,
            cost_after: c,
            budget_hit: false,
        });
    }
    // R2 entry condition: no rule enters the active set unproved —
    // Dec needs its UNSAT artifact on disk, Sem needs its reviewed proof.
    let dec = r_dec::table();
    for r in &dec {
        if !smt::artifact_ok(artifact_dir, r.name) {
            return Err(RefactorError::UndischargedRule(r.name.to_string()));
        }
    }
    let sem = r_sem::table();
    for r in &sem {
        if let RuleClass::Sem { proof } = &r.class {
            if proof.trim().is_empty() {
                return Err(RefactorError::UndischargedRule(r.name.to_string()));
            }
        }
    }
    let approx = if eps_mode { r_approx::table() } else { vec![] };
    let approx_names: Vec<&str> = approx.iter().map(|r| r.name).collect();

    let rewrites: Vec<Rewrite<SigLang, ()>> = dec.iter()
        .chain(sem.iter())
        .chain(approx.iter())
        .map(Rule::to_rewrite)
        .collect();

    let expr = to_egg(p);
    let cost_before = cost.cost(p);
    let ex = saturate_and_extract(&expr, &rewrites, limits, cost);
    let extracted = from_egg(&ex.best);
    let cost_after = cost.cost(&extracted);
    debug_assert!(cost_after <= cost_before, "O4 violated: extraction raised cost");

    let approx_fired = ex.applied.iter().any(|n| approx_names.contains(&n.as_str()));

    let verified = if approx_fired {
        // Tier B mandatory — the approx rule was a search move, not a proof.
        match gate.promote(extracted, p) {
            GateOutcome::Promoted(mut vt) => {
                vt.annotate_rule_trace(ex.applied.clone()); // provenance
                vt
            }
            GateOutcome::Refuted(ce) => {
                return Err(RefactorError::GateRefuted { minimal_env: ce.minimal_env });
            }
        }
    } else {
        // Tier A: every applied rule carries a discharged obligation —
        // Dec rules cite their on-disk UNSAT artifacts; Sem rules are
        // backed by the compiled-in reviewed proof (named in rule_trace).
        let dec_names: Vec<&str> = dec.iter().map(|r| r.name).collect();
        let artifacts: Vec<String> = ex.applied.iter()
            .filter(|n| dec_names.contains(&n.as_str()))
            .map(|n| artifact_dir.join(format!("{n}.out")).display().to_string())
            .collect();
        let cert = Certificate {
            tier: Tier::A { smt_artifacts: artifacts },
            rule_trace: ex.applied.clone(),
            env: EnvFingerprint::capture(),
            fma_contraction: false,
            ext_semantics: vec![], // Tier A path: ext terms never reach rules (guarded)
        };
        Gate::promote_proved(extracted, cert)
    };

    Ok(RefactorOutcome {
        verified,
        rule_trace: ex.applied,
        cost_before,
        cost_after,
        budget_hit: ex.budget_hit,
    })
}
