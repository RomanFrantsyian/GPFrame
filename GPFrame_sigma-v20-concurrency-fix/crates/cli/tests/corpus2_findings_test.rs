//! Field trial №2 findings, pinned (parser/perimeter honesty fixes).
//!
//! 1. `simple-easing 1.0.1` (all-f32): the syn extractor silently read f32
//!    params/returns as f64 Vars — a wrong-precision term. Concrete non-f64
//!    numeric types now refuse; generic `T` keeps the monomorphize-then-
//!    extract f64-instantiation reading (cross-door gate arbitrates it).
//! 2. `statistical 1.0.0`: `assert!(len > 1)` guards inline at -O1 into a
//!    branch carrying `!prof` metadata plus a call-panic block ending in
//!    `unreachable`. The old parser errored on BOTH ("br: …!prof" /
//!    "no terminator"), masking the honest perimeter answer. The parser is
//!    now total over these shapes, and the refusal states the real reason:
//!    panic paths make the function partial; Σ terms are total.

use cli::extract::{extract_fn, ExtractError};
use cli::lift::{lift_ll, rustc_emit_ir};

#[test]
fn f32_pin_flipped_by_v16_ints_still_refuse() {
    // PIN FLIP (Σ v1.6): the f32 refusal this test originally pinned is
    // now an ADMISSION — f32 functions extract with Rnd32 round-at-every-op
    // semantics (gated bitwise in f32_test.rs). Concrete ints still refuse.
    let src = "pub fn linear(t: f32) -> f32 { t * t + 1.0 }";
    extract_fn(src, "linear").expect("f32 admits since v1.6 (Rnd32)");
    // int params refuse for the same reason
    let src2 = "pub fn scale(x: f64, n: u32) -> f64 { x * 2.0 }";
    match extract_fn(src2, "scale") {
        Err(ExtractError::Unsupported(m)) => assert!(m.contains("u32"), "{m}"),
        other => panic!("u32 param must refuse: {other:?}"),
    }
    // generic T stays admitted (f64-instantiation reading) — easer's shape
    let gen = "pub fn ease<T: Into<f64>>(t: f64) -> f64 { t * t }";
    assert!(extract_fn(gen, "ease").is_ok(), "f64 fn with generics must extract");
}

#[test]
fn assert_panic_path_refuses_with_totality_vocabulary() {
    // statistical's variance shape: an assert guard before a fold. At -O1
    // the guard is `br i1 …, !prof !N` into a call-panic + `unreachable`
    // block — both shapes the parser must be TOTAL over, so the refusal
    // can be the honest one.
    let src = r#"
#[no_mangle]
pub fn guarded_mean(v: &[f64]) -> f64 {
    assert!(v.len() > 1, "need two points");
    let mut s = 0.0;
    for i in 0..v.len() { s += v[i]; }
    s / v.len() as f64
}
"#;
    let dir = std::env::temp_dir().join(format!("dge_corpus2_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("guarded.rs");
    std::fs::write(&f, src).unwrap();
    let ir = rustc_emit_ir(&f, "guarded_mean").expect("rustc must emit IR");
    match lift_ll(&ir, "guarded_mean") {
        Err(e) => {
            let m = format!("{e:?}");
            assert!(m.contains("unreachable") && m.contains("panic/assert"),
                "must refuse as a panic path, not a parse error: {m}");
            assert!(!m.contains("Parse"), "parse errors mask perimeter answers: {m}");
        }
        Ok(_) => panic!("a partial (panicking) function must not lift"),
    }
    std::fs::remove_dir_all(&dir).ok();
}
