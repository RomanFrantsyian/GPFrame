//! Hot-path dispatch: interp by default; after `install_threshold` calls,
//! attempt O7 install once. Install failure ⇒ stay on interp forever (the
//! permanent fallback) — a compiler mismatch must never take down the term.

use crate::install::{install, JitFn};
use crate::lower::LowerConfig;
use harness::{Gate, VerifiedTerm};

pub struct HotPolicy {
    pub install_threshold: u64,
}

impl Default for HotPolicy {
    fn default() -> Self { Self { install_threshold: 1_000 } }
}

enum State {
    Interp(Option<VerifiedTerm>), // Some until install attempt consumes it
    Jitted(Box<JitFn>),
    InterpPinned(VerifiedTerm),   // install failed/declined: never retry
}

/// Per-term dispatcher (single-threaded reference impl; the concurrent
/// version wraps this in the memo-style Mutex pattern).
pub struct HotDispatch {
    state: State,
    calls: u64,
    policy: HotPolicy,
    cfg: LowerConfig,
    gate: Gate,
}

impl HotDispatch {
    pub fn new(vt: VerifiedTerm, policy: HotPolicy, cfg: LowerConfig, gate: Gate) -> Self {
        HotDispatch { state: State::Interp(Some(vt)), calls: 0, policy, cfg, gate }
    }

    pub fn is_jitted(&self) -> bool { matches!(self.state, State::Jitted(_)) }

    pub fn call(&mut self, env: &[f64]) -> f64 {
        self.calls += 1;
        if let State::Interp(slot) = &mut self.state {
            if self.calls > self.policy.install_threshold {
                let vt = slot.take().expect("install attempted twice");
                match install(vt.clone(), &self.cfg, &self.gate) {
                    Ok(jf) => self.state = State::Jitted(Box::new(jf)),
                    Err(_) => self.state = State::InterpPinned(vt), // permanent fallback
                }
            }
        }
        match &self.state {
            State::Jitted(jf) => jf.call(env),
            State::Interp(Some(vt)) | State::InterpPinned(vt) => term::eval(vt.term(), env),
            State::Interp(None) => unreachable!(),
        }
    }
}
