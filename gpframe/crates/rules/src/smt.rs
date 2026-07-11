//! Z3 bridge — O1 discharge (offline, once per rule) + mutate's eq-filter.
//!
//! Z3 IS trusted base, but only for O1 artifacts (v2.1 §7). It runs as a
//! SUBPROCESS: the artifact discipline wants the raw .smt2 query + solver
//! output on disk, and subprocess keeps libz3 out of our link.
//!
//! Encoding notes (QF_FP, Float64, RNE):
//! * equality claim = SMT object equality `=`: NaN is a single value,
//!   +0 ≠ −0. That is FINER than our Semantic metric ⇒ an UNSAT(inequiv)
//!   proof is sound for semantic equality (and catches the −0.0 traps).
//! * select(c,a,b): (ite (not (fp.eq c ±0)) a b) — matches interp's
//!   `c != 0.0` including the NaN-goes-then behavior.
//! * transcendentals (sin..ln, pow) have no decidable theory (T2) →
//!   Unsupported → verdict Unknown → rule routes to Tier B / triage.
//! * fp.min/fp.max are UNDERSPECIFIED on ±0 in SMT-LIB — exactly mirroring
//!   Rust's unspecified f64::min(±0,∓0); rules touching that corner will
//!   come back sat/unknown, which is the correct (conservative) answer.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use term::{Op, Term};

pub enum SmtVerdict {
    UnsatProved { artifact_path: String },
    SatRefuted { model: Vec<f64> },
    Unknown,
}

pub trait SmtBackend {
    /// Encode [[lhs]] != [[rhs]] over pattern vars and check.
    fn check_rule_inequiv(&mut self, name: &str, lhs: &str, rhs: &str) -> SmtVerdict;
    /// mutate::eqfilter entry: is m(P) ≡ P decidable-fragment-provable?
    fn check_term_inequiv(&mut self, a: &Term, b: &Term) -> SmtVerdict;
}

// ------------------------------------------------------------- encoder --

#[derive(Debug)]
pub struct Unsupported(pub &'static str);

fn smt_op(op: &str) -> Result<(&'static str, usize, bool), Unsupported> {
    // (smt template head, arity, needs RNE)
    Ok(match op {
        "+" => ("fp.add", 2, true),
        "-" => ("fp.sub", 2, true),
        "*" => ("fp.mul", 2, true),
        "/" => ("fp.div", 2, true),
        "min" => ("fp.min", 2, false),
        "max" => ("fp.max", 2, false),
        "neg" => ("fp.neg", 1, false),
        "abs" => ("fp.abs", 1, false),
        "sqrt" => ("fp.sqrt", 1, true),
        "floor" => ("fp.roundToIntegral RTN", 1, false),
        "ceil" => ("fp.roundToIntegral RTP", 1, false),
        "fma" => ("fp.fma", 3, true),
        "select" => ("SELECT", 3, false),
        "sin" | "cos" | "tan" | "exp" | "ln" | "pow" =>
            return Err(Unsupported("transcendental: no decidable theory (T2)")),
        _ => return Err(Unsupported("unknown op")),
    })
}

fn fp_lit(bits: u64) -> String {
    format!("((_ to_fp 11 53) #x{bits:016x})")
}

/// Encode one pattern s-expression over `?vars` into an SMT term; collects
/// var names into `vars`.
fn encode_pattern(tokens: &[String], pos: &mut usize, vars: &mut Vec<String>)
    -> Result<String, Unsupported>
{
    let tok = &tokens[*pos];
    *pos += 1;
    if tok == "(" {
        let head = tokens[*pos].clone();
        *pos += 1;
        let (smt, arity, rne) = smt_op(&head)?;
        let mut args = Vec::new();
        for _ in 0..arity {
            args.push(encode_pattern(tokens, pos, vars)?);
        }
        assert_eq!(tokens[*pos], ")", "malformed pattern");
        *pos += 1;
        if smt == "SELECT" {
            Ok(format!(
                "(ite (not (fp.eq {} {})) {} {})",
                args[0], fp_lit(0), args[1], args[2]
            ))
        } else if rne {
            Ok(format!("({smt} RNE {})", args.join(" ")))
        } else {
            Ok(format!("({smt} {})", args.join(" ")))
        }
    } else if let Some(v) = tok.strip_prefix('?') {
        let name = format!("pv_{v}");
        if !vars.contains(&name) { vars.push(name.clone()); }
        Ok(name)
    } else if let Ok(f) = tok.parse::<f64>() {
        Ok(fp_lit(f.to_bits()))
    } else {
        Err(Unsupported("unparsable token"))
    }
}

fn lex(src: &str) -> Vec<String> {
    src.replace('(', " ( ").replace(')', " ) ")
        .split_whitespace().map(String::from).collect()
}

/// Full query for `exists e. lhs(e) != rhs(e)`.
pub fn encode_rule_query(lhs: &str, rhs: &str) -> Result<String, Unsupported> {
    let mut vars = Vec::new();
    let lt = lex(lhs);
    let rt = lex(rhs);
    let (mut lp, mut rp) = (0, 0);
    let l = encode_pattern(&lt, &mut lp, &mut vars)?;
    let r = encode_pattern(&rt, &mut rp, &mut vars)?;
    let mut q = String::from("(set-logic QF_FP)\n");
    for v in &vars {
        let _ = writeln!(q, "(declare-const {v} Float64)");
    }
    let _ = writeln!(q, "(define-fun lhs () Float64 {l})");
    let _ = writeln!(q, "(define-fun rhs () Float64 {r})");
    q.push_str("(assert (distinct lhs rhs))\n(check-sat)\n(get-model)\n");
    Ok(q)
}

/// Encode a concrete Term (vars → declared consts) — eq-filter path.
fn encode_term(t: &Term) -> Result<String, Unsupported> {
    let mut sub: Vec<String> = Vec::with_capacity(t.len());
    for n in &t.nodes {
        let s = match n.op {
            Op::Const => fp_lit(t.consts[n.a as usize].to_bits()),
            Op::Var => format!("tv_{}", n.a),
            op => {
                let (smt, arity, rne) = smt_op(op.name())?;
                let args: Vec<&str> = [n.a, n.b, n.c][..arity]
                    .iter().map(|&k| sub[k as usize].as_str()).collect();
                if smt == "SELECT" {
                    format!("(ite (not (fp.eq {} {})) {} {})",
                        args[0], fp_lit(0), args[1], args[2])
                } else if rne {
                    format!("({smt} RNE {})", args.join(" "))
                } else {
                    format!("({smt} {})", args.join(" "))
                }
            }
        };
        sub.push(s);
    }
    Ok(sub[t.root as usize].clone())
}

pub fn encode_term_query(a: &Term, b: &Term) -> Result<String, Unsupported> {
    let ea = encode_term(a)?;
    let eb = encode_term(b)?;
    let arity = a.arity().max(b.arity());
    let mut q = String::from("(set-logic QF_FP)\n");
    for i in 0..arity {
        let _ = writeln!(q, "(declare-const tv_{i} Float64)");
    }
    let _ = writeln!(q, "(assert (distinct {ea} {eb}))");
    q.push_str("(check-sat)\n(get-model)\n");
    Ok(q)
}

// ------------------------------------------------------------- backend --

/// Z3-as-subprocess. Artifacts (query + full solver output) land in
/// `artifact_dir` — they ARE the O1 evidence referenced by certificates.
pub struct Z3Cli {
    pub artifact_dir: PathBuf,
    pub timeout_ms: u64,
}

impl Z3Cli {
    pub fn new(artifact_dir: impl Into<PathBuf>) -> Self {
        Self { artifact_dir: artifact_dir.into(), timeout_ms: 10_000 }
    }

    pub fn available() -> bool {
        Command::new("z3").arg("--version").output()
            .map(|o| o.status.success()).unwrap_or(false)
    }

    fn run_query(&self, stem: &str, query: &str) -> SmtVerdict {
        fs::create_dir_all(&self.artifact_dir).ok();
        let qpath = self.artifact_dir.join(format!("{stem}.smt2"));
        let opath = self.artifact_dir.join(format!("{stem}.out"));
        if fs::write(&qpath, query).is_err() {
            return SmtVerdict::Unknown;
        }
        let out = Command::new("z3")
            .arg(format!("-T:{}", self.timeout_ms.div_ceil(1000)))
            .arg(&qpath)
            .output();
        let Ok(out) = out else { return SmtVerdict::Unknown };
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        fs::write(&opath, &text).ok();
        match text.lines().next().map(str::trim) {
            Some("unsat") => SmtVerdict::UnsatProved {
                artifact_path: opath.display().to_string(),
            },
            Some("sat") => SmtVerdict::SatRefuted { model: parse_fp_model(&text) },
            _ => SmtVerdict::Unknown,
        }
    }
}

impl SmtBackend for Z3Cli {
    fn check_rule_inequiv(&mut self, name: &str, lhs: &str, rhs: &str) -> SmtVerdict {
        match encode_rule_query(lhs, rhs) {
            Ok(q) => self.run_query(name, &q),
            Err(_) => SmtVerdict::Unknown,
        }
    }

    fn check_term_inequiv(&mut self, a: &Term, b: &Term) -> SmtVerdict {
        let stem = format!("eqfilter_{:016x}_{:016x}", a.hash, b.hash);
        match encode_term_query(a, b) {
            Ok(q) => self.run_query(&stem, &q),
            Err(_) => SmtVerdict::Unknown,
        }
    }
}

/// No-solver fallback: everything Unknown ⇒ human triage. Keeps the
/// undecidable-residue path honest on machines without z3.
pub struct NullBackend;

impl SmtBackend for NullBackend {
    fn check_rule_inequiv(&mut self, _n: &str, _l: &str, _r: &str) -> SmtVerdict {
        SmtVerdict::Unknown
    }
    fn check_term_inequiv(&mut self, _a: &Term, _b: &Term) -> SmtVerdict {
        SmtVerdict::Unknown
    }
}

// ----------------------------------------------------------- discharge --

/// O1 discharge for a rule table: every Dec rule must come back UNSAT.
/// Returns (proved names, rejected names, unknown names).
pub fn discharge_all(
    rules: &[crate::Rule],
    backend: &mut dyn SmtBackend,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut proved = Vec::new();
    let mut rejected = Vec::new();
    let mut unknown = Vec::new();
    for r in rules {
        if !matches!(r.class, crate::RuleClass::Dec { .. }) { continue; }
        match backend.check_rule_inequiv(r.name, r.lhs, r.rhs) {
            SmtVerdict::UnsatProved { .. } => proved.push(r.name.to_string()),
            SmtVerdict::SatRefuted { .. } => rejected.push(r.name.to_string()),
            SmtVerdict::Unknown => unknown.push(r.name.to_string()),
        }
    }
    (proved, rejected, unknown)
}

/// Load-time artifact check (R2 entry condition): a Dec rule may only enter
/// the active set if its UNSAT artifact is on disk.
pub fn artifact_ok(dir: &Path, rule_name: &str) -> bool {
    let out = dir.join(format!("{rule_name}.out"));
    fs::read_to_string(&out)
        .map(|s| s.lines().next().map(str::trim) == Some("unsat"))
        .unwrap_or(false)
}

// -------------------------------------------------------- model parsing --

/// Parse Z3's `(get-model)` output into env values, ordered by variable:
/// `tv_<i>` names map to env index i (term eq-filter); `pv_*` names are
/// returned in name order (rule counterexamples, informational).
/// Handles both bit-triple literals `(fp #bS #bEEE #xMMM)` and the special
/// forms `(_ +zero 11 53)`, `-zero`, `NaN`, `+oo`, `-oo`.
pub fn parse_fp_model(text: &str) -> Vec<f64> {
    let mut named: Vec<(String, f64)> = Vec::new();
    let compact = text.replace('\n', " ");
    let mut rest = compact.as_str();
    while let Some(i) = rest.find("(define-fun ") {
        rest = &rest[i + "(define-fun ".len()..];
        let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        // scan forward to the value literal within this define-fun
        let val = if let Some(j) = rest.find("(fp ") {
            let (k, special) = rest.find("(_ ").map(|k| (k, true)).unwrap_or((usize::MAX, false));
            if special && k < j { parse_special(&rest[k..]) } else { parse_fp_triple(&rest[j..]) }
        } else if let Some(k) = rest.find("(_ ") {
            parse_special(&rest[k..])
        } else {
            None
        };
        if let Some(v) = val {
            named.push((name, v));
        }
    }
    // tv_<i> → positional env; otherwise name order
    if named.iter().all(|(n, _)| n.starts_with("tv_")) {
        let mut env = Vec::new();
        for (n, v) in &named {
            if let Ok(i) = n[3..].parse::<usize>() {
                if env.len() <= i { env.resize(i + 1, 0.0); }
                env[i] = *v;
            }
        }
        env
    } else {
        named.sort_by(|a, b| a.0.cmp(&b.0));
        named.into_iter().map(|(_, v)| v).collect()
    }
}

fn parse_fp_triple(s: &str) -> Option<f64> {
    // (fp #b<1> #b<11> #x<13 hex>)  — sign, biased exponent, significand
    let s = s.strip_prefix("(fp ")?;
    let mut parts = s.split_whitespace();
    let sign = parts.next()?.strip_prefix("#b")?;
    let expo = parts.next()?.strip_prefix("#b")?;
    let mant_raw = parts.next()?;
    let mant = mant_raw.trim_end_matches(')');
    let sign = u64::from_str_radix(sign, 2).ok()?;
    let expo = u64::from_str_radix(expo, 2).ok()?;
    let mant = if let Some(h) = mant.strip_prefix("#x") {
        u64::from_str_radix(h, 16).ok()?
    } else if let Some(b) = mant.strip_prefix("#b") {
        u64::from_str_radix(b, 2).ok()?
    } else {
        return None;
    };
    Some(f64::from_bits((sign << 63) | (expo << 52) | mant))
}

fn parse_special(s: &str) -> Option<f64> {
    let s = s.strip_prefix("(_ ")?;
    let head: String = s.chars().take_while(|c| !c.is_whitespace()).collect();
    match head.as_str() {
        "+zero" => Some(0.0),
        "-zero" => Some(-0.0),
        "NaN" => Some(f64::NAN),
        "+oo" => Some(f64::INFINITY),
        "-oo" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}
