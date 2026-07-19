//! Regression for the live bug report (2026-07-18): `rustc_emit_ir`'s
//! temp directory used only the process id, so two THREADS in the same
//! test binary (cargo's default parallel test runner) — or two
//! concurrent `dge-serve` requests — raced on one shared
//! `lift_input.rs`/`.ll` pair. The loser silently read back a DIFFERENT
//! function's compiled IR (`ease_in_out` got `nested`'s body).
//!
//! This test drives many concurrent `rustc_emit_ir` + `lift_ll` calls
//! for DISTINCT functions from real OS threads and checks every result
//! is the RIGHT function, every time — the actual failure mode, not a
//! proxy for it.

use cli::lift::{lift_ll, rustc_emit_ir};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn make_fn(i: usize) -> (String, String, f64) {
    // each function is trivially distinguishable by its constant, so a
    // cross-contamination shows up as a wrong VALUE, not just a parse
    // difference
    let name = format!("concurrent_fn_{i}");
    let k = i as f64;
    let src = format!(
        "#[no_mangle]\npub fn {name}(x: f64) -> f64 {{ x + {k:.1} }}\n");
    (name, src, k)
}

#[test]
fn concurrent_rustc_emit_ir_calls_never_cross_contaminate() {
    let n = 24;
    let mismatches = Arc::new(AtomicUsize::new(0));
    let dir = std::env::temp_dir().join(format!(
        "dge_concurrency_regress_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let handles: Vec<_> = (0..n).map(|i| {
        let mismatches = mismatches.clone();
        let dir = dir.clone();
        std::thread::spawn(move || {
            let (name, src, k) = make_fn(i);
            let f = dir.join(format!("{name}.rs"));
            std::fs::write(&f, &src).unwrap();
            let ir = rustc_emit_ir(&f, &name).expect("rustc must succeed");
            let t = lift_ll(&ir, &name).expect("lift must succeed");
            // f(0.0) must equal exactly k for THIS function — if the IR
            // door read back a DIFFERENT concurrently-compiled function,
            // this will be wrong, exactly like the live bug report
            let got = term::eval(&t, &[0.0]);
            if got != k {
                mismatches.fetch_add(1, Ordering::SeqCst);
                eprintln!("CROSS-CONTAMINATION: fn {name} expected {k}, got {got}");
            }
        })
    }).collect();

    for h in handles { h.join().unwrap(); }
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(mismatches.load(Ordering::SeqCst), 0,
        "concurrent IR-door calls cross-contaminated — see stderr above");
}
