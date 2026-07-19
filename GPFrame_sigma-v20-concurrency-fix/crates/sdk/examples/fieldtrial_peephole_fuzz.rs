//! FIELD TRIAL — `plugins_test.rs` proves the `Peephole` plugin is bit-exact
//! for a handful of hand-picked terms. This trial fuzzes it: many random
//! arithmetic terms (a deliberate mix of constants and `(var 0)`, so constant
//! subtrees and `*1`/`neg neg` opportunities actually occur), each fed to the
//! plugin, checking that EVERY proposed rewrite is (a) no larger than the
//! original and (b) bitwise-equal to it over many μ′ samples — including the
//! nasty values (±0, ±∞, NaN, subnormals) where folding could go wrong.
//!
//! A fuzz campaign proves nothing if the plugin never actually fires, so this
//! counts how many terms were genuinely simplified, not just how many ran.
//!
//! Run: `cargo run --release --example fieldtrial_peephole_fuzz -p sdk [rounds]`

use sdk::plugins::Peephole;
use sdk::{Suggester, Term};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() as usize) % xs.len()]
    }
    fn bool(&mut self) -> bool {
        self.next() & 1 == 0
    }
}

/// Random arithmetic sexpr over `var 0`, depth-bounded, constant-heavy so
/// folding has something to do. Occasionally wraps in `neg` / multiplies by
/// a literal `1` to exercise the identity rules too.
fn random_term(rng: &mut Lcg, depth: u32) -> String {
    if depth == 0 || rng.next() % 3 == 0 {
        return if rng.bool() {
            "(var 0)".into()
        } else {
            format!("{:.3}", (rng.next() % 2000) as f64 / 100.0 - 10.0)
        };
    }
    match rng.next() % 6 {
        0 => format!("(neg {})", random_term(rng, depth - 1)),
        1 => format!("(* 1 {})", random_term(rng, depth - 1)),
        2 => format!("(* {} 1)", random_term(rng, depth - 1)),
        _ => {
            let op = *rng.pick(&["+", "-", "*", "/"]);
            format!("({} {} {})", op, random_term(rng, depth - 1), random_term(rng, depth - 1))
        }
    }
}

fn beq(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn differential(orig: &Term, cand: &Term) -> Result<(), f64> {
    let specials = [
        0.0f64, -0.0, 1.0, -1.0, 2.0, 0.5, f64::INFINITY, f64::NEG_INFINITY,
        f64::NAN, f64::MIN, f64::MAX, f64::MIN_POSITIVE, 1e-300, 1e300, -7.25, 3.5,
    ];
    for &x in &specials {
        if !beq(term::eval_with_seqs(orig, &[x], &[]), term::eval_with_seqs(cand, &[x], &[])) {
            return Err(x);
        }
    }
    let mut st = 0x243F_6A88_85A3_08D3u64;
    for _ in 0..2000 {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        let x = f64::from_bits(st);
        if !beq(term::eval_with_seqs(orig, &[x], &[]), term::eval_with_seqs(cand, &[x], &[])) {
            return Err(x);
        }
    }
    Ok(())
}

fn main() {
    let rounds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let mut rng = Lcg(0x1234_5678_9ABC_DEF1);
    let peephole = Peephole::default();

    let mut ran = 0u64;
    let mut fired = 0u64;
    let mut nodes_removed = 0u64;

    for _ in 0..rounds {
        let src = random_term(&mut rng, 5);
        let original = match sdk::sexpr::parse(&src) {
            Ok(t) => t,
            Err(_) => continue,
        };
        ran += 1;
        for cand in peephole.suggest(&original) {
            assert!(
                cand.nodes.len() <= original.nodes.len(),
                "peephole GREW a term: {} -> {} on {src}",
                original.nodes.len(),
                cand.nodes.len()
            );
            if let Err(x) = differential(&original, &cand) {
                panic!("peephole changed semantics at x={x:?} on term {src}");
            }
            fired += 1;
            nodes_removed += (original.nodes.len() - cand.nodes.len()) as u64;
        }
    }

    println!("rounds parsed: {ran}");
    println!("terms simplified: {fired}");
    println!("total nodes removed: {nodes_removed}");
    assert!(fired > ran / 10, "fuzz too weak: plugin fired on only {fired}/{ran} terms");
    println!("OK — every one of {fired} rewrites was non-growing and bit-exact");
}
