//! Field-trial harness self-test: a controlled fixture with one fn per
//! outcome class, so the report's counting and bucketing are pinned before
//! the trial is pointed at real crates (whose numbers go in
//! docs/FIELD-TRIAL.md, not in tests — real crates change).

use cli::trial::{trial_dir, DoorOutcome};

#[test]
fn trial_classifies_the_fixture_correctly() {
    let dir = std::env::temp_dir().join(format!("dge_trial_fix_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("kernels.rs"), r#"
/// both doors admit; cross-gate must AGREE
pub fn poly(x: f64) -> f64 {
    3.0 * x * x + 2.0 * x + 1.0
}

/// syn refuses (iterator chain), IR admits — the IR door's added coverage
pub fn dot3(a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> f64 {
    [a0, a1, a2].iter().zip([b0, b1, b2].iter()).map(|(x, y)| x * y).sum()
}

/// both doors refuse: data-bound loop -> P2+ bucket at the IR door
pub fn halve(mut x: f64, lim: f64) -> f64 {
    while x > lim { x = x * 0.5; }
    x
}
"#).unwrap();

    let rep = trial_dir(&dir);
    assert_eq!(rep.fns.len(), 3, "audit must see all three fns");
    let by = |n: &str| rep.fns.iter().find(|f| f.name == n).unwrap();

    let poly = by("poly");
    assert!(matches!(poly.syn, DoorOutcome::Admitted { .. }));
    assert!(matches!(poly.ir, DoorOutcome::Admitted { .. }));
    assert!(matches!(poly.cross_gate, Some(Ok(()))), "doors must agree: {poly:?}");

    let dot = by("dot3");
    assert!(matches!(dot.syn, DoorOutcome::Refused(_)), "premise: syn refuses iterators");
    assert!(matches!(dot.ir, DoorOutcome::Admitted { .. }), "IR door coverage: {dot:?}");
    assert!(dot.cross_gate.is_none(), "one admission => ungated in trial");

    let halve = by("halve");
    assert!(matches!(halve.syn, DoorOutcome::Refused(_)));
    match &halve.ir {
        DoorOutcome::Refused(r) => assert_eq!(
            cli::trial::bucket(r), "P2+ (loop shape beyond canonical fold)",
            "raw reason: {r}"),
        o => panic!("expected refusal, got {o:?}"),
    }

    // the report renders the headline numbers
    let text = rep.render();
    assert!(text.contains("syn door admits : 1"), "{text}");
    assert!(text.contains("IR  door admits : 2"), "{text}");
    assert!(text.contains("1 of them REFUSED by syn"), "{text}");
    assert!(text.contains("1/1 agree"), "{text}");
}

/// A file that is not standalone-compilable must land in the build-failed
/// bucket — a finding about IR-door VISIBILITY, not an error.
#[test]
fn trial_buckets_non_standalone_files_honestly() {
    let dir = std::env::temp_dir().join(format!("dge_trial_ns_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dep.rs"), r#"
use some_external_crate::helper;
pub fn uses_dep(x: f64) -> f64 { helper(x) + 1.0 }
"#).unwrap();
    let rep = trial_dir(&dir);
    let f = rep.fns.iter().find(|f| f.name == "uses_dep").expect("audited");
    match &f.ir {
        DoorOutcome::Refused(r) => assert_eq!(
            cli::trial::bucket(r), "build-failed (not standalone-compilable)",
            "raw: {r}"),
        o => panic!("expected build-failed, got {o:?}"),
    }
}
