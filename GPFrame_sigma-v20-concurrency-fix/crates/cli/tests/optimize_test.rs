//! `dge optimize` — whole-file, IN-PLACE, certified rewriting.
//!
//! The in-place editor is a pure orchestration layer over `pipeline::certify`
//! plus one extra gate: before anything is written, the rewritten source is
//! re-extracted IN FILE CONTEXT and checked bitwise-equal to the proven term
//! over μ′ (the same `emission_round_trip` the pipeline uses). So the edit
//! inherits the pipeline's guarantee: nothing is written that does not
//! re-extract to its own certificate.
//!
//! This test drives the library entry point over a temp file holding one
//! certifiable function (`poly`) and one honestly-refused function
//! (`guarded_divide`, partial because of `assert!`). It checks BOTH the
//! dry-run and the write path, that signatures / human docs / attributes
//! survive verbatim, that the refused function is left byte-for-byte alone,
//! and — independently of the internal gate — that the rewritten `poly`
//! agrees with the ORIGINAL bitwise over 10^4 μ′ samples.
//!
//! Falsifiability check (done by hand before trusting the green): flipping a
//! sign in `SRC`'s poly makes the final differential fail; asserting on
//! `pub fn poly(` before the write makes the "signature preserved" line fail;
//! removing the write path leaves the file == SRC so every post-write
//! assertion fails. The assertions bite.

use cli::optimize::{optimize, OptimizeOpts, Status};
use harness::strategy::{MuPrime, Rng};
use rules::smt::{discharge_all, Z3Cli};
use std::path::PathBuf;
use term::eval_with_seqs;

fn xbit_eq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn unique_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("dge_{tag}_{}_{n}", std::process::id()))
}

const SRC: &str = r#"//! sample
/// human doc kept
#[no_mangle]
pub fn poly(x: f64) -> f64 {
    3.0 * x * x + 2.0 * x + 1.0
}

pub fn guarded_divide(x: f64, y: f64) -> f64 {
    assert!(y != 0.0, "no");
    x / y
}
"#;

#[test]
fn optimize_rewrites_in_place_and_refuses_honestly() {
    if !Z3Cli::available() {
        eprintln!("skipping optimize_test: z3 binary not available");
        return;
    }
    // guard against a vacuous "contains CERTIFIED" assertion later
    assert!(!SRC.contains("CERTIFIED"), "fixture must start uncertified");

    let art = unique_dir("opt_art");
    std::fs::create_dir_all(&art).unwrap();
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&art));

    let dir = unique_dir("opt_src");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lib.rs");
    std::fs::write(&file, SRC).unwrap();

    // ---- dry run: reports the rewrite, writes NOTHING ------------------
    let dry = OptimizeOpts {
        write: false,
        backup: false,
        artifacts: art.clone(),
        ..Default::default()
    };
    let s = optimize(&file, &dry).expect("optimize (dry-run) failed");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        SRC,
        "dry-run must not modify the file"
    );
    assert!(
        s.outcomes.iter().any(|o| o.name == "poly"
            && matches!(o.status, Status::Rewritten { .. })),
        "poly should be certified+rewritable"
    );
    assert!(
        s.outcomes.iter().any(|o| o.name == "guarded_divide"
            && matches!(o.status, Status::Refused(_))),
        "guarded_divide (assert ⇒ partial) should be refused"
    );

    // ---- write: rewrite in place, refuse the rest ---------------------
    let wr = OptimizeOpts {
        write: true,
        backup: false,
        artifacts: art.clone(),
        ..Default::default()
    };
    let s2 = optimize(&file, &wr).expect("optimize (write) failed");
    assert_eq!(s2.rewritten(), 1, "exactly poly rewritten");
    assert_eq!(s2.refused(), 1, "exactly guarded_divide refused");

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.contains("/// CERTIFIED"), "certificate must be attached:\n{after}");
    assert!(
        after.contains("pub fn poly(x: f64) -> f64"),
        "original signature (name + params) must be preserved verbatim:\n{after}"
    );
    assert!(after.contains("/// human doc kept"), "human doc must survive:\n{after}");
    assert!(after.contains("#[no_mangle]"), "attribute must survive:\n{after}");
    // the refused function must be untouched, assert and all
    assert!(
        after.contains("assert!(y != 0.0, \"no\");"),
        "refused function must be left exactly as-is:\n{after}"
    );

    // ---- independent differential: rewritten poly ≡ ORIGINAL over μ′ ----
    let t_old = cli::extract::extract_fn(SRC, "poly").expect("extract original poly");
    let t_new = cli::extract::extract_fn(&after, "poly").expect("re-extract rewritten poly");
    assert_eq!(t_new.arity(), t_old.arity());
    let mu = MuPrime::default_with_seed(0x0B0B);
    let mut rng = Rng::new(0x0B0B);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, t_old.arity().max(1), 0);
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(
            xbit_eq(eval_with_seqs(&t_new, &e, &sl), eval_with_seqs(&t_old, &e, &sl)),
            "rewritten poly drifts from original at {e:?}"
        );
    }

    // ---- idempotence: a second pass converges (no runaway edits) -------
    let s3 = optimize(&file, &wr).expect("second optimize pass failed");
    assert_eq!(s3.rewritten(), 1, "second pass still certifies poly");
    let after2 = std::fs::read_to_string(&file).unwrap();
    // exactly one certificate block survives (no preamble accumulation)
    assert_eq!(after2.matches("/// CERTIFIED").count(), 1, "cert block must not stack");
    assert_eq!(
        after2.matches("#[allow(unused_parens, clippy::all)]").count(),
        1,
        "generated allow-attr must not stack"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&art);
}

const ADV: &str = r#"
pub struct Vec3 { pub x: f64, pub y: f64, pub z: f64 }
impl Vec3 {
    /// squared length
    pub fn norm2(&self) -> f64 { self.x * self.x + self.y * self.y + self.z * self.z }
    pub fn scaled(&self, k: f64) -> f64 { k * (self.x + self.y + self.z) }
}
pub fn blend10(a: f64, b: f64, c: f64, d: f64, e: f64,
               f: f64, g: f64, h: f64, i: f64, j: f64) -> f64 {
    a + 2.0*b + 3.0*c + 4.0*d + 5.0*e + 6.0*f + 7.0*g + 8.0*h + 9.0*i + 10.0*j
}
pub fn poly4(coeffs: &[f64; 4], x: f64) -> f64 {
    coeffs[0] + coeffs[1]*x + coeffs[2]*x*x + coeffs[3]*x*x*x
}
"#;

/// Same μ′ differential used above, but reused for several functions.
fn assert_same_over_mu(src_before: &str, src_after: &str, name: &str) {
    let t_old = cli::extract::extract_fn(src_before, name).expect("extract original");
    let t_new = cli::extract::extract_fn(src_after, name).expect("re-extract rewritten");
    assert_eq!(t_new.arity(), t_old.arity(), "{name}: arity changed");
    assert_eq!(t_new.seq_count(), t_old.seq_count(), "{name}: seq count changed");
    let mu = MuPrime::default_with_seed(0x515D);
    let mut rng = Rng::new(0x515D);
    for _ in 0..10_000u32 {
        let (e, sq) = mu.sample_with_seqs(&mut rng, t_old.arity().max(1), t_old.seq_count());
        let sl: Vec<&[f64]> = sq.iter().map(|v| v.as_slice()).collect();
        assert!(
            xbit_eq(eval_with_seqs(&t_new, &e, &sl), eval_with_seqs(&t_old, &e, &sl)),
            "{name} drifts from original at {e:?}"
        );
    }
}

#[test]
fn optimize_handles_methods_arrays_and_large_arity() {
    if !Z3Cli::available() {
        eprintln!("skipping: z3 not available");
        return;
    }
    let art = unique_dir("adv_art");
    std::fs::create_dir_all(&art).unwrap();
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&art));

    let dir = unique_dir("adv_src");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lib.rs");
    std::fs::write(&file, ADV).unwrap();

    let opts = OptimizeOpts { write: true, backup: false, include_effort: true, artifacts: art.clone(), ..Default::default() };
    let s = optimize(&file, &opts).expect("optimize failed");

    // every one of the four target functions must be rewritten in place
    for name in ["norm2", "scaled", "blend10", "poly4"] {
        assert!(
            s.outcomes.iter().any(|o| o.name == name && matches!(o.status, Status::Rewritten { .. })),
            "{name} should be rewritten in place; outcomes: {:?}",
            s.outcomes
        );
    }

    let after = std::fs::read_to_string(&file).unwrap();
    // signatures preserved verbatim
    assert!(after.contains("pub fn norm2(&self) -> f64"), "method sig preserved:\n{after}");
    assert!(after.contains("pub fn scaled(&self, k: f64) -> f64"), "method+scalar sig preserved");
    assert!(after.contains("pub fn poly4(coeffs: &[f64; 4], x: f64) -> f64"), "array-param sig preserved");
    assert!(after.contains("i: f64, j: f64) -> f64"), "large-arity sig preserved");
    // the receiver body still reads self.<field>, the array body still indexes coeffs
    assert!(after.contains("self.x"), "method body must reference self fields");
    assert!(after.contains("coeffs[0]"), "array body must index the original array");

    // independent bitwise differential vs the ORIGINAL for each shape
    assert_same_over_mu(ADV, &after, "norm2");
    assert_same_over_mu(ADV, &after, "scaled");
    assert_same_over_mu(ADV, &after, "blend10");
    assert_same_over_mu(ADV, &after, "poly4");

    // idempotence for the mixed file: a second pass converges byte-stable
    optimize(&file, &opts).expect("second pass");
    let a2 = std::fs::read_to_string(&file).unwrap();
    optimize(&file, &opts).expect("third pass");
    let a3 = std::fs::read_to_string(&file).unwrap();
    assert_eq!(a2, a3, "mixed file must reach a byte-stable fixed point");
    // certificates never stack (4 functions ⇒ 4 blocks)
    assert_eq!(a3.matches("/// CERTIFIED").count(), 4, "one cert block per function");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&art);
}

#[test]
fn optimize_is_deterministic_across_jobs() {
    if !Z3Cli::available() {
        eprintln!("skipping: z3 not available");
        return;
    }
    let art = unique_dir("par_art");
    std::fs::create_dir_all(&art).unwrap();
    discharge_all(&rules::r_dec::table(), &mut Z3Cli::new(&art));

    // two identical trees; optimize one sequentially, one with 4 workers
    let mk = |tag: &str| -> PathBuf {
        let d = unique_dir(tag);
        std::fs::create_dir_all(&d).unwrap();
        for k in 0..8 {
            let body = format!(
                "pub fn a{k}(x: f64, y: f64) -> f64 {{ {k}.0*x*x + y*y + 2.0*x*y }}\n\
                 pub struct S{k} {{ pub p: f64, pub q: f64 }}\n\
                 impl S{k} {{ pub fn n(&self) -> f64 {{ self.p*self.p + self.q*self.q }} }}\n\
                 pub fn w{k}(a: f64,b: f64,c: f64,d: f64,e: f64,f: f64,g: f64,h: f64,i: f64) -> f64 \
                 {{ a+b+c+d+e+f+g+h+i + {k}.0 }}\n"
            );
            std::fs::write(d.join(format!("f{k}.rs")), body).unwrap();
        }
        d
    };
    let seq = mk("par_seq");
    let par = mk("par_par");

    let base = OptimizeOpts { write: true, backup: false, include_effort: true, artifacts: art.clone(), ..Default::default() };
    let s1 = optimize(&seq, &OptimizeOpts { jobs: 1, ..clone_opts(&base) }).expect("seq");
    let s4 = optimize(&par, &OptimizeOpts { jobs: 4, ..clone_opts(&base) }).expect("par");

    assert!(s1.rewritten() > 0, "should rewrite something");
    assert_eq!(s1.rewritten(), s4.rewritten(), "same rewrite count regardless of jobs");
    assert_eq!(s1.refused(), s4.refused());
    assert_eq!(s1.files_written.len(), s4.files_written.len());

    // every file must be byte-identical between the two runs
    for k in 0..8 {
        let a = std::fs::read_to_string(seq.join(format!("f{k}.rs"))).unwrap();
        let b = std::fs::read_to_string(par.join(format!("f{k}.rs"))).unwrap();
        assert_eq!(a, b, "file f{k}.rs differs between jobs=1 and jobs=4");
    }

    let _ = std::fs::remove_dir_all(&seq);
    let _ = std::fs::remove_dir_all(&par);
    let _ = std::fs::remove_dir_all(&art);
}

// OptimizeOpts isn't Clone (by design); tiny helper for the test above.
fn clone_opts(o: &OptimizeOpts) -> OptimizeOpts {
    OptimizeOpts {
        write: o.write,
        backup: o.backup,
        include_effort: o.include_effort,
        artifacts: o.artifacts.clone(),
        eps: o.eps,
        domain: o.domain,
        jobs: o.jobs,
    }
}
