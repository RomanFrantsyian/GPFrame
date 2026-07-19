//! Receiver flattening (trial №2 priced item 1): the syn door reads
//! immutable `&self` methods by flattening the receiver struct's f64 fields
//! into Vars in field DECLARATION order. Non-f64 fields refuse ON READ;
//! mutable receivers refuse outright; the IR door still has no receiver
//! shim (dedicated histogram bucket).
//!
//! The claim discipline is unchanged: extraction is untrusted, the gate
//! arbitrates. Gate 1 below drives the extracted term against the native
//! method over 10^4 samples.

use cli::extract::{extract_fn, ExtractError};
use harness::strategy::{MuPrime, Rng};
use term::eval_with_seqs;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// The receiver struct used by the native mirror of gate 1.
struct Kin { m: f64, v: f64, c: f64 }
impl Kin {
    fn energy(&self) -> f64 { 0.5 * self.m * self.v * self.v + self.c }
}

const KIN_SRC: &str = r#"
pub struct Kin { m: f64, v: f64, c: f64 }
impl Kin {
    pub fn energy(&self) -> f64 { 0.5 * self.m * self.v * self.v + self.c }
}
"#;

#[test]
fn getter_extracts_and_agrees_with_native_bitwise() {
    let t = extract_fn(KIN_SRC, "energy").expect("&self f64 getter must extract");
    // slots follow field declaration order: m=0, v=1, c=2
    let mut rng = Rng::new(0xD1CE);
    let mu = MuPrime::default_with_seed(7);
    for _ in 0..10_000 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 3, 0);
        let native = Kin { m: env[0], v: env[1], c: env[2] }.energy();
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let got = eval_with_seqs(&t, &env, &sl);
        assert!(xbit_eq(native, got),
            "native {native:?} vs term {got:?} at {env:?}");
    }
}

#[test]
fn non_f64_field_refuses_on_read_only() {
    // average 0.16.0's Mean shape: mixed f64/u64 fields, PRIVATE — the
    // real corpus population this feature audits.
    let src = r#"
pub struct Mean { avg: f64, n: u64 }
impl Mean {
    pub fn mean(&self) -> f64 {
        if self.n > 0 { self.avg } else { f64::NAN }
    }
    pub fn raw_avg(&self) -> f64 { self.avg * 1.0 }
}
"#;
    // reads self.n (u64) → honest refusal naming field and type
    match extract_fn(src, "mean") {
        Err(ExtractError::Unsupported(m)) =>
            assert!(m.contains("n: u64") && m.contains("f64-only"), "{m}"),
        other => panic!("u64 field read must refuse: {other:?}"),
    }
    // a sibling method touching ONLY the f64 field extracts
    extract_fn(src, "raw_avg").expect("f64-field-only method must extract");
}

#[test]
fn mutable_receiver_refuses() {
    let src = r#"
pub struct Acc { s: f64 }
impl Acc {
    pub fn bump(&mut self) -> f64 { self.s += 1.0; self.s }
}
"#;
    match extract_fn(src, "bump") {
        Err(ExtractError::Unsupported(m)) =>
            assert!(m.contains("mutable method receiver"), "{m}"),
        other => panic!("&mut self must refuse: {other:?}"),
    }
}

#[test]
fn unknown_struct_and_foreign_field_refuse() {
    // receiver struct not defined in the file → layout unknown
    let src = "impl Ghost { pub fn f(&self) -> f64 { 1.0 } }";
    match extract_fn(src, "f") {
        Err(ExtractError::Unsupported(m)) =>
            assert!(m.contains("no named-field definition"), "{m}"),
        other => panic!("unknown receiver layout must refuse: {other:?}"),
    }
    // field access on a non-self value stays P3 territory
    let src2 = r#"
pub struct P { x: f64 }
pub fn f(p: P) -> f64 { p.x }
"#;
    match extract_fn(src2, "f") {
        Err(ExtractError::Unsupported(m)) =>
            assert!(m.contains("P3 territory") || m.contains("non-receiver"), "{m}"),
        other => panic!("non-self field access must refuse: {other:?}"),
    }
}

#[test]
fn qualified_path_unary_calls_map_to_sigma() {
    // average 0.16.0 spells sqrt num_traits-style — same Σ op, call syntax
    let src = r#"
pub struct V { s: f64, n2: f64 }
impl V {
    pub fn error(&self) -> f64 { num_traits::Float::sqrt(self.s / self.n2) }
}
"#;
    let t = extract_fn(src, "error").expect("qualified sqrt must map");
    let mut rng = Rng::new(0xE44);
    let mu = MuPrime::default_with_seed(9);
    for _ in 0..10_000 {
        let (env, sq) = mu.sample_with_seqs(&mut rng, 2, 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        let native = f64::sqrt(env[0] / env[1]);
        assert!(xbit_eq(native, eval_with_seqs(&t, &env, &sl)));
    }
}
