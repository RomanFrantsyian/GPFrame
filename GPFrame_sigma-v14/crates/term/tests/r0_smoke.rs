//! R0 smoke: interp is definitional, hash is structural, full-key is authority.
use term::{eval, Op, TermBuilder};

fn poly() -> term::Term {
    // 2x^2 + 3x + 1  as fma(fma(2,x,3), x, 1) — wait, that's ((2x+3)x+1). Same poly.
    let mut b = TermBuilder::new();
    let two = b.constant(2.0);
    let x = b.var(0);
    let three = b.constant(3.0);
    let inner = b.ternary(Op::Fma, two, x, three); // 2x+3
    let one = b.constant(1.0);
    let root = b.ternary(Op::Fma, inner, x, one);  // (2x+3)x+1
    b.finish(root)
}

#[test]
fn interp_matches_math() {
    let t = poly();
    for x in [-3.0, 0.0, 1.5, 1e10] {
        assert_eq!(eval(&t, &[x]), 2.0f64.mul_add(x, 3.0).mul_add(x, 1.0));
    }
}

#[test]
fn hash_is_structural_and_bitwise_on_consts() {
    let a = poly();
    let b = poly();
    assert_eq!(a.hash, b.hash);
    assert!(a.structurally_eq(&b));

    // -0.0 vs +0.0 must differ (bitwise discipline feeding memo T6)
    let mk = |z: f64| {
        let mut b = TermBuilder::new();
        let c = b.constant(z);
        b.finish(c)
    };
    let pos = mk(0.0);
    let neg = mk(-0.0);
    assert_ne!(pos.hash, neg.hash);
    assert!(!pos.structurally_eq(&neg));
}

#[test]
fn sexpr_round_trip() {
    use term::sexpr::{parse, print};
    let src = "(fma (+ (var 0) 2.5) (var 1) (neg -3.0))";
    let t1 = parse(src).unwrap();
    let t2 = parse(&print(&t1)).unwrap();
    assert!(t1.structurally_eq(&t2), "canonical round trip");
    // semantics preserved
    for env in [[1.0, 2.0], [-0.5, 3.0], [0.0, 0.0]] {
        assert_eq!(eval(&t1, &env).to_bits(), eval(&t2, &env).to_bits());
    }
    // specials survive
    let t3 = parse("(+ NaN (min inf -inf))").unwrap();
    assert!(eval(&t3, &[]).is_nan());
}

#[test]
fn traced_eval_respects_select_pruning() {
    use term::eval_traced;
    // select(x, x+1, x*sin(x)) at x=1 must not mark the sin branch as used
    let t = term::sexpr::parse("(select (var 0) (+ (var 0) 1.0) (* (var 0) (sin (var 0))))").unwrap();
    let (v, used) = eval_traced(&t, &[1.0]);
    assert_eq!(v, 2.0);
    let sin_used = t.nodes.iter().enumerate()
        .any(|(i, n)| n.op == Op::Sin && used[i]);
    assert!(!sin_used, "untaken branch must be pruned from coverage");
    let (v0, used0) = eval_traced(&t, &[0.0]);
    assert_eq!(v0, 0.0);
    let sin_used0 = t.nodes.iter().enumerate()
        .any(|(i, n)| n.op == Op::Sin && used0[i]);
    assert!(sin_used0, "taken branch must be covered");
}

#[test]
fn fold_semantics_and_validation() {
    use term::eval_with_seqs;
    // dot product: fold(0, acc + elem0*elem1)
    let t = term::sexpr::parse("(fold 0.0 (+ acc (* (elem 0) (elem 1))))").unwrap();
    assert_eq!(t.seq_count(), 2);
    let a = [1.0, 2.0, 3.0];
    let b = [4.0, 5.0, 6.0];
    assert_eq!(eval_with_seqs(&t, &[], &[&a, &b]), 32.0);
    assert_eq!(eval_with_seqs(&t, &[], &[&[], &[]]), 0.0, "L=0 => init");
    // sexpr round trip
    let t2 = term::sexpr::parse(&term::sexpr::print(&t)).unwrap();
    assert!(t.structurally_eq(&t2));
    // hash distinguishes elem payloads
    let e0 = term::sexpr::parse("(fold 0.0 (+ acc (elem 0)))").unwrap();
    let e1 = term::sexpr::parse("(fold 0.0 (+ acc (elem 1)))").unwrap();
    assert_ne!(e0.hash, e1.hash);
    // binder escaping its body is REJECTED
    let bad = term::sexpr::parse("(+ acc 1.0)").unwrap();
    assert!(bad.fold_owners().is_err(), "escaped Acc must be rejected");
    // loop-invariant sharing between outside and body is LEGAL (hoisted)
    let shared = term::sexpr::parse(
        "(+ (var 0) (fold (var 0) (+ acc (* (elem 0) (var 0)))))").unwrap();
    assert!(shared.fold_owners().is_ok());
    let s = [2.0, 3.0];
    // x + fold(x, acc + e*x) with x=10: 10 + (10 + 20 + 30) = 70
    assert_eq!(eval_with_seqs(&shared, &[10.0], &[&s]), 70.0);
}
