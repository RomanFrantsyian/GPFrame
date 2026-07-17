//! Execution spectrum S[test, node] ∈ {0,1} over the suite T.
//!
//! PSEUDOCODE collect(p, tests, phi):
//!   for (ti, e) in tests:
//!       trace = interp with coverage: mark node j iff its value was
//!               consumed on the path to the root (Select prunes a branch)
//!       S[ti][j] = trace[j];  fail[ti] = !phi(eval(p, e))
//!
//! R4 note: needs a coverage-instrumented eval — add
//! `interp::eval_traced(t, env) -> (f64, Vec<bool>)` in `term` at R4
//! (trusted-base delta: reviewed alongside eval, same one-screen budget).

pub struct Spectrum {
    /// row-major: covered[test * n_nodes + node]
    pub covered: Vec<bool>,
    pub failed: Vec<bool>,
    pub n_nodes: usize,
}

/// Collect the spectrum of `p` over `tests`, judging outputs with `phi`
/// (spec predicate over (env, output) — D6).
pub fn collect(
    p: &term::Term,
    tests: &[Vec<f64>],
    phi: &dyn Fn(&[f64], f64) -> bool,
) -> Spectrum {
    let n_nodes = p.len();
    let mut covered = Vec::with_capacity(tests.len() * n_nodes);
    let mut failed = Vec::with_capacity(tests.len());
    for env in tests {
        let (v, used) = term::eval_traced(p, env);
        covered.extend(used);
        failed.push(!phi(env, v));
    }
    Spectrum { covered, failed, n_nodes }
}

impl Spectrum {
    pub fn counts(&self, node: usize) -> (u64, u64, u64, u64) {
        // (ef, ep, nf, np): executed/not-executed × failed/passed
        let mut ef = 0; let mut ep = 0; let mut nf = 0; let mut np = 0;
        for t in 0..self.failed.len() {
            match (self.covered[t * self.n_nodes + node], self.failed[t]) {
                (true, true) => ef += 1,
                (true, false) => ep += 1,
                (false, true) => nf += 1,
                (false, false) => np += 1,
            }
        }
        (ef, ep, nf, np)
    }
}
