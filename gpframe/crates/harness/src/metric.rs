//! [I1] Numeric comparison metrics: abs + rel + ULP; NaN/Inf/−0 policy.
//!
//! Policy (fixed, versioned — a metric change invalidates certificates):
//! * NaN ~ NaN            → EQUAL under `Semantic` (any payload), UNEQUAL under `Bitwise`.
//! * +0 vs −0             → UNEQUAL under `Bitwise` (O7 JIT gate), EQUAL under `Semantic`.
//! * Inf sign-sensitive under both.
//! * `Tolerant{eps}` realizes `~_eps` for R_approx (D2 relaxed).

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Metric {
    /// bit-for-bit: `x.to_bits() == y.to_bits()`. Only meaningful when BOTH
    /// sides come from the SAME code generator — see BitwiseNanClass.
    Bitwise,
    /// FINDING 7 (measured): IEEE-754 leaves NaN payload propagation
    /// unspecified, and it genuinely diverges in practice — LLVM const-fold
    /// yields +qNaN (0x7ff8…) for -inf + +inf where x86 hardware yields the
    /// sign-set "real indefinite" (0xfff8…), and fadd operand
    /// canonicalization can flip which operand's payload survives. So
    /// bit-for-bit equality ACROSS code generators (rustc vs interp vs
    /// cranelift) is unachievable in principle for NaN results. This metric
    /// is the strongest honest cross-generator claim: exact bits for every
    /// non-NaN value (±0 signs INCLUDED — those ARE specified), NaN ≡ NaN
    /// as a class.
    BitwiseNanClass,
    /// IEEE-aware equality: NaN≡NaN, ±0 identified.
    Semantic,
    /// |x−y| ≤ eps_abs  ∨  |x−y| ≤ eps_rel·max(|x|,|y|)  ∨  ulp_dist ≤ max_ulp.
    Tolerant { eps_abs: f64, eps_rel: f64, max_ulp: u64 },
}

impl Metric {
    pub fn eq(self, x: f64, y: f64) -> bool {
        match self {
            Metric::Bitwise => x.to_bits() == y.to_bits(),
            Metric::BitwiseNanClass =>
                x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
            Metric::Semantic => {
                (x.is_nan() && y.is_nan()) || x == y || (x == 0.0 && y == 0.0)
            }
            Metric::Tolerant { eps_abs, eps_rel, max_ulp } => {
                if (x.is_nan() && y.is_nan()) || x == y {
                    return true;
                }
                if x.is_nan() || y.is_nan() || x.is_infinite() || y.is_infinite() {
                    return false; // finite/non-finite mismatch is never "close"
                }
                let d = (x - y).abs();
                d <= eps_abs
                    || d <= eps_rel * x.abs().max(y.abs())
                    || ulp_distance(x, y) <= max_ulp
            }
        }
    }

    /// ≤1 ULP tolerance. WARNING (R2 finding, gate-refuted): this is NOT a
    /// valid metric for fma-contraction — cancellation makes the difference
    /// unbounded in ULPs. Use `fma_mixed()` for contraction gating.
    pub const fn one_ulp() -> Self {
        Metric::Tolerant { eps_abs: 0.0, eps_rel: 0.0, max_ulp: 1 }
    }

    /// The honest ~_eps for fma contraction (rules::r_approx and the O7 jit
    /// relaxation): mixed absolute/relative tolerance covering both the
    /// cancellation regime (abs) and the large-magnitude regime (rel).
    pub const fn fma_mixed() -> Self {
        Metric::Tolerant { eps_abs: 1e-12, eps_rel: 1e-12, max_ulp: 0 }
    }
}

/// Lexicographic-order ULP distance (Bruce Dawson construction):
/// map f64 to a monotone i64 line, distance = |a−b| on that line.
pub fn ulp_distance(x: f64, y: f64) -> u64 {
    fn key(f: f64) -> i64 {
        let b = f.to_bits() as i64;
        // ±0.0 both map to 0; negatives map monotonically below.
        if b >= 0 { b } else { i64::MIN.wrapping_sub(b) }
    }
    if x.is_nan() || y.is_nan() {
        return u64::MAX;
    }
    key(x).abs_diff(key(y))
}
