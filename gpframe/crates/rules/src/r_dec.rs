//! R_dec — rules whose soundness is checkable in QF_FP. O1 discipline:
//! a rule enters the ACTIVE set only with a stored UNSAT(inequiv) artifact
//! (`smt::artifact_ok` is checked at every refactor() entry).
//!
//! The FP trap (v2.0 §4.1): assoc/dist are NOT sound over f64 — they live in
//! r_approx. `x + 0.0 → x` is UNSOUND at x = −0.0 (−0 + +0 = +0) — kept out;
//! the discharge test proves the trap real by getting SAT on it.
//!
//! add-comm / mul-comm are NOT here: Float64 commutativity is a Z3 4.8
//! bit-blasting timeout in practice (measured >12 s) — they route to r_sem
//! with the one-paragraph IEEE-754 proof (the designed O2 escape hatch).

use crate::{Rule, RuleClass};

pub fn table() -> Vec<Rule> {
    vec![
        Rule { name: "mul-one",     lhs: "(* ?a 1.0)",        rhs: "?a",         class: RuleClass::Dec },
        Rule { name: "neg-neg",     lhs: "(neg (neg ?a))",    rhs: "?a",         class: RuleClass::Dec },
        Rule { name: "select-same", lhs: "(select ?c ?a ?a)", rhs: "?a",         class: RuleClass::Dec },
        Rule { name: "sub-to-neg",  lhs: "(- ?a ?b)",         rhs: "(+ ?a (neg ?b))", class: RuleClass::Dec },
        Rule { name: "div-one",     lhs: "(/ ?a 1.0)",        rhs: "?a",         class: RuleClass::Dec },
        // R2 growth: toward the 50-simplification corpus; each addition
        // requires its O1 artifact FIRST (entry condition).
    ]
}
