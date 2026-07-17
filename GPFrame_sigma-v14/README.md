# deductive-gp-engine (`dge`)

**Refactor, test, debug, and JIT your pure numeric Rust/C code — and get a machine-checked certificate for every change, or an explicit refusal with a counterexample.**

[![ci](../../actions/workflows/ci.yml/badge.svg)](../../actions/workflows/ci.yml)
[![deep-verify](../../actions/workflows/deep-verify.yml/badge.svg)](../../actions/workflows/deep-verify.yml)
![rust](https://img.shields.io/badge/rust-stable-orange)
![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)

Most optimizers ask you to trust them. `dge` inverts that: **nothing ships without a certificate**. Every transformation either carries a per-rule SMT proof (Tier A), a quantified statistical-equivalence claim over a recorded input distribution (Tier B: *n*, *α*, *δ_min*), or it is refused — with a concrete witness input showing exactly where it would have broken your numbers. "Probably fine" is not a value in this system.

---

## What do I need it for?

You have `f64` math on a hot path — filters, easings, polynomials, dot products, running statistics — and you want one of these, *without introducing a floating-point bug you'll chase for a week*:

| You want to… | Command | What you get |
|---|---|---|
| **Optimize a function** | `dge pipeline file.rs my_fn` | Rewritten Rust with the proof/claim attached as a doc comment; or a refusal with the input that breaks the rewrite |
| **Optimize when bit-exactness is impossible** (Horner, FMA fusion) | `dge pipeline file.rs my_fn --eps --domain 1e100` | An ε-equivalence certificate stating *exactly* which domain the claim holds over |
| **Generate a test suite that actually kills bugs** | `dge gentest fn.sexpr` | A mutation-adequate golden suite: shrunk pinned inputs + an adequacy report (MS, n, α, δ_min), equivalent mutants filtered out by SMT |
| **Debug a numeric regression** | `dge debug broken.sexpr oracle.sexpr --repair` | A minimal counterexample (shrunk), an Ochiai fault-localization ranking, and optionally a gate-certified repair — or an honest "no fix within budget" |
| **JIT-compile safely** | library: `jit::install` | Cranelift-compiled code that is differentially gated against the reference interpreter *at install time*; any mismatch pins execution to the interpreter forever |
| **Find out if any of this applies to your codebase** | `dge audit src/` | An honest go/no-go score of how much of your code is inside the engine's perimeter — including "DO NOT BUILD" (it says that about its own source) |

**The perimeter is strict by design:** pure, total, `f64` functions. No effects, no I/O, no concurrency. Run `dge audit` first — it exists precisely to tell you whether you have a workload before you invest an hour.

---

## 60-second demo

You wrote a naive polynomial:

```rust
fn inefficient_polynomial(x: f64) -> f64 {
    let term3 = 3.0 * x * x * x;
    let term2 = 5.0 * x * x;
    let term1 = 2.0 * x;
    term3 + term2 + term1 + 7.0
}
```

Watch the engine refuse, refute, and then certify:

```console
$ dge refactor poly.sexpr             # Tier A: bitwise-sound rules only
cost  : 19 -> 19                      # CORRECT refusal: no bitwise-sound rule
                                      # may reassociate f64 addition

$ dge refactor poly.sexpr --eps       # allow ~ε rules, Tier B gate mandatory
REFUTED at x ≈ -1.09e154              # Horner is NOT ε-equivalent over all of
                                      # f64: naive → -inf+inf = NaN, factored
                                      # → -inf. Witness attached.

$ dge pipeline poly.rs inefficient_polynomial --eps --domain 1e100
[1/4] extracted `inefficient_polynomial` (14 nodes, arity 1)
[2/4] refactored: cost 19 -> 10 via [… mul-factor~, fma-contract~ …]
[3/4] emitted `inefficient_polynomial_dge`
[4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)

/// CERTIFIED: equivalent at confidence 0.999 over mu' … DOMAIN |x|<=1.0e100 (A-1);
///            defect regions of measure < 6.9e-4 are invisible (n=10000)
pub fn inefficient_polynomial_dge(v0: f64) -> f64 {
    v0.mul_add(v0.mul_add(3.0_f64.mul_add(v0, 5.0_f64), 2.0_f64), 7.0_f64)
}
```

Three behaviors to notice: it **refuses** what it cannot prove, it **refutes with a concrete witness** instead of silently drifting, and when it accepts, the certificate states **exactly where the claim holds** — and travels with the emitted code. Hand-editing the output voids the certificate, and the header says so.

## Your syntax doesn't matter — the math does

`dge` has **two front doors**, and they certify each other:

1. **syn door** (`dge extract`) — parses clean Rust source directly.
2. **IR door** (`dge lift`) — reads the LLVM IR your compiler already produced (`rustc --emit=llvm-ir` / `clang -emit-llvm`). Syntax is a costume; SSA dataflow is where every costume comes off. One lifter covers Rust *and* C/C++.

So this iterator chain — which no source-level parser wants to touch — just works:

```console
$ dge pipeline kernel.rs iter_dot3
      syn door refused (Unsupported("expression form …")); trying the IR door
[1/4] lifted (LLVM IR) `iter_dot3` (11 nodes, arity 6, 0 seqs)
[2/4] refactored: cost 11 -> 11 via [add-comm, mul-comm]
[3/4] emitted `iter_dot3_dge`
[4/4] emission gate PASSED (emit∘extract ≡ id, 10^4 mu' samples)
/// CERTIFIED: PROVED semantics-preserving; 2 rule(s)
pub fn iter_dot3_dge(v0: f64, v1: f64, v2: f64, v3: f64, v4: f64, v5: f64) -> f64 {
    (((v0 * v3) + (v1 * v4)) + (v2 * v5))
}
```

Runtime-length loops are recurrences, and the engine recovers them as its `fold` operator:

```console
$ dge lift ema.rs ema
lifted `ema` from IR: 10 nodes, arity 1   [UNTRUSTED — run the extraction gate]
(fold 0.0 (+ (* (- 1.0 (var 0)) acc) (* (var 0) (elem 0))))
```

Crucially, **the lifter itself is untrusted**. Its recognition can be wrong in any way whatsoever and correctness does not move: every lifted term must pass the extraction gate — a bitwise differential against your compiler's own output over 10⁴ boundary-heavy samples (NaN, ±0, ±inf, subnormals, zero-length sequences).

## How the trust model works

```
Rust/C fn ──extract|lift──▶ Term ──Gate::promote──▶ VerifiedTerm ──install──▶ JitFn
               ▲                  (the ONLY constructor)      (the ONLY constructor)
               └── front doors are untrusted lowerings, gated bitwise against
                   the compiled original before anything downstream sees them
```

- `VerifiedTerm` and `JitFn` have **no public constructors**. You cannot hold a certified artifact you didn't earn — the type system *is* the audit trail, and the compiler enforces it forever.
- The trusted base is a one-screen definitional interpreter over a 32-op signature. Everything else — the e-graph rewriter (egg), the JIT (cranelift), the GP repair search, both extractors — is refutable, and is refuted *against the interpreter*.
- Rewrite rules enter the active set only with a Z3 proof artifact on disk (`dge discharge`). No artifact ⇒ the refactorer refuses to run.
- Certificates always record the environment fingerprint: runtime CPU features + a *behavioral* libm hash (transcendental output bits at fixed probes — identifies the linked libm even when version strings lie).

This machinery has refuted its own spec, its own shrinker, its own optimizer, and its compiler backend — each finding pinned as a permanent test (e.g.: `x + 0.0 → x` is *unsound* over f64, witness `−0.0`; Cranelift's `fmin` propagates NaN where Rust's `min` doesn't). The full findings table is in [`docs/DEEP-DIVE.md`](docs/DEEP-DIVE.md). A verification system that has never refuted anything is decoration.

## Install & quickstart

Requirements: **Rust stable**; **z3** on PATH for rule discharge (without it, SMT-dependent steps degrade to explicit "Unknown → triage", never to silent acceptance).

```console
$ git clone <this repo> && cd deductive-gp-engine
$ sudo apt-get install z3            # or brew install z3
$ cargo test --workspace             # 103 tests, all gates live
$ cargo install --path crates/cli    # installs the `dge` binary

$ dge audit path/to/your/src         # step 0: is there a workload here?
$ dge discharge                      # step 1: Z3-prove the rule set, once
$ dge pipeline yourfile.rs your_fn   # the whole loop: Rust in, certified Rust out
```

All subcommands: `pipeline`, `audit`, `trial`, `extract`, `lift`, `emit`, `discharge`, `refactor`, `gentest`, `debug`, `calib`. Run `dge` bare for usage.

## CI/CD

**This repo's own pipeline** ([`.github/workflows/`](.github/workflows)) applies the engine's discipline to the engine:

- [`ci.yml`](.github/workflows/ci.yml) — on every push/PR: workspace build + the full 86-test suite with z3 installed, **plus a step that fails the build if any z3-dependent test silently skipped**. Clippy runs informationally; `rustfmt` is deliberately not gated (the source carries intentional notation).
- [`deep-verify.yml`](.github/workflows/deep-verify.yml) — weekly + on demand: the suite in `--release` (catches debug-only masking), and `cargo-mutants` over the workspace — the engine's own mutation-testing discipline pointed at its own test suite.
- [`release.yml`](.github/workflows/release.yml) — on a `v*` tag: re-run every gate (a release is a claim; claims pass gates first), then build and attach `dge` binaries for Linux and macOS.

**Using `dge` in *your* CI** — the gates are ordinary exit codes, so they compose as regression checks:

```yaml
# .github/workflows/numeric-gates.yml (in your project)
- run: sudo apt-get install -y z3
- run: cargo install --git <this repo> cli --bin dge
- run: dge audit src/                          # tracks extractable share over time
- run: dge discharge                           # rule proofs, cached as artifacts
- run: dge pipeline src/kernels.rs hot_kernel --eps --domain 1e6 --out certified.rs
- run: git diff --exit-code certified.rs       # certified output must not drift
```

`dge gentest` output is a plain Rust test file of shrunk golden cases — commit it, and your ordinary `cargo test` becomes mutation-adequate for that function.

## Honest status

Validated end-to-end on real published code (`easer` 0.3.0's cubic easing family) and on the kernels above: `rustc(original) == interp(extracted) == cranelift-jit(refactored)` bitwise across 10⁴ boundary-heavy samples each. Not a finished product:

- **Scope**: pure, total, numeric functions only — the audit measures whether that's enough of your codebase to matter.
- **Coverage**: the syn door handles arithmetic, math methods, branches, fixed arrays, literal-bound loops, accumulators, slice folds; the IR door adds anything whose `-O1` dataflow is straight-line or a canonical counted loop. Multi-accumulator loops, index-dependent bodies, and windowed indexing refuse with roadmap names — grep the refusal message to find the code path.
- **Rules**: 9 discharged rewrite rules against a 50-rule target; growth is proof-first by construction.
- **Perf**: JIT ≥5× target PASS on fold kernels (9.3×), 2.5× on small scalar kernels; memo hit path 83 ns vs ≤50 ns target (fix identified). Perf misses block perf sign-off only — never correctness.

- **Field trial №1 complete** ([`docs/FIELD-TRIAL.md`](docs/FIELD-TRIAL.md)): 62 fns across 3 published crates; 14/14 cross-door bitwise agreements; refusals bucketed into a priced extension list (`Len(k)` symbol first, diamond-CFG select second, P3 confirmed deferred at 24% of fns) — and the top two items are already shipped and re-measured: `Len(k)` unlocked the averaging-statistic family, diamond-CFG recovery + Σ v1.4 (`==`/`!=`/`exp2`) took easer from 18 to 32 of 34 IR admits — 94% of its public API, 18/18 cross-door bitwise.

Full detail: [`docs/STATUS.md`](docs/STATUS.md) (phase log), [`docs/DEEP-DIVE.md`](docs/DEEP-DIVE.md) (architecture, findings, Σ signature), [`docs/RFC-REVIEW-ir-lifting-v3.md`](docs/RFC-REVIEW-ir-lifting-v3.md) (IR-lifting design + measured findings).

## Design commitments (frozen)

No claims of zero debugging, 100% test automation, or flawless software. "Guaranteed" is reserved for what a certificate states — which always includes what is proved, over which inputs, at what confidence, under which environment fingerprint, and nothing more.

## License

MIT OR Apache-2.0. Test fixtures include short excerpts from `easer` 0.3.0 (MIT), attributed in place.
