//! [I5] Fitness(T) = −(Err + β·|T|). Error over a target sample set; the
//! FINAL candidate must still pass the full harness::Gate before leaving gp
//! (fitness is a search heuristic, never evidence — T8 discipline).
//!
//! R5 swap-in: rayon par_iter over the population (v1 M3).

use term::{eval, Term};

pub struct FitnessParams {
    pub beta_size: f64,
}

impl Default for FitnessParams {
    fn default() -> Self {
        Self { beta_size: 1e-3 }
    }
}

/// Sum |y − ŷ| over targets; non-finite prediction ⇒ hard penalty.
pub fn error(t: &Term, targets: &[(Vec<f64>, f64)]) -> f64 {
    let mut err = 0.0;
    for (env, y) in targets {
        let v = eval(t, env);
        if !v.is_finite() {
            return f64::INFINITY;
        }
        err += (v - y).abs();
    }
    err
}

pub fn fitness(t: &Term, targets: &[(Vec<f64>, f64)], p: &FitnessParams) -> f64 {
    -(error(t, targets) + p.beta_size * t.len() as f64)
}
