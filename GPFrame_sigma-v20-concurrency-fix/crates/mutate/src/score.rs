//! MS(T,P) = |killed| / |confirmed-non-equivalent mutants|  (D5).
//! Denominator discipline (v2.0 §4.2): equivalent mutants EXCLUDED; the
//! undecidable residue (T2) sits in a triage queue, excluded until resolved.
//! Report MS as MS-over-M, never as "correctness %" (A-2 scope).

use crate::eqfilter::{classify, MutantClass};
use crate::ops::all_mutants;
use rules::smt::SmtBackend;
use term::{eval, Term};

pub struct ScoreReport {
    pub killed: usize,
    pub confirmed_non_equivalent: usize,
    pub equivalent_excluded: usize,
    pub triage_pending: usize,
    /// surviving non-equivalent mutants — the suite's measured blind spots
    pub survivors: Vec<crate::ops::Mutation>,
}

impl ScoreReport {
    pub fn ms(&self) -> f64 {
        if self.confirmed_non_equivalent == 0 { return 1.0; }
        self.killed as f64 / self.confirmed_non_equivalent as f64
    }

    pub fn render(&self) -> String {
        format!(
            "MS-over-M = {:.3} ({} killed / {} confirmed non-equivalent; \
             {} equivalent excluded, {} in triage)",
            self.ms(), self.killed, self.confirmed_non_equivalent,
            self.equivalent_excluded, self.triage_pending
        )
    }
}

/// A suite distinguishes a mutant iff some env yields a bitwise-different
/// output (the strictest kill criterion — matches the memo/gate discipline).
fn suite_kills(mutant: &Term, original: &Term, suite: &[Vec<f64>]) -> bool {
    suite.iter().any(|e| eval(mutant, e).to_bits() != eval(original, e).to_bits())
}

pub fn mutation_score(p: &Term, suite: &[Vec<f64>], smt: &mut dyn SmtBackend) -> ScoreReport {
    let mut rep = ScoreReport {
        killed: 0, confirmed_non_equivalent: 0,
        equivalent_excluded: 0, triage_pending: 0, survivors: vec![],
    };
    for m in all_mutants(p) {
        // cheap sound-one-way check first: a suite kill IS a non-equivalence
        // witness — no solver needed.
        if suite_kills(&m.term, p, suite) {
            rep.confirmed_non_equivalent += 1;
            rep.killed += 1;
            continue;
        }
        match classify(&m.term, p, smt) {
            MutantClass::Equivalent => rep.equivalent_excluded += 1,
            MutantClass::NonEquivalent { .. } => {
                rep.confirmed_non_equivalent += 1;
                rep.survivors.push(m.mutation); // SMT says different, suite missed it
            }
            MutantClass::Unknown => rep.triage_pending += 1,
        }
    }
    rep
}
