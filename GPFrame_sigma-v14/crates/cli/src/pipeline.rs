//! `dge pipeline <file.rs> <fn_name>` — the engineer loop in one command:
//!
//!   FRONT DOOR (syn extract, or `dge lift` via --lift / automatic fallback
//!   when the syn door refuses a .rs input)
//!     ──▶ refactor (Tier A, or --eps with mandatory Tier B)
//!     ──▶ emit Rust WITH the certificate attached as a doc comment
//!     ──▶ EMISSION round-trip gate (emit∘extract ≡ id over μ′, seq-aware).
//!
//! Rust in → certified Rust out. Any gate failure aborts with the witness;
//! nothing uncertified is printed as a result.
//!
//! Both front doors are UNTRUSTED lowerings (L1). The emission round-trip
//! closure runs through the SYN door regardless of which door the term came
//! in through — emitted code is clean Rust in the Σ v1.2 shape, which is
//! exactly the syn extractor's contract. That closure is also what makes the
//! IR path honest: lift(IR) → emit → extract must agree with the lifted term
//! bitwise over μ′, or the output is withheld.

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
pub fn emission_round_trip(t: &term::Term, code: &str, name: &str) -> Result<(), String> {
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

/// Which front door produced the term (reported, and recorded in stderr).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Syn,
    /// LLVM IR lifting; carries no syn-door coverage guarantee — the term is
    /// whatever the compiler's dataflow said the function means.
    Lift,
}

pub struct PipelineOpts {
    pub eps: bool,
    pub domain: Option<f64>,
    pub artifacts: std::path::PathBuf,
    /// force the IR door (skip syn entirely)
    pub lift: bool,
}

impl Default for PipelineOpts {
    fn default() -> Self {
        Self { eps: false, domain: None, artifacts: "artifacts/o1".into(), lift: false }
    }
}

#[derive(Debug)]
pub struct Certified {
    pub code: String,
    pub door: Door,
    pub cost_before: u64,
    pub cost_after: u64,
}

/// The whole loop as a library call: front door(s) → refactor under the gate
/// → emit with certificate → emission round-trip. Errors are refusals with
/// reasons; nothing uncertified is ever returned.
pub fn certify(file: &str, name: &str, opts: &PipelineOpts) -> Result<Certified, String> {
    // 1. front door
    let (t, door) = if opts.lift {
        (front_door_lift(file, name)?, Door::Lift)
    } else {
        let src = std::fs::read_to_string(file).map_err(|e| format!("read {file}: {e}"))?;
        match extract_fn(&src, name) {
            Ok(t) => (t, Door::Syn),
            Err(syn_err) if file.ends_with(".rs") => {
                // the mission behavior: unsupported SYNTAX is not unsupported
                // MATH — read what the CPU is told instead. Both doors are
                // untrusted; the gates below arbitrate either way.
                eprintln!("      syn door refused ({syn_err:?}); trying the IR door");
                match front_door_lift(file, name) {
                    Ok(t) => (t, Door::Lift),
                    Err(lift_err) => return Err(format!(
                        "both front doors refused `{name}`:\n  syn : {syn_err:?}\n  lift: {lift_err}")),
                }
            }
            Err(e) => return Err(format!("extraction failed: {e:?}")),
        }
    };
    eprintln!("[1/4] {} `{name}` ({} nodes, arity {}, {} seq{})",
        match door { Door::Syn => "extracted", Door::Lift => "lifted (LLVM IR)" },
        t.len(), t.arity(), t.seq_count(), if t.seq_count() == 1 { "" } else { "s" });

    // 2. refactor under the gate
    let mut gate = Gate::default_dial(0x717E);
    if opts.eps { gate.metric = Metric::fma_mixed(); }
    if let Some(mag) = opts.domain {
        gate.mu = MuPrime::bounded(0x717E, mag);
        eprintln!("      (A-1 domain bound |x| <= {mag:e} — enters the certificate)");
    }
    let calib = Path::new("artifacts/calib/cost_table.txt");
    let calibrated = rules::cost::CalibratedCost::load(calib).ok();
    let default_cost = rules::cost::DefaultCost;
    let cost: &dyn rules::cost::CostFn = match &calibrated {
        Some(c) => c, None => &default_cost,
    };
    let out = rules::refactor_with_cost(
        &t, opts.eps, &gate, &opts.artifacts, &SaturationLimits::default(), cost)
        .map_err(|e| match e {
            rules::RefactorError::UndischargedRule(r) => format!(
                "REFUSED: rule `{r}` undischarged — run `dge discharge` first"),
            rules::RefactorError::GateRefuted { minimal_env } => format!(
                "Tier-B gate REFUTED the rewrite; minimal counterexample: \
                 {minimal_env:?}\n(the original function is kept — nothing \
                 uncertified ships)"),
        })?;
    eprintln!("[2/4] refactored: cost {} -> {} via [{}]",
        out.cost_before, out.cost_after, out.rule_trace.join(", "));

    // 3. emit with certificate
    let new_name = format!("{name}_dge");
    let code = emit_rust(out.verified.term(), &new_name, Some(out.verified.certificate()));
    eprintln!("[3/4] emitted `{new_name}`");

    // 4. emission gate (always through the SYN door — the round-trip closure)
    emission_round_trip(out.verified.term(), &code, &new_name)
        .map_err(|e| format!("EMISSION GATE FAILED: {e} — output withheld"))?;
    eprintln!("[4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)");

    Ok(Certified { code, door, cost_before: out.cost_before, cost_after: out.cost_after })
}

fn front_door_lift(file: &str, name: &str) -> Result<term::Term, String> {
    let ir = if file.ends_with(".rs") {
        crate::lift::rustc_emit_ir(Path::new(file), name)?
    } else {
        std::fs::read_to_string(file).map_err(|e| format!("read {file}: {e}"))?
    };
    crate::lift::lift_ll(&ir, name).map_err(|e| e.to_string())
}

pub fn run(args: &[String]) {
    let (Some(file), Some(name)) = (args.first(), args.get(1)) else {
        eprintln!("usage: dge pipeline <file.rs|file.ll> <fn_name> [--lift] \
                   [--eps [--domain <mag>]] [--artifacts <dir>] [--out <file.rs>]");
        return;
    };
    let opts = PipelineOpts {
        eps: args.iter().any(|a| a == "--eps"),
        domain: args.iter().position(|a| a == "--domain")
            .and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<f64>().ok()),
        artifacts: args.iter().position(|a| a == "--artifacts")
            .and_then(|i| args.get(i + 1)).map(String::as_str)
            .unwrap_or("artifacts/o1").into(),
        lift: args.iter().any(|a| a == "--lift") || file.ends_with(".ll"),
    };
    match certify(file, name, &opts) {
        Ok(c) => {
            println!("{}", c.code);
            if let Some(i) = args.iter().position(|a| a == "--out") {
                if let Some(path) = args.get(i + 1) {
                    std::fs::write(path, &c.code).ok();
                    eprintln!("-> {path}");
                }
            }
        }
        Err(e) => eprintln!("{e}"),
    }
}
