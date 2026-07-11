//! R6 smoke: cache is an identity on semantics; hits require full-key match.
use memo::cache::MemoCache;
use term::{Op, TermBuilder};

#[test]
fn cache_identity_and_counters() {
    let mut b = TermBuilder::new();
    let x = b.var(0);
    let s = b.unary(Op::Sin, x);
    let t = b.finish(s);

    let cache = MemoCache::new();
    let v1 = cache.eval(&t, &[1.25]);
    let v2 = cache.eval(&t, &[1.25]);
    assert_eq!(v1.to_bits(), v2.to_bits());
    assert_eq!(v1, term::eval(&t, &[1.25]));
    assert_eq!(cache.hits.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(cache.misses.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn lru_evicts_under_pressure_and_stays_correct() {
    use term::eval;
    // tiny cap forces eviction almost immediately
    let cache = memo::cache::MemoCache::with_capacity(512);
    let mk = |k: f64| {
        let mut b = TermBuilder::new();
        let c = b.constant(k);
        let x = b.var(0);
        let r = b.binary(Op::Mul, c, x);
        b.finish(r)
    };
    let terms: Vec<_> = (0..32).map(|i| mk(i as f64)).collect();
    for t in &terms {
        for x in [1.0, 2.0] {
            assert_eq!(cache.eval(t, &[x]), eval(t, &[x])); // identity on semantics
        }
    }
    let ev = cache.evictions.load(std::sync::atomic::Ordering::Relaxed);
    assert!(ev > 0, "expected evictions under a 512-byte cap");
    // evicted entries recompute correctly (T6: eviction is sound)
    assert_eq!(cache.eval(&terms[0], &[1.0]), 0.0);
    assert!(cache.len() < 32, "cap must bound the live set");
}
