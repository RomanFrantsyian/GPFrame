//! R_sem — rules with reviewed algebraic proofs (O2). This is the designed
//! route for the "decidable in principle, infeasible in practice" residue:
//! Z3 4.8 bit-blasts Float64 commutativity into a SAT instance it cannot
//! close in reasonable time (measured: >12 s timeout), while the IEEE-754
//! argument is one paragraph. Review = code review of the proof text below.

use crate::{Rule, RuleClass};

const COMM_PROOF: &str = r#"
THEOREM (commutativity of fp.add / fp.mul, any IEEE-754 format, any rounding
mode). Let ∘ ∈ {+, ×} on the extended reals and rnd the rounding function.
IEEE-754 §5.4 defines fp-op(a,b) = rnd(a ∘ b) for non-exceptional cases, with
exceptional cases (NaN propagation, ∞ arithmetic, zero-sign rules §6.3)
defined symmetrically in the operands for + and ×:
  * a ∘ b = b ∘ a on the extended reals (commutativity of exact + and ×);
  * rnd is a function of the exact value alone ⇒ rnd(a∘b) = rnd(b∘a);
  * NaN: if either operand is NaN the result is NaN (SMT FP theory has a
    single NaN, so the propagated payload question does not arise there;
    in Rust the payload of (NaN_p + x) vs (x + NaN_p) is the same operand's
    payload — object equality holds at the level our metric observes);
  * zero signs: (+0)+(−0) = (−0)+(+0) = +0 under RNE (§6.3, symmetric rule);
    ±0 products get sign = XOR of operand signs — symmetric.
Hence fp.add and fp.mul are commutative. ∎
NOTE this argument does NOT extend to fp.min/fp.max (±0 order unspecified,
§5.3.1 minNum) — those stay OUT of the rule set, as measured by SMT.
"#;

pub fn table() -> Vec<Rule> {
    vec![
        Rule { name: "add-comm", lhs: "(+ ?a ?b)", rhs: "(+ ?b ?a)",
               class: RuleClass::Sem { proof: COMM_PROOF } },
        Rule { name: "mul-comm", lhs: "(* ?a ?b)", rhs: "(* ?b ?a)",
               class: RuleClass::Sem { proof: COMM_PROOF } },
    ]
}
