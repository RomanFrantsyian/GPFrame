//! `dge debug <broken.sexpr> <oracle.sexpr> [--repair]` — the §4.3 flow:
//! minimal counterexample (T3) + Ochiai localization (AID, NEVER VERDICT),
//! optionally followed by gate-certified repair.

use gp::repair::{repair, RepairParams};
use harness::strategy::Rng;
use harness::{Gate, shrink};
use locate::report::DebugReport;
use locate::{ochiai, spectrum};
use term::eval;

pub fn run(args: &[String]) {
    let (Some(broken_f), Some(oracle_f)) = (args.first(), args.get(1)) else {
        eprintln!("usage: dge debug <broken.sexpr> <oracle.sexpr> [--repair]");
        return;
    };
    let do_repair = args.iter().any(|a| a == "--repair");

    let parse_file = |f: &str| -> Option<term::Term> {
        let src = std::fs::read_to_string(f).map_err(|e| eprintln!("read {f}: {e}")).ok()?;
        term::sexpr::parse(src.trim()).map_err(|e| eprintln!("parse {f}: {e:?}")).ok()
    };
    let (Some(broken), Some(oracle)) = (parse_file(broken_f), parse_file(oracle_f)) else { return };

    let gate = Gate::default_dial(0xdeb);
    let arity = oracle.arity().max(1);
    let mut rng = Rng::new(gate.mu.seed);
    let fails = |env: &[f64]| eval(&broken, env).to_bits() != eval(&oracle, env).to_bits();

    // hunt a counterexample under mu'
    let mut found: Option<Vec<f64>> = None;
    let mut suite: Vec<Vec<f64>> = Vec::new();
    for _ in 0..gate.n {
        let e = gate.mu.sample(&mut rng, arity);
        if suite.len() < 64 { suite.push(e.clone()); }
        if fails(&e) { found = Some(e); break; }
    }
    let Some(e) = found else {
        println!(
            "no counterexample found: equivalent within Bitwise at confidence {} over {}; \
             defect regions of measure < {:.2e} are invisible (n={})",
            1.0 - gate.alpha, gate.mu.spec_string(), gate.delta_min(), gate.n
        );
        return;
    };

    let mut f2 = |env: &[f64]| fails(env);
    let minimal = shrink::shrink(e.clone(), &mut f2);
    let phi = |env: &[f64], out: f64| out.to_bits() == eval(&oracle, env).to_bits();
    let spec = spectrum::collect(&broken, &suite, &phi);
    let ranking = ochiai::rank(&spec);

    let report = DebugReport {
        minimal_ce: harness::CounterExample {
            candidate_val: eval(&broken, &minimal),
            reference_val: eval(&oracle, &minimal),
            minimal_env: minimal,
            original_env: e,
        },
        ranking: ranking.clone(),
    };
    print!("{}", report.render());

    if do_repair {
        println!("-- attempting repair (exit only via gate; honest null on miss) --");
        match repair(&broken, &ranking, &oracle, &gate, &RepairParams::default(), 0xf1f) {
            Some(fix) => {
                println!("REPAIRED: {}", term::sexpr::print(fix.term()));
                println!("claim   : {}", fix.certificate().claim());
            }
            None => println!("no certified repair within budget — original kept (A-3 honest null)"),
        }
    }
}
