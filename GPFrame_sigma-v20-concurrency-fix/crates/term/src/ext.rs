//! Extension operators (Σ-ext) — pluggable semantics WITHOUT kernel edits.
//!
//! # Design position
//!
//! The Gate never needed to understand semantics — it needs to catch
//! lies. Arbitration is black-box (sample, run both sides, compare
//! bits), so *meaning* can be pluggable while the kernel stays sealed.
//! An extension op is a name plus an interpreter closure registered
//! here; terms reference ext ops BY NAME (data stays plain: a `Term`
//! carries its `exts: Vec<String>` table and serializes/round-trips
//! without the registry).
//!
//! # The honesty ledger (what plugging in costs — none of it hidden)
//!
//! * **Registration is identity, not trust.** `fingerprint` is the
//!   plugin's claim about its own semantics (version a hash of its
//!   source, a spec ID, anything stable). It goes into every
//!   certificate that touched the op — a claim made under extension
//!   semantics SAYS SO, forever.
//! * **Determinism is enforced, not assumed.** The Gate double-runs
//!   every sample on ext-bearing terms; a nondeterministic op refutes
//!   ITSELF (candidate run 1 vs run 2 as the counterexample).
//! * **Downstream degrades honestly**: rules never rewrite ext terms
//!   (guarded at every egraph entry), the JIT pins ext terms to the
//!   interpreter (the O7 mismatch-pin mechanism, applied a priori), and
//!   emission prints a call to the plugin's own symbol.
//! * **Purity per call is still required.** An ext op is `&[f64] -> f64`.
//!   Observable state ACROSS calls is the world-gate's job (next layer),
//!   not this one — an op that hides state trips the determinism gate.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// The interpreter closure: args slice (len == arity) → value.
pub type ExtFn = Arc<dyn Fn(&[f64]) -> f64 + Send + Sync>;

#[derive(Clone)]
pub struct ExtOpDef {
    pub name: String,
    pub version: String,
    /// The plugin's semantic identity claim (spec/hash). Certificate-borne.
    pub fingerprint: String,
    pub arity: usize,
    pub f: ExtFn,
}

impl ExtOpDef {
    /// The certificate tag: `name@version#fingerprint`.
    pub fn tag(&self) -> String {
        format!("{}@{}#{}", self.name, self.version, self.fingerprint)
    }
}

fn registry() -> &'static RwLock<HashMap<String, ExtOpDef>> {
    static R: std::sync::OnceLock<RwLock<HashMap<String, ExtOpDef>>> =
        std::sync::OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register an extension op. Idempotent for an identical (version,
/// fingerprint, arity); a CONFLICTING re-registration is an error — two
/// plugins claiming one name with different semantics is exactly the
/// ambiguity certificates exist to prevent.
pub fn register(name: &str, version: &str, fingerprint: &str, arity: usize,
                f: ExtFn) -> Result<(), String> {
    if !(1..=2).contains(&arity) {
        return Err(format!(
            "ext op `{name}`: arity {arity} unsupported -- ext ops are \
             unary or binary (ternary ext: roadmap; nullary: use consts)"));
    }
    let mut r = registry().write().unwrap();
    if let Some(prev) = r.get(name) {
        if prev.version == version && prev.fingerprint == fingerprint
            && prev.arity == arity {
            return Ok(()); // idempotent
        }
        return Err(format!(
            "ext op `{name}` already registered as {} (arity {}) -- \
             conflicting semantics for one name refuse; pick a new name",
            prev.tag(), prev.arity));
    }
    r.insert(name.to_string(), ExtOpDef {
        name: name.into(), version: version.into(),
        fingerprint: fingerprint.into(), arity, f,
    });
    Ok(())
}

pub fn lookup(name: &str) -> Option<ExtOpDef> {
    registry().read().unwrap().get(name).cloned()
}

/// Certificate tags for a term's ext table; `Err` names the first
/// unregistered op (gates refuse honestly on it).
pub fn tags_for(names: &[String]) -> Result<Vec<String>, String> {
    names.iter().map(|n| lookup(n).map(|d| d.tag()).ok_or_else(|| format!(
        "extension op `{n}` is not registered -- register it (or its \
         plugin) before gating"))).collect()
}
