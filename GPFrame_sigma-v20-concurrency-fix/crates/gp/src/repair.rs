//! Repair mode — patch search at locate-ranked nodes (v2.0 §4.3).
//!
//! EXIT ONLY VIA GATE: a candidate patch is returned only after passing the
//! full T8 gate against the spec oracle — a wrong repair hypothesis cannot
//! silently enter code. A budget miss returns None: the HONEST NULL (A-3),
//! never a wrong accept.
//!
//! Two search tiers, cheapest first:
//!   1. op-swap moves at the suspicious node (single-fault hypothesis —
//!      the classic APR quick win);
//!   2. GP subtree search seeded by grafts at the node (gp::evolve).

use crate::evolve::{run_with_pop, EvolveParams};
use crate::fitness::FitnessParams;
use crate::pop::{random_term, GpConfig};
use harness::strategy::Rng;
use harness::{Gate, GateOutcome, VerifiedTerm};
use term::{eval, Op, Term, TermBuilder};

fn swap_candidates(op: Op) -> &'static [Op] {
    use Op::*;
    match op {
        Add => &[Sub, Mul], Sub => &[Add, Div], Mul => &[Div, Add], Div => &[Mul, Sub],
        Min => &[Max], Max => &[Min], Sin => &[Cos], Cos => &[Sin],
        Neg => &[Abs], Abs => &[Neg], Floor => &[Ceil], Ceil => &[Floor],
        _ => &[],
    }
}

fn with_op(t: &Term, at: u32, to: Op) -> Term {
    let mut t2 = t.clone();
    t2.nodes[at as usize].op = to;
    // in-place op change of same arity preserves the topological invariant;
    // rebuild the cached hash
    let mut b = TermBuilder::new();
    let root = b.copy_subtree(&t2, t2.root);
    b.finish(root)
}

pub struct RepairParams {
    pub top_k: usize,
    pub gp_budget_gens: u64,
    pub samples: usize,
}

impl Default for RepairParams {
    fn default() -> Self { Self { top_k: 5, gp_budget_gens: 30, samples: 32 } }
}

/// Repair `broken` against the spec `oracle` (reference term), guided by a
/// locate ranking. Returns a gate-certified fix or the honest null.
pub fn repair(
    broken: &Term,
    ranking: &[(usize, f64)],
    oracle: &Term,
    gate: &Gate,
    rp: &RepairParams,
    seed: u64,
) -> Option<VerifiedTerm> {
    let mut rng = Rng::new(seed);
    let arity = oracle.arity().max(1);
    let targets: Vec<(Vec<f64>, f64)> = (0..rp.samples)
        .map(|_| {
            let e: Vec<f64> = (0..arity).map(|_| rng.uniform01() * 6.0 - 3.0).collect();
            let y = eval(oracle, &e);
            (e, y)
        })
        .collect();
    let err = |t: &Term| crate::fitness::error(t, &targets);

    // Tier 1: single op-swap at each suspicious node, best-first.
    for &(node, _) in ranking.iter().take(rp.top_k) {
        let op = broken.node(node as u32).op;
        for &to in swap_candidates(op) {
            let cand = with_op(broken, node as u32, to);
            if err(&cand) == 0.0 {
                if let GateOutcome::Promoted(vt) = gate.promote(cand, oracle) {
                    return Some(vt); // certificate attached
                }
            }
        }
    }

    // Tier 2: GP subtree search seeded with grafts at the top node.
    let cfg = GpConfig { arity: arity as u32, ..GpConfig::default() };
    if let Some(&(node, _)) = ranking.first() {
        let mut seeded: Vec<Term> = (0..cfg.pop_size / 2)
            .map(|_| {
                let donor = random_term(&cfg, &mut rng, 2, false);
                let mut b = TermBuilder::new();
                let root = b.graft(broken, broken.root, node as u32, &donor, donor.root);
                b.finish(root)
            })
            .collect();
        seeded.extend((0..cfg.pop_size - seeded.len()).map(|i| {
            random_term(&cfg, &mut rng, 2 + (i as u32 % 4), i % 2 == 0)
        }));
        let ep = EvolveParams { max_generations: rp.gp_budget_gens, ..Default::default() };
        let out = run_with_pop(&cfg, &ep, &FitnessParams::default(), &targets, seeded, &mut rng);
        if out.best_error == 0.0 {
            if let GateOutcome::Promoted(vt) = gate.promote(out.best, oracle) {
                return Some(vt);
            }
        }
    }
    None // honest null (A-3): budget exhausted, original program kept
}
