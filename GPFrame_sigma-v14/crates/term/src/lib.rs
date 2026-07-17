//! crate `term` — [D1 D2 O5 | R0] — the foundation. Zero dependencies.
//!
//! Realizes:
//! * D1: many-sorted signature Σ (pure-total fragment Term_p only — O5 is
//!   discharged *by construction*: no effectful symbol exists in `Op`).
//! * D2: observational equivalence collapses to extensional equality on
//!   Term_p; the interpreter below IS the definitional semantics `[[.]]`.
//! * O6→DERIVED: structural hash is an *index*; full-key compare
//!   (`Term::structurally_eq`) is the *authority*.
//!
//! Trusted-base note (v2.1 §7): the interpreter (`interp`) is in the trusted
//! base. Keep it small — one screen of match arms. Everything else in the
//! workspace is refuted against it, never trusted.

pub mod sig;
pub mod ast;
pub mod hash;
pub mod interp;
pub mod sexpr;

pub use ast::{Node, NodeId, Term, TermBuilder};
pub use interp::{eval, eval_traced, eval_with_seqs};
pub use sig::Op;
