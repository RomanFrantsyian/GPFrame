//! R_approx — sound only under ~_eps (D2 relaxed). Admissible ONLY in
//! eps-mode, and then the Tier-B gate on the extraction is MANDATORY —
//! the rule is a search move, not a proof.

use crate::{Rule, RuleClass};

pub fn table() -> Vec<Rule> {
    vec![
        Rule { name: "add-assoc~",    lhs: "(+ (+ ?a ?b) ?c)", rhs: "(+ ?a (+ ?b ?c))",
               class: RuleClass::Approx { rationale: "f64 rounding order" } },
        Rule { name: "mul-dist~",     lhs: "(* ?a (+ ?b ?c))", rhs: "(+ (* ?a ?b) (* ?a ?c))",
               class: RuleClass::Approx { rationale: "f64 rounding order" } },
        // factoring = the reverse direction; needed because e-graph rewrites
        // only ADD the rhs shape — Horner forms are unreachable without it
        Rule { name: "mul-factor~",   lhs: "(+ (* ?a ?b) (* ?a ?c))", rhs: "(* ?a (+ ?b ?c))",
               class: RuleClass::Approx { rationale: "f64 rounding order (reverse dist)" } },
        Rule { name: "mul-factor-r~", lhs: "(+ (* ?b ?a) (* ?c ?a))", rhs: "(* (+ ?b ?c) ?a)",
               class: RuleClass::Approx { rationale: "f64 rounding order (reverse dist)" } },
        // CORRECTED CLAIM (found by the gate, seed 13): fma-contraction is
        // NOT ≤1 ULP — under catastrophic cancellation (a*b ≈ -c) the plain
        // form's multiply rounding becomes O(1) RELATIVE error in the small
        // result, unbounded in ULPs. The honest ~_eps for this rule is a
        // mixed abs/rel tolerance; certificates record it.
        Rule { name: "fma-contract~", lhs: "(+ (* ?a ?b) ?c)", rhs: "(fma ?a ?b ?c)",
               class: RuleClass::Approx {
                   rationale: "exact vs double rounding; unbounded ULP under cancellation — gate with mixed abs/rel eps" } },
    ]
}
