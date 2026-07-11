//! Population init: ramped half-and-half, depth 2..6, node cap 17 (v1 M3).

use harness::strategy::Rng;
use term::{Op, Term, TermBuilder};

#[derive(Clone)]
pub struct GpConfig {
    pub pop_size: usize,
    pub depth_min: u32,
    pub depth_max: u32,
    pub node_cap: usize,
    /// Function set (arity ≥ 1) — restricting it restricts the search space,
    /// never soundness (the gate judges whatever comes out).
    pub ops: Vec<Op>,
    /// Number of input variables.
    pub arity: u32,
    /// Ephemeral constants drawn from this inclusive integer range.
    pub const_range: (i64, i64),
}

impl Default for GpConfig {
    fn default() -> Self {
        Self {
            pop_size: 300,
            depth_min: 2,
            depth_max: 6,
            node_cap: 17,
            ops: vec![Op::Add, Op::Sub, Op::Mul],
            arity: 1,
            const_range: (-5, 5),
        }
    }
}

pub fn random_terminal(cfg: &GpConfig, rng: &mut Rng, b: &mut TermBuilder) -> u32 {
    if rng.next_u64() & 1 == 0 {
        b.var((rng.next_u64() % cfg.arity as u64) as u32)
    } else {
        let (lo, hi) = cfg.const_range;
        let span = (hi - lo + 1) as u64;
        b.constant((lo + (rng.next_u64() % span) as i64) as f64)
    }
}

fn random_node(cfg: &GpConfig, rng: &mut Rng, depth: u32, full: bool, b: &mut TermBuilder) -> u32 {
    let make_terminal = depth == 0 || (!full && rng.next_u64() % 3 == 0);
    if make_terminal {
        return random_terminal(cfg, rng, b);
    }
    let op = cfg.ops[(rng.next_u64() as usize) % cfg.ops.len()];
    match op.arity() {
        1 => {
            let a = random_node(cfg, rng, depth - 1, full, b);
            b.unary(op, a)
        }
        2 => {
            let a = random_node(cfg, rng, depth - 1, full, b);
            let c = random_node(cfg, rng, depth - 1, full, b);
            b.binary(op, a, c)
        }
        _ => {
            let a = random_node(cfg, rng, depth - 1, full, b);
            let c = random_node(cfg, rng, depth - 1, full, b);
            let d = random_node(cfg, rng, depth - 1, full, b);
            b.ternary(op, a, c, d)
        }
    }
}

pub fn random_term(cfg: &GpConfig, rng: &mut Rng, depth: u32, full: bool) -> Term {
    let mut b = TermBuilder::new();
    let root = random_node(cfg, rng, depth, full, &mut b);
    b.finish(root)
}

/// Ramped half-and-half.
pub fn init(cfg: &GpConfig, rng: &mut Rng) -> Vec<Term> {
    (0..cfg.pop_size)
        .map(|i| {
            let depth = cfg.depth_min + (i as u32) % (cfg.depth_max - cfg.depth_min + 1);
            random_term(cfg, rng, depth, i % 2 == 0)
        })
        .collect()
}
