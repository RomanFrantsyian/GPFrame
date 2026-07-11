//! crate `gp` — [D7 T7 | R5] — search & repair. Deps: term, harness.
//!
//! T7: elitism + strictly positive mutation over depth-capped Term_p
//! ⇒ ergodic chain, best-so-far monotone, global optimum a.s. (asymptotic).
//! A-3: finite-budget adequacy is ASSUMED, calibrated once. A miss returns
//! an HONEST NULL (original program kept) — never a wrong accept, because
//! the only exit is harness::Gate.

pub mod pop;
pub mod evolve;
pub mod fitness;
pub mod refine;
pub mod repair;
