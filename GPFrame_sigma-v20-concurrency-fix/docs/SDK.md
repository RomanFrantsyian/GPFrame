# DGE SDK — the in-process integration contract

Audience: developers adding features to DGE **without reading core
code**. This file plus `docs/API.md` (the HTTP surface) is the whole
contract. If your task seems to require peeking into `crates/term`,
`crates/harness`, or the doors — that's an SDK gap: file it.

This document covers what's usable **today**. If you're weighing a
bigger idea — a new output target, a new sampling strategy, anything
touching state or cross-function interaction — see
[SDK-ROADMAP.md](SDK-ROADMAP.md), especially its §9 decision framework,
before concluding something isn't possible.

Crate: `sdk` (`crates/sdk`). One dependency line integrates you:

```toml
[dependencies]
sdk = { path = "…/deductive-gp-engine/crates/sdk" }
```

## 1. The one rule that shapes the whole surface

DGE's soundness is a typestate spine:

```
Term  --Gate::promote-->  VerifiedTerm  --jit::install-->  JitFn
```

`VerifiedTerm` is privately constructed inside the core. **Nothing you
can write — no door, no hook, no server call — mints one.** So every
extension point sits on the *untrusted* side of the spine:

* your code produces **candidates** (`Term`s) and **observes** events;
* the core's **Gate** arbitrates: n = 10⁴ μ′ samples, bit-for-bit
  (NaN ≡ NaN), sequence lengths including L = 0;
* a wrong candidate costs you nothing but a refutation — the Gate
  returns a ⊏-minimal counterexample you can replay.

Consequence you should lean on: your extension can be heuristic,
experimental, even model-generated. It cannot corrupt a certificate.

## 2. Terms without internals: the s-expression surface

You never need `Term`'s memory layout. Terms parse from and print to
s-expressions (`sdk::sexpr::parse` / `sdk::sexpr::print`):

```
(+ (var 0) 1.0)                     x₀ + 1
(fold 0.0 (+ acc (elem 0)))         Σ over sequence 0
(rnd32 (* (var 0) (var 0)))         f32-rounded square (Σ v1.6)
(select (lt (var 0) 0.0) (neg (var 0)) (var 0))    |x| by hand
```

Grammar: `expr := f64-literal | (op expr…) | (var N) | (elem N) |
(len N) | acc`. Op names and arities are enumerable at runtime
(`GET /v1/alphabet` on the server, or `sdk::Op::from_name`); the Σ v1.6
alphabet is: `neg abs sqrt floor ceil sin cos tan exp exp2 ln rnd32`
(unary), `+ - * / min max pow lt gt le ge eq ne fold` (binary),
`fma select` (ternary), `var/const/acc/elem/len` (leaves).
Semantics notes you may rely on:

* everything is f64; comparisons are 1.0/0.0-valued with IEEE NaN rules
  (`lt` etc. false on NaN, `ne` true on NaN);
* `fold` iterates the runtime sequences' shared length; L = 0 yields the
  init; `acc`/`elem` are only valid inside a fold body;
* `rnd32` is `(x as f32) as f64` — the f32-semantics symbol.
* `(ext:<name> a)` / `(ext:<name> a b)` reference an extension op by
  NAME — see §6. This is plain, registry-free syntax: it parses and
  prints without the plugin present; only GATING a term that uses it
  requires the name to be registered.

## 3. Plugging in a front door

A *front door* translates source text into candidate terms. Implement
one trait:

```rust
use sdk::{Engine, ExtractRequest, FrontDoor, Refusal, Term};
use std::sync::Arc;

struct MyDoor;
impl FrontDoor for MyDoor {
    fn name(&self) -> &str { "my-door" }
    fn extract(&self, req: &ExtractRequest) -> Result<Term, Refusal> {
        // parse req.source however you like; return a Term via
        // sdk::sexpr::parse, or refuse with an honest reason:
        Err(Refusal(format!("my-door does not read `{}` yet", req.fn_name)))
    }
}

let mut e = Engine::new(seed);          // syn + ir doors pre-registered
e.register_door(Arc::new(MyDoor));
let (per_door, report) = e.certify(&ExtractRequest {
    source: SRC, fn_name: "f",
});
```

`certify` runs every door, cross-gates all admissions against each
other, and returns `GateReport::Promoted { n, alpha, emitted }` (the
`emitted` string is compilable Rust carrying the certificate comment) or
`GateReport::Refuted(counterexample)`.

Refusal etiquette (the project takes this seriously): refuse with a
message that names the exact unsupported construct and, where known,
whether it's roadmap or out-of-scope. `Refusal::class()` buckets your
message with the same classifier the field-trial histograms use, so
well-worded refusals become roadmap pricing data automatically.

## 4. Hooks (observers)

Hooks are **taps, not gates** — they see events, they veto nothing:

```rust
use sdk::{GateReport, Observer, Refusal, Term};

struct Audit;
impl Observer for Audit {
    fn on_extract(&self, door: &str, f: &str, out: &Result<Term, Refusal>) {
        eprintln!("{door}/{f}: {}", if out.is_ok() { "admit" } else { "refuse" });
    }
    fn on_gate(&self, f: &str, r: &GateReport) { /* metrics, logs, … */ }
}
e.register_observer(Arc::new(Audit));
```

Defined events, in order per `certify` call: `on_extract` once per
registered door, then `on_gate` once per gate run (cross-door runs
included). There are no mutation hooks and none are planned: a hook that
could edit a term between extraction and gating would be a hole in the
spine.

## 5. What the SDK will not give you

* A `VerifiedTerm` value, a way to skip the gate, or a gate with a
  weakened metric. (Emitted code + certificate text is the deliverable.)
* JIT installation (requires holding verified state; use the core CLI's
  pipeline for that path).
* A guarantee that malformed plugin input can NEVER reach a kernel
  panic — this is the goal (§1's "zero authority" argument depends on
  it) and holds for every class discovered so far (unregistered ext
  ops, arity mismatches — the latter found by fuzzing, fixed same day,
  see `docs/HISTORY.md` Addendum 11), but the honest status is "every
  known class is covered," not "proven exhaustive." File anything that
  panics instead of refusing; it's a bug, not a documented limitation.
* Stateful or effectful semantics (mutation, multi-output, memory across
  calls) — §6 covers pure per-call extension ops only; see §6.4.
* Tier A (SMT-proved) certificates for anything you supply — §6's
  extension ops and §7's suggester-sourced rewrites are always Tier B
  (statistical). SMT discharge is a kernel-only path.
* Stability of *internal* crates: `term`'s node layout, the doors'
  internals, and gate internals may change without notice. The stable
  surface is exactly: this trait set, the sexpr grammar, the alphabet
  (additive-only between minor versions), and `docs/API.md`.

## 6. Extension operators — new Σ SEMANTICS, no kernel PR

Front doors (§3) plug in new *readings of source code*. This section
plugs in something deeper: new *meanings inside the term language
itself* — `Op::Ext1` / `Op::Ext2`, resolved by name at gate/eval time
through a runtime registry. This is how you add real functionality
(a custom nonlinearity, a domain-specific transform, a lookup-table
op) without a kernel commit.

### 6.1 Why this is safe

The Gate never needed to understand semantics — it needs to catch
lies. Arbitration is black-box: sample inputs, run both sides, compare
bits. That doesn't care whether "both sides" are built entirely from
core Σ ops or reference a plugin closure by name. So:

* a **wrong** ext op is refuted exactly like a wrong `FrontDoor` — bit
  disagreement, ⊏-minimal counterexample, no special case;
* a **nondeterministic** ext op is refuted by the Gate's own
  determinism pre-gate, which double-runs every sample on any term
  touching an ext op (run 1 vs run 2 becomes the counterexample) —
  hidden state or a badly-seeded RNG inside your op cannot silently
  weaken a claim, it fails loudly instead;
* a claim that depended on your op **says so, forever**: the
  certificate carries `ext_semantics: ["yourop@1.0#<fingerprint>"]`
  and the emitted comment reads `MODULO extension semantics: …` instead
  of an unqualified `CERTIFIED:`.

### 6.2 Registering an op

```rust
use sdk::register_ext_op;

// name, version, fingerprint (YOUR semantic-identity claim — a spec ID
// or a hash of your source; it becomes part of every certificate that
// uses this op), arity (1 or 2), and the closure itself.
register_ext_op(
    "relu", "1.0", "spec:max(x,0)", 1,
    |args| args[0].max(0.0),
)?;
```

Idempotent re-registration (same name+version+fingerprint+arity) is a
no-op; a *conflicting* re-registration (same name, different semantics)
is refused — one name claiming two meanings is exactly the ambiguity
certificates exist to prevent.

Reference the op from any term you build, via the sexpr surface (§2) or
the builder:

```lisp
(ext:relu (var 0))
(* 0.5 (ext:gauss (+ (var 0) (var 1))))     ; composes with core Σ freely
(fold 0.0 (+ acc (ext:halfsq (elem 0))))    ; legal inside fold bodies
```

Then gate it exactly like any other term:

```rust
let candidate = sdk::sexpr::parse("(ext:relu (var 0))")?;
// NOTE: the reference must match NaN handling exactly — Rust's
// `f64::max` returns the NON-NaN argument when one side is NaN, so a
// hand-rolled `select(lt(x,0), 0, x)` (which PROPAGATES NaN, since `lt`
// is false on NaN) is a different function, and the gate will correctly
// refute it against a `.max(0.0)`-based op. `(max (var 0) 0.0)` mirrors
// `.max()` exactly, including at NaN — this is the gate doing its job on
// the very first example in this document.
let reference = sdk::sexpr::parse("(max (var 0) 0.0)")?;
match engine.gate("relu_like", candidate, &reference) {
    sdk::GateReport::Promoted { emitted, .. } => { /* emitted contains
        the MODULO line and a call to your `relu` symbol */ }
    sdk::GateReport::Refuted(witness) => { /* your op disagreed with
        the reference at witness.minimal_env — a real bug */ }
    sdk::GateReport::Refused(msg) => { /* an op in the term wasn't
        registered when the gate ran — register it and retry */ }
}
```

### 6.3 What you get, and what you don't

| Consumer | Ext-op behavior |
|---|---|
| Interpreter, cross-door gates | full support — this is the reference path |
| JIT (O7) | full support — a Cranelift trampoline calls your SAME closure; still O7-differentialed against the interpreter |
| Rewriting (Tier A rules) | **skipped, not attempted** — no rule has a proof of your op's algebra; the term still gates (Tier B, identity-checked) but doesn't get optimized around your op |
| Emission | prints a call to your Rust symbol by NAME — the emitted file only compiles if you also ship a crate providing `fn <name>(f64...) -> f64` with matching semantics |
| Certificate tier | **always Tier B** — statistical equivalence over μ′, never an SMT proof, because SMT has no theory for semantics it hasn't seen |

Cost model, stated plainly: you trade Tier A eligibility and JIT-of-your-op
optimization potential for zero kernel changes. For most integrations —
one nonlinearity, one domain transform — that's a good trade; if your op
turns out to be algebraically simple and broadly useful, graduating it
into a real Σ op (`term/src/sig.rs` et al. — see the main README's
"add-a-Σ-op" checklist) is a follow-up, not a prerequisite.

### 6.4 The boundary: this is NOT effects

An ext op is `&[f64] -> f64` — pure, and evaluated fresh per call. It
cannot: retain state across calls, mutate anything outside its return
value, or produce more than one output. An op that tries to hide state
(a counter, an RNG without a fixed seed, a cache) will usually get
caught by the determinism pre-gate (§6.1) — but design purely on
purpose; don't rely on the pre-gate as your only safety net.

True stateful/effectful integration (P3 in the project's own
vocabulary) is **not available through this mechanism** and is not
currently available through the SDK at all — it needs a different,
larger extension (a "world gate" generalizing the judged object from
`env → f64` to `(World, env) → (World′, outputs)`) that is sketched on
the roadmap but not implemented. If your integration needs mutation,
multiple outputs, or cross-call memory, ext ops are the wrong tool;
ask before building around this limitation, since the right answer may
be "this specific case is fine as an ext op because the state is really
just an extra parameter" rather than "you need the world gate."

## 7. Suggesters — new OPTIMIZATION hypotheses, no kernel PR

Ext ops (§6) add new *semantics*. This section adds new *optimization
knowledge* — domain-specific rewrite hypotheses the kernel's rule table
and Z3 discharge don't know, without a kernel PR and without ever
trusting your math.

### 7.1 Why this is safe (same argument, one more layer)

A `Suggester` proposes; it never decides. Every candidate you return is
re-gated by the core Gate — same bitwise, 10⁴-sample, ⊏-minimal-witness
arbitration as everything else in this document. The one property worth
being explicit about, because it's the one a naive implementation gets
wrong: **every candidate is gated against the ORIGINAL term you were
asked to optimize, never against the current "best."** This means a
sequence of individually-plausible-looking suggestions can never chain
into semantic drift — suggester 2 cannot "build on" suggester 1's
mistake, because suggester 1's mistake was never adopted in the first
place if it disagreed with the original.

### 7.2 Writing one

```rust
use sdk::{Engine, Suggester};
use std::sync::Arc;

struct MySuggester;
impl Suggester for MySuggester {
    fn name(&self) -> &str { "sdf-smoothing" }
    fn suggest(&self, t: &sdk::Term) -> Vec<sdk::Term> {
        // Inspect `t` however you like — pattern match its sexpr,
        // run your own analysis, whatever. Return candidate REWRITES;
        // wrong or no-op candidates are fine, they just won't be
        // adopted (or will cost you a wasted gate run — see §7.3).
        vec![/* candidate Terms, e.g. via sdk::sexpr::parse */]
    }
}

let mut e = Engine::bare(seed);
e.register_suggester(Arc::new(MySuggester));
let report = e.optimize("my_fn", &original_term);
println!("{}", report.emitted);       // always certified — the
                                       // unchanged original if nothing won
for p in &report.proposals {          // full audit trail, including
    println!("{p:?}");                // every rejection and why
}
```

### 7.3 The cost gate runs BEFORE the correctness gate

`Engine::optimize` checks your candidate's cost (`rules::cost::DefaultCost`
— node count, transcendentals weighted ×32, unrelated to correctness:
L2 "cost irrelevance" means swapping cost functions changes which
*equally-certified* term wins, never whether it's certified) before
spending a 10⁴-sample gate run on it. A candidate that isn't strictly
cheaper than the current best is rejected immediately
(`ProposalOutcome::RefusedNotCheaper`) — this is a performance
courtesy, not a correctness mechanism, so don't rely on it to filter out
wrong candidates; a wrong-but-cheaper candidate WILL reach the gate and
WILL be refuted there (`ProposalOutcome::Refuted`, with the same
⊏-minimal counterexample shape as everywhere else).

### 7.4 What you get, and what you don't

| | |
|---|---|
| Tier | **Always Tier B.** Suggester-sourced acceptances are statistical equivalence over μ′, never an SMT proof — Z3 discharge (Tier A) stays a kernel-only path (`dge discharge`, the Dec rule table). If your rewrite is provably-general algebra, the graduation path is the same as for ext ops: propose it as a real Dec rule with a Z3 proof, not as a permanent suggester. |
| Failure mode on a bad suggester | Wasted cost/gate-run cycles, nothing worse — a suggester that always proposes garbage just never gets anything adopted. |
| Determinism requirement | None beyond what the Gate already assumes — `suggest()` can be arbitrarily expensive or even nondeterministic in WHAT it proposes; only the semantics of each proposed *term* has to be well-defined (the term itself is evaluated deterministically by the interpreter, same as any other term). |

## 7a. Output & cost openness (v1.9, roadmap Phase 1)

Two small, independent additions — no new trust boundary, since both
run strictly after (emission) or alongside-but-never-instead-of
(cost) promotion:

```rust
// target any language, not just Rust — emission can never affect
// what got certified, only how it's printed
trait Emitter { fn emit(&self, term: &sdk::Term, cert: Option<&harness::Certificate>) -> String; }
let glsl = MyGlslEmitter;
let code = engine.emit_with(&verified_term_data, Some(&cert), &glsl);

// weight optimization by your own priorities, not generic node count
// (safe by L2 "cost irrelevance": cost only picks a representative
// among already-certified-equal terms, never affects whether something
// IS certified)
let report = engine.optimize_with_cost("f", &original, &my_cost_fn);
```

`sdk::RustEmitter` is the default every `GateReport`/`OptimizeReport`
uses internally; `engine.emit_with(term, cert, &RustEmitter)` reproduces
that output exactly if you want to confirm parity before switching
targets.

## 8. Process isolation

If you'd rather not link Rust at all, run `dge-serve` and integrate over
HTTP from any language — same operations, same arbitration, contract in
`docs/API.md`. The SDK and the server expose the *same* Engine; choose
in-process for latency, out-of-process for isolation and polyglot teams.
