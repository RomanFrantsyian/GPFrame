//! R4 smoke: a fault localized. Program: select(x<0 ? ... ) style — we plant
//! the bug in the negative branch and check Ochiai ranks a faulty-branch node
//! above the shared/correct nodes.
use locate::ochiai::rank;
use locate::spectrum::collect;
use term::sexpr::parse;

#[test]
fn ochiai_ranks_faulty_branch_first() {
    // intended: |x| computed as select(max(x,0)==x ? x : -x)
    // buggy: negative branch returns x (missing neg) →
    //   p(x) = select(max(x, 0) - x, ???, ...)  — build directly:
    // cond = (max x 0) - x   (0 when x>=0 → else-branch; nonzero when x<0 → then-branch)
    // then-branch (x<0): BUGGY: (var 0)     [should be (neg (var 0))]
    // else-branch (x>=0): (var 0)           [correct]
    let p = parse("(select (- (max (var 0) 0.0) (var 0)) (var 0) (var 0))").unwrap();

    let tests: Vec<Vec<f64>> = vec![
        vec![1.0], vec![2.5], vec![0.0],          // pass (x>=0)
        vec![-1.0], vec![-2.0], vec![-0.5],       // fail (x<0): p(x)=x != |x|
    ];
    let phi = |env: &[f64], out: f64| out == env[0].abs();
    let s = collect(&p, &tests, &phi);
    let ranking = rank(&s);

    // The top-ranked node must be one executed ONLY on failing runs — i.e.
    // inside the then-branch (the buggy one). In this term the then-branch
    // 'x' is a distinct Var node; find nodes with ef>0, ep==0 and check the
    // top of the ranking is among them.
    let (top_node, top_score) = ranking[0];
    let (ef, ep, _, _) = s.counts(top_node);
    assert!(top_score > 0.9, "top suspiciousness too low: {top_score}");
    assert!(ef > 0 && ep == 0, "top-ranked node should be failing-only (ef={ef}, ep={ep})");
}
