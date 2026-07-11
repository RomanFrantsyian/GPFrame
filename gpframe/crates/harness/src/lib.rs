//! crate `harness` — [T3 T4 D6 O3 O8 | R1] — THE JUDGE. Deps: `term` only.
//!
//! Architecture rule (spec AST): harness never depends on rules/gp —
//! the judge must not import the contestants.
//!
//! Realizes:
//! * T4: sampling guarantee — Gate{n, α, δ_min = ln(1/α)/n} surfaced in API.
//! * T3: shrinking over well-founded ⊏ → minimal counterexample.
//! * D6: failure sets, spec predicates φ.
//! * O8: environment fingerprint pinned into every Tier-B certificate.
//!
//! Typestate spine, first door:
//!   Term --Gate::promote--> VerifiedTerm      (sole constructor)
//! No public constructor for `VerifiedTerm` exists anywhere else.

pub mod metric;
pub mod strategy;
pub mod shrink;
pub mod gate;
pub mod cert;

pub use cert::{Certificate, EnvFingerprint, Tier};
pub use gate::{CounterExample, Gate, GateOutcome, VerifiedTerm};
