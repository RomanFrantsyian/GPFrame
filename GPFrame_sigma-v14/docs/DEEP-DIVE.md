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
