//! Cost function — pluggable per L2 (cost irrelevance): any monotone
//! cost: Term -> N picks a representative of P's e-class; every
//! representative is certified-equal, so calibration is soundness-free.
//!
//! O4 (cost monotonicity over subterm order) is one induction lemma on
//! Term constructors: cost(node) = w(op) + Σ cost(children) with w(op) ≥ 1
//! is monotone by construction.
//!
//! R7: per-op weights come from perf-counter calibration (v1 M7), possibly
//! per-target-CPU tables — imported unchanged, touches performance only.

use term::{Op, Term};

pub trait CostFn {
    fn op_weight(&self, op: Op) -> u64;
    fn cost(&self, t: &Term) -> u64 {
        // Arena is topological: one pass, subcosts by index.
        let mut sub = Vec::with_capacity(t.len());
        for n in &t.nodes {
            let kids: u64 = match n.op.arity() {
                0 => 0,
                1 => sub[n.a as usize],
                2 => sub[n.a as usize] + sub[n.b as usize],
                _ => sub[n.a as usize] + sub[n.b as usize] + sub[n.c as usize],
            };
            sub.push(self.op_weight(n.op) + kids);
        }
        sub[t.root as usize]
    }
}

/// Default: node count, transcendentals ×32 (placeholder until calibration).
pub struct DefaultCost;

impl CostFn for DefaultCost {
    fn op_weight(&self, op: Op) -> u64 {
        if op.is_transcendental() { 32 } else { 1 }
    }
}

/// Calibrated table (R7 output of `dge calib`) — a perf-only import: by L2,
/// swapping cost functions changes WHICH equal term extraction picks, never
/// WHETHER it is equal.
///
/// File format (one per line, `#` comments allowed):
///   # env: behavioral:0123456789abcdef fma=true
///   + 1
///   sin 47
pub struct CalibratedCost {
    weights: std::collections::HashMap<String, u64>,
    fallback: DefaultCost,
}

impl CalibratedCost {
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let mut weights = std::collections::HashMap::new();
        for line in std::fs::read_to_string(path)?.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let mut it = line.split_whitespace();
            if let (Some(name), Some(w)) = (it.next(), it.next()) {
                if let Ok(w) = w.parse::<u64>() {
                    weights.insert(name.to_string(), w.max(1)); // O4: w ≥ 1
                }
            }
        }
        Ok(CalibratedCost { weights, fallback: DefaultCost })
    }
}

impl CostFn for CalibratedCost {
    fn op_weight(&self, op: Op) -> u64 {
        self.weights.get(op.name()).copied()
            .unwrap_or_else(|| self.fallback.op_weight(op))
    }
}
