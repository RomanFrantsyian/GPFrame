//! `dge gentest <fn.sexpr>` — mutation-adequate suite generation (§6 flow).
//!
//! Suite growth loop: sample μ'; keep an env iff it kills a still-alive
//! mutant (then T3-shrink it while it still kills); stop when every mutant
//! is dead, SMT-excluded (equivalent), or the sample budget runs out.
//! Adequacy is reported as the derived-dial pair:
//!   MS-over-M (D5 denominator discipline) + (n, α, δ_min) for the μ' block.

use harness::strategy::{MuPrime, Rng};
use mutate::ops::{all_mutants, Mutant};
use mutate::pin::emit_golden_suite;
use rules::smt::{SmtBackend, SmtVerdict, Z3Cli};
use term::{eval, Term};

pub struct GentestReport {
    pub suite: Vec<(Vec<f64>, f64)>, // (env, golden output)
    pub killed: usize,
    pub equivalent_excluded: usize,
    pub survivors: usize,
    pub triage: usize,
    pub samples_used: u64,
    pub emitted: String,
}

impl GentestReport {
    pub fn ms(&self) -> f64 {
        let denom = self.killed + self.survivors;
        if denom == 0 { 1.0 } else { self.killed as f64 / denom as f64 }
    }
}

fn kills(mutant: &Mutant, p: &Term, env: &[f64]) -> bool {
    eval(&mutant.term, env).to_bits() != eval(p, env).to_bits()
}

pub fn generate(
    p: &Term,
    fn_name: &str,
    seed: u64,
    sample_budget: u64,
    smt: Option<&mut dyn SmtBackend>,
) -> GentestReport {
    let mutants = all_mutants(p);
    let mut alive: Vec<usize> = (0..mutants.len()).collect();
    let mut suite: Vec<(Vec<f64>, f64)> = Vec::new();
    let mu = MuPrime::default_with_seed(seed);
    let mut rng = Rng::new(seed);
    let arity = p.arity().max(1);
    let mut killed = 0usize;
    let mut samples_used = 0u64;

    while !alive.is_empty() && samples_used < sample_budget {
        samples_used += 1;
        let e = mu.sample(&mut rng, arity);
        let hit: Vec<usize> = alive.iter().copied()
            .filter(|&i| kills(&mutants[i], p, &e))
            .collect();
        if hit.is_empty() { continue; }
        // T3: shrink while it still kills at least one of the same mutants
        let mut fails = |env: &[f64]| hit.iter().any(|&i| kills(&mutants[i], p, env));
        let minimal = harness::shrink::shrink(e, &mut fails);
        // recompute the actual kill set of the minimal env (may differ)
        let final_hit: Vec<usize> = alive.iter().copied()
            .filter(|&i| kills(&mutants[i], p, &minimal))
            .collect();
        killed += final_hit.len();
        alive.retain(|i| !final_hit.contains(i));
        let golden = eval(p, &minimal);
        suite.push((minimal, golden));
    }

    // remaining alive: eq-filter (SMT) or triage
    let (mut equivalent_excluded, mut survivors, mut triage) = (0, 0, 0);
    if let Some(smt) = smt {
        for &i in &alive {
            match smt.check_term_inequiv(&mutants[i].term, p) {
                SmtVerdict::UnsatProved { .. } => equivalent_excluded += 1,
                SmtVerdict::SatRefuted { .. } => survivors += 1, // provably distinct, suite missed it
                SmtVerdict::Unknown => triage += 1,
            }
        }
    } else {
        triage = alive.len();
    }

    let emitted = emit_golden_suite(fn_name, &suite);
    GentestReport { suite, killed, equivalent_excluded, survivors, triage, samples_used, emitted }
}

pub fn run(args: &[String]) {
    let Some(file) = args.first() else {
        eprintln!("usage: dge gentest <fn.sexpr> [--out <suite.rs>] [--budget <n>]");
        return;
    };
    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1)).cloned()
        .unwrap_or_else(|| "generated_suite.rs".into());
    let budget = args.iter().position(|a| a == "--budget")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok())
        .unwrap_or(5_000);

    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {file}: {e}"); return; }
    };
    let p = match term::sexpr::parse(src.trim()) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse: {e:?}"); return; }
    };

    let mut z3;
    let smt: Option<&mut dyn SmtBackend> = if Z3Cli::available() {
        z3 = Z3Cli::new("artifacts/eqfilter");
        Some(&mut z3)
    } else {
        eprintln!("(z3 unavailable — unkilled mutants go to triage, not the denominator)");
        None
    };

    let rep = generate(&p, "target_fn", 0xde, budget, smt);
    std::fs::write(&out_path, &rep.emitted).ok();
    println!("suite: {} envs (from {} mu' samples) -> {out_path}", rep.suite.len(), rep.samples_used);
    println!(
        "MS-over-M = {:.3} ({} killed / {} confirmed non-equivalent; {} equivalent excluded, {} triage)",
        rep.ms(), rep.killed, rep.killed + rep.survivors, rep.equivalent_excluded, rep.triage
    );
    let g = harness::Gate::default_dial(0);
    println!(
        "mu' block adequacy: n={} alpha={} => delta_min={:.2e} (T4)",
        g.n, g.alpha, g.delta_min()
    );
}
