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
| cli | R7 | **ALL SUBCOMMANDS LIVE** — audit / discharge / refactor (auto-loads the calibrated cost table) / gentest / debug / calib. `gentest`: mutation-adequate suite growth under μ' with T3-shrunk pinned envs, SMT eq-filter on survivors, golden-suite emitter, (MS, n, α, δ_min) adequacy report. `debug`: CE hunt → T3 shrink → spectrum/Ochiai → optional gate-certified repair; prints the quantified no-CE claim when nothing is found. `calib`: per-op cost table from JITTED bounded-input sum kernels (pow 19×, tan 11×, sin/cos 8×, exp/ln 6× vs add=1 on this box) + the §6 perf-targets report with MEASURED values (jit 2.5×/≥5× MISS, memo 83 ns/≤50 ns MISS, SR 4 gen/≤40 PASS — misses block perf sign-off only). Audit **calibrated on real crates**: syn-based O5 classifier — 3 classes with per-reason diagnostics; transitive demotion over local calls (fixed point); impl-block + inline-mod recursion with impl-scoped generics; test code excluded from workload; panic paths (`assert!`, `.unwrap()`) = effort (totality guards), generic-numeric params = effort (monomorphize-then-extract); LOC-weighted `s_strict`/`s_loose`, §9 verdict. Other subcommands still skeleton. |

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
