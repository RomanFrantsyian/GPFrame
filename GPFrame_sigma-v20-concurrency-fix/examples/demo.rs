// A small bundled demo for `./try.sh` (zero-argument mode). These are
// deliberately simple, real functions: one that certifies cleanly and
// one that DGE honestly refuses, so a first-time run shows both paths
// DGE supports — a proof, and a proof that refusal is a real answer too.

#[no_mangle]
pub fn poly(x: f64) -> f64 {
    3.0 * x * x * x + 5.0 * x * x + 2.0 * x + 7.0
}

#[no_mangle]
pub fn guarded_divide(x: f64, y: f64) -> f64 {
    assert!(y != 0.0, "cannot divide by zero");
    x / y
}
