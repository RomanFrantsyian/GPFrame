//! `dge pipeline <file.rs> <fn_name>` — the engineer loop in one command:
//!
//!   extract ──▶ EXTRACTION GATE (interp vs nothing to compare against a
//!               compiled original here, so the gate is emit∘extract
//!               round-trip closure) ──▶ refactor (Tier A, or --eps with
//!               mandatory Tier B) ──▶ emit Rust WITH the certificate
//!               attached as a doc comment ──▶ EMISSION round-trip check.
//!
//! Rust in → certified Rust out. Any gate failure aborts with the witness;
//! nothing uncertified is printed as a result.

use crate::emit::emit_rust;
use crate::extract::extract_fn;
use harness::metric::Metric;
use harness::strategy::{MuPrime, Rng};
use harness::Gate;
use rules::extract::SaturationLimits;
use std::path::Path;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// emit∘extract round-trip closure over μ' (BitwiseNanClass) — seq-aware.
fn emission_round_trip(t: &term::Term, code: &str, name: &str) -> Result<(), String> {
    let t2 = extract_fn(code, name).map_err(|e| format!("re-extraction: {e:?}"))?;
    let arity = t.arity().max(1);
    let k = t.seq_count();
    let mu = MuPrime::default_with_seed(0x717E);
    let mut rng = Rng::new(0x717E);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, arity, k);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let (a, b) = (term::eval_with_seqs(t, &e, &sl), term::eval_with_seqs(&t2, &e, &sl));
        if !xbit_eq(a, b) {
            return Err(format!("round-trip drift at scalars {e:?} seqs {sq:?}: {a} vs {b}"));
        }
    }
    Ok(())
}

pub fn run(args: &[String]) {
    let (Some(file), Some(name)) = (args.first(), args.get(1)) else {
        eprintln!("usage: dge pipeline <file.rs> <fn_name> [--eps [--domain <mag>]] \
                   [--artifacts <dir>] [--out <file.rs>]");
        return;
    };
    let eps = args.iter().any(|a| a == "--eps");
    let artifacts = args.iter().position(|a| a == "--artifacts")
        .and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("artifacts/o1");

    // 1. extract
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s, Err(e) => { eprintln!("read {file}: {e}"); return; }
    };
    let t = match extract_fn(&src, name) {
        Ok(t) => t, Err(e) => { eprintln!("extraction failed: {e:?}"); return; }
    };
    eprintln!("[1/4] extracted `{name}` ({} nodes, arity {})", t.len(), t.arity());

    // 2. refactor under the gate
    let mut gate = Gate::default_dial(0x717E);
    if eps { gate.metric = Metric::fma_mixed(); }
    if let Some(mag) = args.iter().position(|a| a == "--domain")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<f64>().ok())
    {
        gate.mu = MuPrime::bounded(0x717E, mag);
        eprintln!("      (A-1 domain bound |x| <= {mag:e} — enters the certificate)");
    }
    let calib = Path::new("artifacts/calib/cost_table.txt");
    let calibrated = rules::cost::CalibratedCost::load(calib).ok();
    let default_cost = rules::cost::DefaultCost;
    let cost: &dyn rules::cost::CostFn = match &calibrated {
        Some(c) => c, None => &default_cost,
    };
    let out = match rules::refactor_with_cost(
        &t, eps, &gate, Path::new(artifacts), &SaturationLimits::default(), cost)
    {
        Ok(o) => o,
        Err(rules::RefactorError::UndischargedRule(r)) => {
            eprintln!("REFUSED: rule `{r}` undischarged — run `dge discharge` first");
            return;
        }
        Err(rules::RefactorError::GateRefuted { minimal_env }) => {
            eprintln!("Tier-B gate REFUTED the rewrite; minimal counterexample: {minimal_env:?}");
            eprintln!("(the original function is kept — nothing uncertified ships)");
            return;
        }
    };
    eprintln!("[2/4] refactored: cost {} -> {} via [{}]",
        out.cost_before, out.cost_after, out.rule_trace.join(", "));

    // 3. emit with certificate
    let new_name = format!("{name}_dge");
    let code = emit_rust(out.verified.term(), &new_name, Some(out.verified.certificate()));
    eprintln!("[3/4] emitted `{new_name}`");

    // 4. emission gate
    match emission_round_trip(out.verified.term(), &code, &new_name) {
        Ok(()) => eprintln!("[4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)"),
        Err(e) => { eprintln!("[4/4] EMISSION GATE FAILED: {e} — output withheld"); return; }
    }

    println!("{code}");
    if let Some(i) = args.iter().position(|a| a == "--out") {
        if let Some(path) = args.get(i + 1) {
            std::fs::write(path, &code).ok();
            eprintln!("-> {path}");
        }
    }
}
