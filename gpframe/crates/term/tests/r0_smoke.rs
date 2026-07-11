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
