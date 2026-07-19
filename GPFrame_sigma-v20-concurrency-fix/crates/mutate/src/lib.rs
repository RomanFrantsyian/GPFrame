//! crate `mutate` — [D5 T5 | R3] — testing. Deps: term, harness, rules(smt).
//!
//! T5: T ⊆ T' ⇒ ~_{T'} ⊆ ~_T — larger suites monotonically refine ~_T
//! toward ~. MS measures the refinement over the perturbation set M.
//! A-2 (coupling hypothesis) covers ONLY the extrapolation to unseen faults;
//! report MS as MS-over-M, never as "correctness %".

pub mod ops;
pub mod score;
pub mod eqfilter;
pub mod pin;
