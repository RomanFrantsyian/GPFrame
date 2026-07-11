//! crate `jit` — [L1 O7 | R7] — lowering. Deps: term, harness.
//!
//! L1 (non-transfer): T ~ P proved says NOTHING about [[L(T)]] = [[T]].
//! Cranelift is NOT in the trusted base — O7 keeps it refutable:
//! the compiler output is differentially gated against the interpreter,
//! per term, at install time. Interp remains permanent fallback + oracle.

pub mod lower;
pub mod install;
pub mod hot;

pub use install::{install, InstallError, JitFn};
pub use lower::{LowerConfig, LowerError};
