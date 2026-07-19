//! FIELD TRIAL — `suggester_test.rs`'s
//! `a_chain_of_suggestions_cannot_drift_meaning` proves the "gate every
//! candidate against the ORIGINAL, never the running best" rule holds
//! for ONE hand-picked case. This trial fuzzes it: many random original
//! terms, each fed to two chained suggesters that always propose one
//! deliberately-cheap, usually-WRONG candidate (`(var 0)` — cost 1,
//! always clears the cost check against any non-trivial generated term,
//! and correct only in the rare case the random term IS exactly `x`),
//! checking across every round that a genuinely wrong candidate is
//! NEVER accepted.
//!
//! A fuzz campaign proves nothing if it never actually generates a
//! wrong candidate — this trial counts how many wrong candidates were
//! exercised, not just how many rounds ran.
//!
//! Run: `cargo run --release --example fieldtrial_suggester_fuzz -p sdk`

use sdk::{Engine, ProposalOutcome, Suggester, Term};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T { &xs[(self.next() as usize) % xs.len()] }
    fn bool(&mut self) -> bool { self.next() & 1 == 0 }
}

/// Random small arithmetic sexpr over var 0, depth-bounded.
fn random_term(rng: &mut Lcg, depth: u32) -> String {
    if depth == 0 || rng.next() % 4 == 0 {
        return if rng.bool() { "(var 0)".into() }
            else { format!("{:.3}", (rng.next() % 1000) as f64 / 100.0 - 5.0) };
    }
    let op = *rng.pick(&["+", "-", "*"]);
    format!("({} {} {})", op, random_term(rng, depth - 1), random_term(rng, depth - 1))
}

/// Proposes the identity (harmless, exercises the not-cheaper path) and
/// ALWAYS proposes the cheapest possible term, `(var 0)` — at index 1.
/// Cost 1 clears the cost check against nearly any generated term, so
/// this is the candidate that actually reaches the Gate for real
/// refutation, not just the cost pre-filter.
struct FuzzSuggester;
impl Suggester for FuzzSuggester {
    fn name(&self) -> &str { "fuzz" }
    fn suggest(&self, t: &Term) -> Vec<Term> {
        vec![t.clone(), sdk::sexpr::parse("(var 0)").unwrap()]
    }
}

fn main() {
    let rounds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(500);
    println!("FIELD TRIAL: Suggester chain-drift guarantee under fuzzing");
    println!("({rounds} random terms, 2 chained fuzz-suggesters each proposing `(var 0)`)\n");

    let mut term_rng = Lcg(0x51DE_1234);
    let mut wrong_checked = 0u64;
    let mut wrong_accepted = 0u64;
    let mut skipped_cost = 0u64;

    for round in 0..rounds {
        let src = random_term(&mut term_rng, 3);
        let original = match sdk::sexpr::parse(&src) { Ok(t) => t, Err(_) => continue };
        // arity-0 terms (pure constants) can't be gated against a
        // `(var 0)` candidate — same finding this trial run surfaced
        // and the SDK now refuses honestly instead of panicking; skip
        // them here so the campaign measures the DRIFT question, not
        // the (separately-fixed, separately-tested) arity question
        if original.arity() == 0 { continue; }

        let mut e = Engine::bare(0x2000 + round);
        e.register_suggester(std::sync::Arc::new(FuzzSuggester));
        e.register_suggester(std::sync::Arc::new(FuzzSuggester)); // chained

        let report = e.optimize("f", &original);

        for p in &report.proposals {
            if p.suggester != "fuzz" || p.index != 1 { continue; } // the `(var 0)` slot only
            if src == "(var 0)" { continue; } // trivially correct here, not a wrong candidate
            wrong_checked += 1;
            match &p.outcome {
                ProposalOutcome::Accepted { .. } => {
                    wrong_accepted += 1;
                    println!("  !! DRIFT round {round}: `(var 0)` accepted for `{src}`");
                }
                ProposalOutcome::RefusedNotCheaper { .. } => {
                    // only possible if `src` itself already costs <= 1
                    // (shouldn't happen once the exact-match case above
                    // is excluded, but kept for honesty if it does)
                    skipped_cost += 1;
                }
                ProposalOutcome::Refuted(_) => {} // expected, informative outcome
                ProposalOutcome::Refused(_) => {}
            }
        }
    }

    let gate_checked = wrong_checked - skipped_cost;
    println!("rounds run:                          {rounds}");
    println!("`(var 0)` candidates generated:       {wrong_checked}");
    println!("  rejected by cost gate (cheap path): {skipped_cost}");
    println!("  reached the Gate and were refuted:  {gate_checked}");
    println!("  ACCEPTED (must be 0):                {wrong_accepted}");

    if wrong_accepted > 0 {
        println!("\nVERDICT: FAIL — {wrong_accepted} drift(s) found.");
        std::process::exit(1);
    } else if gate_checked == 0 {
        println!("\nVERDICT: INCONCLUSIVE — no wrong candidate ever reached the Gate.");
        std::process::exit(2);
    } else {
        println!("\nVERDICT: PASS — {gate_checked} deliberately-wrong candidates reached \
            the Gate across {rounds} random terms; zero were ever accepted.");
    }
}
