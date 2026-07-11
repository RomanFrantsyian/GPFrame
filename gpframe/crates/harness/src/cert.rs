//! [T8 exit-door payload, O8] Certificate — the ONLY thing a stage may emit
//! as evidence. "Guaranteed" is reserved for what this struct states.
//!
//! v2.0 §7: certificate = {rule trace, tier, SMT proofs | (n, α, δ_min)}.
//! v2.1 O8: Tier-B claims are relative to the execution environment —
//! libm build, CPU feature flags (fma/avx), rounding mode. Env change ⇒
//! certificate STALE ⇒ re-gate (cheap: rerun V_gate).

/// Which rung of the T2-forced two-tier gate produced this evidence.
#[derive(Debug, Clone, PartialEq)]
pub enum Tier {
    /// PROVED: every rule applied carries a stored SMT UNSAT artifact (O1)
    /// or a reviewed algebraic proof (O2).
    A { smt_artifacts: Vec<String> },
    /// CONFIRMED TO CONFIDENCE 1−α over μ' — never "proved".
    /// δ_min = ln(1/α)/n: the smallest defect measure this run can see.
    B { n: u64, alpha: f64, delta_min: f64, mu_spec: String },
}

/// O8 — pin of the execution environment. All Tier-B (and O7) claims are
/// relative to this fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFingerprint {
    pub target_triple: String,
    /// CPU feature flags relevant to FP results.
    pub fma: bool,
    pub avx: bool,
    /// libm identity (build hash / version string).
    pub libm: String,
    /// IEEE rounding mode; default round-to-nearest-even.
    pub rounding: &'static str,
}

impl EnvFingerprint {
    /// Capture the current environment.
    /// * CPU features: RUNTIME detection where the arch supports it
    ///   (compile-time cfg! is only a lower bound — a binary compiled
    ///   generic but run on FMA hardware must still be pinned correctly).
    /// * libm identity: BEHAVIORAL hash — the FNV of the output bit
    ///   patterns of the transcendentals at fixed probe inputs. Two libm
    ///   builds that answer identically on the probes are interchangeable
    ///   for our purposes; ones that differ are caught even if their
    ///   version strings match.
    pub fn capture() -> Self {
        EnvFingerprint {
            target_triple: std::env::consts::ARCH.to_string(),
            fma: detect_fma(),
            avx: detect_avx(),
            libm: format!("behavioral:{:016x}", libm_behavioral_hash()),
            rounding: "round-nearest-even",
        }
    }

    /// Staleness check: any mismatch ⇒ every Tier-B cert minted under `self`
    /// must be re-gated before reuse.
    pub fn matches(&self, other: &EnvFingerprint) -> bool {
        self == other
    }
}

/// The evidence object. Refactoring output contract: `(P', Certificate)`.
#[derive(Debug, Clone)]
pub struct Certificate {
    pub tier: Tier,
    /// Applied rule names in order (empty for pure gate promotions).
    pub rule_trace: Vec<String>,
    pub env: EnvFingerprint,
    /// v2.1 §1: fma-contraction flag enters the certificate when the O7
    /// gate is relaxed to ≤1 ULP.
    pub fma_contraction: bool,
}

impl Certificate {
    pub fn is_stale(&self, current: &EnvFingerprint) -> bool {
        !self.env.matches(current)
    }

    /// Human-readable claim — states exactly what is proved and no more.
    pub fn claim(&self) -> String {
        match &self.tier {
            Tier::A { smt_artifacts } => format!(
                "PROVED semantics-preserving; {} rule(s) [{} SMT artifact(s)]",
                self.rule_trace.len(),
                smt_artifacts.len()
            ),
            Tier::B { n, alpha, delta_min, mu_spec } => format!(
                "equivalent within metric at confidence {} over {}; \
                 defect regions of measure < {:.3e} are invisible (n={})",
                1.0 - alpha, mu_spec, delta_min, n
            ),
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn detect_fma() -> bool { std::arch::is_x86_feature_detected!("fma") }
#[cfg(target_arch = "x86_64")]
fn detect_avx() -> bool { std::arch::is_x86_feature_detected!("avx") }
#[cfg(not(target_arch = "x86_64"))]
fn detect_fma() -> bool { cfg!(target_feature = "fma") }
#[cfg(not(target_arch = "x86_64"))]
fn detect_avx() -> bool { cfg!(target_feature = "avx") }

/// FNV-1a over the output bits of every libm-backed operation at fixed
/// probe inputs. Deterministic per libm build + CPU; any change ⇒ different
/// hash ⇒ Tier-B certificates minted under the old env read as STALE.
pub fn libm_behavioral_hash() -> u64 {
    const PROBES: [f64; 6] = [0.5, 1.0, 2.75, -3.5, 1e-8, 123.456];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: f64| {
        for b in v.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for x in PROBES {
        mix(x.sin()); mix(x.cos()); mix(x.tan());
        mix(x.exp()); mix(x.abs().ln()); mix(x.abs().sqrt());
        mix(x.abs().powf(1.5)); mix(x.mul_add(2.0, 0.25));
        mix(x.floor()); mix(x.ceil());
    }
    h
}
