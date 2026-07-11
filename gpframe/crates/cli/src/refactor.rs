//! `dge refactor <fn.sexpr> [--eps]` — in: fn slice; out: (P', certificate).

use harness::metric::Metric;
use harness::Gate;
use rules::extract::SaturationLimits;
use std::path::Path;

pub fn run(args: &[String]) {
    let Some(file) = args.first() else {
        eprintln!("usage: dge refactor <fn.sexpr> [--eps] [--artifacts <dir>]");
        return;
    };
    let eps = args.iter().any(|a| a == "--eps");
    let artifacts = args.iter().position(|a| a == "--artifacts")
        .and_then(|i| args.get(i + 1)).map(String::as_str)
        .unwrap_or("artifacts/o1");

    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {file}: {e}"); return; }
    };
    let p = match term::sexpr::parse(src.trim()) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse: {e:?}"); return; }
    };

    let mut gate = Gate::default_dial(0xD6E);
    if eps { gate.metric = Metric::fma_mixed(); }

    let calib_path = Path::new("artifacts/calib/cost_table.txt");
    let calibrated = rules::cost::CalibratedCost::load(calib_path).ok();
    let default_cost = rules::cost::DefaultCost;
    let cost: &dyn rules::cost::CostFn = match &calibrated {
        Some(c) => { eprintln!("(using calibrated cost table {})", calib_path.display()); c }
        None => &default_cost,
    };
    match rules::refactor_with_cost(&p, eps, &gate, Path::new(artifacts), &SaturationLimits::default(), cost) {
        Ok(out) => {
            println!("input : {}", term::sexpr::print(&p));
            println!("output: {}", term::sexpr::print(out.verified.term()));
            println!("cost  : {} -> {}{}", out.cost_before, out.cost_after,
                if out.budget_hit { "  (budget hit: best-so-far, I7)" } else { "" });
            println!("rules : {}", out.rule_trace.join(", "));
            println!("claim : {}", out.verified.certificate().claim());
        }
        Err(rules::RefactorError::UndischargedRule(r)) => {
            eprintln!("REFUSED: rule `{r}` has no O1 artifact in {artifacts} — run `dge discharge` first");
        }
        Err(rules::RefactorError::GateRefuted { minimal_env }) => {
            eprintln!("Tier-B gate REFUTED the extraction; minimal counterexample: {minimal_env:?}");
        }
    }
}
