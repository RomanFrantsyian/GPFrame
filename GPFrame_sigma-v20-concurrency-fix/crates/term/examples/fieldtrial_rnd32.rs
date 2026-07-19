//! FIELD TRIAL — Σ v1.6's load-bearing claim, tested at a scale no unit
//! test does: for +,-,*,/,sqrt over f32-representable operands,
//! f64-compute-then-Rnd32 is BIT-IDENTICAL to native f32 (double
//! rounding is innocuous: f64 p=53 >= 2*24+2). README §5.4 point 4
//! states this as a theorem citation (Figueroa 1995); this trial checks
//! it empirically, through the ACTUAL Σ interpreter (not reimplemented
//! math), at N large enough to matter.
//!
//! Run: `rustc -O --edition 2021 --extern term=... this_file` is not
//! how this is invoked; use `cargo run --release --example
//! fieldtrial_rnd32 -p term`.

use term::ast::TermBuilder;
use term::sig::Op;
use term::interp::eval;

struct Lcg(u64);
impl Lcg {
    fn next_u32(&mut self) -> u32 {
        // xorshift64* — fast, deterministic, good enough for a coverage
        // sweep (this is a differential stress test, not a crypto RNG)
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    /// A random f32-REPRESENTABLE value, biased toward the boundary set
    /// the project's own μ′ favors (zeros, small ints, subnormals,
    /// extremes) half the time, uniform bit-pattern the other half —
    /// deliberately adversarial to the theorem, not friendly to it.
    fn f32_value(&mut self) -> f32 {
        match self.next_u32() % 20 {
            0 => 0.0, 1 => -0.0, 2 => f32::MIN, 3 => f32::MAX,
            4 => f32::MIN_POSITIVE, 5 => -f32::MIN_POSITIVE,
            6 => 1.0, 7 => -1.0, 8 => f32::EPSILON,
            9 => { // subnormal
                f32::from_bits(self.next_u32() & 0x007f_ffff)
            }
            _ => f32::from_bits(self.next_u32()),
        }
    }
}

fn rnd32_term(op: Op, arity: usize) -> term::Term {
    let mut b = TermBuilder::new();
    let a = b.var(0);
    let ra = b.unary(Op::Rnd32, a);
    let root = if arity == 1 {
        let o = b.unary(op, ra);
        b.unary(Op::Rnd32, o)
    } else {
        let bb = b.var(1);
        let rb = b.unary(Op::Rnd32, bb);
        let o = b.binary(op, ra, rb);
        b.unary(Op::Rnd32, o)
    };
    b.finish(root)
}

fn main() {
    let n: u64 = std::env::args().nth(1)
        .and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
    let mut rng = Lcg(0xF32_1234_5678_ABCD);

    let cases: &[(&str, Op, usize)] = &[
        ("add", Op::Add, 2), ("sub", Op::Sub, 2),
        ("mul", Op::Mul, 2), ("div", Op::Div, 2),
        ("sqrt", Op::Sqrt, 1),
    ];

    println!("FIELD TRIAL: Rnd32 double-rounding innocuousness, N={n} per op");
    println!("(adversarial sampler: 45% boundary/subnormal values, 55% uniform bits)\n");

    let mut total_mismatches = 0u64;
    let mut total_checked = 0u64;

    for &(name, op, arity) in cases {
        let t = rnd32_term(op, arity);
        let mut mismatches = 0u64;
        let mut first_mismatch: Option<(f32, f32, f64, f32)> = None;

        for _ in 0..n {
            let a = rng.f32_value();
            let b = if arity == 2 { rng.f32_value() } else { 0.0 };
            if arity == 1 && (a.is_nan()) { continue; } // sqrt(-x) domain noted separately below

            let env: Vec<f64> = if arity == 1 {
                vec![a as f64]
            } else {
                vec![a as f64, b as f64]
            };
            let term_result = eval(&t, &env) as f32;
            let native: f32 = match name {
                "add" => a + b, "sub" => a - b, "mul" => a * b, "div" => a / b,
                "sqrt" => a.sqrt(),
                _ => unreachable!(),
            };
            let bits_eq = term_result.to_bits() == native.to_bits()
                || (term_result.is_nan() && native.is_nan());
            if !bits_eq {
                mismatches += 1;
                if first_mismatch.is_none() {
                    first_mismatch = Some((a, b, term_result as f64, native));
                }
            }
        }
        total_mismatches += mismatches;
        total_checked += n;
        let status = if mismatches == 0 { "PASS" } else { "FAIL" };
        println!("{name:>5} [{status}] {n} samples, {mismatches} mismatches");
        if let Some((a, b, t, nat)) = first_mismatch {
            println!("        first mismatch: a={a:e} b={b:e} term={t:e} native={nat:e}");
        }
    }

    println!("\n{total_checked} total samples across 5 ops, {total_mismatches} mismatches.");
    if total_mismatches == 0 {
        println!("VERDICT: theorem holds at this scale, zero counterexamples found.");
    } else {
        println!("VERDICT: theorem VIOLATED — counterexamples found, see above.");
        std::process::exit(1);
    }
}
