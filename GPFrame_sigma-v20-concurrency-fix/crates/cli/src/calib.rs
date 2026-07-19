//! `dge calib` — R7 calibration: THE ONLY EMPIRICAL ZONE, and it touches
//! performance only, never correctness (L2).
//!
//! Produces:
//!   1. per-op cost table (relative to Add=1) from interp microbenchmarks,
//!      written for `rules::cost::CalibratedCost` — extraction then targets
//!      real op costs instead of the ×32 placeholder;
//!   2. the v2.1 §6 perf-targets report with MEASURED values against the
//!      HYPOTHESIZED targets. Misses block PERF SIGN-OFF ONLY, never
//!      correctness (which lives in the certificates).
//!
//! Method: wall-clock medians over JIT-COMPILED op-sum kernels minus a
//! same-shape baseline — the JIT is what extraction targets, and jitted
//! kernels have no per-eval interpreter overhead to dilute the signal.
//! Inputs stay bounded (sum of op(c_k + tiny*x) shapes), so saturating ops
//! (exp → inf fast path) don't fake cheapness. Honest but not
//! cycle-accurate; v1 M7 upgrade path is perf_event counters. Run with
//! `--release` (a note is printed otherwise).

use harness::{Gate, GateOutcome};
use jit::{install, LowerConfig};
use std::fmt::Write as _;
use std::time::Instant;
use term::{eval, Op, Term, TermBuilder};
#[allow(unused_imports)]
use term::sexpr;

const K: usize = 16;      // independent op applications per kernel
const EVALS: u32 = 50_000;
const RUNS: usize = 7;

/// Σ_{k<K} op(e_k[, c_k…]) with e_k = c_k + 1e-3·x — inputs bounded near
/// c_k ∈ [0.3, 1.9], so no op saturates. `with_op=false` builds the SAME
/// shape minus the op application (the subtraction baseline).
fn sum_kernel(op: Op, with_op: bool) -> Term {
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let eps = b.constant(1e-3);
    let xe = b.binary(Op::Mul, x, eps);
    let mut acc = b.constant(0.0);
    for k in 0..K {
        let ck = b.constant(0.3 + (k as f64) * 0.1);
        let e = b.binary(Op::Add, ck, xe);
        let v = if !with_op {
            e
        } else {
            match op.arity() {
                1 => b.unary(op, e),
                2 => {
                    let c2 = b.constant(0.7 + (k as f64) * 0.05);
                    b.binary(op, e, c2)
                }
                _ => {
                    let c2 = b.constant(1.0001);
                    let c3 = b.constant(0.001);
                    b.ternary(op, e, c2, c3)
                }
            }
        };
        acc = b.binary(Op::Add, acc, v);
    }
    b.finish(acc)
}

/// median ns/eval of the JIT-compiled kernel.
fn median_ns(t: &Term) -> f64 {
    let vt = match Gate::for_target(1e-2, 1e-2, 5).promote(t.clone(), t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(_) => unreachable!("identity"),
    };
    let jf = install(vt, &LowerConfig::default(), &Gate::for_target(1e-2, 1e-2, 6))
        .expect("calib kernel must lower");
    let env = [0.7355];
    let mut samples: Vec<f64> = (0..RUNS)
        .map(|_| {
            let t0 = Instant::now();
            let mut acc = 0.0;
            for _ in 0..EVALS { acc += jf.call(&env); }
            std::hint::black_box(acc);
            t0.elapsed().as_nanos() as f64 / EVALS as f64
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[RUNS / 2]
}

pub struct CostTable {
    pub rows: Vec<(Op, u64)>,
}

pub fn measure_cost_table() -> CostTable {
    use Op::*;
    let ops = [
        Add, Sub, Mul, Div, Min, Max, Pow, Neg, Abs, Sqrt,
        Floor, Ceil, Sin, Cos, Tan, Exp, Ln, Fma, Select,
    ];
    let base = median_ns(&sum_kernel(Add, false));
    let add_full = median_ns(&sum_kernel(Add, true));
    let add_unit = ((add_full - base) / K as f64).max(0.05); // ns per add

    let mut rows = Vec::new();
    for op in ops {
        let t = median_ns(&sum_kernel(op, true));
        let per = ((t - base) / K as f64).max(add_unit * 0.5);
        let w = ((per / add_unit).round() as u64).max(1); // O4: w ≥ 1
        rows.push((op, w));
    }
    CostTable { rows }
}

pub fn render_cost_table(t: &CostTable) -> String {
    let env = harness::EnvFingerprint::capture();
    let mut out = String::new();
    let _ = writeln!(out, "# dge calib cost table — relative op weights (add=1)");
    let _ = writeln!(out, "# env: {} fma={} avx={} libm={}",
        env.target_triple, env.fma, env.avx, env.libm);
    for (op, w) in &t.rows {
        let _ = writeln!(out, "{} {}", op.name(), w);
    }
    out
}

/// v2.1 §6 perf-targets report. Values are MEASURED here; the targets stay
/// labeled as they are in the spec.
pub fn perf_targets_report() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== perf targets (v2.1 §6) — misses block PERF SIGN-OFF ONLY ==");
    if cfg!(debug_assertions) {
        let _ = writeln!(out, "NOTE: debug build — numbers below understate release performance.");
    }

    // (a) jit speedup on a 24-op Horner kernel
    let mut src = String::from("(var 0)");
    for k in 0..24 { src = format!("(+ (* {src} (var 0)) {k}.5)"); }
    let t = term::sexpr::parse(&src).unwrap();
    let vt = match Gate::default_dial(77).promote(t.clone(), &t) {
        GateOutcome::Promoted(v) => v,
        GateOutcome::Refuted(_) => unreachable!(),
    };
    match install(vt, &LowerConfig::default(), &Gate::default_dial(78)) {
        Ok(jf) => {
            let env = [1.2345];
            let iters = 100_000u64;
            let t0 = Instant::now();
            let mut a = 0.0;
            for _ in 0..iters { a += jf.call(&env); }
            let jit_ns = t0.elapsed().as_nanos() as f64 / iters as f64;
            let t1 = Instant::now();
            let mut b = 0.0;
            for _ in 0..iters { b += jf.interp(&env); }
            let interp_ns = t1.elapsed().as_nanos() as f64 / iters as f64;
            std::hint::black_box((a, b));
            let ratio = interp_ns / jit_ns;
            let _ = writeln!(out,
                "jit speedup (24-op kernel) : {ratio:.1}x   target >=5x (HYPOTHESIZED)  [{}]",
                if ratio >= 5.0 { "PASS" } else { "MISS" });
        }
        Err(e) => { let _ = writeln!(out, "jit speedup: install failed: {e:?}"); }
    }

    // (b) memo hit latency
    let cache = memo::cache::MemoCache::new();
    let probe = term::sexpr::parse("(sin (+ (var 0) 1.0))").unwrap();
    let env = [0.25];
    cache.eval(&probe, &env); // prime
    let iters = 50_000u64;
    let t0 = Instant::now();
    let mut acc = 0.0;
    for _ in 0..iters { acc += cache.eval(&probe, &env); }
    std::hint::black_box(acc);
    let hit_ns = t0.elapsed().as_nanos() as f64 / iters as f64;
    let _ = writeln!(out,
        "memo hit latency           : {hit_ns:.0} ns  target <=50 ns (HYPOTHESIZED)  [{}]",
        if hit_ns <= 50.0 { "PASS" } else { "MISS" });

    // (c) SR convergence on the reference task (x^2 + x, 32 samples)
    let oracle = term::sexpr::parse("(+ (* (var 0) (var 0)) (var 0))").unwrap();
    let mut srng = harness::strategy::Rng::new(7);
    let targets: Vec<(Vec<f64>, f64)> = (0..32)
        .map(|_| { let x = srng.uniform01() * 6.0 - 3.0; (vec![x], eval(&oracle, &[x])) })
        .collect();
    let cfg = gp::pop::GpConfig::default();
    let out5 = gp::evolve::run(&cfg, &gp::evolve::EvolveParams::default(),
        &gp::fitness::FitnessParams::default(), &targets,
        &mut harness::strategy::Rng::new(1));
    let _ = writeln!(out,
        "SR generations (x^2+x ref) : {}   target <=40 gen (HYPOTHESIZED)  [{}]",
        out5.generations,
        if out5.best_error == 0.0 && out5.generations <= 40 { "PASS" } else { "MISS" });

    out
}

pub fn run(args: &[String]) {
    let out_path = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1)).map(String::as_str)
        .unwrap_or("artifacts/calib/cost_table.txt");

    println!("measuring per-op costs (jitted {K}-op sum kernels, median of {RUNS} runs)...");
    let table = measure_cost_table();
    let rendered = render_cost_table(&table);
    if let Some(dir) = std::path::Path::new(out_path).parent() {
        std::fs::create_dir_all(dir).ok();
    }
    match std::fs::write(out_path, &rendered) {
        Ok(()) => println!("cost table -> {out_path}\n{rendered}"),
        Err(e) => eprintln!("write {out_path}: {e}\n{rendered}"),
    }
    println!("{}", perf_targets_report());
}
