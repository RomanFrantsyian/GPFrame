//! crate `cli` — [T8 | R7] — composition. Deps: ALL.
//! Library half so integration tests can drive the pipelines; `main.rs`
//! is a thin argv dispatcher.

pub mod audit;
pub mod extract;
pub mod emit;
pub mod pipeline;
pub mod refactor;
pub mod gentest;
pub mod debug;
pub mod calib;
pub mod discharge;
