//! Pre-R0 audit test: a fixture codebase with one fn per classification rule.
use cli::audit::{audit_dir, Class};
use std::fs;

const FIXTURE: &str = r#"
// -- EXTRACTABLE: pure f64 expression code over Σ ------------------------

pub fn horner3(x: f64, a: f64, b: f64, c: f64) -> f64 {
    a.mul_add(x, b).mul_add(x, c)
}

pub fn smooth_min(a: f64, b: f64, k: f64) -> f64 {
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

pub fn branchy(x: f64) -> f64 {
    if x > 0.0 { x.sqrt() } else { -x }        // if/else -> Select
}

pub fn calls_extractable(x: f64) -> f64 {
    horner3(x, 2.0, 3.0, 1.0) + branchy(x)      // inline-able local calls
}

// -- WITH_EFFORT: pure numeric, needs manual extraction ------------------

pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len() { s += a[i] * b[i]; }   // loop + accumulator
    s
}

pub fn fact(n: u64) -> u64 {
    if n == 0 { 1 } else { n * fact(n - 1) }    // recursion + int sort
}

pub fn calls_effort(x: f64) -> f64 {
    dot(&[x], &[x])                             // demoted transitively
}

// -- NOT_EXTRACTABLE: outside the perimeter by construction --------------

pub fn logs(x: f64) -> f64 {
    println!("x = {x}");                        // IO macro
    x
}

pub fn mutates(out: &mut f64, x: f64) -> f64 {
    *out = x;                                   // &mut param
    x
}

pub fn stringy(s: &str) -> f64 {
    s.len() as f64                              // non-numeric type
}

pub fn calls_unknown(x: f64) -> f64 {
    std_rand_helper(x)                          // unknown callee
}
"#;

#[test]
fn audit_classifies_and_gates() {
    let dir = std::env::temp_dir().join(format!("dge_audit_test_{}", std::process::id()));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), FIXTURE).unwrap();
    // target/ must be skipped by the walker
    fs::create_dir_all(dir.join("target")).unwrap();
    fs::write(dir.join("target/junk.rs"), "fn x() -> f64 { 1.0 }").unwrap();

    let report = audit_dir(&dir);
    let class_of = |name: &str| {
        report.fns.iter().find(|f| f.name == name)
            .unwrap_or_else(|| panic!("fn `{name}` missing from audit")).class
    };

    for f in ["horner3", "smooth_min", "branchy", "calls_extractable"] {
        assert_eq!(class_of(f), Class::Extractable, "{f}");
    }
    for f in ["dot", "fact", "calls_effort"] {
        assert_eq!(class_of(f), Class::WithEffort, "{f}");
    }
    for f in ["logs", "mutates", "stringy", "calls_unknown"] {
        assert_eq!(class_of(f), Class::NotExtractable, "{f}");
    }
    assert!(!report.fns.iter().any(|f| f.name == "x"), "target/ must be skipped");

    // s numbers are LOC-weighted and total; verdict string is one of the
    // three §9 branches.
    assert!(report.s_strict() > 0.0 && report.s_strict() < 1.0);
    assert!(report.s_loose() >= report.s_strict());
    assert!(report.verdict().starts_with("BUILD")
        || report.verdict().starts_with("BORDERLINE")
        || report.verdict().starts_with("DO NOT BUILD"));

    println!("{}", report.render());
    fs::remove_dir_all(&dir).ok();
}
