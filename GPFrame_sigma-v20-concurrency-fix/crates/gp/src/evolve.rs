//! Generation loop — T7 holds BY CONSTRUCTION:
//!   * elitism: best individual copied unmodified ⇒ best-so-far monotone
//!   * p_mutation > 0 on every offspring slot ⇒ ergodicity
//! A budget miss returns the best-so-far (honest null upstream, A-3).

use crate::fitness::{error, fitness, FitnessParams};
use crate::pop::{random_term, GpConfig};
use harness::strategy::Rng;
use term::{Term, TermBuilder};

pub struct EvolveParams {
    pub tournament_k: usize,
    pub p_crossover: f64,
    /// MUST stay > 0 — T7's ergodicity premise (asserted in `run`).
    pub p_mutation: f64,
    pub max_generations: u64,
    /// stop early when training error reaches this
    pub err_tol: f64,
}

impl Default for EvolveParams {
    fn default() -> Self {
        Self { tournament_k: 5, p_crossover: 0.9, p_mutation: 0.15, max_generations: 60, err_tol: 0.0 }
    }
}

fn tournament<'a>(pop: &'a [(Term, f64)], k: usize, rng: &mut Rng) -> &'a Term {
    let mut best: Option<&(Term, f64)> = None;
    for _ in 0..k {
        let cand = &pop[(rng.next_u64() as usize) % pop.len()];
        if best.map_or(true, |b| cand.1 > b.1) {
            best = Some(cand);
        }
    }
    &best.unwrap().0
}

fn random_node_id(t: &Term, rng: &mut Rng) -> u32 {
    (rng.next_u64() % t.len() as u64) as u32
}

/// Subtree crossover: replace a random subtree of `a` with a random subtree
/// of `b`, via TermBuilder::graft (invariant preserved by construction).
fn crossover(a: &Term, b: &Term, rng: &mut Rng) -> Term {
    let at = random_node_id(a, rng);
    let from = random_node_id(b, rng);
    let mut bld = TermBuilder::new();
    let root = bld.graft(a, a.root, at, b, from);
    bld.finish(root)
}

/// Subtree mutation: graft a small fresh random subtree at a random node.
fn mutate(t: &Term, cfg: &GpConfig, rng: &mut Rng) -> Term {
    let donor = random_term(cfg, rng, 2, false);
    let at = random_node_id(t, rng);
    let mut bld = TermBuilder::new();
    let root = bld.graft(t, t.root, at, &donor, donor.root);
    bld.finish(root)
}

pub struct Evolved {
    pub best: Term,
    pub best_error: f64,
    pub generations: u64,
}

pub fn run(
    cfg: &GpConfig,
    ep: &EvolveParams,
    fp: &FitnessParams,
    targets: &[(Vec<f64>, f64)],
    rng: &mut Rng,
) -> Evolved {
    let pop = crate::pop::init(cfg, rng);
    run_with_pop(cfg, ep, fp, targets, pop, rng)
}

/// Seeded entry (repair mode injects grafted candidates as the population).
pub fn run_with_pop(
    cfg: &GpConfig,
    ep: &EvolveParams,
    fp: &FitnessParams,
    targets: &[(Vec<f64>, f64)],
    initial: Vec<Term>,
    rng: &mut Rng,
) -> Evolved {
    assert!(ep.p_mutation > 0.0, "T7 premise: mutation probability must be > 0");
    assert!(!initial.is_empty(), "empty initial population");

    let score = |t: Term| -> (Term, f64) {
        let f = fitness(&t, targets, fp);
        (t, f)
    };
    let mut pop: Vec<(Term, f64)> = initial.into_iter().map(score).collect();

    let mut gens = 0;
    for g in 0..ep.max_generations {
        gens = g + 1;
        // elitism: index of best
        let elite_idx = (0..pop.len()).max_by(|&i, &j| pop[i].1.total_cmp(&pop[j].1)).unwrap();
        let elite = pop[elite_idx].clone();
        if error(&elite.0, targets) <= ep.err_tol {
            return Evolved { best_error: error(&elite.0, targets), best: elite.0, generations: gens };
        }

        let mut next: Vec<(Term, f64)> = Vec::with_capacity(pop.len());
        next.push(elite); // T7: monotone best-so-far
        while next.len() < pop.len() {
            let a = tournament(&pop, ep.tournament_k, rng);
            let mut child = if rng.uniform01() < ep.p_crossover {
                let b = tournament(&pop, ep.tournament_k, rng);
                crossover(a, b, rng)
            } else {
                a.clone()
            };
            if rng.uniform01() < ep.p_mutation {
                child = mutate(&child, cfg, rng);
            }
            // bloat control: reject oversize children, keep parent
            if child.len() > cfg.node_cap || child.depth() > cfg.depth_max + 2 {
                child = a.clone();
            }
            next.push(score(child));
        }
        pop = next;
    }

    let (best, _) = pop
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let e = error(&best, targets);
    Evolved { best, best_error: e, generations: gens }
}
