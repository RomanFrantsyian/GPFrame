//! [I3/I7] Bounded saturation + cost-monotone extraction over egg.
//! Budget hit ⇒ best-so-far, NOT failure (I7): egg's Runner stops at the
//! limit and extraction runs on whatever e-graph exists at that point.

use crate::cost::CostFn;
use crate::lang::{op_of, SigLang};
use egg::{CostFunction, Extractor, Id, Language, RecExpr, Rewrite, Runner};
use std::time::Duration;

pub struct SaturationLimits {
    pub node_cap: usize,
    pub iter_cap: usize,
    pub time_cap_ms: u64,
}

impl Default for SaturationLimits {
    fn default() -> Self {
        Self { node_cap: 100_000, iter_cap: 30, time_cap_ms: 2_000 }
    }
}

struct EggCost<'a>(&'a dyn CostFn);

impl CostFunction<SigLang> for EggCost<'_> {
    type Cost = u64;
    fn cost<C>(&mut self, enode: &SigLang, mut costs: C) -> u64
    where
        C: FnMut(Id) -> u64,
    {
        // O4 shape: w(op) + Σ child costs, w ≥ 1 ⇒ monotone by construction.
        self.0.op_weight(op_of(enode))
            + enode.children().iter().map(|&c| costs(c)).sum::<u64>()
    }
}

pub struct Extracted {
    pub best: RecExpr<SigLang>,
    pub best_cost: u64,
    /// rule names with ≥1 application during saturation. NOTE: superset of
    /// the rules on the extraction path (egg reports per-iteration counts,
    /// not per-extraction provenance) — sound for T1 because EVERY applied
    /// rule must carry a discharged obligation anyway.
    pub applied: Vec<String>,
    pub budget_hit: bool,
}

pub fn saturate_and_extract(
    expr: &RecExpr<SigLang>,
    rewrites: &[Rewrite<SigLang, ()>],
    limits: &SaturationLimits,
    cost: &dyn CostFn,
) -> Extracted {
    let runner = Runner::default()
        .with_expr(expr)
        .with_node_limit(limits.node_cap)
        .with_iter_limit(limits.iter_cap)
        .with_time_limit(Duration::from_millis(limits.time_cap_ms))
        .run(rewrites.iter());

    let budget_hit = !matches!(runner.stop_reason, Some(egg::StopReason::Saturated));

    let mut applied: Vec<String> = Vec::new();
    for it in &runner.iterations {
        for (name, &n) in &it.applied {
            if n > 0 {
                let s = name.to_string();
                if !applied.contains(&s) {
                    applied.push(s);
                }
            }
        }
    }

    let extractor = Extractor::new(&runner.egraph, EggCost(cost));
    let (best_cost, best) = extractor.find_best(runner.roots[0]);
    Extracted { best, best_cost, applied, budget_hit }
}
