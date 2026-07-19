//! dge-sdk — the stable integration surface for third-party features.
//!
//! # The one invariant that shapes everything here
//!
//! DGE's soundness lives in a typestate spine:
//!
//! ```text
//!   Term  --Gate::promote-->  VerifiedTerm  --jit::install-->  JitFn
//! ```
//!
//! `VerifiedTerm` has a private constructor inside `harness` — nothing in
//! this SDK, and nothing a plugin can write, mints one. Therefore EVERY
//! extension point sits on the UNTRUSTED side of the spine:
//!
//! * **Front doors** (the plugin trait) produce `Term`s. A door may be
//!   wrong, lazy, or malicious — the Gate arbitrates bitwise over μ′ and
//!   refutes lies with a ⊏-minimal counterexample. Registering a door
//!   grants zero authority.
//! * **Observers** (the hook trait) SEE events (extractions, gate
//!   verdicts, refusals) and can veto nothing. Hooks are taps, not gates.
//! * The Engine returns *reports* and *emitted code*, never a
//!   `VerifiedTerm` — the verified state does not cross the SDK boundary,
//!   and it does not cross a network boundary either (see the server
//!   crate): remote callers get certificates-as-data, which claim nothing
//!   until re-gated locally.
//!
//! Integration is documentation-only by design: `docs/SDK.md` (this
//! crate) and `docs/API.md` (the HTTP surface) are the contract. If a
//! task seems to require reading core internals, that is an SDK gap —
//! file it, don't peek.

use std::sync::Arc;

/// SDK-side optimization plugins (Suggesters) built on the sealed engine.
pub mod plugins;

pub use harness::gate::{CounterExample, Gate, GateOutcome};
pub use term::{eval_with_seqs, sexpr, Op, Term};
pub use term::ext::{ExtFn, ExtOpDef};

/// Register an extension operator (Σ-ext): pluggable SEMANTICS without
/// kernel edits. `fingerprint` is your semantic identity claim (source
/// hash, spec ID) — it enters every certificate the op touches, so
/// claims made under your semantics say so forever. Ops must be pure
/// and deterministic per call (&[f64] -> f64); the Gate double-runs
/// every sample and a nondeterministic op refutes itself. Arity 1 or 2.
pub fn register_ext_op(
    name: &str, version: &str, fingerprint: &str, arity: usize,
    f: impl Fn(&[f64]) -> f64 + Send + Sync + 'static,
) -> Result<(), String> {
    term::ext::register(name, version, fingerprint, arity, Arc::new(f))
}

// ---------------------------------------------------------------- types --

/// What a front door is asked to translate.
#[derive(Debug, Clone)]
pub struct ExtractRequest<'a> {
    /// Rust source text (a file's worth is fine — doors search it).
    pub source: &'a str,
    /// The function to extract, by name.
    pub fn_name: &'a str,
}

/// An honest "no": the door names the exact reason in the project's
/// refusal vocabulary. Refusals are DATA (they feed roadmap pricing);
/// `class()` buckets them with the same classifier the field trials use.
#[derive(Debug, Clone)]
pub struct Refusal(pub String);

impl Refusal {
    /// Histogram bucket, identical to `dge trial` reports.
    pub fn class(&self) -> &'static str {
        cli::trial::bucket(&self.0)
    }
}

/// Serializable gate verdict — what crosses the SDK boundary instead of
/// `VerifiedTerm`.
#[derive(Debug)]
pub enum GateReport {
    /// The candidate survived n μ′ samples bit-for-bit. `emitted` is the
    /// certified-comment Rust code for the candidate; the certificate
    /// parameters are echoed so callers can print honest claims.
    Promoted {
        n: u64,
        alpha: f64,
        emitted: String,
    },
    /// Bit-level disagreement, shrunk ⊏-minimally.
    Refuted(Box<CounterExample>),
    /// No verdict COULD be produced — e.g. the term uses an extension op
    /// that is not registered. Not a refutation: register the plugin and
    /// gate again.
    Refused(String),
}

// --------------------------------------------------------------- traits --

/// A pluggable extraction front door. Implementations translate source
/// text into candidate `Term`s — or refuse honestly.
///
/// Doors are UNTRUSTED by architecture: nothing you return here is
/// believed until the Gate says so. That is a feature — it means a door
/// can be experimental, heuristic, even LLM-backed, without any core
/// review: the worst a wrong door yields is a refuted candidate with a
/// counterexample attached.
pub trait FrontDoor: Send + Sync {
    /// Stable identifier (shows up in reports and observer events).
    fn name(&self) -> &str;
    fn extract(&self, req: &ExtractRequest) -> Result<Term, Refusal>;
}

/// Read-only hooks at the engine's defined events. Default methods are
/// no-ops — implement what you need. Observers cannot mutate terms,
/// verdicts, or refusals (taps, not gates).
pub trait Observer: Send + Sync {
    /// After any door runs (admission or refusal).
    fn on_extract(&self, _door: &str, _fn_name: &str,
                  _outcome: &Result<Term, Refusal>) {}
    /// After a gate run completes.
    fn on_gate(&self, _fn_name: &str, _report: &GateReport) {}
}

// ------------------------------------------------------- built-in doors --

/// The core syn door (`cli::extract::extract_fn`) behind the trait.
pub struct SynDoor;
impl FrontDoor for SynDoor {
    fn name(&self) -> &str { "syn" }
    fn extract(&self, req: &ExtractRequest) -> Result<Term, Refusal> {
        cli::extract::extract_fn(req.source, req.fn_name)
            .map_err(|e| Refusal(format!("{e:?}")))
    }
}

/// The core LLVM-IR door: rustc -O1 emission then `lift_ll`. Requires a
/// `rustc` on PATH; source is compiled from a temp file.
pub struct IrDoor;
impl FrontDoor for IrDoor {
    fn name(&self) -> &str { "ir" }
    fn extract(&self, req: &ExtractRequest) -> Result<Term, Refusal> {
        // unique per CALL, not just per process: two concurrent requests
        // (e.g. two simultaneous dge-serve callers) for the SAME fn_name
        // would otherwise collide writing their own source file before
        // rustc even runs — the same collision class fixed in
        // rustc_emit_ir itself (see its doc comment), one layer up.
        let dir = cli::lift::unique_tmp_dir("dge_sdk_ir");
        std::fs::create_dir_all(&dir).map_err(|e| Refusal(e.to_string()))?;
        let f = dir.join(format!("{}.rs", req.fn_name));
        std::fs::write(&f, req.source).map_err(|e| Refusal(e.to_string()))?;
        let ir = cli::lift::rustc_emit_ir(&f, req.fn_name)
            .map_err(Refusal)?;
        cli::lift::lift_ll(&ir, req.fn_name)
            .map_err(|e| Refusal(format!("{e:?}")))
    }
}

/// A pluggable OPTIMIZATION hypothesis source (v1.8). A `Suggester`
/// proposes candidate rewrites of a term — it does NOT get to decide
/// whether they're correct. Every candidate is re-gated by the core
/// Gate exactly like a `FrontDoor`'s output; a suggester holds zero
/// authority over what ships, only over what gets TRIED. This is how
/// domain-specific optimization knowledge (an SDF-smoothing identity, a
/// DSP simplification Z3 doesn't know) becomes usable without ever
/// trusting the plugin's math — same argument as FrontDoor and
/// register_ext_op, applied to the rewrite side of the pipeline instead
/// of the extraction side.
pub trait Suggester: Send + Sync {
    fn name(&self) -> &str;
    /// Propose zero or more candidate rewrites of `t`. Candidates need
    /// not be smaller, faster, or even different — the Gate and the
    /// cost function decide what sticks. A suggester that proposes
    /// nothing for a given term is normal, not an error.
    fn suggest(&self, t: &Term) -> Vec<Term>;
}

/// Outcome of one candidate: accepted (gated AND strictly cheaper than
/// the current best) or not, with the reason either way. Never leaks a
/// `VerifiedTerm` — same boundary discipline as `GateReport`.
#[derive(Debug)]
pub enum ProposalOutcome {
    Accepted { cost_before: u64, cost_after: u64 },
    RefusedNotCheaper { cost: u64, current_best: u64 },
    Refuted(Box<CounterExample>),
    /// No verdict could be produced (e.g. an unregistered extension op in
    /// the candidate) — distinct from a bit-level Refuted, same
    /// distinction as `GateReport::Refused`.
    Refused(String),
}

/// One proposal's full provenance: which suggester, which candidate
/// (by index in that suggester's batch), what happened.
#[derive(Debug)]
pub struct ProposalRecord {
    pub suggester: String,
    pub index: usize,
    pub outcome: ProposalOutcome,
}

pub struct OptimizeReport {
    /// Certified Rust for the best term found — the ORIGINAL if nothing
    /// beat it. Always gated: this is a `VerifiedTerm`'s emission, never
    /// an unarbitrated suggestion.
    pub emitted: String,
    pub original_cost: u64,
    pub final_cost: u64,
    /// Every proposal from every suggester, in trial order — the full
    /// audit trail, including rejections (rejections are data, same
    /// principle as door refusals feeding trial histograms).
    pub proposals: Vec<ProposalRecord>,
}

/// Pluggable output target (v1.9, roadmap Phase 1). Emission runs
/// strictly AFTER promotion — it can never affect what gets certified,
/// only how the already-certified result is printed. Zero new trust
/// boundary: an `Emitter` cannot lie its way into a certificate, only
/// mis-print a real one (a bug, not a soundness hole).
pub trait Emitter: Send + Sync {
    fn emit(&self, term: &Term, cert: Option<&harness::Certificate>) -> String;
}

/// The core Rust emitter (`cli::emit::emit_rust`) behind the trait —
/// the default every `Engine` uses unless told otherwise.
pub struct RustEmitter;
impl Emitter for RustEmitter {
    fn emit(&self, term: &Term, cert: Option<&harness::Certificate>) -> String {
        cli::emit::emit_rust(term, "f", cert)
    }
}

/// The SDK facade. Owns registered doors and observers; exposes the
/// engine's operations WITHOUT exposing its internals.
pub struct Engine {
    doors: Vec<Arc<dyn FrontDoor>>,
    observers: Vec<Arc<dyn Observer>>,
    suggesters: Vec<Arc<dyn Suggester>>,
    seed: u64,
}

impl Engine {
    /// Core doors pre-registered (syn, ir). The seed parameterizes the
    /// gate's μ′ stream — same seed, same verdict, reproducible reports.
    pub fn new(seed: u64) -> Self {
        Engine {
            doors: vec![Arc::new(SynDoor), Arc::new(IrDoor)],
            observers: vec![],
            suggesters: vec![],
            seed,
        }
    }

    /// A bare engine with NO doors — for embedders that want full control.
    pub fn bare(seed: u64) -> Self {
        Engine { doors: vec![], observers: vec![], suggesters: vec![], seed }
    }

    pub fn register_door(&mut self, d: Arc<dyn FrontDoor>) { self.doors.push(d); }
    pub fn register_observer(&mut self, o: Arc<dyn Observer>) { self.observers.push(o); }
    pub fn register_suggester(&mut self, s: Arc<dyn Suggester>) { self.suggesters.push(s); }
    pub fn door_names(&self) -> Vec<&str> {
        self.doors.iter().map(|d| d.name()).collect()
    }

    /// Run ONE door by name.
    pub fn extract(&self, door: &str, req: &ExtractRequest)
        -> Result<Term, Refusal>
    {
        let d = self.doors.iter().find(|d| d.name() == door)
            .ok_or_else(|| Refusal(format!("no door named `{door}`")))?;
        let out = d.extract(req);
        for o in &self.observers { o.on_extract(door, req.fn_name, &out); }
        out
    }

    /// Run EVERY registered door; per-door outcomes in registration order.
    pub fn extract_all(&self, req: &ExtractRequest)
        -> Vec<(String, Result<Term, Refusal>)>
    {
        self.doors.iter().map(|d| {
            let out = d.extract(req);
            for o in &self.observers { o.on_extract(d.name(), req.fn_name, &out); }
            (d.name().to_string(), out)
        }).collect()
    }

    /// THE arbitration point. Gates `candidate` against `reference` over
    /// n = 10⁴ μ′ samples (bitwise; L = 0 exercised for folds). On
    /// promotion the verified term is emitted to Rust source INSIDE this
    /// call — `VerifiedTerm` never leaves.
    pub fn gate(&self, fn_name: &str, candidate: Term, reference: &Term)
        -> GateReport
    {
        // Arity pre-flight: Gate::promote asserts (panics) on mismatch —
        // that's a reasonable contract for kernel callers who pre-
        // validate, but the SDK boundary exists precisely so a caller's
        // mistake (or a malicious FrontDoor) never crashes the host.
        // Found by field-trial fuzzing, 2026-07-18.
        if candidate.arity() != reference.arity() {
            let r = GateReport::Refused(format!(
                "candidate arity {} != reference arity {} -- cannot gate \
                 two terms with different numbers of scalar inputs",
                candidate.arity(), reference.arity()));
            for o in &self.observers { o.on_gate(fn_name, &r); }
            return r;
        }
        // Σ-ext pre-flight: unregistered ops get an honest Refused (the
        // core gate would panic — data-plane misuse; the SDK boundary
        // converts it to a report)
        for exts in [&candidate.exts, &reference.exts] {
            if let Err(m) = term::ext::tags_for(exts) {
                let r = GateReport::Refused(m);
                for o in &self.observers { o.on_gate(fn_name, &r); }
                return r;
            }
        }
        let g = Gate::default_dial(self.seed);
        let (n, alpha) = (g.n, g.alpha);
        let report = match g.promote(candidate, reference) {
            GateOutcome::Promoted(vt) => GateReport::Promoted {
                n, alpha,
                emitted: cli::emit::emit_rust(vt.term(), fn_name,
                                              Some(vt.certificate())),
            },
            GateOutcome::Refuted(w) => GateReport::Refuted(w),
        };
        for o in &self.observers { o.on_gate(fn_name, &report); }
        report
    }

    /// Convenience: extract via every door, cross-gate all admitted pairs,
    /// and gate the first admitted term against itself (identity gate) if
    /// only one door admitted. Returns per-door outcomes plus the final
    /// report for the first admitted candidate.
    pub fn certify(&self, req: &ExtractRequest)
        -> (Vec<(String, Result<Term, Refusal>)>, Option<GateReport>)
    {
        let outs = self.extract_all(req);
        let admitted: Vec<(&String, &Term)> = outs.iter()
            .filter_map(|(d, r)| r.as_ref().ok().map(|t| (d, t)))
            .collect();
        let report = match admitted.as_slice() {
            [] => None,
            [(_, only)] => Some(self.gate(req.fn_name, (*only).clone(), only)),
            [(_, first), rest @ ..] => {
                // cross-door: every other admit must agree with the first
                for (_, t) in rest {
                    match self.gate(req.fn_name, (*t).clone(), first) {
                        GateReport::Refuted(w) =>
                            return (outs, Some(GateReport::Refuted(w))),
                        GateReport::Refused(m) =>
                            return (outs, Some(GateReport::Refused(m))),
                        GateReport::Promoted { .. } => {}
                    }
                }
                Some(self.gate(req.fn_name, (*first).clone(), first))
            }
        };
        (outs, report)
    }

    /// Evaluate a term (interpreter semantics — the reference semantics).
    pub fn eval(&self, t: &Term, env: &[f64], seqs: &[&[f64]]) -> f64 {
        eval_with_seqs(t, env, seqs)
    }

    /// Run every registered `Suggester` against `original` and keep the
    /// cheapest candidate that GATES against `original` — never against
    /// the running best, so acceptance is always "equivalent to the
    /// thing you started with," and a chain of individually-plausible
    /// suggestions can never drift the meaning (no transitive-equivalence
    /// trust chain to exploit). Deterministic order: suggesters run in
    /// registration order, candidates within a suggester in the order
    /// returned; ties keep the earlier (cheaper-to-reproduce) proposal.
    ///
    /// This is Tier B by construction (the SDK boundary never touches
    /// Tier A / SMT — that stays a kernel-only path via `dge refactor`);
    /// a suggester is a hypothesis source, exactly like a `FrontDoor`,
    /// and the Gate is the only thing that ever decides what ships.
    pub fn optimize(&self, fn_name: &str, original: &Term) -> OptimizeReport {
        self.optimize_with_cost(fn_name, original, &rules::cost::DefaultCost)
    }

    /// Same as `optimize`, with a caller-supplied cost function (roadmap
    /// Phase 1). Safe by L2 ("cost irrelevance"): a cost function only
    /// picks a representative among already-certified-equal terms — it
    /// can never change WHETHER something is certified, only which
    /// certified-equal candidate wins. A domain plugin can therefore
    /// safely say "minimize texture-sample-equivalent ops" or "minimize
    /// estimated cycles" instead of generic node count.
    pub fn optimize_with_cost(&self, fn_name: &str, original: &Term,
        cost: &dyn rules::cost::CostFn) -> OptimizeReport
    {
        let original_cost = cost.cost(original);
        let g = Gate::default_dial(self.seed);
        // seed `best_vt` with the identity gate so there is ALWAYS a
        // VerifiedTerm to emit, even if every suggestion is rejected —
        // "no improvement found" still yields certified output (of the
        // unchanged original), never an unarbitrated one
        let mut best_vt = match g.promote(original.clone(), original) {
            GateOutcome::Promoted(vt) => vt,
            GateOutcome::Refuted(w) =>
                unreachable!("identity gate refuted: {w:?}"),
        };
        let mut best_cost = original_cost;
        let mut proposals = Vec::new();

        for s in &self.suggesters {
            for (i, cand) in s.suggest(original).into_iter().enumerate() {
                // pre-flight: unregistered ext ops get an honest record,
                // not a panic (mirrors Engine::gate's own pre-flight)
                let ext_check = term::ext::tags_for(&cand.exts)
                    .and_then(|_| term::ext::tags_for(&original.exts));
                if let Err(m) = ext_check {
                    proposals.push(ProposalRecord {
                        suggester: s.name().into(), index: i,
                        outcome: ProposalOutcome::Refused(m),
                    });
                    continue;
                }
                // arity pre-flight: a mismatched-arity candidate would
                // hit Gate::promote's assert and PANIC the host process
                // — found by field-trial fuzzing (2026-07-18). A
                // careless or malicious Suggester must get an honest
                // Refused, exactly like an unregistered ext op, never a
                // crash. (Gate::promote itself still asserts for
                // KERNEL callers that are expected to pre-validate —
                // this pre-flight is the SDK boundary's job, same
                // division of labor as the ext-op registration check.)
                if cand.arity() != original.arity() {
                    proposals.push(ProposalRecord {
                        suggester: s.name().into(), index: i,
                        outcome: ProposalOutcome::Refused(format!(
                            "candidate arity {} != original arity {} -- \
                             a suggester must propose a rewrite of the \
                             SAME function, not a different one",
                            cand.arity(), original.arity())),
                    });
                    continue;
                }
                let cand_cost = cost.cost(&cand);
                if cand_cost >= best_cost {
                    // cheap check FIRST: don't spend 10^4 samples proving
                    // equivalence of something that couldn't win anyway
                    proposals.push(ProposalRecord {
                        suggester: s.name().into(), index: i,
                        outcome: ProposalOutcome::RefusedNotCheaper {
                            cost: cand_cost, current_best: best_cost,
                        },
                    });
                    continue;
                }
                // ALWAYS gate against the ORIGINAL, never the running
                // best — acceptance is always "equivalent to the thing
                // you started with," so a chain of individually-plausible
                // suggestions can never drift the meaning
                match g.promote(cand.clone(), original) {
                    GateOutcome::Promoted(vt) => {
                        proposals.push(ProposalRecord {
                            suggester: s.name().into(), index: i,
                            outcome: ProposalOutcome::Accepted {
                                cost_before: best_cost, cost_after: cand_cost,
                            },
                        });
                        best_vt = vt;
                        best_cost = cand_cost;
                    }
                    GateOutcome::Refuted(w) => {
                        proposals.push(ProposalRecord {
                            suggester: s.name().into(), index: i,
                            outcome: ProposalOutcome::Refuted(w),
                        });
                    }
                }
            }
        }

        let emitted = cli::emit::emit_rust(
            best_vt.term(), fn_name, Some(best_vt.certificate()));
        OptimizeReport { emitted, original_cost, final_cost: best_cost, proposals }
    }

    /// Print a term/certificate through any `Emitter` (roadmap Phase 1) —
    /// use in place of the `emitted` field on `GateReport`/`OptimizeReport`
    /// (which are always `RustEmitter`) to target another language.
    /// Emission runs strictly after promotion, so this never touches
    /// what was certified, only how it's printed.
    pub fn emit_with(&self, term: &Term, cert: Option<&harness::Certificate>,
        emitter: &dyn Emitter) -> String
    {
        emitter.emit(term, cert)
    }
}

// ---------------------------------------------------------------- tests --

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// SOUNDNESS PIN: a malicious door's term is refuted with a
    /// counterexample — registration grants zero authority.
    #[test]
    fn malicious_door_is_refuted_not_believed() {
        struct LyingDoor;
        impl FrontDoor for LyingDoor {
            fn name(&self) -> &str { "liar" }
            fn extract(&self, _r: &ExtractRequest) -> Result<Term, Refusal> {
                // claims f(x) = x + 1 for whatever it was asked about
                sexpr::parse("(+ (var 0) 1.0)").map_err(|e| Refusal(format!("{e:?}")))
            }
        }
        let mut e = Engine::bare(0xBAD);
        e.register_door(Arc::new(SynDoor));
        e.register_door(Arc::new(LyingDoor));
        let req = ExtractRequest {
            source: "pub fn twice(x: f64) -> f64 { x * 2.0 }",
            fn_name: "twice",
        };
        let (outs, report) = e.certify(&req);
        assert_eq!(outs.len(), 2);
        assert!(outs.iter().all(|(_, r)| r.is_ok()));
        match report {
            Some(GateReport::Refuted(w)) => {
                // the witness pins the disagreement in evaluable data
                assert!(w.candidate_val != w.reference_val
                        || w.candidate_val.is_nan() != w.reference_val.is_nan());
            }
            other => panic!("the liar must be refuted: {other:?}"),
        }
    }

    /// A well-behaved custom door integrates end-to-end: extract → gate →
    /// emitted certified code, all through the SDK surface.
    #[test]
    fn custom_door_certifies_through_the_gate() {
        struct ConstDoor;
        impl FrontDoor for ConstDoor {
            fn name(&self) -> &str { "const-door" }
            fn extract(&self, r: &ExtractRequest) -> Result<Term, Refusal> {
                if r.fn_name != "tau" {
                    return Err(Refusal("const-door only knows `tau`".into()));
                }
                sexpr::parse("(* 2.0 3.141592653589793)")
                    .map_err(|e| Refusal(format!("{e:?}")))
            }
        }
        let mut e = Engine::bare(0x7A0);
        e.register_door(Arc::new(SynDoor));
        e.register_door(Arc::new(ConstDoor));
        let req = ExtractRequest {
            source: "pub fn tau() -> f64 { 2.0 * 3.141592653589793 }",
            fn_name: "tau",
        };
        let (_, report) = e.certify(&req);
        match report {
            Some(GateReport::Promoted { n, emitted, .. }) => {
                assert_eq!(n, 10_000);
                assert!(emitted.contains("fn tau"));
            }
            other => panic!("agreeing doors must promote: {other:?}"),
        }
    }

    /// Observers see every event and can veto none.
    #[test]
    fn observers_are_taps_not_gates() {
        #[derive(Default)]
        struct Tape(Mutex<Vec<String>>);
        impl Observer for Tape {
            fn on_extract(&self, door: &str, f: &str,
                          out: &Result<Term, Refusal>) {
                self.0.lock().unwrap().push(format!(
                    "extract {door}/{f}/{}", if out.is_ok() {"ok"} else {"refused"}));
            }
            fn on_gate(&self, f: &str, r: &GateReport) {
                self.0.lock().unwrap().push(format!(
                    "gate {f}/{}", matches!(r, GateReport::Promoted{..})));
            }
        }
        let tape = Arc::new(Tape::default());
        let mut e = Engine::new(0x0B5);
        e.register_observer(tape.clone());
        let req = ExtractRequest {
            source: "#[no_mangle]\npub fn half(x: f64) -> f64 { x * 0.5 }",
            fn_name: "half",
        };
        let (_, report) = e.certify(&req);
        assert!(matches!(report, Some(GateReport::Promoted { .. })));
        let t = tape.0.lock().unwrap();
        assert!(t.iter().any(|l| l.starts_with("extract syn/half/ok")));
        assert!(t.iter().any(|l| l.starts_with("extract ir/half/ok")));
        assert!(t.iter().any(|l| l.starts_with("gate half/true")));
    }

    /// Refusals classify with the trial's own histogram buckets.
    #[test]
    fn refusals_carry_trial_buckets() {
        let e = Engine::new(1);
        let req = ExtractRequest {
            source: "pub fn s(t: f32) -> f32 { t.sin() }", fn_name: "s",
        };
        let out = e.extract("syn", &req);
        let r = out.err().expect("f32 sin must refuse");
        assert!(r.0.contains("innocuous"));
    }
}
