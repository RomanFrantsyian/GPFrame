# DGE engineering log (append-only archive)

> Merged 2026-07-18 from the four historical documents (verbatim, in
> order: DEEP-DIVE, STATUS, FIELD-TRIAL, RFC-REVIEW-ir-lifting-v3).
> Normative engineering documentation now lives in the top-level
> [README](../README.md); external contracts in [SDK.md](SDK.md) and
> [API.md](API.md). This file is the project's memory: phase logs,
> field-trial addenda, and review verdicts. The refusal-message
> vocabulary quoted here is LOAD-BEARING — messages in code echo these
> strings, and `grep` on either side finds the other.

---
# Part I — Deep dive (architecture narrative)

# Deep dive: architecture, findings, and the Σ signature

> This is the full technical narrative behind the project. For the
> product overview, quickstart, and CI/CD, see the top-level
> [README](../README.md).


**A refactoring, testing, debugging, and JIT engine for pure numeric code — where nothing ships without a certificate.**

The engine extracts pure `f64` functions from Rust source into a small verified term language (Term_p), then rewrites, mutation-tests, fault-localizes, evolves, and JIT-compiles them. Every stage has exactly one exit door, and that door is typed: a result either carries a **Tier A certificate** (per-rule SMT proofs / reviewed algebra) or a **Tier B certificate** (statistical equivalence quantified as *n*, *α*, *δ_min* over a recorded input distribution). There is no third state. "Probably fine" is not a value in this system.

```
Rust fn ──extract──▶ Term ──Gate::promote──▶ VerifiedTerm ──install (O7)──▶ JitFn
            ▲              (the ONLY constructor)        (the ONLY constructor)
            └── extraction is itself untrusted: gated bitwise against the
                rustc-compiled original before anything downstream sees it
```

`VerifiedTerm` and `JitFn` have no public constructors. The type system *is* the audit trail.

---

## Showcase: what happens to a naive polynomial

Input (real user-submitted code):

```rust
fn inefficient_polynomial(x: f64) -> f64 {
    let term3 = 3.0 * x * x * x;
    let term2 = 5.0 * x * x;
    let term1 = 2.0 * x;
    let term0 = 7.0;
    term3 + term2 + term1 + term0
}
```

```console
$ dge extract poly.rs inefficient_polynomial --out poly.sexpr
$ dge discharge                       # prove every rewrite rule via Z3 first
$ dge refactor poly.sexpr             # Tier A: bitwise-sound rules only
cost  : 19 -> 19                      # correct: nothing bitwise-sound may
                                      # reassociate f64 addition — so it won't

$ dge refactor poly.sexpr --eps       # admit ~ε rules, Tier B gate mandatory
REFUTED at x ≈ -1.09e154              # Horner is NOT ε-equivalent over all
                                      # of f64: naive → -inf+inf = NaN,
                                      # factored → -inf. Witness attached.

$ dge refactor poly.sexpr --eps --domain 1e100
output: (fma (var 0) (fma (var 0) (fma 3.0 (var 0) 5.0) 2.0) 7.0)
cost  : 19 -> 10                      # Horner found via factoring + fma
claim : equivalent at confidence 0.999 over mu' ... DOMAIN |x|<=1.0e100 (A-1);
        defect regions of measure < 6.9e-4 are invisible (n=10000)
```

And the whole loop in one command — **Rust in, certified Rust out**:

```console
$ dge pipeline poly.rs inefficient_polynomial --eps --domain 1e100
[1/4] extracted `inefficient_polynomial` (14 nodes, arity 1)
[2/4] refactored: cost 19 -> 10 via [… mul-factor~, fma-contract~ …]
[3/4] emitted `inefficient_polynomial_dge`
[4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)

/// CERTIFIED: equivalent at confidence 0.999 over mu' … DOMAIN |x|<=1.0e100 (A-1);
///            defect regions of measure < 6.9e-4 are invisible (n=10000)
/// rules applied: add-comm, mul-comm, add-assoc~, mul-factor-r~, fma-contract~, …
/// env: x86_64 fma=true avx=true libm=behavioral:da9da77b07139d63
pub fn inefficient_polynomial_dge(v0: f64) -> f64 {
    v0.mul_add(v0.mul_add(3.0_f64.mul_add(v0, 5.0_f64), 2.0_f64), 7.0_f64)
}
```

Three behaviors worth noticing: the engine **refuses** to optimize what it cannot prove, **refutes with a concrete witness** instead of silently drifting, and when it does accept, the certificate states **exactly where the claim holds** — and travels with the emitted code as a doc comment. Emission is itself a lowering and therefore untrusted: it passes its own gate (re-extraction round trip over μ′ plus a rustc compile-and-run differential) before anything is printed. Hand-editing the output voids the certificate, and the header says so.

(Epilogue, also pinned as a test: the stopwatch then showed Horner is ~1.0× vs naive on a modern CPU — fewer ops, but a serial dependency chain vs the naive form's instruction-level parallelism. The cost model's blindness to latency is documented as Finding 6 with an identified fix, not hidden.)

## The second front door: LLVM IR lifting (v3-Exp P1)

`dge lift <file.ll|file.rs> <fn_name>` reads what the CPU is told instead of
what the human wrote. Syntax is a costume; the compiler's lowering to SSA
removes every costume, and the lifter recovers the math from the dataflow:

```console
$ dge lift kernel.rs iter_dot3        # .iter().zip().map(..).sum() —
                                      # syntax the syn extractor REFUSES
lifted `iter_dot3` from IR: 11 nodes, arity 6   [UNTRUSTED — run the extraction gate]
(+ (+ (* (var 0) (var 3)) (* (var 1) (var 4))) (* (var 2) (var 5)))
```

The loop was never a loop: at `-O1` the fixed-window iterator chain is
straight-line fmul/fadd, and P1 lifts exactly that. The lifter is UNTRUSTED
(a lowering in reverse, L1) — every lifted term must pass the extraction
gate (BitwiseNanClass vs the compiled original over 10⁴ μ′ samples), which
is how all six pinned P1 gates hold. P1 scope is straight-line pure f64:
`br`/`phi` refuse with the P2 roadmap (CFG+phi → Fold), memory ops with the
P3 roadmap, calls outside a closed 17-symbol libm map and fast-math flags
refuse on claim discipline. One lifter covers Rust and C/C++ (`rustc
--emit=llvm-ir` / `clang -O1 -emit-llvm -S -ffp-contract=off` meet in the
same text). Details + measured findings: `docs/RFC-REVIEW-ir-lifting-v3.md`.

**P2 (loops-as-math) is live**: the canonical counted loop lifts to Σ Fold —
a loop is not "unsupported syntax"; at instruction level it is what it
always was, a recurrence:

```console
$ dge lift ema.rs ema                 # runtime-length slice + hoisted invariant
lifted `ema` from IR: 10 nodes, arity 1   [UNTRUSTED — run the extraction gate]
(fold 0.0 (+ (* (- 1.0 (var 0)) acc) (* (var 0) (elem 0))))
```

Recognition targets the measured canonical shape (entry → preheader? →
loop → LCSSA-tail? → merge), is positive-only, and never interprets the
integer index machinery — μ′ samples random lengths including 0 and the
gate arbitrates the whole reading. Data-bound `while` loops,
multi-accumulator folds, and index-dependent bodies refuse with their
roadmap names.

And the whole loop closes: `dge pipeline` falls back syn → IR automatically
when the syn door refuses, so iterator-chain source goes in and certified
Rust comes out — with the emission round-trip re-read through the SYN door,
the two doors certifying each other on every output:

```console
$ dge pipeline kernel.rs iter_dot3
      syn door refused (Unsupported("expression form …")); trying the IR door
[1/4] lifted (LLVM IR) `iter_dot3` (11 nodes, arity 6, 0 seqs)
[2/4] refactored: cost 11 -> 11 via [add-comm, mul-comm]
[3/4] emitted `iter_dot3_dge`
[4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)
/// CERTIFIED: PROVED semantics-preserving; 2 rule(s) [0 SMT artifact(s)]
pub fn iter_dot3_dge(v0: f64, v1: f64, v2: f64, v3: f64, v4: f64, v5: f64) -> f64 {
    (((v0 * v3) + (v1 * v4)) + (v2 * v5))
}
```

## Imperative kernels (extractor v2 + Σ v1.1)

Loops, accumulators, arrays, and conditional updates are pure computations
wearing imperative clothing, and the extractor now treats them that way:

| Rust pattern | Translation | Validated on |
|---|---|---|
| `[f64; N]` / `&[f64; N]` params | N var slots, compile-time indexing | dot product |
| `let mut acc` + `=` / `+= -= *= /=` | SSA rebinding (O5 intact — nothing escapes) | dot, EMA |
| `for i in LO..HI` / `..=` (literal bounds) | unrolling, cap 1024; loop var usable as index | dot, EMA, Newton |
| statement `if` with assignments | phi-merge: diverging bindings become `select` | clamped sum |
| `< > <= >=` | **first-class Σ ops** (v1.1): exact Rust semantics — false on NaN, ±0 equal; SMT-decidable (`fp.lt`), lowered as ordered `fcmp` | clamp branch |

All four kernels (dot4, EMA-8, Newton-invsqrt, clamped running sum) pass the
extraction gate against their rustc-compiled originals over 10⁴ μ′ samples.

## Dynamic-length data: the fold operator (Σ v1.2)

`fold(init, body)` iterates over K parallel same-length runtime sequences;
inside the body, `acc` is the accumulator and `elem k` the current element.
Body nodes are OWNED by their fold (validator: binders can never escape;
loop-invariant shared values are hoisted — the semantics, not an
optimization). Term_p stays total: iteration count is the runtime length.

* **Extractor**: `&[f64]` params are sequences; `for i in 0..s.len()` with a
  single scalar accumulator becomes a fold — including conditional updates
  (`if s > cap { s = cap }`) inside the loop. Refused with "roadmap" in the
  message: iterator adapters, offset/windowed indexing, non-zero starts,
  multi-accumulator loops.
* **Gate**: μ′ extends with a sequence measure (lengths {0,1,2} boundary ∪
  uniform[3,32], recorded in certificates); the shrinker minimizes sequence
  length first — wrong candidates get witnesses of length ≤ 2.
* **JIT**: real cranelift loop codegen (block params for acc/index, hoisted
  base pointers). **The ≥5× perf target is now a measured PASS: 9.3× on a
  len-4096 dot product** — loops are where compiled code beats a tree-walker,
  exactly as hypothesized.
* **Rules/SMT**: folds pass through the e-graph opaquely; SMT refuses them
  with `unbounded data — Tier B only (T2)`. Memo refuses fold terms
  (sequence-aware keys: roadmap).
* Validated end-to-end: dynamic dot, scaled norm², clamped running sum —
  extraction gate vs rustc originals, O7, emission round trip; `dge
  pipeline` emits `pub fn dot_dge(s0: &[f64], s1: &[f64]) -> f64` with the
  fold as a plain `for` loop and the certificate attached.

**What this does NOT unlock — by frozen design (§8):** effects, concurrency,
dynamic-length data, and *architecture itself* stay outside the perimeter.
No certificate can cover "this pipeline redesign is correct." The engine's
role in production architecture is indirect and honest: it certifies the
numeric kernels inside your pipeline, and `dge audit` tells you how much of
your codebase that covers.

## The engine caught real bugs — including its own

Every one of these was discovered *by the machinery* and is pinned as a permanent test:

| # | Finding | Caught by |
|---|---------|-----------|
| 1 | `x + 0.0 → x` is **unsound** over f64 (witness: `−0.0`); `x − 0.0 → x` is provable | O1 SMT discharge (SAT vs UNSAT) |
| 2 | Float64 commutativity is a >12 s Z3 bit-blasting timeout despite being trivially true — routed to the reviewed-proof tier | discharge measurement |
| 3 | The spec's "fma contraction is ≤1 ULP" claim is **false** under catastrophic cancellation (`a·b ≈ −c`) | Tier B gate refutation |
| 4 | Cranelift's `fmin` propagates NaN; Rust's `f64::min` returns the other operand — a real compiler-semantics mismatch | O7 differential gate (NaN witness, shrunk) |
| 5 | The counterexample shrinker had a `u64` overflow in its rank function | the fmin test above |
| 6 | Horner form trades ILP for op count; a Σ-of-weights cost model cannot see it | release-mode measurement |
| 7 | **NaN payloads are not portable observables**: LLVM const-fold gives `0x7ff8…` for `−inf + inf` where x86 hardware gives `0xfff8…`, and operand canonicalization flips which payload survives — so cross-generator "bitwise" is unachievable in principle; the strongest honest claim is bitwise-modulo-NaN-class (`Metric::BitwiseNanClass`, ±0 signs still exact), now the extraction-gate and O7 default | dot-product extraction gate |
| 3b | Finding 3 reconfirmed on a production kernel: fma-fusing an EMA filter is unsafe under cancellation *regardless of domain bounds* (cancellation is scale-free) — the ε-gate refuted it with a witness | Tier B gate, EMA-8 |

A verification system that has never refuted anything is decoration. This one has refuted its own spec, its own shrinker, its own optimizer, and its compiler backend.

## What each crate does

```
                         ┌─────┐
                         │ cli │  T8 composition: audit / extract / discharge /
                         └──▲──┘  refactor / gentest / debug / calib
              ┌──────┐      │      ┌─────┐
              │ memo │──────┼──────│ jit │  O7 door: cranelift, differential-gated
              └──▲───┘      │      └──▲──┘  against the interpreter per install
   ┌───────┬────┼───────┬───┴───┐     │
┌──┴──┐ ┌──┴───┐ ┌──────┴┐ ┌────┴┐    │
│rules│ │mutate│ │locate │ │ gp  │    │
└──▲──┘ └──▲───┘ └───▲───┘ └──▲──┘    │
   │  e-graph │ MS+SMT │ Ochiai │ SR+repair
   └──────────┴───┬────┴────────┴──────┘
             ┌────┴────┐
             │ harness │  THE JUDGE: μ' sampler, metrics, T3 shrinker,
             └────▲────┘  Gate (sole VerifiedTerm constructor), certificates
             ┌────┴────┐
             │  term   │  trusted base: Σ (21 ops), arena, definitional
             └─────────┘  interpreter (one screen), structural hash
```

The dependency DAG is enforced by Cargo: `term` depends on nothing; `harness` never imports the crates it judges. The trusted base is the one-screen interpreter (plus Z3, only for offline rule proofs). Everything else — egg, cranelift, the GP search, the extractor itself — is refutable and refuted-against-the-interpreter.

| Crate | Role |
|---|---|
| `term` | Σ signature, arena AST (topological invariant), definitional interpreter + coverage-traced variant, s-expr I/O, structural surgery for GP |
| `harness` | boundary-mixture sampler μ′ (seeded, certificate-recorded, A-1 domain bounds), abs/rel/ULP metrics with explicit NaN/±0 policy, well-founded shrinking, the Gate, certificates + behavioral env fingerprint (runtime CPU features + libm output hash) |
| `rules` | e-graph rewriting over egg; rule tiers R_dec (Z3-proved, artifact on disk — **no rule enters the active set unproved**), R_sem (reviewed IEEE-754 proofs in source), R_approx (ε-only, Tier B mandatory); bounded saturation, calibratable cost extraction |
| `mutate` | first-order mutant enumeration, mutation score with the equivalent-mutant SMT filter and triage queue, regression-suite emitters |
| `locate` | execution spectra over traced eval (Select-pruned coverage), Ochiai ranking — shipped as *aid, never verdict* |
| `gp` | symbolic regression (elitism + p_mut > 0 asserted at runtime), Nelder-Mead constant refinement, two-tier repair — exit only via gate, honest null on budget miss |
| `memo` | pure-function cache: allocation-free hit path, full-key authority compare, lazy-LRU under a byte cap — eviction is trivially sound |
| `jit` | cranelift lowering (semantically delicate ops via wrappers over the interpreter's own methods), the O7 install door, hot dispatch that pins to interp forever on any O7 failure |
| `cli` | the `dge` binary + the pre-build **audit** (syn-based classifier measuring what fraction of a codebase is extractable — run this first; it will tell you honestly if this engine has no workload for you, including on its own source, which scores `DO NOT BUILD`) |

## Quickstart

Requirements: Rust stable, and `z3` on PATH for rule discharge (everything else degrades gracefully to "Unknown → triage").

```console
$ cargo test --workspace        # 86 tests
$ make calib                    # per-op cost table + §6 perf report (release)
$ dge audit  path/to/your/src   # is there a workload? (the §9 go/no-go gate)
$ dge pipeline file.rs fn_name [--eps [--domain MAG]]   # the whole loop
$ dge extract file.rs fn_name --out fn.sexpr
$ dge discharge                 # Z3-prove the rewrite rules, artifacts to disk
$ dge refactor fn.sexpr [--eps [--domain MAG]]
$ dge gentest  fn.sexpr         # mutation-adequate golden suite + (MS, n, α, δ)
$ dge debug    broken.sexpr oracle.sexpr --repair
```

## Honest status

Validated end-to-end on real published code (the `easer` crate's cubic easing family and the polynomial above): `rustc(original) ==bitwise== interp(extracted) ==bitwise== cranelift-jit(refactored)` across 10⁴ μ′ samples each, NaN/±0/Inf/subnormal boundaries included.

Not a finished product. Known limits, by design and by measurement:

- **Scope is the perimeter, not a slice of it**: pure, total, numeric functions only. Effects, loops/iterators, concurrency, and architecture are *outside* — the audit tool exists precisely to measure whether your codebase has enough inside the perimeter to care.
- Extraction covers arithmetic, Σ math methods, let-shadowing, `if/else`, fixed-size arrays, literal-bound loops, mutable accumulators, and conditional updates; still outside: dynamic-length loops/iterators (fold roadmap item), generics (monomorphize first), per-parameter A-1 domains (single magnitude bound today).
- 9 rewrite rules against a 50-rule corpus target; growth is discharge-first by construction.
- Perf targets: SR convergence PASS; **jit ≥5× PASS on fold kernels (9.3×)**, still 2.5× on small scalar kernels; memo ≤50 ns remains a measured MISS (83 ns, DashMap swap identified) — misses block perf sign-off only, never correctness.
- "Real code" so far means real library code, three small kernels and one user function — a first validation, not a field trial.

Full phase-by-phase detail: [`docs/STATUS.md`](docs/STATUS.md).

## Roadmap (ordered)

1. **IR lifting front door (v3-Exp)** — *understand the math, not the
   syntax*: all code is instructions to a CPU, and the compiler's lowering
   already equates every surface syntax with its dataflow — so DGE reads
   the instruction-level form (LLVM IR), recovers the theorem hiding in it
   (`Term_p`; a loop is a recurrence = `Fold`), improves it, and writes it
   back — able to rewrite ANY code whose instruction-level meaning is pure
   math, regardless of what the source looked like. `dge lift` is a second
   front door (syn stays for clean code + emission round trips); the
   lifter is untrusted and every recovery passes the extraction gate. Direction accepted, spec corrected in
   [`docs/RFC-REVIEW-ir-lifting-v3.md`](docs/RFC-REVIEW-ir-lifting-v3.md):
   phased P1 straight-line / P2 loop→Fold / P3 side-effect slicing; every
   lift passes the existing extraction gate.
2. **Fold v1.3** — iterator adapters, offset/windowed indexing, non-zero
   range starts, multi-accumulator loops; sequence-aware memo keys.
3. **Per-parameter A-1 domains** — express `alpha ∈ [0,1]` instead of one
   global magnitude bound.
4. **`cargo dge` + CI mode** — audit on merge, gentest for new extractable
   fns, extraction/emission gates as regression checks.
5. **Latency-aware cost** (Finding 6) and rule corpus → 50 (discharge-first).
6. **Field trial** over a crate corpus; collect gate refutations to drive
   the next round.

(Historical phase-by-phase log: [`docs/STATUS.md`](docs/STATUS.md).)

## Design commitments (frozen)

No claims of zero debugging, 100 % test automation, or flawless software. "Guaranteed" is reserved for what a certificate states — which always includes what is proved, over which inputs, at what confidence, under which environment fingerprint, and nothing more.

## License

MIT OR Apache-2.0. Test fixtures include short excerpts from `easer` 0.3.0 (MIT), attributed in place.

---
# Part II — STATUS (phase log)

# deductive-gp-engine — skeleton (v2.0 normative + v2.1 execution merge)

Nine-crate Cargo workspace realizing the project AST. **Engine crates compile with zero
external dependencies** (cli uses `syn` for the audit — build tooling, not
trusted base); production backends (egg, Z3, cranelift, dashmap,
rayon, proptest) plug in behind trait seams at the phase where their proof
obligation is discharged — each seam is marked in the crate's `Cargo.toml`.

## The two load-bearing structures (already live in the skeleton)

**1. Typestate spine (T8 exit-door, in types):**

```
Term ──harness::Gate::promote (O1..O8 upstream)──▶ VerifiedTerm ──jit::install (O7)──▶ JitFn
```

`VerifiedTerm` and `JitFn` have **no public constructors** outside those two
functions. This is enforced by field privacy today, checked by the compiler
forever. No stage can convert an unproved claim into an accepted artifact.

**2. Dependency DAG (Cargo enforces it; violations = build failure):**

```
term ─▶ harness ─▶ { rules, mutate, locate, gp } ─▶ cli
  └──▶ memo (term only)          term+harness ─▶ jit ─▶ cli
```

Inversions forbidden: `term` depends on nothing; `harness` never imports
`rules`/`gp` — the judge must not import the contestants.

## What is implemented vs skeleton

| Crate | Phase | Status |
|---|---|---|
| term | R0 | **COMPLETE**: Σ (21 ops), arena + topological invariant, FNV structural hash, definitional interpreter + `eval_traced` coverage variant (trusted base), full-key compare (O6→DERIVED), s-expr parse/print with round-trip test, `graft`/`copy_subtree` structural surgery. |
| harness | R1 | **LIVE, O8 hardened**: Metric (abs/rel/ULP, NaN/±0 policy, `fma_mixed`), μ' boundary-mixture sampler (seeded, cert-recorded), T3 shrinker (bounded sentinels, saturating rank), `Gate::promote` with (n, α, δ_min) surfaced; EnvFingerprint with RUNTIME CPU-feature detection + BEHAVIORAL libm hash (FNV over transcendental output bits at fixed probes — identifies the linked libm even when version strings lie); staleness tested. Remaining: proptest strategy adapter. |
| rules | R2 | **LIVE end-to-end**: egg language mirror of Σ with bitwise-const terminals + arena bridges; Z3-as-subprocess O1 discharge writing .smt2+.out artifacts; QF_FP encoder (object equality — catches ±0 traps; transcendentals → Unknown per T2); `refactor()` with bounded saturation (I7 best-so-far), cost-monotone extraction (O4), tier routing (Dec/Sem-only → Tier A; any Approx → mandatory Tier B gate), and the entry condition ENFORCED (no artifact ⇒ refusal). Commutativity routed to R_sem with a compiled-in IEEE-754 proof (Z3 4.8 bit-blasting times out >12s on Float64 comm — the designed O2 escape hatch, measured). |
| mutate | R3 | **COMPLETE**: mutant enumeration; `mutation_score` end-to-end with the D5 denominator discipline (suite-kill as cheap one-way witness first, then SMT eq-filter, triage queue for Unknown) — tested with real equivalent mutants (dead-branch guards) excluded by Z3; SAT models parse into concrete witness envs verified against our own interpreter; pin emitter renders the proptest regression file. |
| locate | R4 | **LIVE end-to-end**: `spectrum::collect` over `term::eval_traced` (Select-pruned coverage) + Ochiai ranking — tested: a planted branch fault ranks top with failing-only coverage. Report carries the "aid, never verdict" framing. |
| gp | R5 | **COMPLETE**: evolve loop (T7 premises asserted at runtime) + seeded entry `run_with_pop`; Nelder-Mead constant refinement over the out-of-line const pool (tested: recovers 2x+3 coefficients to 1e-5); two-tier repair mode — op-swap at locate-ranked nodes first, GP subtree search seeded by grafts second, EXIT ONLY VIA GATE, honest null on budget miss (A-3). Tested: localize→fix→certify on a planted x²−x fault. |
| memo | R6 | **COMPLETE**: allocation-free hit path (double-hash key: term structural hash + env FNV; full authority compare over stored env bits AND term on hit — O6 DERIVED), lazy-LRU under a byte cap with counters; eviction soundness tested (T6). Measured 83 ns/hit; the ≤50 ns gap is the Mutex+HashMap — the documented DashMap swap (perf-only). |
| jit | R7 | **LIVE**: full cranelift lowering (arith/select native CLIF at opt_level=speed; sin..pow/fma/min/max via extern "C" wrappers over the SAME Rust methods the interpreter calls — bitwise agreement by construction, libm pinned by O8); `install` = the O7 door running the n=10⁴ differential gate with T3 shrink on mismatch; hot dispatch (install after threshold, PIN to interp forever on O7 failure). Tested: bitwise across every op category incl. NaN/±0/Inf/subnormal boundaries; the deliberately-naive CLIF fmin lowering is REFUTED by O7 with a NaN witness (cranelift fmin propagates NaN, Rust min returns the other operand — a real compiler-semantics catch); release perf smoke 2.5× on a 24-op chain (≥5× target stays HYPOTHESIZED until calibration on real kernels). |
| cli | R7+v3P1 | **ALL SUBCOMMANDS LIVE + `dge lift` (v3-Exp P1)**: LLVM IR second front door — straight-line pure-f64 lifting (hand-rolled parser, zero new deps), closed 17-symbol libm/intrinsic call map, phased refusals (br/phi→P2, memory→P3, fmf→IEEE discipline, one documented nsz-on-min/max carve-out); rustc driver at -C opt-level=1 with #[no_mangle] injection (cross-crate-inline deferral otherwise emits NO define — measured). 13 tests in lift_p1_test.rs: 5 rustc extraction gates + 1 clang-shaped gate (10^4 mu-prime, BitwiseNanClass) incl. an ITERATOR CHAIN the syn door refuses, 6 refusal pins, hex-const bit-exactness. **P2 SHIPPED same session**: canonical counted loops -> Sigma Fold (entry/(preheader?)/loop/(LCSSA tail?)/merge, all shapes measured; positive-only recognition, int world uninterpreted, gate arbitrates incl. L=0; driver adds `-C llvm-args=--unroll-runtime=false` -- rustc -O1 runtime-unrolls by 4 otherwise). 9 tests in lift_p2_test.rs: 5 fold gates (sum, capped+scalar, zip-dot, sqrt-tail, EMA-preheader), 3 phased refusals, cross-door fold agreement. **E2E SHIPPED same session**: `dge pipeline` is two-doored (--lift / .ll input / automatic syn->IR fallback with both refusals reported); core exposed as `cli::pipeline::certify` for tests; emission round-trip closure always runs through the syn door, so the doors certify each other. 3 e2e tests (iterator-chain fallback, EMA fold, both-doors-refuse) each with an independent third differential vs the rustc original. See RFC-REVIEW-ir-lifting-v3.md "P1/P2/E2E STATUS" for seven measured findings. **FIELD TRIAL No.1 complete** (`dge trial`, trial.rs + trial_test.rs): 3 published crates, 62 fns, instantiation-shim architecture (per-file mode measured ZERO on real crates), 14/14 cross-door bitwise agreements, refusal histogram -> priced extensions in docs/FIELD-TRIAL.md: (1) Sigma Len(k) symbol — quadratic_mean is a perfect fold refused only for uitofp(len); (2) diamond-CFG Select recovery — all 10 easer P2+ refusals are one if/else shape; (3) P3 confirmed DEFERRED (24% of fns, genuinely stateful algorithms). **Sigma v1.3 Len(k) SHIPPED same session** (trial item 1, gate-first at every layer incl. O7 JIT door; 5 tests in len_v13_test.rs): statistical trial re-measured 2->6 IR admits, P2+ bucket 4->0 — the full averaging family (mean/harmonic/geometric/quadratic/std-error) lifts. **Trial item 2 SHIPPED same session** (diamond-CFG Select recovery, lift_acyclic + 5 gates in diamond_test.rs): easer re-measured 18->26 IR admits, P2+ bucket 10->0, cross-door 18/18 bitwise. Corpus-wide P2+ EMPTY. Follow-up found that bucket was a CLASSIFIER BUG (fcmp-oeq refusals matched 'IEEE'); real item = **Sigma v1.4 Eq/Ne/Exp2, SHIPPED** (all layers, asymmetric oeq/une pair, 5 gates incl. real easer expo+elastic bodies): easer re-measured 26->32 of 34 IR admits (94% of public API), 14 fns beyond syn. **Sigma v1.5 FISSION SHIPPED same session** (multi-accumulator folds at BOTH doors, no Sigma change — N accumulators become N sibling Folds; the interpreter/JIT/emitter/fold_owners already accepted siblings): per-accumulator no-co-accumulator-read precondition (backward SSA slice at the IR door, Binding::Foreign at the syn door) — coupled Welford-style recurrences refuse with roadmap vocabulary at both doors; measured LCSSA-tail-sinking merge shapes (exit phis mix raw and tail values); syn door duplicates loop-invariant outer scalars per body (dup_subtree) and both doors validate fold_owners pre-return (ill-formed => honest refusal, never a gate panic); orphan Acc/Elem tolerated at interp+JIT as 0.0. 7 tests in multiacc_test.rs (variance IR gate, no-tail cross-door sum+product, three moments+shared scalar, min/max range, both-door coupling refusals, pipeline closure, O7 JIT differential). Honest: existing 3-crate corpus P2+ bucket was already EMPTY — zero measured fission instances there; value claim tests against the P3 re-quantification corpus. **FIELD TRIAL No.2 complete same session** (P3 re-quantification, 6 crates / 335 fns): P3 stays DEFERRED (<4%); measured binding constraint = shim signature coverage at 82% (method receivers / f32 / trait generics). Trial surfaced+fixed 3 defects (pinned in corpus2_findings_test.rs): syn door silently read f32 as f64 (concrete non-f64 numerics now refuse; generic T keeps f64-instantiation reading), `br !prof` metadata parse error (metadata dropped), `unreachable` = "no terminator" (now a parsed terminator; panic/assert paths refuse with totality vocabulary + dedicated bucket() class + render() prints raw reasons). Fission wild-corpus scorecard: zero instances — average 0.16.0's multi-accumulator stats are Welford-COUPLED behind receivers (refusal vocabulary already covers them). Next priced items: method-receiver shims, f32 lifting, panic-path refactoring assistance. **Receiver flattening SHIPPED same session** (priced item 1): syn door reads immutable `&self` methods — receiver f64 fields flatten to Vars in field declaration order; `&mut self` and non-f64 field reads refuse into named buckets; qualified-path unary calls (num_traits::Float::sqrt) map to Σ; render() prints syn raws symmetrically. 5 gates in receiver_methods_test.rs (incl. 10^4 bitwise differential vs native getter). Corpus yield: syn admits 25→30; average's 153-method wall decomposed into honest classes (55 &mut self / 31 non-f64 state) — remaining surface is STATE-shaped, unreachable without effects. Next: f32 lifting. **Sigma v1.6 f32 lifting SHIPPED same session** (priced item 2): ONE new op Rnd32 = (x as f32) as f64 — by double-rounding innocuousness (p=53 >= 2*24+2) f64-compute-then-Rnd32 is BIT-IDENTICAL to native f32 for +,-,*,/,sqrt, so f32 fns keep real bitwise gates. Extractor wraps every rounding op + every param Var (terms total over raw f64 mu'); transcendentals/fmaf/powf/mixed-precision/f32-seqs refuse with rounding vocabulary; pre-existing transparent-cast unsoundness fixed (casts now type-aware; (x as f32) as f64 IS Rnd32, no longer identity). Interp cast / JIT fdemote+fpromote / emit round-trip / egraph-inert. 6 gates in f32_test.rs. Corpus: syn admits 30->40 (simple-easing 0->7, keyframe 2->5), single-door + theorem-backed (IR float parsing roadmap). Next: panic-path refactoring assistance. **SDK + isolation boundary SHIPPED same session** (crates R8 sdk, R9 server): the core is now integrable documentation-only. Design rule = the typestate spine extended outward: VerifiedTerm's private constructor means EVERY extension point sits on the untrusted side — third-party FrontDoor impls produce candidate Terms (a lying door is REFUTED with a ⊏-minimal counterexample: pinned in sdk tests), Observer hooks are taps-not-gates (see events, veto nothing), and neither the SDK nor the HTTP API ever surfaces a VerifiedTerm (reports/sexprs/certificates-as-data/emitted code only). Engine facade: register_door/register_observer/extract/extract_all/gate/certify/eval; syn+ir doors pre-registered. dge-serve: hand-rolled HTTP/1.1 + JSON, endpoints /v1/{version,alphabet,extract,eval,gate,certify}, loopback-default, remote certificates documented as hearsay-until-regated. Contracts: docs/SDK.md + docs/API.md, self-sufficient by construction (curl examples verified live incl. exact bit patterns). 4 sdk tests + 5 live-socket server tests; 132 workspace total. **ALL other SUBCOMMANDS LIVE** — audit / discharge / refactor (auto-loads the calibrated cost table) / gentest / debug / calib. `gentest`: mutation-adequate suite growth under μ' with T3-shrunk pinned envs, SMT eq-filter on survivors, golden-suite emitter, (MS, n, α, δ_min) adequacy report. `debug`: CE hunt → T3 shrink → spectrum/Ochiai → optional gate-certified repair; prints the quantified no-CE claim when nothing is found. `calib`: per-op cost table from JITTED bounded-input sum kernels (pow 19×, tan 11×, sin/cos 8×, exp/ln 6× vs add=1 on this box) + the §6 perf-targets report with MEASURED values (jit 2.5×/≥5× MISS, memo 83 ns/≤50 ns MISS, SR 4 gen/≤40 PASS — misses block perf sign-off only). Audit **calibrated on real crates**: syn-based O5 classifier — 3 classes with per-reason diagnostics; transitive demotion over local calls (fixed point); impl-block + inline-mod recursion with impl-scoped generics; test code excluded from workload; panic paths (`assert!`, `.unwrap()`) = effort (totality guards), generic-numeric params = effort (monomorphize-then-extract); LOC-weighted `s_strict`/`s_loose`, §9 verdict. Other subcommands still skeleton. |

## Build order (v2.1 §5)

```
pre-R0: dge audit  →  s ≥ 0.2–0.3 or DO NOT BUILD
R0 → R1 → {R2, R3, R5 in parallel} → R4 → R6 → R7
```

## Real-code status (first target: easer 0.3.0, MIT)

`dge extract` is the front door: syn-based Rust→Term_p translation for the
audit's EXTRACT class (params→vars, arithmetic, Σ methods, let-shadowing,
if/else→Select with documented comparison encodings whose NaN caveats the
extraction gate arbitrates). **Extraction is itself a lowering and is NOT
trusted**: its output must pass the extraction gate — a bitwise differential
of interp(term) against the rustc-compiled original over μ'.

Validated end-to-end on the Cubic easing family from the published `easer`
crate (real generic source with the `f()` const-lift helper, read straight
from the crate file):

```
rustc(original) ==bitwise== interp(extract(src))              [extraction gate]
                ==bitwise== cranelift-jit(refactor(term))     [Tier A + O7]
```

over 10,000 μ' samples each (NaN/±0/Inf/subnormal boundaries included),
for ease_in, ease_out, and ease_in_out (branch + let-shadowing). The CLI
chain `extract → refactor → gentest` runs on the crate file directly
(ease_in: MS-over-M = 1.000 from 5 shrunk golden envs).

HONEST LIMITS of the real-code claim: one crate, three small kernels, the
strict-extractable subset only (no loops/iterators — the audit's WITH_EFFORT
class still requires manual extraction), comparisons limited to </>/<=/>=,
and "real code" here means real *published library* code, not a production
codebase under load. This is a first validation, not a completed field trial.

## User-code case study (naive cubic polynomial → Horner)

Submitted function: `3x³+5x²+2x+7` computed term-by-term. Pipeline results
(all pinned in `user_polynomial_test.rs`):
* extraction gate: rustc ==bitwise== interp(extract), 10⁴ μ' samples;
* Tier A refuses to reassociate (cost 19→19) — correctly, since no
  bitwise-sound rule may change f64 association;
* UNBOUNDED eps gate REFUTES Horner at |x|≈1e154: naive computes
  −inf+inf=NaN where the factored form gives −inf — reassociation is not
  ε-equivalent over all of f64, and the gate proves it with a witness;
* A-1 domain-bounded μ' (new: `MuPrime::bounded`, `--domain` flag) promotes
  Horner-fma at cost 19→10 with `DOMAIN |x|<=1e100` recorded verbatim in
  the certificate;
* the calibrated cost table steers extraction from wrapper-fma Horner
  (measured 0.66× — slower!) to mul/add Horner — L2 in action.

**Finding 6 (measured, pinned):** mul/add Horner vs naive is ~1.0× on this
CPU despite 6 vs 9 flops — Horner serializes the dependency chain while the
naive form's independent terms exploit superscalar ILP. `Σ op_weights` is a
THROUGHPUT model blind to latency; a critical-path cost term (also
O4-monotone) is the identified fix, listed under remaining work.

## R3–R7 findings (discovered by the machinery, now pinned as tests)

4. **O7 caught a real compiler-semantics mismatch**: CLIF `fmin` propagates
   NaN while Rust `f64::min` returns the other operand. The naive lowering is
   kept behind a flag and its O7 refutation (NaN witness, shrunk) is a
   permanent test; production lowering routes min/max/fma/transcendentals
   through extern wrappers over the interpreter's own methods.
5. **The shrinker had an overflow bug** found by that same test: NaN
   complexity was u64::MAX and rank() summed coordinates. Sentinels are now
   bounded and the sum saturates.

## R2 findings (discovered by the machinery, now pinned as tests)

1. **The −0.0 trap is real and O1 catches it**: `x + 0.0 → x` comes back SAT
   from Z3 (witness −0.0) while `x − 0.0 → x` proves UNSAT. Pinned in
   `o1_catches_the_minus_zero_trap`.
2. **Float64 commutativity is a practical Z3 timeout** (>12 s bit-blasting)
   though trivially true — routed to R_sem with the one-paragraph IEEE-754
   proof compiled into the source. The O2 route exists for exactly this.
3. **SPEC CORRECTION — v2.1 §1's "≤1 ULP" fma claim is FALSE**: the Tier-B
   gate refuted fma-contraction under catastrophic cancellation (a·b ≈ −c
   turns mul-rounding into O(1) relative error, unbounded ULPs). The honest
   ~_eps is a mixed abs/rel tolerance (`Metric::fma_mixed()`); the O7 jit
   relaxation inherits the same correction. Pinned in
   `gate_refutes_the_one_ulp_fma_claim`.

## Audit calibration (real crates, v3 rules)

| crate | kind | s_strict | s_loose | reading |
|---|---|---|---|---|
| easer 0.3 | easing formulas, generic `F: Float` | 0.000 | 0.509 | formula-level code; effort = mechanical monomorphization + const lift |
| micromath 2.1 | embedded approximations | 0.000 | 0.049 | mostly bit-level, below Σ |
| libm 0.2.8 | float kernels | 0.004 | 0.029 | **implements** Σ's primitives — below the perimeter by definition |
| fastapprox 0.3 | bit-trick approximations | 0.000 | 0.000 | entirely below Σ |
| serde 1.0 | serialization | 0.000 | 0.001 | outside the domain, correctly rejected |
| test fixture | concrete-f64 app code | 0.342 | 0.632 | the intended workload shape → BUILD |

Two calibration lessons now encoded in the tool: (1) `s_strict` measures
concrete-f64 code; the Rust *library* ecosystem is generic-over-Float, so
libraries register in `s_loose` (their effort is mechanical monomorphization) —
point the audit at APPLICATION code for the §9 decision. (2) A near-zero score
on a "numeric" crate can mean the code is BELOW Σ (implements the primitives,
e.g. libm) rather than above it; the per-fn reasons distinguish the two.

## Remaining work

0. **Latency-aware cost**: add a critical-path (max-over-children) term to
   the cost model — O4-monotone, calibrated from op latencies — so
   extraction stops trading ILP for op count blindly (Finding 6).
0. **Field trial**: run the extract→gate→refactor→gentest chain across a
   corpus of real crates' EXTRACT-class functions (the audit already finds
   them); collect extraction-gate refutation cases to drive encoding fixes.

1. **R1**: proptest strategy adapter over the μ' reference sampler.
2. **R2**: grow the Dec table toward the 50-rule corpus (discharge-first per
   rule); egg Runner hooks for per-extraction rule provenance (trace is
   currently the sound superset of applied rules).
3. **Perf sign-off** (blocks nothing else): DashMap swap for the memo ≤50 ns
   target; jit call-path tightening toward ≥5× (measured 2.5× on the 24-op
   reference; grows with kernel size); perf_event counters for `dge calib`
   (v1 M7 upgrade from wall-clock medians).
4. **CI**: `make mutants` (cargo-mutants over this workspace's own suite,
   v1 M6) — target present in the Makefile, long-running by nature.

## Running

```
cargo test --workspace     # 36 tests (see Makefile: test / calib / discharge / mutants): R0 interp/hash/parse/trace, R1 gate/shrink,
                           #           R3 mutant-kill, R4 fault localization,
                           #           R5 SR-through-the-gate, R6 cache
cargo run -p cli --bin dge # usage
```

## Non-claims (frozen, §8)

No "zero debugging / 100% test automation / flawless software." Effects,
concurrency, architecture, unstatable properties are outside the perimeter by
construction. Perf numbers (≥5× JIT, ≤50ns memo hit, ≤40 gen SR) are
HYPOTHESIZED until R7 measurement; "guaranteed" is reserved for certificates.

---
# Part III — Field trials

# Field trial №1 — three published crates, both doors, everything bucketed

**Date**: 2026-07-15 · **Corpus**: `easer 0.3.0`, `statistical 1.0.0`,
`interpolation 0.3.0` (fetched from crates.io) · **Harness**: `dge trial`
(`cli/src/trial.rs`), rustc 1.97, -O1, `--unroll-runtime=false`.

This is roadmap item 1 executed: point both front doors at real published
code, cross-gate every double admission, and bucket every refusal by the
extension that would admit it. The buckets below are the pricing data for
the next round — nothing here is estimated.

## Headline numbers (62 fns audited across 3 crates)

| crate | fns | syn admits | IR admits | IR-only (added coverage) | cross-gate |
|---|---|---|---|---|---|
| easer 0.3.0 | 34 | 18 | 18 | **5** | **13/13 agree** |
| statistical 1.0.0 | 25 | 1 | 2 | 1 | 1/1 agree |
| interpolation 0.3.0 | 3 | 0 | 0 | 0 | — |

**Every function both doors admitted, they agreed on bitwise** over 2·10³ μ′
samples (14/14 total). Two independent untrusted lowerings — one reading
syntax, one reading the compiler's SSA — recovering identical semantics on
real code is the strongest evidence yet for both.

## The trial's own first finding: the shim architecture

The per-file IR mode scored **zero** admissions on the whole corpus — real
crate files don't compile standalone, and generic fns emit no IR at all.
The fix, now shipped in `dge trial`: an **instantiation shim** per candidate
fn — a generated crate that path-depends on the target and exposes one
`#[no_mangle]` wrapper calling it with f64/&[f64]/None arguments.
Monomorphization then materializes the generic body inside the shim's
codegen unit, and the IR door reads the result. (The shim is untrusted build
tooling; the cross-door gate is what vouches for it.)

## Refusal histogram → priced extension list

Aggregated IR-door refusals (44 across the corpus), highest value first:

1. **`Len(k)` — a Σ symbol for the sequence length. THE #1 ITEM.**
   `statistical`'s `quadratic_mean` lifts to a *perfect canonical fold*
   whose exit block is `sqrt(fold / uitofp(len))` — refused only because
   Σ cannot say "the length, as a value". Every averaging statistic (mean,
   harmonic/geometric/quadratic mean, the whole moment family — most of the
   14 P2+/P3-adjacent refusals in `statistical`) is one nullary op away.
   Cost: ~60 lines across term/interp/emit/recognizer. Value: unlocks the
   single most common numeric-loop idiom in the wild.
2. **Diamond-CFG select recovery (P2 family).** All 10 of easer's P2+
   refusals are the SAME shape: an `if/else` whose arms both bind lets
   lowers at -O1 to a two-armed branch diamond, not a `select`. A diamond
   with straight-line f64 arms is `Select(cond, armA, armB)` — no new Σ
   needed, only CFG recognition. Unlocks every `ease_in_out` in easer.
3. **P3 (memory/side effects): 15 refusals** — sorting/median in
   `statistical` (genuinely effectful: allocation + in-place partition) and
   easer's `bounce` family. This is the honest P3 population: real, but a
   minority (15/62 = 24%), concentrated in genuinely stateful algorithms
   the perimeter *should* exclude. **Recommendation: P3 stays deferred.**
   Items 1–2 buy far more coverage for ~5% of P3's cost.
4. **Shim signature coverage** (9 refusals, honest limits): `&T`-reference
   generics (`interpolation`), integer-generic params (`PrimInt` sizes),
   private helper fns (not in the public API — correctly untouchable from
   outside). These bound what any external tool can see; only the first is
   plausibly worth extending.

## Raw reports (verbatim)

### easer-0.3.0

```
FIELD TRIAL: 34 fns audited
  syn door admits : 18
  IR  door admits : 18  (5 of them REFUSED by syn — the IR door's added coverage)
  cross-door gate : 13/13 agree bitwise over 2e3 mu' samples

IR-door refusal histogram (the roadmap pricing data):
    10  P2+ (loop shape beyond canonical fold)
     5  P3 (memory / side effects)
     1  not in the crate's public API (shim)

per-fn detail:
  ease_in                        syn=OK(14n) ir=OK(13n) gate=AGREE
  ease_out                       syn=OK(18n) ir=OK(17n) gate=AGREE
  ease_in_out                    syn=OK(39n) ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in                        syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  ease_out                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  ease_in_out                    syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  ease_in                        syn=refused[other (see raw reasons)] ir=OK(13n)
  ease_out                       syn=refused[other (see raw reasons)] ir=OK(13n)
  ease_in_out                    syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in                        syn=OK(9n) ir=OK(9n) gate=AGREE
  ease_out                       syn=OK(13n) ir=OK(13n) gate=AGREE
  ease_in_out                    syn=OK(26n) ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in                        syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_out                       syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in_out                    syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  ease_in                        syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_out                       syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in_out                    syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  ease_in                        syn=OK(7n) ir=OK(7n) gate=AGREE
  ease_out                       syn=OK(7n) ir=OK(7n) gate=AGREE
  ease_in_out                    syn=OK(7n) ir=OK(7n) gate=AGREE
  ease_in                        syn=OK(8n) ir=OK(8n) gate=AGREE
  ease_out                       syn=OK(11n) ir=OK(11n) gate=AGREE
  ease_in_out                    syn=OK(27n) ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in                        syn=OK(10n) ir=OK(10n) gate=AGREE
  ease_out                       syn=OK(15n) ir=OK(14n) gate=AGREE
  ease_in_out                    syn=OK(29n) ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in                        syn=OK(11n) ir=OK(11n) gate=AGREE
  ease_out                       syn=OK(15n) ir=OK(15n) gate=AGREE
  ease_in_out                    syn=OK(30n) ir=refused[P2+ (loop shape beyond canonical fold)]
  ease_in                        syn=refused[other (see raw reasons)] ir=OK(11n)
  ease_out                       syn=refused[other (see raw reasons)] ir=OK(10n)
  ease_in_out                    syn=refused[other (see raw reasons)] ir=OK(14n)
  f                              syn=refused[other (see raw reasons)] ir=refused[not in the crate's public API (shim)]
```

### statistical-1.0.0

```
FIELD TRIAL: 25 fns audited
  syn door admits : 1
  IR  door admits : 2  (1 of them REFUSED by syn — the IR door's added coverage)
  cross-door gate : 1/1 agree bitwise over 2e3 mu' samples

IR-door refusal histogram (the roadmap pricing data):
    10  P3 (memory / side effects)
     6  unsupported signature (shim)
     4  P2+ (loop shape beyond canonical fold)
     2  generic bounds not f64-instantiable (shim)
     1  not in the crate's public API (shim)

per-fn detail:
  std_moment                     syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  mean                           syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  median                         syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  sum_square_deviations          syn=refused[other (see raw reasons)] ir=refused[not in the crate's public API (shim)]
  variance                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  population_variance            syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  standard_deviation             syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  population_standard_deviation  syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  standard_scores                syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  select_pivot                   syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  partition                      syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  quicksort                      syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  harmonic_mean                  syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  geometric_mean                 syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  quadratic_mean                 syn=refused[other (see raw reasons)] ir=refused[P2+ (loop shape beyond canonical fold)]
  mode                           syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  average_deviation              syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  pearson_skewness               syn=OK(5n) ir=OK(5n) gate=AGREE
  skewness                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  pskewness                      syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  kurtosis                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  pkurtosis                      syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  standard_error_mean            syn=refused[other (see raw reasons)] ir=OK(4n)
  standard_error_skewness        syn=refused[other (see raw reasons)] ir=refused[generic bounds not f64-instantiable (shim)]
  standard_error_kurtosis        syn=refused[other (see raw reasons)] ir=refused[generic bounds not f64-instantiable (shim)]
```

### interpolation-0.3.0

```
FIELD TRIAL: 3 fns audited
  syn door admits : 0
  IR  door admits : 0  (0 of them REFUSED by syn — the IR door's added coverage)
  cross-door gate : 0/0 agree bitwise over 2e3 mu' samples

IR-door refusal histogram (the roadmap pricing data):
     3  unsupported signature (shim)

per-fn detail:
  lerp                           syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  quad_bez                       syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  cub_bez                        syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
```

## Methodology notes

* Cross-gate = syn-term vs lifted-term, BitwiseNanClass, 2·10³ μ′ samples
  including NaN/±0/Inf/subnormal and zero-length sequences. It requires no
  FFI to the compiled original; a disagreement would indict one lowering.
* Single admissions are reported **ungated** — the trial makes no
  equivalence claim for them (the full extraction gate needs a callable
  original, which arbitrary third-party code does not provide in-trial).
* `Option<T>` params are driven with `None` (the crate computes its own
  default — still a pure function of the slice).
* All buckets are produced by `cli::trial::bucket()` from the refusal
  strings themselves; the P2/P3 vocabulary in refusal messages exists
  precisely so this trial could count it.

## Addendum — Σ v1.3 `Len(k)` shipped, unlock MEASURED (same day)

Item 1 above was implemented (`Op::Len`, ~70 lines across
term/interp/sexpr/hash/rules-mirror/jit/emit/extract/lift, gated in
`cli/tests/len_v13_test.rs` at every layer including the O7 JIT install
door and the full pipeline closure). Re-running THIS trial on
`statistical 1.0.0`:

| | before Len | after Len |
|---|---|---|
| IR admits | 2 | **6** |
| P2+ bucket | 4 | **0** |

The entire averaging family — `mean`, `harmonic_mean`, `geometric_mean`,
`quadratic_mean`, `standard_error_mean` — now lifts through the IR door
(the last three are transcendental folds: `exp(mean(ln x))` etc.). The
P2+ bucket for this crate is EMPTY: every loop `statistical` exposes
publicly is now inside the perimeter or honestly P3. `easer`'s numbers are
unchanged (its P2+ population is the diamond-CFG shape — item 2, still
open). Verbatim post-Len report:

```
FIELD TRIAL: 25 fns audited
  syn door admits : 1
  IR  door admits : 6  (5 of them REFUSED by syn — the IR door's added coverage)
  cross-door gate : 1/1 agree bitwise over 2e3 mu' samples

IR-door refusal histogram (the roadmap pricing data):
    10  P3 (memory / side effects)
     6  unsupported signature (shim)
     2  generic bounds not f64-instantiable (shim)
     1  not in the crate's public API (shim)

per-fn detail:
  std_moment                     syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  mean                           syn=refused[other (see raw reasons)] ir=OK(7n)
  median                         syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  sum_square_deviations          syn=refused[other (see raw reasons)] ir=refused[not in the crate's public API (shim)]
  variance                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  population_variance            syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  standard_deviation             syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  population_standard_deviation  syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  standard_scores                syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  select_pivot                   syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  partition                      syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  quicksort                      syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  harmonic_mean                  syn=refused[other (see raw reasons)] ir=OK(9n)
  geometric_mean                 syn=refused[other (see raw reasons)] ir=OK(9n)
  quadratic_mean                 syn=refused[other (see raw reasons)] ir=OK(9n)
  mode                           syn=refused[other (see raw reasons)] ir=refused[unsupported signature (shim)]
  average_deviation              syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  pearson_skewness               syn=OK(5n) ir=OK(5n) gate=AGREE
  skewness                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  pskewness                      syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  kurtosis                       syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  pkurtosis                      syn=refused[other (see raw reasons)] ir=refused[P3 (memory / side effects)]
  standard_error_mean            syn=refused[other (see raw reasons)] ir=OK(4n)
  standard_error_skewness        syn=refused[other (see raw reasons)] ir=refused[generic bounds not f64-instantiable (shim)]
  standard_error_kurtosis        syn=refused[other (see raw reasons)] ir=refused[generic bounds not f64-instantiable (shim)]
```

The prediction→implementation→measurement loop closed in one session:
the trial priced the extension, the extension shipped gate-first, and the
same trial measured the unlock. That loop is the methodology working.

## Addendum 2 — item 2 (diamond-CFG Select recovery) shipped, unlock MEASURED

The acyclic recognizer landed in `cli/src/lift.rs` (`lift_acyclic`):
multi-block CFGs with no back-edge dispatch to a branch-tree resolver that
turns each phi at a merge into nested Selects. Two shapes it had to learn
from real IR: LLVM's textual block order is a LAYOUT order (the merge can
print before its own predecessors — a real topological sort is required),
and nested if/else collapses into ONE n-way phi (the tree resolver unfolds
it back). Eager materialization of both arms is sound because the alphabet
is total; the gates check it over full μ′ anyway. Five tests in
`diamond_test.rs`: the ease_in_out diamond (10⁴ μ′ vs the original AND
cross-door vs the 39-node syn term), a handwritten triangle, nested
diamonds, an else-if chain (3-way phi → nested Selects), and the
integer-condition refusal.

Re-running THIS trial on `easer 0.3.0`:

| | before item 2 | after |
|---|---|---|
| IR admits | 18 | **26** |
| IR-only (beyond syn) | 5 | **8** |
| cross-door agreements | 13/13 | **18/18** |
| P2+ bucket | 10 | **0** |

```
FIELD TRIAL: 34 fns audited
  syn door admits : 18
  IR  door admits : 26  (8 of them REFUSED by syn — the IR door's added coverage)
  cross-door gate : 18/18 agree bitwise over 2e3 mu' samples

IR-door refusal histogram (the roadmap pricing data):
     6  fast-math flags
     1  P3 (memory / side effects)
     1  not in the crate's public API (shim)

```

The P2+ bucket is now EMPTY on the whole corpus. Newly visible underneath
it: **6× fast-math flags** (elastic/expo easings — rustc stamps fmf on some
of its own float intrinsic lowerings), which is the honest next pricing
question: which specific rustc-emitted flags are semantics-preserving
enough to carve out, the way `nsz`-on-min/max already was. Priced list
after two shipped items: (1) rustc-fmf carve-outs [6 fns], (2)
multi-accumulator folds, (3) P3 [16 fns, still deferred].

## Addendum 3 — the "fast-math" bucket was a bug; the real item (Σ v1.4) shipped

Investigating the 6-fn fast-math bucket found NO fmf tokens in any shim IR.
The raw refusals were `fcmp oeq` — easer's `t == 0.0` guards — and
`bucket()` had matched the word "IEEE" inside the fcmp refusal message
("Rust/IEEE ordered comparisons"). **A measured bucketing bug, now fixed
and confessed here**: histograms are only as honest as their classifier.

The correctly-priced item was Σ v1.4: `Eq`/`Ne` (Rust's exact `==`/`!=` =
the ASYMMETRIC IEEE pair oeq/une — NaN==NaN false but NaN!=NaN true,
-0.0==+0.0 true) and `Exp2` (LLVM rewrites `powf(2.0, x)` → llvm.exp2;
O8: interp's f64::exp2 links the same libm). Shipped through all layers
(interp, sexpr, hash, egg mirror, JIT FloatCC::Equal/NotEqual + w_exp2
wrapper, emit, syn `==`/`!=`/`exp2`, lift oeq/une/llvm.exp2), with the
`c != 0.0` emitter round-trip special case preserved for structural
idempotence. Five gates in `v14_eq_ne_exp2_test.rs`, including the REAL
easer expo body and the REAL elastic guard chain (two sequential
`if t == … { return … }` guards — rustc merges them into one n-way phi,
which item 2's branch-tree resolver unfolds; the two items compose).

Re-running THIS trial on `easer 0.3.0`:

| | trial №1 | after Len | after diamonds | **after v1.4** |
|---|---|---|---|---|
| IR admits (of 34) | 18 | 18 | 26 | **32** |
| beyond syn's reach | 5 | 5 | 8 | **14** |
| cross-door | 13/13 | 13/13 | 18/18 | **18/18** |

```
FIELD TRIAL: 34 fns audited
  syn door admits : 18
  IR  door admits : 32  (14 of them REFUSED by syn — the IR door's added coverage)
  cross-door gate : 18/18 agree bitwise over 2e3 mu' samples

IR-door refusal histogram (the roadmap pricing data):
     1  P3 (memory / side effects)
     1  not in the crate's public API (shim)

```

Remaining on easer: 1 genuinely-P3 fn and 1 private helper — **94%
public-API coverage**. Corpus-wide open buckets: P3 [11 fns], unsupported
shim signatures [9], private/int-generic [6]. Next priced items:
multi-accumulator folds, then the P3 re-quantification on a larger corpus.

## Addendum 4: Σ v1.5 fission (multi-accumulator folds), SHIPPED

The last loop-shaped item the P2 refusals priced: bodies mutating N
scalars / N f64 accumulator phis. Shipped as FISSION at both doors — each
accumulator becomes its OWN sibling Fold over the same 0..L space, so Σ
itself is unchanged (fold_owners, interp, JIT, emitter already accepted
siblings). Per-accumulator soundness precondition at both doors: the
update slice must not read a co-accumulator — coupled recurrences
(Welford-style online variance) have no fission and refuse with the
roadmap vocabulary. Measured shape handled: rustc -O1 sinks invariant
recombination (`s*s`) into the LCSSA tail, so exit phis mix raw
next-values and tail values. Seven gates in `multiacc_test.rs`.

**Honest accounting**: this corpus's P2+ bucket was ALREADY EMPTY after
v1.3/diamonds/v1.4 — fission has **zero measured instances on the 3-crate
corpus**. It was built on the priced conjecture that the mean=sum/count
family is ubiquitous in wild numeric code. That conjecture is exactly what
the next item must test — the re-quantification corpus below either
vindicates the spend or prices the lesson.

# Field trial №2 — P3 re-quantification on a doubled corpus

**Date**: 2026-07-17 · **Corpus**: trial №1's three crates PLUS
`average 0.16.0` (Welford-style online statistics), `keyframe 1.1.1`
(generic animation easing), `simple-easing 1.0.1` (all-f32 easings) —
fetched from crates.io · **Harness**: `dge trial`, rustc 1.97, -O1,
`--unroll-runtime=false`, with Σ v1.5 fission in both doors.

## Headline numbers (335 fns audited across 6 crates)

| crate | fns | syn admits | IR admits | cross-gate |
|---|---|---|---|---|
| easer 0.3.0 | 34 | 18 | 32 | 18/18 agree |
| statistical 1.0.0 | 25 | 1 | 6 | 1/1 agree |
| interpolation 0.3.0 | 3 | 0 | 0 | — |
| average 0.16.0 | 159 | 6 | 0 | — |
| keyframe 1.1.1 | 81 | 0 | 0 | — |
| simple-easing 1.0.1 | 33 | 0 | 0 | — |

Aggregate IR-door refusal histogram (297 refusals):

```
   275  unsupported signature (shim)      = 82% of the corpus
     7  shim-build-failed
     6  panic path (partial fn -- totality effort, audit class)
     5  P3 (memory / side effects)
     2  not in the crate's public API (shim)
     2  generic bounds not f64-instantiable (shim)
```

## The trial paid for itself before producing the histogram

Three defects surfaced and were fixed IN this trial (each pinned in
`corpus2_findings_test.rs`):

1. **The syn door read f32 as f64** — `simple-easing` (33 all-f32 fns)
   showed 21 syn admits whose extracted terms had f64 rounding at every
   op: wrong-precision terms. No false certificate could have issued (the
   extraction gate's 10⁴ differential arbitrates), but the trial's own
   "syn admits" column was inflated. Concrete non-f64 numeric types now
   refuse honestly; generic `T` keeps the monomorphize-then-extract
   f64-instantiation reading the cross-gate vouches for. Corrected
   numbers: simple-easing 21→0, keyframe 3→0 syn admits.
2. **`br … !prof` metadata was a parse error** — statistical's `assert!`
   guards carry profile metadata at -O1. The parser now drops
   instruction-level `!`-metadata (annotation, not semantics).
3. **`unreachable` was "no terminator"** — inlined panic paths end in
   `unreachable`, which fell through to a parse error, masking the honest
   perimeter answer. It is now a parsed terminator, and the refusal states
   the real reason: panic/assert paths make the function PARTIAL, Σ terms
   are total (the audit's own totality-guard vocabulary). This bucket
   (6 fns, statistical's whole variance family) was previously reported
   as unreadable `other` — a reporting cousin of Addendum 3's classifier
   bug. `render()` now prints raw reasons so an unpriced bucket is
   visible on sight.

## The re-quantification verdict

**P3 stays deferred — its bucket did not grow.** Visible P3 is 5/335;
even adding the 6 panic-path fns (refactorable, not P3) the effectful
population is under 4% of the doubled corpus. The binding constraint in
the wild is not Σ's alphabet at all: **82% of refusals are shim
signature coverage** — method receivers (`average` is 153/159 methods on
statistics structs), concrete f32 (`simple-easing` entirely), and
trait-generic `Self`/associated types (`keyframe`). The priced next
items, in measured order of value:

1. **Method-receiver shims** — flatten `&self` structs of f64 fields
   into scalar params. Unlocks `average`'s 153-fn surface for AUDITING
   (most bodies are then Welford-coupled or stateful and will refuse
   honestly — but they refuse for their real reason, which is the data
   the roadmap needs).
2. **f32 lifting** — an f32 world with round-at-every-op semantics, or
   an f32→f64 shim with a weakened (non-bitwise) gate. Unlocks
   `simple-easing`'s 33 fns and keyframe's f32 surface.
3. **Panic-path refactoring assistance** — the 6 statistical fns are one
   `assert!` away from lifting; the audit already classes them as effort.

## Σ v1.5 fission: honest scorecard on the wild corpus

Zero fission instances measured. `average` — the crate chosen precisely
because it is multi-accumulator statistics — implements the family as
Welford-style COUPLED recurrences behind method receivers: no fission
exists for them (the refusal vocabulary is already in both doors), and
the receivers keep them out of shim reach besides. The mean=sum/count
conjecture that priced fission is so far unvindicated by field data;
the feature's measured value remains its seven synthetic gates and the
cross-door agreement they exercise. This is what re-quantification is
for: the next Σ spend should go to the shim-signature buckets above,
which are measured at 82%, not conjectured.

## Addendum 5: receiver flattening (trial №2 priced item 1), SHIPPED

The syn door now reads immutable `&self` methods by flattening the
receiver struct's f64 fields into Vars in field DECLARATION order
(deterministic slot meaning; struct must be defined in the same file).
Refusal discipline: `&mut self` refuses outright (effectful — the
audit's effort/P3 class); non-f64 fields refuse ON READ, naming field
and type, so a method touching only f64 state still extracts. The IR
door still has no receiver shim (private fields make struct
construction impossible in a wrapper — MEASURED on `average`, whose
estimator fields are all private); its refusals moved to a dedicated
bucket instead of drowning in "unsupported signature". Bonus fix
measured on `average::Variance::error()`: qualified-path unary calls
(`num_traits::Float::sqrt(x)`) now map to the same Σ op as `.sqrt()`.
Five gates in `receiver_methods_test.rs`, including a 10⁴ bitwise
differential against a native `&self` getter.

**Measured yield on the corpus**: syn admits 25 → 30 (average 6→9,
keyframe 0→2). The larger yield is diagnostic: 153 `average` methods
that were one opaque signature bucket now audit and refuse for their
REAL reasons — 55 `&mut self` (effectful by design: the online-update
half of every estimator), 31 non-f64 state reads (sample counts,
weights vectors), the rest op-outside-Σ and constructor machinery.
That decomposition is the pricing data the roadmap wanted: the
remaining `average` surface is not signature-blocked, it is
STATE-shaped — mutation and non-f64 state — which no Σ extension
short of effects can claim. Item 1 is closed; the measured next
frontier on this corpus is item 2 (f32), which gates simple-easing's
33 fns and keyframe's f32 surface.

## Addendum 6: Σ v1.6 — f32 lifting via Rnd32 (trial №2 priced item 2), SHIPPED

The item the pricing called hard, made tractable by one theorem: for
+, -, *, /, sqrt over f32-representable operands, f64-compute-then-
round-to-f32 is BIT-IDENTICAL to native f32 — double rounding is
innocuous because f64's p=53 ≥ 2·24+2 (Figueroa 1995). So Σ gains ONE
unary op, `Rnd32` = `(x as f32) as f64` (interp cast / JIT
fdemote+fpromote / emit round-trip cast / egraph-representable but
rewritten by NO rule — rounding kills every algebraic identity), and
f32 functions get REAL bitwise gates, not weakened ones. The extractor
wraps every rounding op AND every param Var, so terms are TOTAL over
raw f64 μ′: `term(e) == widen(native_f32(round32(e)))` for all f64
envs — the rounding lives inside the term, not in the sampler.

Non-innocuous shapes refuse with the rounding vocabulary: transcendentals
(libm sinf ≠ round64(sin)), f32 `mul_add` (fmaf rounds ONCE at 24 bits),
`powf`, mixed f32/f64 signatures, f32 sequences (roadmap). The work also
surfaced and fixed a PRE-EXISTING soundness gap: casts were transparent,
so `(x as f32) as f64` extracted as the identity — dropping the rounding.
Casts are now type-aware (`as f64` widening transparent; the f32
round-trip IS Rnd32; bare `as f32` in an f64 fn refuses — downstream f32
ops are invisible to a syntax walk; integer casts refuse as truncation,
except the emitter's exact `(cmp) as u8` form). Six gates in
`f32_test.rs`: three 10⁴ native-f32 bitwise differentials, an O7 JIT
differential through the fdemote path, emit∘extract closure, and the
refusal pins.

**Measured yield**: syn admits 30 → 40 (simple-easing 0→7, keyframe
2→5). simple-easing's remaining refusals are honest and named — module
consts (`PI`, `C3`) and cross-fn calls (`bounce_out`) are extraction
plumbing, and `powf` is correctly non-innocuous. Honest caveat: these
are single-door admissions, ungated in the trial (no IR term to cross
against — float-op IR parsing is roadmap); their semantics rest on the
innocuousness theorem plus the pinned native differentials, which is a
proof-shaped claim, not a per-fn gate. The IR-door refusal names this
precisely: "f32 signature (no IR shim -- float-op parsing is roadmap;
the syn door reads f32 via Rnd32)".

---
# Part IV — RFC review: IR lifting v3

# Review: "DGE v3-Exp — Cranelift-Based Mathematical Lifting" RFC

Verdict: **direction accepted, specification rejected as written.**
IR-level extraction is the correct long-term answer to the extractor's
syntax treadmill; three technical errors and one claim-discipline
regression must be fixed before implementation.

## Accepted premises

1. SSA IR normalizes surface syntax: `while`/`for`/iterators/method sugar
   collapse into one small, CLOSED op alphabet. The extraction long tail
   becomes finite instead of open-ended.
2. Our architecture makes the experiment safe: a lifter is a lowering in
   reverse (L1), untrusted by construction. The extraction gate
   (differential vs. the compiled original) arbitrates ANY front door —
   an unsound lifter costs coverage, never correctness.
3. Keep the syn extractor. It stays the emission round-trip closure and
   the clean-code fast path; IR lifting is a SECOND front door
   (`dge lift`), not a replacement.

## Corrections to the RFC

1. **Front-end**: rustc does not emit Cranelift IR (cg_clif is
   nightly/awkward as a capture library); clang→Cranelift does not exist.
   Use **LLVM IR text**: `rustc --emit=llvm-ir` and `clang -emit-llvm`
   are stable and one lifter covers Rust AND C/C++.
   Recommended flags: `-O1` (mem2reg already done; -O0 buries math in
   stack loads/stores) with `fp-contract=off` (no pre-fused fma).
2. **Loops are unpriced**: RFC §3.3 covers only straight-line
   `fmul → Mul` lifting. At IR level loops are CFGs with phi nodes;
   recovering `Fold` needs natural-loop detection, induction-variable
   and accumulator recognition — HARDER than at AST level. §3.5
   ("re-insert side effects in original order") is the hardest problem
   in the document and is essentially unspecified.
3. **KPIs contradict pinned findings**: "Z3 bit-to-bit equivalence" for
   Horner/FMA packing is refuted by Findings 1, 2, 3, 7 (see README).
   "100% bit-to-bit" and ">2× via FMA packing" are JOINTLY impossible —
   FMA packing is precisely the non-bit-equal transformation.
   Replace with our certificate discipline: Tier A per-rule proofs,
   Tier B (n, α, δ_min) over recorded μ′, Metric::BitwiseNanClass for
   cross-generator claims, A-1 domain bounds in certificates.

## Adopted plan (phased, gate-arbitrated)

* **P1 — straight-line**: parse LLVM IR of pure f64 functions (no
  branches beyond select, no memory ops, no calls outside a libm map);
  trace SSA values into Term_p; run the EXISTING extraction gate vs. the
  compiled original. Small; proves the pipe.
* **P2 — loop recovery**: single natural loop, single induction variable
  `0..n`, single f64 accumulator phi → `Fold` (mirrors the Σ v1.2
  contract). Everything else refused with reasons.
* **P3 — side-effect slicing + re-stitching**: defer until the field
  trial quantifies the coverage gap; est. 5–10× the effort of P1+P2.

## KPIs (replacing RFC §4)

* Every lifted term passes the extraction gate: 10⁴ μ′ samples incl.
  NaN/±0/Inf/subnormal, BitwiseNanClass vs. the rustc/clang-compiled
  original. Failures are refusals with witnesses, not bugs to hide.
* Output = certified high-level Rust via the existing emit/pipeline path
  (certificate as doc comment; hand edits void it).
* Performance claims only from `dge calib`-style measurement; misses
  block perf sign-off only.

## Mission statement (author's intent — read this first)

DGE must understand the MATHEMATICS, not the SYNTAX.

All source code is ultimately instructions to a CPU. Syntax is a human
costume; the compiler's lowering to IR/instructions is where every costume
is removed. Two functions that look nothing alike — a `for` loop, an
iterator chain, a `while` with an index, hand-unrolled arithmetic —
compile to the same SSA dataflow when they compute the same math. The
equivalence between syntax and math is not something we build: the
compiler already established it. Lifting RECOVERS the math from it.

So the pipeline's meaning is:

    DGE reads what the CPU is told (the instructions / IR dataflow),
    recovers the theorem hiding in those instructions (Term_p),
    improves the theorem (rules + gate),
    and writes it back as code (emit) —
    with the extraction gate certifying the recovery was faithful.

Consequence: DGE becomes able to rewrite ANY code whose instruction-level
meaning is pure math — regardless of what the source syntax looked like —
because it understands the code by reading its CPU-level form, not its
surface form. A loop is not "unsupported syntax"; at instruction level it
is what it always was: a recurrence, i.e. our Fold.

Calibration (keep honest): the IR does not hand us math LABELED — it hands
us dataflow. Recognizing the math (this phi-cycle is an accumulator, this
CFG diamond is a select, this pointer walk is a sequence read) is the
lifter's job, and the lifter is UNTRUSTED (L1): the extraction gate checks
every recovered term against the compiled original, BitwiseNanClass over
μ′, every time. Recognition failures are refusals with reasons, never
silent guesses.

P1 = trivial recognition (straight-line float dataflow).
P2 = the payoff of this mission: loops-as-math (CFG+phi → Fold).
P3 = side-effect slicing around the recovered math.

---

## P1 STATUS: SHIPPED (2026-07-15) — `cli/src/lift.rs`, gated in `cli/tests/lift_p1_test.rs`

`dge lift <file.ll|file.rs> <fn_name>` is the second front door. Hand-rolled
parser (zero new deps) over the P1 alphabet: fneg/fadd/fsub/fmul/fdiv,
ordered fcmp (olt/ogt/ole/oge), scalar select, calls in a 17-symbol closed
libm/intrinsic map, decimal + 0x-bits float literals. One basic block only.
Refusals name the admitting phase: br/phi/switch → P2, memory ops → P3,
unknown calls → closed map, fast-math flags → IEEE claim discipline.

Five extraction gates pass (10⁴ μ′, BitwiseNanClass, vs rustc-compiled
in-process originals): naive cubic; tuples+match+mul_add; a 6-arity
ITERATOR CHAIN; sin/cos/exp/sqrt/abs through the call map; max∘min NaN
semantics. Plus a clang-shaped IR select gate, six refusal pins, and a
hex-constant −0.0 bit-exactness pin. Cross-door agreement (syn vs lift,
10⁴ μ′) pinned on the cubic.

MEASURED findings for P2 planning (this box, rustc 1.97):

1. At `-C opt-level=1`, small pub fns in a lib crate are CROSS-CRATE-INLINE
   deferred — no `define` reaches the IR at all. `#[no_mangle]` forces
   codegen; the `rustc_emit_ir` driver injects it into a temp copy of the
   source (untrusted side; the gate arbitrates the whole transformation).
2. rustc 1.97 lowers f64::min/max to the IEEE 754-2019
   `llvm.minimumnum/maximumnum` intrinsics with an `nsz` call-site flag
   (Rust leaves the zero-result sign unspecified; so does Σ Min/Max, which
   is DEFINED as Rust min/max). Exactly one fmf carve-out exists for this;
   the NaN-PROPAGATING `llvm.minimum/maximum` stay outside the closed map —
   they are the CLIF-fmin semantics O7 refuted.
3. The mission statement's iterator example VINDICATED at P1 already:
   `-O1` fully inlines + unrolls a fixed-window `.iter().zip().map().sum()`
   into straight-line fmul/fadd. The syn door refuses that syntax; the IR
   door lifts it and the gate certifies the recovery. (Runtime-bound loops
   correctly refuse to P2 — pinned against real rustc IR of a `while`.)
4. Mangled-symbol matching must prefer exact hits: closures nested in the
   target embed the same `{len}{name}` segment (`…9iter_dot30E…` is
   closure #0 IN iter_dot3).

## P2 STATUS: SHIPPED (2026-07-15) — `cli/src/lift.rs` mod fold, gated in `cli/tests/lift_p2_test.rs`

Loop recovery is live: the canonical counted loop lifts to Σ Fold. Grammar
(all four variants MEASURED from rustc 1.97 -O1 + --unroll-runtime=false):

    entry [len guard] → (preheader?) → loop [acc phi + index phis +
    gep/load + f64 body] → (LCSSA tail?) → merge [phi, ret]

Recognition is POSITIVE-ONLY (every line individually identified as f64
dataflow / sequence read / integer index machinery, else refuse with a
reason) and the integer world is deliberately UNINTERPRETED: we never prove
the trip count equals the sequence length — μ′ samples random lengths
including 0, and the gate arbitrates, per L1. `t.fold_owners()` (the Σ v1.2
binding validator) runs as a free internal check before any term is
returned.

Nine P2 tests: five gates (slice sum; conditional update w/ hoisted scalar
cap; two-sequence zip-dot via `llvm.umin` trip count; sqrt-of-fold through
an LCSSA tail block, where LLVM constant-folds sqrt(init) into the merge's
entry arm — accepted WITHOUT verification, L=0 in μ′ arbitrates it; EMA
through a LICM preheader), three phased refusals (f64-data-bound `while` —
no runtime LENGTH exists, so no Σ reading; two-accumulator loops; `uitofp`
index-into-body), and cross-door fold agreement (syn Σ v1.2 vs IR, 10⁴ μ′).

MEASURED P2 findings:

5. rustc 1.97's -O1 pipeline RUNTIME-UNROLLS loops by 4 with an epilogue
   loop (7 blocks for a plain slice sum). The driver now passes
   `-C llvm-args=--unroll-runtime=false`, which restores the canonical form
   while leaving compile-time FULL unrolling intact (P1's fixed-window
   iterator chains still arrive straight-line).
6. LLVM materializes Σ's own semantics in two places: LICM's preheader IS
   v1.2 outside-node hoisting (EMA's `1.0 - alpha`), and the zip trip count
   `umin(len_a, len_b)` collapses to L under the parallel-same-length
   contract that eval_with_seqs asserts and μ′ guarantees.
7. `&[f64]` arrives as a (ptr, i64) ABI pair; scalars-after-slices order the
   var indices by double-param position (capped_sum(s, cap): cap = var 0).

## E2E STATUS: SHIPPED (2026-07-15) — the IR door drives the full pipeline

`dge pipeline` now has two front doors. `--lift` forces the IR door; a `.ll`
input implies it; and on a `.rs` input the pipeline AUTOMATICALLY falls back
syn → IR when the syn extractor refuses, reporting both refusals if neither
door admits the function. The core is a library call
(`cli::pipeline::certify`) so the whole chain is integration-testable.

The emission round-trip closure runs through the SYN door regardless of
entry door — emitted code is clean Σ v1.2-shaped Rust, which is exactly the
syn extractor's contract — so the two doors certify each other on every
output. Live, verbatim:

    $ dge pipeline kernel.rs iter_dot3        # syntax the syn door refuses
          syn door refused (Unsupported("expression form ...")); trying the IR door
    [1/4] lifted (LLVM IR) `iter_dot3` (11 nodes, arity 6, 0 seqs)
    [2/4] refactored: cost 11 -> 11 via [add-comm, mul-comm]
    [3/4] emitted `iter_dot3_dge`
    [4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)
    /// CERTIFIED: PROVED semantics-preserving; 2 rule(s) ...
    pub fn iter_dot3_dge(v0: f64, ...) -> f64 { (((v0 * v3) + (v1 * v4)) + (v2 * v5)) }

and a FOLD end-to-end (`dge pipeline ema.rs ema --lift`): the lifted Fold
refactors (sub-to-neg + comm, Tier A, 1 SMT artifact), emits as the loop
form, and the syn door re-reads it in the emission gate. Three e2e tests in
`lift_e2e_test.rs` (iterator-chain via fallback, EMA fold via --lift,
both-doors-refuse honesty), each with an INDEPENDENT third differential
against the in-binary rustc original over fresh μ′. Tests O1-discharge into
their own artifact dirs and skip when z3 is absent (same pattern as
real_code_test); z3 4.8.12 is installed in this environment, so the full
86-test suite runs with nothing skipped.

FIELD TRIAL №1 (2026-07-15): DONE — see docs/FIELD-TRIAL.md. Verdict on this
section's own question: P3 measured at 15/62 fns (24%), concentrated in
genuinely stateful algorithms (sorting, in-place partition) — DEFERRAL
CONFIRMED. The trial priced two cheaper extensions above it: Σ Len(k)
(unlocks the averaging-statistic family; quadratic_mean is a perfect
canonical fold refused only for `uitofp(len)` in its exit block) and
diamond-CFG Select recovery (all 10 easer P2+ refusals are one if/else
shape). Cross-door agreement 14/14 bitwise on real code.

## Σ v1.3 STATUS: SHIPPED (2026-07-15) — `Len(k)`

The trial's item 1, implemented gate-first. `Op::Len` = length of sequence
k as f64: nullary, payload-carrying, position-FREE (unlike Acc/Elem — a
length is loop-invariant by nature, so fold_owners places no constraint).
Layers touched: sig/interp (incl. the eval_traced L=0 convention: absent
seqs read as 0.0), sexpr `(len k)`, structural hash, egg mirror token
`L<idx>`, JIT lowering (`fcvt_from_uint` of the shared-length ABI value —
one value for every k, asserted at the door), emit `(s{k}.len() as f64)`,
syn extractor (`s.len()` on a Seq binding, closing the emission round
trip), and the IR fold recognizer (`uitofp` of a length param; any other
int→float still refuses as index-dependent).

Five tests in len_v13_test.rs: semantics + L=0 (mean of the empty set is
NaN on every layer — the mathematically honest answer), sexpr round trip,
the quadratic-mean IR gate (10⁴ μ′ vs the rustc original, zero-length
exercised), the full pipeline closure (lift mean → certified Rust →
syn re-extraction), and the O7 JIT door (install's internal differential
plus a seq-API spot check).

MEASURED unlock on the trial corpus: statistical 2→6 IR admits, P2+
bucket 4→0 (see FIELD-TRIAL.md addendum).

## Item 2 STATUS: SHIPPED (2026-07-15) — diamond-CFG Select recovery

`lift_acyclic` in cli/src/lift.rs: back-edge-free multi-block CFGs dispatch
to a branch-tree resolver — real topological sort (MEASURED: LLVM prints
blocks in layout order, merges can precede their predecessors), then each
phi resolves recursively from the entry decider: Select(cond, resolve(true
subtree), resolve(false subtree)), with direct merge edges contributing the
phi's incoming value. Nested if/else, which LLVM collapses into ONE n-way
phi (MEASURED), unfolds back for free; else-if chains likewise. Guards:
single exit, tree-shaped upstream (shared arms / sequential merges refuse
with roadmap vocabulary), f64 branch conditions only (icmp = integer data,
no Σ reading). Eager materialization of both arms is sound by totality of
the alphabet.

Five gates in diamond_test.rs incl. cross-door agreement with the syn
extractor's if/else reading (25-node IR term vs 39-node syn term, 10⁴ μ′
bitwise). MEASURED unlock on easer: 18→26 IR admits, P2+ bucket 10→0,
cross-door 18/18. Corpus-wide P2+ is now EMPTY; the bucket it uncovered is
6× rustc-emitted fast-math flags (elastic/expo easings) — the next carve-out
pricing question, in the mold of the nsz-on-min/max precedent.

## Σ v1.4 STATUS: SHIPPED (2026-07-15) — Eq / Ne / Exp2

The fmf bucket was a bucketing BUG (the fcmp refusal message contained
"IEEE"; no fmf tokens exist in any shim IR — confessed in FIELD-TRIAL.md
Addendum 3). The real refusals: `t == 0.0` guards (fcmp oeq) and
powf(2,x)→llvm.exp2. Σ v1.4 adds exactly Rust's operators: Eq=oeq (NaN ⇒
false, -0==+0), Ne=une (NaN ⇒ true) — the asymmetric pair, nothing else
(`one`/`ueq`/… remain refused: no Rust operator means them) — and Exp2
(same libm as llvm.exp2 lowers to, O8). All layers swept; the `c != 0.0`
round-trip special case in the syn extractor is preserved so emit∘extract
stays structurally idempotent. Five gates incl. the real expo/elastic
easer bodies (elastic's guard chain composes item 2's tree resolver with
v1.4's Eq). MEASURED: easer 26→32 of 34 IR admits (94% of the public API).

multi-accumulator folds STATUS: SHIPPED (2026-07-17), as Σ v1.5 FISSION.
No Σ change: N f64 accumulator phis (IR door) / N mutated scalars (syn
door) become N SIBLING Folds over the same 0..L space — fold_owners, the
interpreter, the JIT loop codegen, and the emitter already accepted
sibling folds; only the two recognizers refused. Soundness precondition
checked per accumulator at BOTH doors: its update slice must not read a
co-accumulator (backward SSA slice in lift; Binding::Foreign in extract) —
coupled recurrences (Welford-style) have no fission and refuse with the
roadmap vocabulary. MEASURED merge shapes handled: rustc -O1 sinks
loop-invariant recombination (s*s) into the LCSSA tail, so exit phis MIX
raw next-values and tail values; no-tail exits require entry arms
bit-equal to inits. Ownership discipline: the syn door duplicates
loop-invariant outer scalars per body (dup_subtree — tree-ification:
sharing is space, not semantics); both doors validate fold_owners before
returning, so ill-formed fissions are honest refusals, not gate panics.
Orphan Acc/Elem nodes (dead on their fold's dataflow after sinking) are
tolerated at interp+JIT as 0.0 placeholders, matching fold_owners.
Seven gates in multiacc_test.rs: measured variance shape, no-tail
sum+product cross-door, three moments with a shared scalar, min/max range,
coupled-recurrence refusals at both doors, full pipeline closure, O7 JIT
differential. Honest note: the existing 3-crate corpus's P2+ bucket was
already EMPTY, so fission has zero measured instances there — its value
claim rests on the mean=sum/count family being ubiquitous in the wild,
which the P3 re-quantification below must test.

NEXT: P3 re-quantification on a larger corpus.

P3 re-quantification STATUS: EXECUTED (2026-07-17) — Field trial №2,
6 crates, 335 fns (docs/FIELD-TRIAL.md). Verdict: **P3 stays deferred**
(<4% of the doubled corpus, incl. panic paths); the measured binding
constraint is SHIM SIGNATURE COVERAGE at 82% — method receivers
(average 0.16.0), concrete f32 (simple-easing 1.0.1), trait generics
(keyframe 1.1.1). The trial surfaced and fixed three defects, pinned in
corpus2_findings_test.rs: the syn door silently read f32 as f64
(wrong-precision terms; concrete non-f64 numerics now refuse, generic T
keeps the f64-instantiation reading), `br … !prof` metadata was a parse
error (metadata now dropped), and `unreachable` terminators reported
"no terminator" (now parsed; panic/assert paths refuse with the audit's
totality-guard vocabulary — a parse error had been masking the honest
perimeter answer for statistical's entire variance family). Fission
scorecard on the wild corpus: zero instances; average's multi-accumulator
statistics are Welford-COUPLED behind receivers — the refusal vocabulary
already covers them. Next priced items (measured order): method-receiver
shims, f32 lifting, panic-path refactoring assistance.

receiver flattening STATUS: SHIPPED (2026-07-17, same session as trial
№2) — priced item 1. Syn door reads immutable `&self` methods: receiver
f64 fields flatten to Vars in declaration order; `&mut self` and non-f64
field READS refuse with named honest classes; qualified-path unary calls
(num_traits::Float::sqrt) map to Σ. IR door keeps no receiver shim
(private fields defeat wrapper construction — measured on average) but
buckets receivers separately. Five gates in receiver_methods_test.rs
(incl. 10⁴ bitwise differential vs native getter). Corpus yield: syn
admits 25→30; average's 153-method surface decomposed into honest audit
classes (55 &mut self, 31 non-f64 state reads) — the remaining surface
is STATE-shaped, not signature-shaped. NEXT: item 2, f32 lifting.

f32 lifting STATUS: SHIPPED (2026-07-17, same session) as Σ v1.6 — one
new unary op, Rnd32 = (x as f32) as f64. Foundation: double-rounding
innocuousness (f64 p=53 ≥ 2·24+2, Figueroa 1995) makes f64-compute-then-
Rnd32 BIT-IDENTICAL to native f32 for +,-,*,/,sqrt — so f32 functions
keep REAL bitwise gates. Extractor wraps every rounding op and every
param Var (terms total over raw f64 μ′; rounding inside the term).
Non-innocuous refuses: transcendentals, f32 mul_add, powf, mixed
signatures, f32 seqs. Fixed pre-existing unsoundness: transparent casts
read (x as f32) as f64 as identity; casts are now type-aware. Rnd32 is
egraph-representable, rewritten by no rule. Six gates in f32_test.rs
(3×10⁴ native bitwise differentials, O7 fdemote JIT, emit closure,
refusal pins). Corpus: syn admits 30→40 (simple-easing 0→7, keyframe
2→5) — single-door, theorem-backed (IR float parsing: roadmap).
NEXT: item 3, panic-path refactoring assistance.

SDK / isolation STATUS: SHIPPED (2026-07-17) — crates sdk (R8) and
server (R9). The monolith concern is answered by the architecture's own
invariant, extended outward: because VerifiedTerm is privately minted by
the Gate, extension points can ONLY exist on the untrusted side — so the
plugin surface is exactly the front-door seam (FrontDoor trait: source →
candidate Term or honest Refusal, trial-bucket classified) plus
read-only Observer hooks; the gate stays sealed in core and arbitrates
every plugin's output (malicious-door refutation is a pinned test).
Process isolation: dge-serve exposes the same Engine over HTTP/JSON
(/v1/extract|eval|gate|certify|alphabet|version); verified state never
crosses the wire; remote certificates are explicitly hearsay until
re-gated locally. Integration is documentation-only: docs/SDK.md +
docs/API.md are the whole contract (stable = trait set, sexpr grammar,
additive-only alphabet, API v1 shapes; internals explicitly unstable).

P3 entry point (when its bucket grows): side-effect slicing + re-stitching
— defer until a larger corpus re-quantifies the coverage gap (est. 5–10× P1+P2 effort). Nearer
extensions surfaced by P2 refusal messages, in rough order of value:
multi-accumulator folds (mean = sum+count), last-iteration live-outs,
index-dependent bodies (needs an Idx symbol or affine Elem offsets in Σ),
offset/windowed indexing, non-zero range starts. Each currently refuses
with exactly these words — grep the refusal to find the code path.

---

# Part V — Σ-ext: pluggable extension operators (2026-07-18)

## Addendum 7: Σ v1.7 — extension operators (`Ext1`/`Ext2`), SHIPPED

Motivating question from the person: "I want the ability to integrate
ANY functionality without going into the kernel — in other words, to be
able to do P3 entirely through the SDK." The honest answer landed in two
parts, and only the first is built this session.

**The insight that makes it safe**: the Gate never needed to understand
semantics — it needs to catch lies. Arbitration is black-box (sample,
run both sides, compare bits), and nothing in that loop is specific to
core-Σ ops. So *meaning* can be pluggable while the Gate itself, and
every downstream consumer's soundness argument, stays exactly as sealed
as before.

**What shipped**: two new ops, `Ext1`/`Ext2` (arity 1/2), resolved by
NAME through a runtime registry (`term::ext`, plus `sdk::register_ext_op`
at the integration boundary) rather than by fixed kernel semantics.
Registration is a name + version + a **fingerprint** (the plugin's own
semantic-identity claim) + a `&[f64] -> f64` closure. Terms stay plain
data: `Term.exts: Vec<String>` is a name table; a term referencing
`(ext:relu (var 0))` parses, prints, and hashes with no registry present
— only EVALUATING or GATING it needs the name registered.

**Every consumer's guard, and why each is the correct answer, not a
convenient one**:

* **Gate**: added a determinism PRE-GATE — every sample on an ext-bearing
  term is double-run; a nondeterministic op refutes ITSELF (run 1 vs
  run 2 becomes the counterexample). This was not optional: without it,
  a plugin with hidden state could pass the existing single-run gate by
  accident on some seeds and silently corrupt a certificate on others.
  Unregistered ops get an honest pre-flight `Refused` at the SDK
  boundary (never a panic); the core gate itself panics with the exact
  remedy, on the theory that reaching the core gate with an
  unregistered op is a caller bug the SDK boundary should have already
  prevented.
* **Rules (egraph)**: `Term::has_ext()` guards the ONE entry point
  (`to_egg`, which now asserts rather than silently mis-converting);
  `rules::refactor_with_cost` checks it FIRST and takes an ext-bypass
  path — Tier B identity-gate, no rewriting attempted, `rule_trace:
  ["<ext-term: rewriting skipped>"]`. Not an error: "no rewrite
  available" is a valid outcome, same as it always was for any term the
  saturation loop can't improve.
* **JIT**: `w_ext1`/`w_ext2` Cranelift trampolines take a leaked pointer
  to the SAME `Arc<dyn Fn>` the interpreter dispatches through — one
  semantics, two call sites, still O7-differentialed at install time.
  `lower()` pre-scans `t.exts` and refuses honestly before codegen if
  anything is unregistered (codegen then safely assumes resolution
  succeeds).
* **Emission**: prints a call to the plugin's OWN Rust symbol by name.
  The emitted file only compiles against a crate providing that symbol
  — this is stated, not hidden, in the emit.rs doc comment.
* **Certificate**: gained `ext_semantics: Vec<String>` — tags of the
  form `name@version#fingerprint` for every ext op either side of a gate
  depended on. The emitted comment prints `MODULO extension semantics:
  …` instead of an unqualified `CERTIFIED:` when non-empty. A claim made
  under plugin semantics says so, in the artifact, forever.
* **Structural rebuild passes** (`mutate::ops::apply`, `term::ast`'s
  `copy_subtree`/`dup_subtree`/`graft`): these walk node payload slots
  generically for most ops (child ids) but Ext1/Ext2's payload slots are
  NAME-TABLE INDICES, not children. Every one of these four functions
  got an explicit Ext1/Ext2 arm; the generic arm was audited across the
  whole workspace by chasing every `Op` match-exhaustiveness compiler
  error to completion (the standard "MEASURED, not assumed" discipline
  applied to code paths instead of compiler output this time) — the
  workspace does not compile with a forgotten arm, by construction,
  except for the two silent-corruption sites (structural rebuilds) which
  needed manual auditing since Rust's exhaustiveness check can't see a
  payload-vs-child confusion.
* **Hash**: `structural_hash` hashes the ext op's resolved NAME (not the
  raw table index), so two structurally-equal terms with permuted ext
  tables still hash equal.

**Sexpr syntax**: `(ext:<name> a)` / `(ext:<name> a b)` — the `ext:`
prefix is lexed as part of the head token (the existing lexer already
treats non-paren/whitespace runs as one token, so no lexer change was
needed) and dispatched before the `Op::from_name` lookup.

**SDK surface** (`sdk::register_ext_op`, `crates/sdk/tests/ext_test.rs`,
7 gates): plugin op certifies end-to-end with the certificate naming its
semantics; a lying op is refuted with a counterexample (same mechanism
as a lying `FrontDoor` — registration is identity, not trust); a
nondeterministic op refutes itself; an unregistered op yields the new
`GateReport::Refused` variant, not a panic, at the SDK boundary; the JIT
trampoline is bit-identical to the interpreter (O7); terms round-trip as
plain data with no registry; conflicting re-registration under one name
refuses; ext ops compose freely inside `fold` bodies.

**HTTP surface**: version bumped to v1.7; `/v1/alphabet` now documents
`ext:` syntax and states plainly that wire-side registration does not
exist yet — `dge-serve` can gate/eval terms using ALREADY-registered
(in-process) ops, but cannot accept a new closure over the network this
release. `/v1/gate` and `/v1/certify` gained the `refused` response
shape for unregistered-op requests.

## The other half of the question — honestly not built this session

"Do P3 entirely through the SDK" has two readings, and only the first
is what shipped:

1. *Pure functions with unusual per-call semantics* (a custom
   nonlinearity, a domain transform) — **yes, this is Σ-ext**, fully
   shipped and gated.
2. *True effects* (mutation, multiple outputs, state persisting across
   calls — the corpus's actual P3 population: `average` crate's
   `&mut self` estimators) — **not built**. The design exists (sketched
   in conversation, recorded in README §15 roadmap item 7 and SDK.md
   §6.4): generalize the judged object from `env → f64` to
   `(World, env) → (World′, outputs)`, with a plugin-supplied state
   sampler and canonical serialization so the Gate can still compare
   bitwise on serialized observations. This is deliberately NOT
   attempted this session — it is a larger, riskier change (a new
   sampler-trust boundary, a new certificate-tagging discipline for
   "equivalent modulo this world's sampler," and likely its own
   RFC-review document) and folding it into the same session as the
   simpler, clearly-safe ext-op layer would have risked rushing the part
   that actually touches the soundness perimeter's hardest edge.

**Corpus consequence, stated honestly**: this session's feature does
NOT unlock `average` 0.16.0's `&mut self` wall (55 fns, per the trial №2
histogram) — those are true item-2-shaped P3, not item-1-shaped. Nothing
in the existing field-trial numbers changes; Σ-ext's value is to
integrators bringing their OWN pure semantics, not to widening what the
existing corpus doors admit.

---

# Part VI — Σ-suggest: pluggable optimization hypotheses (2026-07-18)

## Addendum 8: SDK v1.8 — `Suggester` hook + `Engine::optimize`, SHIPPED

Follow-on to Addendum 7 (Σ-ext, same day). That session closed the
"new semantics" gap; this one closes the adjacent "new optimization
knowledge" gap the person identified from the priced roadmap list
(README §15's original item 8) after a long back-and-forth mapping out
exactly what the SDK could and couldn't reach (network calls: wrong fit;
audio instruments: state wall; whole-codebase reasoning: no
multi-function object model; the honest running tally landed on a table
of yes/no per capability). Asked to pick one update to build from that
list, the person chose the Suggester hook.

**The shape**: `sdk::Suggester::suggest(&self, t: &Term) -> Vec<Term>` —
propose zero or more candidate rewrites. `Engine::register_suggester`
adds one; `Engine::optimize(fn_name, &original) -> OptimizeReport` runs
them all and returns certified output plus a full proposal audit trail
(every acceptance AND every rejection, with its reason — same
"refusals are data" discipline as doors and ext ops).

**The one property that had to be right, and very nearly wasn't stated
carefully enough in the first draft**: every candidate is gated against
the ORIGINAL term, never against the running "best." This was a
deliberate design choice, not an accident of implementation — gating
against a running best would let suggester N's acceptance rest on
trusting suggester N-1's acceptance, and a chain of individually-
plausible-looking rewrites could in principle drift meaning across
several accepted hops even though each single hop passed a real gate.
Gating everything against the fixed original closes that off entirely:
there is no transitive trust chain to exploit, by construction. This is
pinned as `a_chain_of_suggestions_cannot_drift_meaning` — a suggester
that would agree with a PRIOR suggester's wrong output, but not with the
original, is refuted exactly the same as any other wrong candidate.

**Cost runs before correctness, and only as a performance courtesy**:
`rules::cost::DefaultCost` (or any `CostFn` — L2 "cost irrelevance": a
cost function only picks a representative among certified-equal terms,
never affects whether something IS equal, so exposing it to the SDK
carries no soundness risk) filters out candidates that aren't strictly
cheaper than the current best BEFORE spending a 10⁴-sample gate run.
This is documented explicitly as non-load-bearing for correctness — a
wrong-but-cheaper candidate still reaches the gate and is still refuted
there; the cost check only saves cycles on candidates that couldn't win
even if they were right.

**Tier discipline unchanged**: suggester-sourced acceptances are always
Tier B. Z3 discharge (Tier A) stays a kernel-only path through the Dec
rule table (`dge discharge`) — this was a deliberate non-extension, not
an oversight: SMT proofs require a fixed, reviewed theory the kernel
owns; letting a plugin claim Tier A would mean trusting the plugin's
proof obligations, which is exactly the authority boundary every other
extension point in this system (FrontDoor, ext ops) refuses to cross.

**Dependency note**: `sdk` gained a direct dependency on `rules` (for
the public `CostFn` trait), not just its existing transitive one through
`cli`. This does not violate the DAG's inversion rule (`rules` still
never depends on `sdk`/`cli`; the new edge points the correct direction)
— documented explicitly in `crates/sdk/Cargo.toml`'s header comment,
alongside the L2 justification for why exposing cost specifically is
soundness-free where exposing anything else in `rules` (the rewrite
engine itself) would not be.

**7 gates in `suggester_test.rs`**: correct cheaper suggestion adopted
and certified; wrong-but-cheaper suggestion reaches the gate and is
refuted, never adopted; the chain-cannot-drift pin (two suggesters, the
second agreeing with the first's WRONG output rather than the original —
both refused); worse-cost candidates skipped before any gate run;
zero suggesters still yields certified output (the unchanged original);
registration-order accumulation across multiple correct candidates;
unregistered ext op inside a suggestion yields `ProposalOutcome::Refused`,
not a panic (mirrors `GateReport::Refused` from Addendum 7). One
correctness note preserved from implementation, not just testing: the
first test-fixture draft used cost-TIED candidates (e.g. `x+x` vs
`2*x`, both cost 3 under `DefaultCost`'s uniform node-count weighting)
and silently exercised the WRONG code path (the cost-skip, not the
gate) — caught by the tests failing, fixed by choosing genuinely
cost-asymmetric fixtures. Recorded here because it's the same lesson as
Addendum 7's NaN-boundary doc bug: even test/doc code for a system built
around bitwise differentials benefits from being checked BY that system,
not just written to describe it.

**Docs updated**: README §11 (new Suggester bullet, tied to the pinned
chain-safety test), §13.1 invariant 8, §12 test table, §15 roadmap item
8 marked shipped (with the Tier A non-inclusion stated explicitly, not
left implicit); SDK.md gained §7 (full guide, parallel in structure and
rigor to §6's ext-op guide) and one boundary bullet in §5; crate map row
for `sdk` updated. HTTP surface (`dge-serve`) explicitly NOT extended
this session — `Suggester` registration, like ext-op registration, is
in-process only; `/v1/alphabet`'s existing "wire-side registration
doesn't exist yet" language was extended to cover suggesters too rather
than left to go stale.

**What this does and doesn't change about the earlier "no" list**: this
closes exactly one item — "can I extend what gets optimized" — and
closes it for real, not partially. It does not touch state (still the
world-gate gap), cross-function reasoning (still no multi-function
object model), or new sampling/emission targets (items 1–3 of the
original v1.8 proposal, still unbuilt). The person's running tally table
from the prior conversation gains exactly one row flipped from "No" to
"Yes, shipped": new optimization rules.

---

# Part VII — Roadmap Phase 1: output & cost openness (2026-07-18)

## Addendum 9: SDK v1.9 — `Emitter` trait + `optimize_with_cost`, SHIPPED

Same day as Addenda 7–8. After Addendum 8 shipped, the person asked for
a full SDK roadmap toward their "unlimited extensibility" vision
(`docs/SDK-ROADMAP.md`, 5 phases + a decision framework, none of it
code — a planning artifact). Immediately after, asked for Phase 1 to be
executed if deliverable in a small fraction of session budget. It was:
two small, independent, zero-new-trust-boundary additions.

**`Emitter` trait + `RustEmitter`**: pluggable output target.
`engine.emit_with(term, cert, &dyn Emitter)` prints a term/certificate
through any implementation; `RustEmitter` wraps the existing
`cli::emit::emit_rust` as the default every `GateReport`/
`OptimizeReport` already used internally. Soundness argument: emission
runs strictly after promotion, so an `Emitter` cannot affect what gets
certified — the worst a broken one can do is mis-print a real
certificate, not manufacture a false one.

**`Engine::optimize_with_cost`**: `Engine::optimize` now delegates to
it with `rules::cost::DefaultCost`; callers can supply any
`rules::CostFn` instead. Soundness argument: unchanged from the
kernel's own pre-existing L2 principle ("cost irrelevance") — a cost
function only selects a representative among already-certified-equal
terms, so exposing it to the SDK carries the same zero risk that
exposing it to `dge calib`/`CalibratedCost` already carried internally.
No new argument was needed here; the existing one just had to be
noticed as applying at the SDK boundary too.

**3 gates in `crates/sdk/tests/phase1_test.rs`**: the trait-based
`RustEmitter` path reproduces the kernel's own emission byte-for-byte
on the parts that matter (found and fixed a test-writing bug along the
way — `"UNCERTIFIED".contains("CERTIFIED")` is true, so the first draft
of the "no certificate" assertion was checking the wrong thing; fixed to
assert the actual `UNCERTIFIED` marker); a trivial custom `Emitter` is
used verbatim, proving the trait dispatches to the caller's
implementation and not a hardcoded path; a custom `CostFn` that inverts
the kernel's default preference (weights `Mul` at 100 instead of 1) is
accepted and the resulting report is still gated exactly like the
default-cost path — cost changed nothing about SOUNDNESS, confirming
the phase's own stated safety argument empirically, not just by
assertion.

**Docs**: README crate-map row, new §11 bullet, test table, workspace
count (147→150); `SDK.md` gained §7a; `SDK-ROADMAP.md` §4 and its
sequencing diagram marked Phase 1 SHIPPED with a pointer to this
addendum — the roadmap document written earlier the same day updated
same-day to reflect that its own first recommended step was already
done, rather than left to go stale.

**Pace note, stated plainly**: three roadmap-scale asks (Σ-ext,
Suggester, and now Phase 1 of the SDK roadmap) shipped in one session,
each with its own test suite, each with its own doc-fidelity check, each
recorded here. The `docs/SDK-ROADMAP.md` phases remaining (2 through 5)
are sized honestly as medium-to-large specifically BECAUSE Phase 1 was
small — the roadmap's own size estimates are not padding, and Phase 4
in particular should not be inferred to be similarly fast just because
Phase 1 was.

---

# Part VIII — Field Trial №3: SDK-era claims, empirically checked (2026-07-18)

Two separate asks arrived together: the workflow was "extremely
difficult... like being a compiler engineer" for a first-time user, and
separately — every claim made about the v1.7/v1.8/v1.9 SDK features
should be backed by an actual field trial, not just a unit test. Both
are addressed here; this section is the second one.

## Addendum 10: `try.sh` — the one-command workflow

The concrete evidence for the first complaint was reproduced directly:
a cold `dge pipeline` run on a function that SHOULD certify returned
`REFUSED: rule mul-one undischarged — run dge discharge first` — a
correct, honest refusal, but exactly the kind of "you must already know
the internal pipeline order" friction that makes a first run feel like
compiler engineering. `dge discharge` itself takes ~2 seconds; the
problem was never speed, it was sequencing knowledge.

`try.sh` (repo root) collapses the documented multi-step sequence
(build → discharge → pipeline → read a raw certificate comment) into
one command with no shortcuts and no weakened guarantees — every real
step still runs, in the same order, against the same binaries; the
script only removes the requirement to already know that order:

* `./try.sh` with no arguments: checks prerequisites (Rust, z3) with
  plain-language install hints, builds once (cached after), discharges
  once (cached after), then runs TWO bundled examples
  (`examples/demo.rs`: one that certifies, one that's honestly refused)
  and prints a plain-English summary for each, translating the exact
  refusal reason rather than hiding it.
* `./try.sh myfile.rs my_fn`: the same, on the caller's own function.

Live-verified cold (rm -rf'd `target/` and `artifacts/` first) and warm
(second run skips both the build and discharge steps, near-instant) —
both paths tested directly, not assumed. The certified path prints
"✅ CERTIFIED" plus a one-sentence explanation of what 10⁴-sample
bitwise equivalence actually means; the refused path prints
"◯ NOT CERTIFIED — and that's a real, honest answer, not an error"
followed by the exact refusal text, because the project's own
discipline (refusals are data, never hidden) applies to the friendly
wrapper too, not just the raw CLI.

## Addendum 11: Field Trial №3 — SDK-era claims, checked at scale

Four campaigns, run for real in this session, numbers below are actual
tool output, not projected:

### Campaign A — corpus re-validation

Re-ran `dge trial` on all six corpus crates (easer, statistical,
interpolation, average, keyframe, simple-easing) fresh. Every syn/IR
admit count and cross-door agreement number reproduced **exactly**
README §14's table, bit-for-bit — confirming the table is still
accurate after every SDK-era change (v1.7–v1.9 touched only the
extension surface; corpus extraction was never expected to move, and
measurement confirms it didn't).

### Campaign B — the Rnd32 double-rounding theorem, at scale

README §5.4 states the theorem (f64-compute-then-Rnd32 ≡ native f32 for
`+,-,*,/,sqrt`) citing Figueroa 1995. `crates/term/examples/
fieldtrial_rnd32.rs` checks it empirically, through the ACTUAL Σ
interpreter (not a reimplementation), with an adversarial sampler
(45% boundary/subnormal/extreme values, 55% uniform bit patterns):

```
  add [PASS] 2000000 samples, 0 mismatches
  sub [PASS] 2000000 samples, 0 mismatches
  mul [PASS] 2000000 samples, 0 mismatches
  div [PASS] 2000000 samples, 0 mismatches
 sqrt [PASS] 2000000 samples, 0 mismatches

10000000 total samples across 5 ops, 0 mismatches.
```

Ten million samples, zero counterexamples, through the real kernel path.
The theorem's prior evidence was 3×10⁴ samples across three pinned unit
tests (`f32_test.rs`); this is over 300× that, still zero mismatches.

### Campaign C — ext-op determinism pre-gate, catch rate vs theory

The claim ("the Gate double-runs every sample; a nondeterministic op
refutes itself") was previously proven to CAN-happen once
(`ext_test.rs`). `crates/sdk/examples/fieldtrial_ext_determinism.rs`
measures the actual catch RATE across 300 independent `Gate::promote`
calls at five flip probabilities, against the naive single-comparison
theoretical prediction 1-(1-p)ⁿ:

```
 flip_rate     measured       theory        gap
    0.5000       1.0000       1.0000     0.0000
    0.1000       1.0000       1.0000     0.0000
    0.0100       1.0000       1.0000     0.0000
    0.0010       1.0000       1.0000     0.0000
    0.0001       0.8633       0.6321     0.2312

Zero-false-positive check (p=0, 500 trials): 0/500
```

**Honest, unsmoothed finding**: at the lowest flip rate, MEASURED catch
rate (86.3%) exceeds the naive theoretical formula (63.2%) by a wide
margin — not a discrepancy to explain away, but a real property of this
specific test's shape. The trial identity-gates a flaky term against
itself, which means the Gate's own per-sample logic (candidate
double-run, THEN — if that passed — reference double-run, THEN the
ordinary candidate-vs-reference correctness comparison) gives a flaky
op *multiple independent chances per sample* to be caught, not one. A
precise closed-form derivation of the multi-channel rate is left as
future work; the number that matters operationally is the measured
one, and it errs in the safe direction (stronger protection than the
simple formula suggests, not weaker). The zero-false-positive result
(500/500 clean on a genuinely deterministic op) is the other half of
the claim and holds exactly as stated.

### Campaign D — Suggester chain-drift, fuzzed

`suggester_test.rs`'s `a_chain_of_suggestions_cannot_drift_meaning`
proves the "gate every candidate against the ORIGINAL, never the
running best" rule on one hand-picked case.
`crates/sdk/examples/fieldtrial_suggester_fuzz.rs` fuzzes it: random
small arithmetic terms, two chained suggesters that always propose a
cheap, usually-wrong candidate, checked across every round.

**First attempt was a null result, reported honestly rather than
discarded**: the first design proposed `(neg t)` as the wrong
candidate, but `(neg t)` always costs exactly one more node than `t`,
so the SDK's own cost pre-filter rejected it every time — 1,000
candidates generated, 0 ever reached the actual Gate. A fuzz campaign
that never exercises the path under test proves nothing; this was
caught and fixed, not silently swapped for a flattering number. The
corrected design proposes `(var 0)` (cost 1, clears the cost filter
against nearly any generated term):

```
rounds run:                          2000
`(var 0)` candidates generated:       2858
  rejected by cost gate (cheap path): 0
  reached the Gate and were refuted:  2858
  ACCEPTED (must be 0):                0
```

2,858 genuinely wrong candidates reached the real Gate across 2,000
independent random terms; zero were ever accepted.

**A second, more valuable finding came from this same campaign**: early
runs crashed with `assertion 'left == right' failed: arity mismatch at
gate` — `Gate::promote` asserts (panics) on arity mismatch, a
reasonable contract for kernel callers that pre-validate, but NOT a
safe contract for the SDK boundary, where a careless or malicious
`FrontDoor`/`Suggester` proposing a wrong-arity term should never be
able to crash the host process. This directly contradicted the "an
ext op only refuted, never panics" promise Addendum 7 established —
the promise existed for ext-op registration but not for arity. **Fixed
in this session**: both `Engine::gate` and `Engine::optimize_with_cost`
now pre-check arity and return `GateReport::Refused` /
`ProposalOutcome::Refused` instead of reaching the kernel's assert.
Two regression tests added (`mismatched_arity_is_refused_not_a_panic`,
`suggester_proposing_wrong_arity_is_refused_not_a_panic`,
`crates/sdk/tests/phase1_test.rs`) — the fuzz campaign that found this
now runs clean because the bug it found is fixed, not because the
campaign was narrowed to avoid it.

### What this field trial actually demonstrates

Not just "the claims are true" — that was already true from unit tests.
The value here is the same the project has stated since Field Trial №1:
a field trial's job is to find real things, including a real bug, and a
campaign that never found anything wrong should be trusted less than
one that found something and got it fixed. This one found one real bug
(arity panic, fixed) and one real surprising-but-benign fact (the
determinism catch rate is stronger than the naive formula, not weaker)
— both are now part of the documented record instead of assumed away.

---

# Part IX — Concurrency bug: IR-door temp-file collision (2026-07-19)

## Addendum 12: cross-contamination under concurrent IR-door calls, FOUND AND FIXED

Found through a live user bug report, not a planned trial — the most
valuable kind of finding, and reported here in full because of where it
reached: production code, including the HTTP server.

**Symptom, as reported**: `cargo test --workspace` on a different
machine (Windows, same rustc version verified) intermittently failed
one test in `diamond_test.rs` with a bare `FnNotFound`. The diagnostic
improvement from the same session (Addendum 11-adjacent — added
specifically because this failure couldn't be explained blind) turned
the NEXT occurrence into a self-diagnosing message:

```
diamond_gate_ease_in_out ... FAILED
lift: no `define` matching `ease_in_out` -- rustc emitted 1 other
function(s) instead: ["nested"]
```

**Root cause, confirmed by code inspection**: `cli::lift::rustc_emit_ir`
computed its working directory as `temp_dir()/dge_lift_{pid}` — process
ID only. Cargo's default test runner executes every test in a binary as
a THREAD within one process, so `std::process::id()` is identical across
concurrently-running tests. Two tests (`nested_diamond_gate` and
`diamond_gate_ease_in_out`) calling `rustc_emit_ir` at overlapping times
raced on the exact same `lift_input.rs`/`lift_input.ll` pair; the loser
silently read back whichever function the winner had compiled.
Reproduced synthetically (`cargo test -p cli --test
concurrency_regress_test` against the pre-fix code: 24 concurrent
threads compiling 24 distinct functions, cross-contamination confirmed
directly — not inferred).

**Why this matters beyond test flakiness**: the identical pattern
(`temp_dir().join(format!("..._{}",", std::process::id()))`) existed at
THREE production sites:

1. `cli::lift::rustc_emit_ir` — the universal bottleneck; every IR-door
   caller funnels through this one function.
2. `cli::trial::trial_crate`'s work directory (`dge_trial_{pkg}_{pid}`)
   — lower risk (package name included) but the same class of bug for
   concurrent trials of the same package.
3. **`sdk::IrDoor::extract`** — the code path `dge-serve` uses for
   `/v1/extract`, `/v1/gate`, `/v1/certify` when the IR door is
   selected. This is the serious one: **two simultaneous HTTP requests
   to a running `dge-serve` instance could silently receive each
   other's compiled results** if their timing overlapped — a real
   correctness bug in a shipped, documented, network-facing surface,
   not a theoretical one.

**Fix**: a new `cli::lift::unique_tmp_dir(prefix)` helper combines the
process ID with a process-wide atomic counter, guaranteeing every CALL
gets its own directory regardless of thread or request concurrency.
Applied to all three sites. `sdk::IrDoor` needed the fix at two layers
— its own outer directory (writing the request's source) and, via the
now-fixed `rustc_emit_ir`, the inner compilation step — because two
concurrent requests for the SAME function name would otherwise collide
at the outer layer even before reaching the inner one.

**Regression test**: `crates/cli/tests/concurrency_regress_test.rs`
spawns 24 real OS threads, each compiling and lifting a DISTINCT
function (distinguished by an embedded constant so cross-contamination
shows up as a wrong VALUE, not just a different-looking error),
asserting every result matches its own function. Verified this test
actually catches the bug before claiming the fix: reverted
`rustc_emit_ir` to the old PID-only path, confirmed the test fails
with an explicit `CROSS-CONTAMINATION` message, then restored the fix
and confirmed it passes — the same "prove the test can fail before
trusting that it passes" discipline used throughout this project's
testing. Post-fix: 10 repeated full runs of `diamond_test.rs`, clean
every time.

**Scope note, stated honestly**: 34 occurrences of the same PID-only
temp-dir pattern exist across 17 test files, all pre-dating this
session (R1–R7 era). These were NOT individually audited or patched —
each uses a test-specific tag/name in its directory string, which in
practice differentiates them from EACH OTHER (the collision that
mattered was inside the shared `rustc_emit_ir` bottleneck they all
call into, which is now fixed). If any of those 34 sites turns out to
have its own independent collision (two tests coincidentally sharing a
tag), that would be a separate, smaller-blast-radius bug than the one
fixed here, and is not currently known to exist — flagged for future
attention rather than assumed clean.

**The general lesson, worth stating plainly**: this bug was reachable
from the very first field trial run of this project and shipped
through Σ v1.4 - v1.9 undetected, because single-threaded/serial usage
never exercises it. It took a real user running the real test suite on
real (parallel, multi-core) hardware to surface it. That is itself an
argument for the "run it for real, on a machine you don't control"
value the person has been pushing for this whole session — a sandbox
that always runs tests the same serialized way cannot find this class
of bug no matter how many field trials it runs internally.

---

## `dge optimize` — the whole-codebase, in-place front door

Motivation: the per-function loop (discharge → pipeline → hand-copy the
certificate comment back into the source, once per function) was the real
UX cost of the engine. The ask was to make the workflow easy at scale:
one command that optimizes an entire codebase and rewrites the very same
files in place.

`dge optimize <path>` does exactly that over a file or a directory tree.
It is a pure ORCHESTRATION layer — it proves nothing itself. It reuses
`audit::audit_dir` for codebase-wide function discovery and
`pipeline::certify` for the actual certificate, then rewrites each
certifiable free `f64` function in place, attaching the certificate as a
doc comment and preserving the original signature, parameter names, human
doc comments, and attributes (`#[no_mangle]`, etc.). Everything else is
left untouched with the honest refusal reason printed. The proof table is
auto-discharged on first run (no manual `dge discharge` step); `.bak`
backups are kept by default; `--dry-run` previews; `--all` includes the
audit's with-effort class.

The safety argument is the same one the rest of the engine already makes,
reused verbatim. On top of `certify`'s own emission gate, the in-place
editor adds one more before writing a single byte: it builds the rewritten
source, re-extracts the function *in file context*, and runs the existing
`emission_round_trip` closure to prove the rewritten source is bitwise-equal
to the certified term over 10^4 μ′ samples. If that fails, the edit is
refused and reported as "certified but not in-place" — the original is left
alone. So the invariant holds unchanged:

> **nothing is written that does not re-extract to its own certificate.**

That gate earned its place immediately: the first sequence/fold rewrite
(`&[f64]` params) was caught by it — the initial shim lowering
(`let s0 = s;`) did not survive re-extraction ("sequence used as a scalar"),
so it was refused rather than written. The fix was to remap the emitted
`vK`/`sK` slots back onto the original parameter names directly in the body
(CSE temporaries `tN` and fold internals `__acc`/`__i` left untouched),
which round-trips for scalars and sequences alike; the gate stayed as the
backstop either way.

Implementation notes / scope (v1): the in-place edit covers free functions
whose parameters are all scalar `f64`/`f32` (arity ≤ 8) and/or `&[f64]`
sequences — the audit's EXTRACT class. A function that certifies but whose
shape can't be shimmed in place yet (large-arity array params, impl methods)
is reported CERTIFIED-not-in-place and left for `dge pipeline`; it is never
silently dropped. Re-running `optimize` is idempotent up to a single
certified-equivalent commutation from the refactor search (it converges to a
byte-stable fixed point within two passes); the generated preamble is
stripped and re-added rather than stacked, so certificates never accumulate.

Verified: a bit-exact differential of the rewritten output vs the pristine
originals (poly, dist2, mean) over 410,210 comparisons — including 0, ±0,
±∞, NaN, MIN/MAX, and subnormals — was bit-identical. `optimize_test`
exercises the dry-run and write paths, signature/doc/attribute preservation,
the honest refusal, idempotence, and an independent 10^4-sample differential
of the rewritten function against the original. Full workspace suite green.

Changes were additive: `PipelineOpts` gained a `quiet` flag (so a
whole-codebase run doesn't drown its own summary) and `Certified` gained the
proven `term` (so the in-place layer can re-verify against it without
re-deriving). No existing call site or test changed behavior.

---

## `dge optimize` — in-place scope extended to methods, arrays, and large arity

The first cut of the in-place editor covered free functions with scalar
`f64`/`f32` params (arity ≤ 8) and/or `&[f64]` sequences, and reported
everything else as "certified but not in-place." That "everything else" is
now handled directly. Three shapes were added:

  * **impl methods with a `&self`/`self` receiver** — the receiver's struct
    (defined in the same file) flattens to var slots exactly the way the
    extractor does it: each f64 field, in declaration order, becomes a slot,
    and the rewritten body reads it back as `self.<field>`. `&mut self`
    stays refused (effectful), and a receiver whose struct isn't in the file
    is refused honestly (the extractor can't know the field layout).
  * **fixed-size array params** `&[f64; N]` / `[f64; N]` — each element is a
    slot, mapped back to `arr[j]`.
  * **large arity (> 8)** — the emitter switches to a single `v: &[f64; N]`
    array parameter with `v[k]` indexing at that size; the in-place editor
    reads those `v[k]` back onto the original params (scalars by name, array
    elements as `arr[j]`), so the original signature is preserved unchanged.

The unifying move was to stop thinking in terms of "scalars and seqs" and
instead build one **slot → original-expression** table per function that
mirrors the extractor's slot assignment precisely (receiver f64 fields
first in declaration order, then params in source order; scalar = 1 slot,
`[f64; N]` = N slots, `&[f64]` = a separate seq slot). Body rewriting then
substitutes emitted slot references (`v{k}`, or `v[k]` in array shape, and
`s{j}` for sequences) with that table's entries; CSE temporaries `tN` and
fold internals `__acc`/`__i` are left alone. Because the mapping is derived
from the same rule the extractor uses, and because the re-extraction gate
still runs before any write, a wrong mapping can only ever cause a refusal,
never a bad rewrite.

Output stays tidy: generated cert-doc/attr lines and the emitted body are
re-indented to the item's own indentation (a no-op for top-level items, so
their output is byte-for-byte what it was before), with the copied signature
prefix de-indented one level first so repeated runs don't compound. A subtle
idempotence bug specific to methods was fixed here: after stripping a prior
generated preamble the surviving first line could be a former continuation
line still carrying the method's indent, which then re-indented on every
pass; the stripped prefix is now left-trimmed so methods reach a byte-stable
fixed point like free functions do.

Verified: bit-exact differential of the rewritten output vs the pristine
originals across all the new shapes — a `&self` getter, a `&self` method
with an extra scalar, a 10-argument function (array-shape emission), and an
`&[f64; 4]` array-param function — **405,502 comparisons, all bit-identical**
(0, ±0, ±∞, NaN, MIN/MAX, subnormals included). `optimize_test` gained a
second case exercising all four shapes, signature preservation, an
independent 10^4-sample differential per function, and byte-stable
idempotence. Refusal paths checked by hand: `&mut self` and an out-of-file
receiver struct are both declined without touching the source. Full
workspace suite green; zero warnings. Changes remained confined to the
`optimize` module — no edits to the extractor, emitter, or pipeline.

---

## `dge optimize --jobs` — parallel across files

`optimize` processes a codebase file by file, and with the rule table already
discharged those files are fully independent: `certify` runs here with
`lift = false`, so it spawns no subprocess and only reads the input file and
the artifacts directory read-only before doing its refactor/emit/gate work in
memory, and each file writes only its own output and `.bak`. That made
file-level parallelism a small, safe change.

`--jobs <n>` (or `--jobs auto` for `available_parallelism`) fans the candidate
files out across `n` scoped threads sharing an atomic work cursor; each thread
pulls the next file index, processes it, and the per-file results are
reassembled in candidate order before merging into the summary. Because the
merge is order-independent of completion time, the output — every rewritten
byte and every line of the summary — is identical to the sequential run for
any worker count. `--jobs 1` (the default) is exactly the old path.

The safety argument didn't move: each worker still runs the full
re-extraction gate before writing, so parallelism cannot introduce an
uncertified rewrite; the worst a scheduling quirk could do is reorder work,
which the deterministic reassembly erases.

Verified: on a 40-file / 120-function tree, `--jobs 1`, `4`, and `8` produce
byte-identical output; five consecutive `--jobs 16` runs each match the
sequential baseline exactly (this box has a single core, so preemptive
scheduling still interleaves the workers — a genuine race would surface as a
mismatch, and none did). A new `optimize_is_deterministic_across_jobs` test
optimizes two identical trees at `jobs = 1` and `jobs = 4` and asserts equal
rewrite/refuse counts and byte-identical files. Full workspace suite green;
zero warnings. Wall-clock speedup isn't observable in this single-core
sandbox, but the work is pure CPU per file and scales with cores on real
hardware. Changes stayed within the `optimize` module.

---

## SDK plugins — optimization features on the Suggester hook (no core edits)

New capability is moving to the SDK so the eleven core crates stay small. The
SDK is the sanctioned place for this: its `Suggester` hook lets a plugin
propose candidate rewrites while the sealed Gate arbitrates every one against
the ORIGINAL over 10^4 μ′ samples and the cost function drops the ones that
don't actually get cheaper. A plugin therefore needs no trust — a wrong
rewrite is refuted, a pointless one is discarded — which is exactly why the
boundary exists. `crates/sdk/src/plugins.rs` is the first library of these.

Two suggesters, both deliberately BIT-EXACT (they preserve the IEEE-754
result, not merely the real value, because the Gate is bitwise):

  * **`Peephole`** — constant folding plus identity elimination, iterated to a
    fixed point. Folding reuses the REAL interpreter (it evaluates a throwaway
    `op(Const..)` term), so a folded constant is exactly what the compiled
    kernel would have produced — Fma's single rounding, Pow via libm, Rnd32's
    f32 round-trip all handled without re-implementing any of them. The
    identities are only the ones exact for every f64 including −0.0/NaN/∞:
    `x*1`, `1*x`, `x/1`, and `neg(neg x)`. Notably NOT `x+0.0`, which is
    −0.0-unsafe — the Gate would refute it and we don't bother proposing it.
    Wins under the default (node-count) cost.
  * **`StrengthReduce`** — `x / 2^k → x * 2^-k`, firing only on exact powers of
    two where the two forms are the same correctly-rounded scaling. Under the
    default cost (`Div` and `Mul` priced equally) this is correctly a no-op;
    under a cost that prices division higher — as real hardware does — the
    same rewrite is accepted. That contrast is a live demonstration of L2:
    the cost function chooses among certified-equal terms, it never decides
    whether something is certified.

Terms containing external ops (`Ext1`/`Ext2`) are left untouched (their value
can be caller-defined and their payload indices aren't worth reconstructing
here) — a missed optimization, never a wrong one.

Verified. `plugins_test.rs` (6 tests) drives both plugins through the real
`Engine::optimize`: folding/identity acceptance with a certified emission, the
neg-chain fixed point, the no-op-on-minimal case, strength reduction being
bit-exact and cost-gated (no-op under default cost, accepted under a div-heavy
cost, both certified), non-power-of-two divisors ignored, and a SOUNDNESS PIN
— a deliberately lying suggester whose cheaper-but-wrong candidate reaches the
Gate is REFUTED, never emitted, and the original ships unchanged. Each test
also re-checks bit-exactness independently of the Gate over 10^4 μ′ samples.
A new field trial, `fieldtrial_peephole_fuzz`, fuzzed 2000 random arithmetic
terms: the plugin fired on 1238 of them (6154 nodes removed) and every single
rewrite was non-growing and bitwise-identical across ±0, ±∞, NaN, subnormals
and 2000 random samples apiece. Full workspace suite green; zero warnings. Not
one line of any core crate changed — the whole feature lives behind the SDK's
existing hooks.
