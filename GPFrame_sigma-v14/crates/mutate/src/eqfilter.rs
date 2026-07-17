//! Equivalent-mutant SMT filter (new in v2.1 merge, R3).
//!
//! The equivalent-mutant problem is undecidable (T2); we handle it by
//! SMT-check where the term pair lands in a decidable fragment, else the
//! mutant goes to the human triage queue. No silent guessing.

use rules::smt::{SmtBackend, SmtVerdict};
use term::Term;

pub enum MutantClass {
    /// SMT proved m(P) ≡ P — excluded from the MS denominator.
    Equivalent,
    /// SMT found (or sampling found) a distinguishing input.
    NonEquivalent { witness: Option<Vec<f64>> },
    /// Undecidable residue — human triage queue.
    Unknown,
}

pub fn classify(mutant: &Term, original: &Term, smt: &mut dyn SmtBackend) -> MutantClass {
    match smt.check_term_inequiv(mutant, original) {
        SmtVerdict::UnsatProved { .. } => MutantClass::Equivalent,
        SmtVerdict::SatRefuted { model } => MutantClass::NonEquivalent { witness: Some(model) },
        SmtVerdict::Unknown => MutantClass::Unknown,
        // R3 refinement: before giving up, try the harness sampler — a hit
        // is a NonEquivalent witness (cheap, sound in that direction only).
    }
}
