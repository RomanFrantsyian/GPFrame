//! [T3, O3] Shrinking: well-founded descent over ⊏ to a ⊏-minimal CE.
//!
//! T3: ⊏ well-founded ⇒ every shrink chain finite ⇒
//!     `while ∃ e' ⊏ e with e' ∈ F: e := e'` terminates at a ⊏-minimal
//!     element of the failure set F.
//!
//! O3 (well-foundedness) is discharged structurally here:
//!     rank(e) = Σ_i complexity(e_i) ∈ ℕ, and every candidate produced by
//!     `candidates()` strictly reduces rank ⇒ no infinite descent.
//!
//! Complexity order on a single f64 (coarse → fine):
//!     0.0 ⊏ ±1.0 ⊏ small ints ⊏ finite "round" ⊏ finite arbitrary ⊏ ±Inf ⊏ NaN
//! realized numerically by `complexity()` below.

/// Structural rank of one value. Smaller = simpler.
pub fn complexity(x: f64) -> u64 {
    // Sentinels are LARGE, not MAX: rank() sums per-coordinate complexities
    // and u64::MAX would overflow the sum (found by the O7 fmin test).
    const NAN_RANK: u64 = 1 << 40;
    const INF_RANK: u64 = (1 << 40) - 1;
    if x == 0.0 { return 0; }
    if x == 1.0 || x == -1.0 { return 1; }
    if x.is_nan() { return NAN_RANK; }
    if x.is_infinite() { return INF_RANK; }
    let mut c = 2u64;
    if x.fract() != 0.0 { c += 2; }          // non-integers are more complex
    c + (x.abs().log2().abs().ceil() as u64) // magnitude term
}

pub fn rank(env: &[f64]) -> u64 {
    env.iter().fold(0u64, |acc, &x| acc.saturating_add(complexity(x)))
}

/// Strictly-rank-reducing shrink candidates for one env.
/// PSEUDOCODE of the candidate schedule (ddmin-flavored per coordinate):
///   for each coordinate i with complexity > 0:
///     yield env[i := 0.0]
///     yield env[i := 1.0], env[i := -1.0]        (if simpler)
///     yield env[i := trunc(env[i])]              (drop fraction)
///     yield env[i := env[i]/2]                   (halve magnitude)
pub fn candidates(env: &[f64]) -> Vec<Vec<f64>> {
    let mut out = Vec::new();
    let r0 = rank(env);
    let mut push = |mut e: Vec<f64>, i: usize, v: f64| {
        e[i] = v;
        if rank(&e) < r0 {
            out.push(e);
        }
    };
    for i in 0..env.len() {
        let x = env[i];
        if complexity(x) == 0 { continue; }
        push(env.to_vec(), i, 0.0);
        push(env.to_vec(), i, 1.0);
        push(env.to_vec(), i, -1.0);
        if x.is_finite() {
            push(env.to_vec(), i, x.trunc());
            push(env.to_vec(), i, x / 2.0);
        } else {
            push(env.to_vec(), i, f64::MAX.copysign(if x.is_nan() { 1.0 } else { x }));
        }
    }
    out
}

/// T3 realization. `fails(e) == true` means e ∈ F.
/// Terminates: each accepted step strictly reduces `rank` (ℕ, well-founded).
/// Returns a ⊏-minimal counterexample w.r.t. the candidate schedule.
pub fn shrink(mut env: Vec<f64>, fails: &mut dyn FnMut(&[f64]) -> bool) -> Vec<f64> {
    debug_assert!(fails(&env), "shrink called on a non-failing input");
    'outer: loop {
        for cand in candidates(&env) {
            if fails(&cand) {
                env = cand;
                continue 'outer; // descend
            }
        }
        return env; // no candidate fails ⇒ ⊏-minimal
    }
}
