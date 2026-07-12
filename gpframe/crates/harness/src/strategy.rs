//! [T4/A-1] μ' = (1−w)·μ_base + w·uniform(B) — boundary-mixture sampling.
//!
//! T4 in one line: n i.i.d. draws from μ' miss a defect region of measure ≥ δ
//! with probability ≤ (1−δ)^n ≤ e^{−nδ}. Boundary set B raises δ for the
//! known-pathological inputs (NaN, ±Inf, ±0, subnormals, overflow edges).
//!
//! μ itself is ASSUMED (A-1) to reflect operational input distribution;
//! the spec of μ' is therefore *recorded in the certificate*, never implicit.

/// The boundary atom set B. Order is part of the μ'-spec string.
pub const BOUNDARY: &[f64] = &[
    0.0, -0.0,
    f64::NAN,
    f64::INFINITY, f64::NEG_INFINITY,
    f64::MIN_POSITIVE,            // smallest normal
    5e-324,                       // smallest subnormal
    f64::MAX, f64::MIN,
    1.0, -1.0,
    f64::EPSILON,
];

/// Deterministic split-mix RNG: reproducible runs, seed goes in the cert.
/// (R1 swap-in: `proptest` strategies realize μ'; this stays as the
///  no-dependency reference implementation.)
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed) }

    pub fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn uniform01(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// μ' spec — everything the certificate needs to reconstruct sampling.
#[derive(Debug, Clone)]
pub struct MuPrime {
    /// mixture weight w on uniform(B)
    pub boundary_weight: f64,
    /// base measure: log-uniform magnitude, random sign.
    /// (Documented per-domain overrides = A-1 containment.)
    pub base_spec: &'static str,
    pub seed: u64,
    /// A-1 domain bound: samples and boundary atoms are clamped to
    /// |x| <= max_mag (INFINITY = unbounded, the default).
    pub max_mag: f64,
}

impl MuPrime {
    pub fn default_with_seed(seed: u64) -> Self {
        MuPrime { boundary_weight: 0.1, base_spec: "log-uniform |x| in [1e-300,1e300], ±", seed, max_mag: f64::INFINITY }
    }

    /// A-1 made concrete: a DOMAIN-BOUNDED μ' for ~_eps claims that are only
    /// true on bounded inputs (e.g. polynomial reassociation, which the
    /// unbounded gate correctly refutes at overflow magnitudes). The bound
    /// enters the spec string — and therefore the CERTIFICATE — verbatim:
    /// the claim is honest about where it holds. ±Inf boundary atoms are
    /// clamped to ±max_mag; NaN/±0/subnormals stay.
    pub fn bounded(seed: u64, max_mag: f64) -> Self {
        MuPrime {
            boundary_weight: 0.1,
            base_spec: "log-uniform bounded",
            seed,
            max_mag,
        }
    }

    /// Human/certificate-readable spec string (goes into Certificate).
    pub fn spec_string(&self) -> String {
        let dom = if self.max_mag.is_finite() {
            format!(" DOMAIN |x|<={:.1e} (A-1)", self.max_mag)
        } else {
            String::new()
        };
        format!(
            "mu' = {:.3}*uniform(B[{}]) + {:.3}*({}); seed={}{dom}",
            self.boundary_weight,
            BOUNDARY.len(),
            1.0 - self.boundary_weight,
            self.base_spec,
            self.seed
        )
    }

    /// Σ v1.2: draw scalars plus `seq_count` parallel sequences sharing one
    /// length. Length mixture: boundary lengths {0, 1, 2} with the boundary
    /// weight, else uniform 3..=32 — short lengths are where fold bugs live
    /// (L = 0 ⇒ init passthrough). Recorded in the spec string.
    pub fn sample_with_seqs(
        &self,
        rng: &mut Rng,
        arity: usize,
        seq_count: usize,
    ) -> (Vec<f64>, Vec<Vec<f64>>) {
        let scalars = self.sample(rng, arity);
        if seq_count == 0 {
            return (scalars, vec![]);
        }
        let len = if rng.uniform01() < self.boundary_weight {
            [0usize, 1, 2][(rng.next_u64() as usize) % 3]
        } else {
            3 + (rng.next_u64() as usize) % 30
        };
        let seqs = (0..seq_count)
            .map(|_| self.sample(rng, len))
            .collect();
        (scalars, seqs)
    }

    /// spec-string clause for the sequence measure (goes in certificates).
    pub fn seq_spec_clause() -> &'static str {
        "; seqs: parallel equal-length, len in {0,1,2} (boundary) U uniform[3,32]"
    }

    /// Draw one environment of width `arity`.
    pub fn sample(&self, rng: &mut Rng, arity: usize) -> Vec<f64> {
        (0..arity)
            .map(|_| {
                let raw = if rng.uniform01() < self.boundary_weight {
                    BOUNDARY[(rng.next_u64() as usize) % BOUNDARY.len()]
                } else {
                    // log-uniform magnitude, random sign
                    let hi = if self.max_mag.is_finite() { self.max_mag.log10() } else { 300.0 };
                    let exp = rng.uniform01() * (hi + 300.0) - 300.0;
                    let mag = 10f64.powf(exp);
                    if rng.next_u64() & 1 == 0 { mag } else { -mag }
                };
                // A-1 clamp (keeps NaN; folds ±Inf atoms to ±max_mag)
                if raw.is_nan() || raw.abs() <= self.max_mag { raw }
                else { self.max_mag.copysign(raw) }
            })
            .collect()
    }
}
