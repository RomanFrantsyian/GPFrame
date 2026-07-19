//! crate `locate` — [§4.3 | R4] — debugging. Deps: term, harness.
//!
//! Debugging = ⊏-minimal counterexample (T3, from harness) + localization.
//! Ochiai is DERIVED as a conditional-probability estimator; ranking quality
//! on real faults is empirical folklore — SHIPPED AS AID, NEVER AS VERDICT.

pub mod spectrum;
pub mod ochiai;
pub mod report;
