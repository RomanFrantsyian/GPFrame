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
