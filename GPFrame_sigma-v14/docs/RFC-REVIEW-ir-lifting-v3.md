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

NEXT: multi-accumulator folds, then P3 re-quantification on a larger corpus.

P3 entry point (when its bucket grows): side-effect slicing + re-stitching
— defer until a larger corpus re-quantifies the coverage gap (est. 5–10× P1+P2 effort). Nearer
extensions surfaced by P2 refusal messages, in rough order of value:
multi-accumulator folds (mean = sum+count), last-iteration live-outs,
index-dependent bodies (needs an Idx symbol or affine Elem offsets in Σ),
offset/windowed indexing, non-zero range starts. Each currently refuses
with exactly these words — grep the refusal to find the code path.
