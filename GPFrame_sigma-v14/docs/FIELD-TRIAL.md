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
