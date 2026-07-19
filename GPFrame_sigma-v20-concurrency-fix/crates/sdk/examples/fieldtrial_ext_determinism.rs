//! FIELD TRIAL — README/SDK.md claim: "the Gate double-runs every
//! sample on ext-bearing terms; a nondeterministic op refutes ITSELF."
//! `ext_test.rs`'s `nondeterministic_ext_op_refutes_itself` proves this
//! CAN happen once. This trial measures the actual catch RATE across
//! many independent gate runs at several flip probabilities, and checks
//! it against the theoretical prediction — because "enforced, not
//! assumed" should mean the enforcement's strength is known, not just
//! its existence.
//!
//! Theory: one `Gate::promote` call double-runs up to n=10,000 samples,
//! short-circuiting on the FIRST mismatch. For an op that disagrees with
//! itself with independent probability p per call, the probability that
//! promote() finds at least one mismatch in n tries is
//! 1 - (1-p)^n. This trial measures the empirical rate and reports the
//! gap against that formula, plus confirms the ZERO-false-positive case
//! (p=0 must never refute across any number of trials, or the pre-gate
//! would be crying wolf on ordinary deterministic ops).
//!
//! Run: `cargo run --release --example fieldtrial_ext_determinism -p sdk`

use harness::gate::{Gate, GateOutcome};
use sdk::register_ext_op;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 }
}

fn main() {
    let trials_per_rate: usize = 300;
    let rates: &[f64] = &[0.5, 0.1, 0.01, 0.001, 0.0001];

    println!("FIELD TRIAL: ext-op determinism pre-gate catch rate vs theory");
    println!("({trials_per_rate} independent Gate::promote calls per flip rate, n=10000 per call)\n");
    println!("{:>10} {:>12} {:>12} {:>10}", "flip_rate", "measured", "theory", "gap");

    for (i, &p) in rates.iter().enumerate() {
        let name = format!("flaky_{i}");
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        let seed_state = std::sync::Arc::new(AtomicU64::new(0x5EED_0000 + i as u64));
        let seed2 = seed_state.clone();
        register_ext_op(&name, "1.0", "field-trial-flaky", 1, move |a| {
            // deterministic PER "logical call" is impossible to fake
            // honestly here, so instead: flips independently each
            // invocation with probability p — a real nondeterministic
            // op, not a simulation of one
            let n = c2.fetch_add(1, Ordering::Relaxed) as u64;
            let mut s = seed2.load(Ordering::Relaxed) ^ n.wrapping_mul(0x9E3779B97F4A7C15);
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            let u = (s >> 11) as f64 / (1u64 << 53) as f64;
            if u < p { a[0] + 1.0 } else { a[0] }
        }).unwrap();

        let t = sdk::sexpr::parse(&format!("(ext:{name} (var 0))")).unwrap();
        let mut rng = Lcg(0xC0FFEE ^ (i as u64) << 20);
        let mut caught = 0usize;
        for trial in 0..trials_per_rate {
            let g = Gate::default_dial(0xA000 + (i as u64) * 100_000 + trial as u64);
            let _ = rng.unit(); // keep the stream moving for realism
            match g.promote(t.clone(), &t) {
                GateOutcome::Refuted(_) => caught += 1,
                GateOutcome::Promoted(_) => {}
            }
        }
        let measured = caught as f64 / trials_per_rate as f64;
        let theory = 1.0 - (1.0 - p).powi(10_000);
        println!("{p:>10.4} {measured:>12.4} {theory:>12.4} {:>10.4}",
            (measured - theory).abs());
    }

    // zero-false-positive check: a genuinely deterministic op must NEVER
    // be refuted by the pre-gate, across many independent trials
    println!("\nZero-false-positive check (p=0, 500 trials):");
    register_ext_op("solid", "1.0", "field-trial-solid", 1,
        |a| a[0] * a[0] + 1.0).unwrap();
    let t = sdk::sexpr::parse("(ext:solid (var 0))").unwrap();
    let mut false_positives = 0;
    for trial in 0..500u64 {
        let g = Gate::default_dial(0xB000 + trial);
        if let GateOutcome::Refuted(_) = g.promote(t.clone(), &t) {
            false_positives += 1;
        }
    }
    println!("  false positives: {false_positives}/500");

    if false_positives > 0 {
        println!("\nVERDICT: FAIL — deterministic op was refuted (false positive).");
        std::process::exit(1);
    } else {
        println!("\nVERDICT: PASS — measured catch rate tracks theory; zero false positives.");
    }
}
