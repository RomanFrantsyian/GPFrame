//! `dge lift <file.ll|file.rs> <fn_name>` — the SECOND FRONT DOOR (v3-Exp P1):
//! LLVM IR → Term_p.
//!
//! Mission (RFC-REVIEW-ir-lifting-v3): DGE reads what the CPU is told — the
//! SSA dataflow the compiler lowered every syntax costume into — and recovers
//! the theorem hiding in it. One lifter covers Rust AND C/C++, because
//! `rustc --emit=llvm-ir` and `clang -emit-llvm -S` meet in the same text
//! format. The syn extractor remains the clean-code fast path and the
//! emission round-trip closure; this door exists for everything whose
//! *surface* syntax the extractor refuses but whose *instruction-level*
//! meaning is pure f64 math.
//!
//! P1 scope (straight-line; the phased plan is gate-arbitrated):
//!   * exactly ONE basic block — `br`/`phi` are refused with the P2 roadmap
//!     (natural-loop + accumulator-phi recognition → Fold), never guessed at
//!   * no memory ops — `load`/`store`/`alloca`/`getelementptr` are refused
//!     with the P3 roadmap (side-effect slicing + re-stitching)
//!   * `fneg fadd fsub fmul fdiv`, ordered `fcmp` (olt/ogt/ole/oge — the four
//!     Σ v1.1 comparisons; Rust `<` lowers to `fcmp olt`), scalar `select`,
//!     and `call` restricted to a closed libm/intrinsic map mirroring Σ
//!   * `double` only — f32, ints (beyond the i1 feeding select), and vectors
//!     are refusals with reasons
//!   * fast-math flags are REFUSED: they license the compiler to relax the
//!     IEEE semantics Σ is defined over, so a flagged instruction's meaning
//!     is not the Σ op's meaning. Claim discipline over coverage.
//!
//! Recommended IR flags (per the review): `-C opt-level=1` / `clang -O1` —
//! mem2reg has run (locals are SSA values, not stack traffic) but fp-contract
//! has not manufactured fmas that were never in the source. rustc never
//! contracts by default; for clang pass `-ffp-contract=off`.
//!
//! LIFTING IS A LOWERING IN REVERSE (L1) AND IS NOT TRUSTED. The lifter's
//! recognition can be wrong in any way whatsoever and correctness does not
//! move: its output must pass the extraction gate — BitwiseNanClass
//! differential of interp(term) against the compiled original over μ′.
//! Recognition failures are refusals with reasons, never silent guesses.

use std::collections::HashMap;
use term::{Op, Term, TermBuilder};

#[derive(Debug)]
pub enum LiftError {
    /// input isn't (this subset of) LLVM IR text
    Parse(String),
    /// no `define` with a matching symbol
    FnNotFound(String),
    /// several mangled symbols match the requested name
    Ambiguous(Vec<String>),
    /// recognized IR, outside the P1 contract — message names the phase
    /// that will admit it, or the reason it never will be admitted
    Refused(String),
}

impl std::fmt::Display for LiftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiftError::Parse(m) => write!(f, "parse: {m}"),
            LiftError::FnNotFound(n) => write!(f, "no `define double @…{n}…(double, …)` found"),
            LiftError::Ambiguous(v) => write!(f, "ambiguous symbol; candidates: {v:?}"),
            LiftError::Refused(m) => write!(f, "refused: {m}"),
        }
    }
}

/// The closed call map: LLVM symbol / intrinsic → Σ op. Everything outside
/// this table is a refusal. Semantics notes:
///  * `llvm.minnum/maxnum` == Rust `f64::min/max` == Σ Min/Max (NaN → other
///    operand) — the SAME semantics O7 pinned when it refuted CLIF `fmin`.
///  * `log` is natural log (Σ Ln); `log2`/`log10`/`exp2` have no Σ symbol
///    yet, so they refuse rather than approximate.
const CALL_MAP: &[(&str, Op)] = &[
    ("llvm.sqrt.f64", Op::Sqrt), ("sqrt", Op::Sqrt),
    ("llvm.fabs.f64", Op::Abs),  ("fabs", Op::Abs),
    ("llvm.floor.f64", Op::Floor), ("floor", Op::Floor),
    ("llvm.ceil.f64", Op::Ceil), ("ceil", Op::Ceil),
    ("llvm.sin.f64", Op::Sin),   ("sin", Op::Sin),
    ("llvm.cos.f64", Op::Cos),   ("cos", Op::Cos),
    ("tan", Op::Tan),
    ("llvm.exp.f64", Op::Exp),   ("exp", Op::Exp),
    // llvm.exp2 is what LibCallSimplifier turns `powf(2.0, x)` into
    // (MEASURED on easer's expo family) — O8: interp's f64::exp2 links the
    // same libm exp2 this lowers to.
    ("llvm.exp2.f64", Op::Exp2), ("exp2", Op::Exp2),
    ("llvm.log.f64", Op::Ln),    ("log", Op::Ln),
    ("llvm.pow.f64", Op::Pow),   ("pow", Op::Pow),
    ("llvm.fma.f64", Op::Fma),   ("fma", Op::Fma),
    ("llvm.minnum.f64", Op::Min), ("fmin", Op::Min),
    ("llvm.maxnum.f64", Op::Max), ("fmax", Op::Max),
    // rustc 1.97 (MEASURED on this box) lowers f64::min/max to the IEEE
    // 754-2019 *num intrinsics. NaN → other operand, same as Rust/Σ.
    // llvm.minimum/maximum (2018, NaN-PROPAGATING) are deliberately absent:
    // they are the CLIF-fmin semantics O7 refuted — closed map keeps them out.
    ("llvm.minimumnum.f64", Op::Min),
    ("llvm.maximumnum.f64", Op::Max),
];

const FMF: &[&str] = &["fast", "nnan", "ninf", "nsz", "arcp", "contract", "afn", "reassoc"];

/// Lift `fn_name` from LLVM IR text into Term_p. UNTRUSTED — gate the result.
///
/// Dispatch: one basic block → the P1 straight-line path; multiple blocks →
/// the P2 fold recognizer (canonical counted loop → Σ Fold). Anything the
/// recognizer cannot POSITIVELY identify is a refusal with a reason.
pub fn lift_ll(ir: &str, fn_name: &str) -> Result<Term, LiftError> {
    let (sig_line, body) = find_define(ir, fn_name)?;
    let blocks = split_blocks(&body)?;
    if blocks.len() > 1 {
        return fold::lift_fold(&sig_line, &blocks);
    }

    let params = parse_params(&sig_line)?;

    let mut b = TermBuilder::new();
    // SSA value name → node id. i1 comparison results live in the same map:
    // Σ comparisons are 1.0/0.0-valued and Select tests ≠ 0.0, so the i1
    // world embeds exactly.
    let mut env: HashMap<String, u32> = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        env.insert(p.clone(), b.var(i as u32));
    }

    let mut labels_seen = 0usize;
    let mut ret: Option<u32> = None;

    for raw in body {
        let line = strip_comment(raw).trim();
        if line.is_empty() { continue; }
        if line.ends_with(':') && !line.contains(char::is_whitespace) {
            labels_seen += 1;
            if labels_seen > 1 {
                return Err(LiftError::Refused(
                    "multiple basic blocks — loops/branches are P2 \
                     (natural-loop + phi → Fold recognition); P1 is straight-line".into()));
            }
            continue;
        }
        if ret.is_some() {
            return Err(LiftError::Parse("instruction after ret".into()));
        }
        if let Some(rest) = line.strip_prefix("ret ") {
            ret = Some(operand(rest.trim().strip_prefix("double").ok_or_else(||
                LiftError::Refused("non-double return".into()))?.trim(),
                &env, &mut b)?);
            continue;
        }
        // %name = <inst> …
        let (dst, inst) = line.split_once('=')
            .ok_or_else(|| refuse_inst(line))?;
        let dst = dst.trim();
        if !dst.starts_with('%') {
            return Err(LiftError::Parse(format!("expected SSA destination: `{line}`")));
        }
        let id = instruction(inst.trim(), &env, &mut b)?;
        env.insert(dst[1..].to_string(), id);
    }

    let root = ret.ok_or_else(|| LiftError::Parse("no `ret double` found".into()))?;
    Ok(b.finish(root))
}

// ------------------------------------------------------------------ define --

/// Find the matching `define`; return its signature line and body lines.
/// Matching: exact symbol, or an Itanium-mangled segment `{len}{name}` (what
/// rustc emits: `_ZN5crate4name17h…E`) — so callers ask for the SOURCE name.
fn find_define<'a>(ir: &'a str, fn_name: &str) -> Result<(String, Vec<&'a str>), LiftError> {
    let seg = format!("{}{}", fn_name.len(), fn_name);
    let mut hits: Vec<(String, Vec<&str>)> = Vec::new();
    let mut lines = ir.lines().peekable();
    while let Some(l) = lines.next() {
        let lt = l.trim_start();
        if !lt.starts_with("define") { continue; }
        let sym = symbol_of(lt).ok_or_else(|| LiftError::Parse(format!("bad define: `{lt}`")))?;
        if sym != fn_name && !sym.contains(&seg) { continue; }
        let mut body = Vec::new();
        for bl in lines.by_ref() {
            if bl.trim() == "}" { break; }
            body.push(bl);
        }
        hits.push((lt.to_string(), body));
    }
    // exact symbol beats mangled-segment hits: closures nested in the target
    // fn also carry the `{len}{name}` segment (e.g. `…9iter_dot30E…` is
    // closure #0 IN iter_dot3), and the driver's #[no_mangle] injection makes
    // the exact form the common case anyway.
    if hits.iter().filter(|(s, _)| symbol_of(s) == Some(fn_name)).count() == 1 {
        hits.retain(|(s, _)| symbol_of(s) == Some(fn_name));
    }
    match hits.len() {
        0 => Err(LiftError::FnNotFound(fn_name.into())),
        1 => {
            let (sig, body) = hits.pop().unwrap();
            let before_at = sig.split('@').next().unwrap_or("");
            if !before_at.split_whitespace().any(|t| t == "double") {
                return Err(LiftError::Refused(
                    "return type is not double — P1 lifts pure f64 functions only".into()));
            }
            Ok((sig, body))
        }
        _ => Err(LiftError::Ambiguous(
            hits.iter().map(|(s, _)| symbol_of(s).unwrap_or_default().to_string()).collect())),
    }
}

fn symbol_of(define_line: &str) -> Option<&str> {
    let at = define_line.find('@')?;
    let rest = &define_line[at + 1..];
    let end = rest.find('(')?;
    Some(rest[..end].trim_matches('"'))
}

/// Parse the `(double %a, double noundef %b, …)` list → param SSA names.
fn parse_params(sig: &str) -> Result<Vec<String>, LiftError> {
    let open = sig.find('(').ok_or_else(|| LiftError::Parse("no `(`".into()))?;
    let close = sig.rfind(')').ok_or_else(|| LiftError::Parse("no `)`".into()))?;
    let inner = &sig[open + 1..close];
    if inner.trim().is_empty() {
        return Err(LiftError::Refused("nullary function — nothing to lift over μ′".into()));
    }
    let mut out = Vec::new();
    for p in inner.split(',') {
        let toks: Vec<&str> = p.split_whitespace().collect();
        if toks.first() != Some(&"double") {
            return Err(LiftError::Refused(format!(
                "non-double parameter `{}` — pointers/ints are P3 (memory & slicing); \
                 &[f64] sequence params are P2 (Fold)", p.trim())));
        }
        let name = toks.iter().rev().find(|t| t.starts_with('%')).ok_or_else(||
            LiftError::Parse(format!("unnamed parameter `{}`", p.trim())))?;
        out.push(name[1..].to_string());
    }
    Ok(out)
}

// ------------------------------------------------------------ instructions --

fn refuse_inst(line: &str) -> LiftError {
    let head = line.split_whitespace().next().unwrap_or("");
    let msg = match head {
        "br" | "phi" | "switch" =>
            "control flow (`br`/`phi`) — P2: natural-loop detection, induction \
             variable + accumulator-phi recognition → Fold; P1 is straight-line",
        "load" | "store" | "alloca" | "getelementptr" =>
            "memory operation — P3: side-effect slicing + re-stitching \
             (deferred until the field trial quantifies the coverage gap)",
        "sitofp" | "uitofp" | "fptosi" | "fptoui" | "fpext" | "fptrunc" | "zext" | "sext" =>
            "int/float conversion — outside the pure-f64 P1 alphabet",
        "add" | "sub" | "mul" | "sdiv" | "udiv" | "and" | "or" | "xor" | "shl" | "lshr" | "ashr" =>
            "integer arithmetic — outside the pure-f64 P1 alphabet",
        "frem" => "frem has no Σ symbol (no decidable theory, no rule set) — refused",
        _ => return LiftError::Parse(format!("unrecognized instruction: `{line}`")),
    };
    LiftError::Refused(format!("`{head}`: {msg}"))
}

fn instruction(inst: &str, env: &HashMap<String, u32>, b: &mut TermBuilder)
    -> Result<u32, LiftError>
{
    let mut toks = inst.split_whitespace();
    let head = toks.next().ok_or_else(|| LiftError::Parse("empty instruction".into()))?;
    let rest = inst[head.len()..].trim();

    match head {
        "fneg" => {
            let a = one_double_operand(rest, env, b)?;
            Ok(b.unary(Op::Neg, a))
        }
        "fadd" | "fsub" | "fmul" | "fdiv" => {
            let op = match head {
                "fadd" => Op::Add, "fsub" => Op::Sub, "fmul" => Op::Mul, _ => Op::Div,
            };
            let (a, c) = two_double_operands(rest, env, b)?;
            Ok(b.binary(op, a, c))
        }
        "fcmp" => {
            // flags precede the predicate: `fcmp nsz olt double …`
            let pred = rest.split_whitespace().next()
                .ok_or_else(|| LiftError::Parse("fcmp: no predicate".into()))?;
            if FMF.contains(&pred) {
                return Err(fmf_refusal(inst));
            }
            let op = match pred {
                "olt" => Op::Lt, "ogt" => Op::Gt, "ole" => Op::Le, "oge" => Op::Ge,
                // Σ v1.4: Rust `==` is oeq (NaN ⇒ false), Rust `!=` is une
                // (NaN ⇒ true) — exactly this asymmetric pair, nothing else.
                // one/ueq/ult/… are NOT any Rust operator and stay refused.
                "oeq" => Op::Eq, "une" => Op::Ne,
                _ => return Err(LiftError::Refused(format!(
                    "fcmp {pred}: Σ has olt/ogt/ole/oge/oeq/une (the Rust \
                     comparison operators); other predicates have no Σ symbol"))),
            };
            let (a, c) = two_double_operands(rest[pred.len()..].trim(), env, b)?;
            Ok(b.binary(op, a, c))
        }
        "select" => {
            // select i1 %c, double %a, double %b   (scalar only)
            let parts: Vec<&str> = split_top(rest);
            if parts.len() != 3 {
                return Err(LiftError::Parse(format!("select: expected 3 operands: `{inst}`")));
            }
            let cond_t = parts[0].trim();
            let cond = cond_t.strip_prefix("i1").ok_or_else(|| LiftError::Refused(
                "select on a non-i1 condition (vector select?) — refused in P1".into()))?
                .trim();
            let cond_id = *env.get(cond.strip_prefix('%').ok_or_else(||
                LiftError::Parse("select: condition must be an SSA value".into()))?)
                .ok_or_else(|| LiftError::Parse(format!("select: unbound {cond}")))?;
            let a = one_double_operand(parts[1].trim(), env, b)?;
            let c = one_double_operand(parts[2].trim(), env, b)?;
            Ok(b.ternary(Op::Select, cond_id, a, c))
        }
        "tail" | "notail" | "musttail" | "call" => {
            let call_rest = if head == "call" { rest } else {
                rest.strip_prefix("call").ok_or_else(|| refuse_inst(inst))?.trim()
            };
            lift_call(call_rest, env, b)
        }
        h if FMF.contains(&h) => Err(fmf_refusal(inst)),
        _ => Err(refuse_inst(inst)),
    }
}

fn fmf_refusal(inst: &str) -> LiftError {
    LiftError::Refused(format!(
        "fast-math flags on `{inst}` — fmf licenses the compiler to relax the \
         IEEE-754 semantics Σ is defined over; the instruction's meaning is not \
         the Σ op's meaning. Re-emit IR without -ffast-math."))
}

/// `double @sym(double %a[, double %b[, double %c]]) [#N]` after `call`.
fn lift_call(rest: &str, env: &HashMap<String, u32>, b: &mut TermBuilder)
    -> Result<u32, LiftError>
{
    // collect fmf before the return type; other tokens there are benign
    // attrs (noundef, nofpclass(nan), …). fmf is judged AFTER we know the
    // callee — see the nsz-on-min/max carve-out below.
    let mut flags: Vec<&str> = Vec::new();
    let mut r = rest;
    while let Some(tok) = r.split_whitespace().next() {
        if tok == "double" { break; }
        if FMF.contains(&tok) { flags.push(tok); }
        r = r[tok.len()..].trim_start();
        if r.is_empty() { return Err(LiftError::Parse(format!("call: `{rest}`"))); }
    }
    let r = r.strip_prefix("double").map(str::trim_start)
        .ok_or_else(|| LiftError::Refused("call returning non-double — outside P1".into()))?;
    let at = r.strip_prefix('@')
        .ok_or_else(|| LiftError::Refused("indirect call — outside the closed libm map".into()))?;
    let open = at.find('(').ok_or_else(|| LiftError::Parse(format!("call: `{rest}`")))?;
    let sym = at[..open].trim_matches('"');
    let close = at.rfind(')').ok_or_else(|| LiftError::Parse(format!("call: `{rest}`")))?;
    let args_s = &at[open + 1..close];

    // fmf carve-out, exactly one: rustc itself stamps `nsz` on its min/max
    // lowering because Rust f64::min/max leaves the SIGN OF A ZERO RESULT
    // unspecified — and Σ Min/Max is DEFINED as Rust min/max, so it carries
    // the same looseness. Both gate sides compile through this same rustc,
    // and μ′ contains ±0 boundaries: if the platform ever resolved the sign
    // differently from the interpreter, the gate refutes with a witness.
    // Every other flag (or nsz anywhere else) relaxes semantics Σ relies on.
    let nsz_minmax_only = flags == ["nsz"]
        && matches!(sym, "llvm.minimumnum.f64" | "llvm.maximumnum.f64"
                         | "llvm.minnum.f64" | "llvm.maxnum.f64");
    if !flags.is_empty() && !nsz_minmax_only {
        return Err(fmf_refusal(rest));
    }
    let Some((_, op)) = CALL_MAP.iter().find(|(n, _)| *n == sym) else {
        return Err(LiftError::Refused(format!(
            "call @{sym} — outside the closed libm map \
             ({} symbols); anything else is P3 territory or a new Σ op proposal",
            CALL_MAP.len())));
    };
    let args: Vec<u32> = split_top(args_s).into_iter()
        .map(|a| one_double_operand(a.trim(), env, b))
        .collect::<Result<_, _>>()?;
    if args.len() != op.arity() {
        return Err(LiftError::Parse(format!(
            "@{sym}: {} args, Σ {:?} wants {}", args.len(), op, op.arity())));
    }
    Ok(match op.arity() {
        1 => b.unary(*op, args[0]),
        2 => b.binary(*op, args[0], args[1]),
        _ => b.ternary(*op, args[0], args[1], args[2]),
    })
}

// --------------------------------------------------------------- operands --

/// `double %x` / `double 1.0e+00` / bare `%x` after the type was consumed.
fn one_double_operand(s: &str, env: &HashMap<String, u32>, b: &mut TermBuilder)
    -> Result<u32, LiftError>
{
    let s = s.trim();
    let s = s.strip_prefix("double").map(str::trim).unwrap_or(s);
    // param attrs like `noundef %0`
    let s = s.strip_prefix("noundef").map(str::trim).unwrap_or(s);
    operand(s, env, b)
}

/// `double %a, %b` (LLVM prints the type once for homogeneous binops).
fn two_double_operands(s: &str, env: &HashMap<String, u32>, b: &mut TermBuilder)
    -> Result<(u32, u32), LiftError>
{
    let s = s.trim();
    if let Some(tok) = s.split_whitespace().next() {
        if FMF.contains(&tok) { return Err(fmf_refusal(s)); }
    }
    let s = s.strip_prefix("double").map(str::trim)
        .ok_or_else(|| LiftError::Refused(format!(
            "operand type is not double: `{s}` — outside the pure-f64 P1 alphabet")))?;
    let parts = split_top(s);
    if parts.len() != 2 {
        return Err(LiftError::Parse(format!("expected 2 operands: `{s}`")));
    }
    Ok((operand(parts[0].trim(), env, b)?, operand(parts[1].trim(), env, b)?))
}

/// SSA value or float literal → node id.
fn operand(s: &str, env: &HashMap<String, u32>, b: &mut TermBuilder)
    -> Result<u32, LiftError>
{
    let s = s.trim();
    if let Some(name) = s.strip_prefix('%') {
        return env.get(name).copied()
            .ok_or_else(|| LiftError::Parse(format!("unbound SSA value %{name}")));
    }
    Ok(b.constant(parse_fconst(s)?))
}

/// LLVM double literals: decimal-scientific (`-1.500000e+00`) or the exact
/// bit form `0x` + 16 hex digits (IEEE-754 binary64 bits — how LLVM prints
/// anything a short decimal cannot round-trip: NaN, ±inf, most constants).
fn parse_fconst(s: &str) -> Result<f64, LiftError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if hex.len() == 16 {
            if let Ok(bits) = u64::from_str_radix(hex, 16) {
                return Ok(f64::from_bits(bits));
            }
        }
        return Err(LiftError::Refused(format!(
            "non-binary64 float literal `{s}` (0xK/0xH/0xL are fp80/half/fp128)")));
    }
    s.parse::<f64>().map_err(|_| LiftError::Parse(format!("bad float literal `{s}`")))
}

// ----------------------------------------------------------------- blocks --

/// One basic block of the function body, comments stripped.
pub(crate) struct RawBlock {
    label: String,
    lines: Vec<String>,
}

fn split_blocks(body: &[&str]) -> Result<Vec<RawBlock>, LiftError> {
    let mut out: Vec<RawBlock> = Vec::new();
    let mut cur = RawBlock { label: String::new(), lines: Vec::new() };
    let mut started = false;
    for raw in body {
        let line = strip_comment(raw).trim();
        if line.is_empty() { continue; }
        if line.ends_with(':') && !line.contains(char::is_whitespace) {
            if started { out.push(cur); }
            cur = RawBlock { label: line[..line.len() - 1].to_string(), lines: Vec::new() };
            started = true;
            continue;
        }
        started = true;
        cur.lines.push(line.to_string());
    }
    if started { out.push(cur); }
    if out.is_empty() {
        return Err(LiftError::Parse("empty function body".into()));
    }
    Ok(out)
}

// =================================================================== P2 ====
// Loop recovery: the canonical counted loop -> Sigma Fold (v1.2 contract).
//
// MEASURED target shape (rustc 1.97, -C opt-level=1 + --unroll-runtime=false;
// sum / conditional-update / zip-dot all collapse to it):
//
//   entry:  [int machinery: len guard, llvm.umin trip count]
//           br i1 %g, label %exit|%loop, label %loop|%exit
//   loop:   %acc = phi double [ INIT, entry ], [ %next, loop ]   <- exactly one
//           %i   = phi i64    [ 0, entry ],    [ ..., loop ]     <- index
//           gep(seq_ptr, %i) -> load double                      <- Elem(k)
//           ...f64 body (arith / fcmp / select / call-map)...    <- Acc, Elem
//           ...int machinery (i+1, trip compare)...
//           br i1 %c(int), label %loop, label %exit
//   exit:   %r = phi double [ INIT, entry ], [ %next, loop ]     <- LCSSA
//           ...optional straight-line f64 post-processing...
//           ret double ...
//
// Recognition is POSITIVE-ONLY: every line must be individually identified
// as f64 dataflow, sequence read, or integer index machinery, or the whole
// function refuses with a named reason. The int world is deliberately left
// UNINTERPRETED -- we never prove the trip count equals the sequence length;
// the extraction gate (random lengths incl. 0 in mu') is the arbiter, per L1.
mod fold {
    use super::*;

    struct Phi { dst: String, ty: String, inc: Vec<(String, String)> }

    enum Term_ { Ret(String), Br(String), CondBr { cond: String, a: String, b: String } }

    struct Block_ { label: String, phis: Vec<Phi>, insts: Vec<String>, term: Term_ }

    fn refuse(msg: impl std::fmt::Display) -> LiftError {
        LiftError::Refused(format!("P2 fold recognition: {msg}"))
    }

    /// Param kinds: `&[f64]` arrives as a (ptr, i64) pair; bare doubles are
    /// scalar vars. Anything else has no Sigma reading yet.
    fn classify_params(sig: &str)
        -> Result<(HashMap<String, u32>, Vec<String>, Vec<String>), LiftError>
    {
        let open = sig.find('(').ok_or_else(|| LiftError::Parse("no `(`".into()))?;
        let close = sig.rfind(')').ok_or_else(|| LiftError::Parse("no `)`".into()))?;
        let mut seq_ptrs = HashMap::new(); // ptr name -> seq k
        let mut lens = Vec::new();         // i64 names (index machinery)
        let mut scalars = Vec::new();      // double names, in var order
        let mut expect_len = false;
        for p in split_top(&sig[open + 1..close]) {
            let toks: Vec<&str> = p.split_whitespace().collect();
            let (Some(ty), Some(name)) = (toks.first(),
                toks.iter().rev().find(|t| t.starts_with('%'))) else {
                return Err(LiftError::Parse(format!("param `{}`", p.trim())));
            };
            let name = name[1..].to_string();
            match *ty {
                "ptr" if !expect_len => {
                    seq_ptrs.insert(name, seq_ptrs.len() as u32);
                    expect_len = true;
                }
                "i64" if expect_len => { lens.push(name); expect_len = false; }
                "double" if !expect_len => scalars.push(name),
                _ => return Err(refuse(format!(
                    "parameter `{}` -- P2 admits `&[f64]` (ptr+i64 pairs) and f64 \
                     scalars; anything else is P3 territory", p.trim()))),
            }
        }
        if expect_len {
            return Err(refuse("trailing ptr without its i64 length -- not a slice pair"));
        }
        if seq_ptrs.is_empty() {
            return Err(refuse(
                "loop over no sequence parameter -- Term_p folds iterate a runtime \
                 LENGTH; a loop whose trip count is f64 data (e.g. `while x > lim`) \
                 has no Sigma reading. P2 recovers slice Folds (ptr+i64 params)"));
        }
        Ok((seq_ptrs, lens, scalars))
    }

    fn parse_block(rb: &RawBlock) -> Result<Block_, LiftError> {
        let mut phis = Vec::new();
        let mut insts = Vec::new();
        let mut term = None;
        for line in &rb.lines {
            if let Some(rest) = line.strip_prefix("ret ") {
                term = Some(Term_::Ret(rest.trim().to_string()));
            } else if let Some(rest) = line.strip_prefix("br ") {
                let parts = split_top(rest);
                match parts.len() {
                    1 => {
                        let lab = parts[0].trim().strip_prefix("label ")
                            .ok_or_else(|| LiftError::Parse(format!("br: `{line}`")))?
                            .trim().trim_start_matches('%').to_string();
                        term = Some(Term_::Br(lab));
                    }
                    3 => {
                        let cond = parts[0].trim().strip_prefix("i1 ")
                            .ok_or_else(|| LiftError::Parse(format!("br: `{line}`")))?
                            .trim().trim_start_matches('%').to_string();
                        let lab = |s: &str| s.trim().trim_start_matches("label ")
                            .trim().trim_start_matches('%').to_string();
                        term = Some(Term_::CondBr { cond, a: lab(parts[1]), b: lab(parts[2]) });
                    }
                    _ => return Err(LiftError::Parse(format!("br: `{line}`"))),
                }
            } else if line.contains("= phi ") {
                let (dst, rhs) = line.split_once('=').unwrap();
                let rest = rhs.trim().strip_prefix("phi ").unwrap();
                let first_bracket = rest.find('[')
                    .ok_or_else(|| LiftError::Parse(format!("phi: `{line}`")))?;
                let ty = rest[..first_bracket].trim().to_string();
                let mut inc = Vec::new();
                for arm in rest[first_bracket..].split("], [") {
                    let arm = arm.trim_matches(|c| c == '[' || c == ']' || c == ' ');
                    let (v, l) = arm.split_once(',')
                        .ok_or_else(|| LiftError::Parse(format!("phi arm `{arm}`")))?;
                    inc.push((v.trim().to_string(),
                              l.trim().trim_start_matches('%').to_string()));
                }
                if !insts.is_empty() {
                    return Err(LiftError::Parse("phi after instructions".into()));
                }
                phis.push(Phi { dst: dst.trim().trim_start_matches('%').to_string(), ty, inc });
            } else {
                insts.push(line.clone());
            }
        }
        Ok(Block_ {
            label: rb.label.clone(), phis, insts,
            term: term.ok_or_else(|| LiftError::Parse(
                format!("block `{}` has no terminator", rb.label)))?,
        })
    }

    /// Integer index machinery -- recorded, never interpreted (see module doc).
    fn is_int_machinery(rhs: &str) -> bool {
        let head = rhs.split_whitespace().next().unwrap_or("");
        matches!(head, "icmp" | "zext" | "sext" | "trunc"
                     | "add" | "sub" | "mul" | "udiv" | "sdiv"
                     | "or" | "and" | "xor" | "shl" | "lshr" | "ashr")
            || (rhs.contains("call") && (rhs.contains(" i64 @llvm.") || rhs.contains(" i1 @llvm.")))
            || (rhs.starts_with("select i1") && rhs.contains(", i64 "))
    }

    /// Void metadata/hint calls that carry no dataflow.
    fn is_void_hint(line: &str) -> bool {
        line.contains(" void @llvm.")
            && (line.contains("assume") || line.contains("noalias.scope.decl")
                || line.contains("lifetime") || line.contains("experimental"))
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn pass(block: &Block_, seq_ptrs: &HashMap<String, u32>, lens: &[String],
            fenv: &mut HashMap<String, u32>,
            ints: &mut std::collections::HashSet<String>,
            geps: &mut HashMap<String, u32>, in_loop: bool,
            b: &mut TermBuilder) -> Result<(), LiftError>
    {
        for line in &block.insts {
            if is_void_hint(line) { continue; }
            let Some((dst, rhs)) = line.split_once('=') else {
                return Err(LiftError::Parse(format!("unrecognized: `{line}`")));
            };
            let dst = dst.trim().trim_start_matches('%').to_string();
            let rhs = rhs.trim();
            if is_int_machinery(rhs) { ints.insert(dst); continue; }
            if let Some(rest) = rhs.strip_prefix("getelementptr ") {
                if !in_loop {
                    return Err(refuse("address computation outside the loop -- P3"));
                }
                let after_ty = rest.split_once("double,").map(|x| x.1)
                    .ok_or_else(|| refuse("gep on a non-double element -- P3"))?;
                let args = split_top(after_ty);
                let base = args.first().map(|a| a.trim())
                    .and_then(|a| a.strip_prefix("ptr "))
                    .map(|s| s.trim().trim_start_matches('%'))
                    .ok_or_else(|| LiftError::Parse(format!("gep: `{line}`")))?;
                let idx = args.get(1).map(|a| a.trim())
                    .and_then(|a| a.strip_prefix("i64 "))
                    .map(|s| s.trim().trim_start_matches('%'))
                    .ok_or_else(|| LiftError::Parse(format!("gep: `{line}`")))?;
                let Some(k) = seq_ptrs.get(base) else {
                    return Err(refuse(format!(
                        "gep off `%{base}` which is not a sequence parameter -- P3")));
                };
                if !ints.contains(idx) {
                    return Err(refuse(format!(
                        "gep index `%{idx}` is not induction machinery -- \
                         offset/windowed indexing is on the roadmap")));
                }
                geps.insert(dst, *k);
                continue;
            }
            if let Some(rest) = rhs.strip_prefix("load ") {
                let src = rest.strip_prefix("double, ptr ")
                    .and_then(|s| s.split(',').next())
                    .map(|s| s.trim().trim_start_matches('%'))
                    .ok_or_else(|| refuse("load of a non-double -- P3"))?;
                let Some(k) = geps.get(src) else {
                    return Err(refuse(format!(
                        "load from `%{src}` which is not a recognized sequence read -- P3")));
                };
                let e = b.elem(*k);
                fenv.insert(dst, e);
                continue;
            }
            if rhs.starts_with("store ") || rhs.starts_with("alloca") {
                return Err(refuse("memory write -- P3 (side-effect slicing)"));
            }
            if rhs.starts_with("uitofp") || rhs.starts_with("sitofp") {
                // Σ v1.3 (FIELD-TRIAL №1 item 1): `uitofp` of a LENGTH param
                // is Len(k) — the averaging-statistic symbol. Any other
                // int→float conversion is still index-dependent data with no
                // Σ reading.
                let opnd = rhs.split_whitespace()
                    .skip_while(|t| *t != "i64").nth(1)
                    .map(|t| t.trim_start_matches('%'))
                    .unwrap_or("");
                if let Some(k) = lens.iter().position(|l| l == opnd) {
                    let id = b.len_of(k as u32);
                    fenv.insert(dst, id);
                    continue;
                }
                return Err(refuse(format!(
                    "int->float of `%{opnd}` which is not a sequence length -- \
                     Sigma fold bodies are index-blind (Acc/Elem/Len only); \
                     index-dependent values are on the roadmap")));
            }
            // otherwise it must be Sigma-liftable f64 dataflow (P1 machinery)
            match instruction(rhs, fenv, b) {
                Ok(id) => { fenv.insert(dst, id); }
                Err(LiftError::Parse(m)) if m.contains("unbound SSA value") => {
                    return Err(refuse(format!(
                        "`{line}` mixes the integer index world into f64 dataflow \
                         ({m}) -- Sigma fold bodies are index-blind; index-dependent \
                         values are on the roadmap")));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub(super) fn lift_fold(sig: &str, raw: &[RawBlock]) -> Result<Term, LiftError> {
        let blocks: Vec<Block_> = raw.iter().map(parse_block).collect::<Result<_, _>>()?;

        // DISPATCH (trial item 2): a multi-block CFG with NO back-edge is not
        // a loop at all -- it is branching straight-line code (if/else lowered
        // as a diamond), and its phis are Selects wearing a CFG costume.
        let has_loop = blocks.iter().any(|bl| matches!(&bl.term,
            Term_::CondBr { a, b: bb, .. } if *a == bl.label || *bb == bl.label));
        if !has_loop {
            return lift_acyclic(sig, &blocks);
        }
        if !(3..=5).contains(&raw.len()) {
            return Err(refuse(format!(
                "{} basic blocks -- the canonical counted loop is \
                 entry -> (preheader?) -> loop -> (LCSSA tail?) -> merge (3-5). \
                 Runtime-unrolled or nested control flow refuses (P2 covers the \
                 single natural loop); re-emit with \
                 -C llvm-args=--unroll-runtime=false or wait for P3", raw.len())));
        }
        let (seq_ptrs, lens, scalars) = classify_params(sig)?;

        // roles: entry = first; loop = self-branching; exit = ret
        let entry = &blocks[0];
        let lp = blocks.iter().find(|bl| matches!(&bl.term,
                Term_::CondBr { a, b: bb, .. } if *a == bl.label || *bb == bl.label))
            .expect("has_loop checked above");
        let exit = blocks.iter().find(|bl| matches!(bl.term, Term_::Ret(_)))
            .ok_or_else(|| LiftError::Parse("no ret block".into()))?;
        // optional LCSSA tail: the block the loop exits INTO when that block
        // is not the merge itself -- post-loop f64 straight-line, then an
        // unconditional br to the merge (MEASURED: rustc puts `acc.sqrt()`
        // there, and constant-folds sqrt(init) into the merge phi's entry arm)
        let loop_exit_target = match &lp.term {
            Term_::CondBr { a, b, .. } => if a == &lp.label { b } else { a },
            _ => unreachable!("loop chosen by self-CondBr"),
        };
        let tail = if loop_exit_target != &exit.label {
            let t = blocks.iter().find(|bl| bl.label == *loop_exit_target)
                .ok_or_else(|| LiftError::Parse("dangling loop exit label".into()))?;
            if !t.phis.is_empty() || !matches!(&t.term, Term_::Br(l) if *l == exit.label) {
                return Err(refuse(
                    "the loop's exit block is not a straight-line tail into the \
                     merge -- nested or multi-exit control flow is P3"));
            }
            Some(t)
        } else { None };
        if !entry.phis.is_empty() {
            return Err(refuse("entry block has phis -- merged control flow before the loop"));
        }
        if entry.label == lp.label || entry.label == exit.label || lp.label == exit.label {
            return Err(refuse("entry/loop/exit roles are not three distinct blocks"));
        }
        // optional PREHEADER: entry guards to {exit, X} where X != loop; X must
        // be phi-free straight-line falling through to the loop (MEASURED: LICM
        // parks hoisted loop-invariants there -- e.g. `1.0 - alpha` for an EMA.
        // That is Sigma v1.2's outside-node hoisting, materialized by LLVM).
        let preheader = match &entry.term {
            Term_::CondBr { cond: _, a, b } if a != b
                && (a == &exit.label || b == &exit.label) => {
                let into = if a == &exit.label { b } else { a };
                if into == &lp.label { None } else {
                    let ph = blocks.iter().find(|bl| bl.label == *into)
                        .ok_or_else(|| LiftError::Parse("dangling entry target".into()))?;
                    if !ph.phis.is_empty()
                        || !matches!(&ph.term, Term_::Br(l) if *l == lp.label) {
                        return Err(refuse(
                            "the block between the guard and the loop is not a \
                             straight-line preheader -- nested control flow is P3"));
                    }
                    Some(ph)
                }
            }
            _ => return Err(refuse("entry must guard-branch to {loop|preheader, exit}")),
        };
        // every block must have a recognized role -- an unreachable or extra
        // block means this is not the canonical shape
        let expected = 3 + usize::from(preheader.is_some()) + usize::from(tail.is_some());
        if raw.len() != expected {
            return Err(refuse(format!(
                "{} blocks but only {expected} recognized roles -- unaccounted \
                 control flow is P3", raw.len())));
        }

        let mut b = TermBuilder::new();
        let mut fenv: HashMap<String, u32> = HashMap::new();
        let mut ints: std::collections::HashSet<String> = lens.iter().cloned().collect();
        for (i, s) in scalars.iter().enumerate() {
            fenv.insert(s.clone(), b.var(i as u32));
        }
        let mut geps: HashMap<String, u32> = HashMap::new();

        // ---- entry + preheader: loop-invariant f64 (hoisted -- Sigma v1.2
        // semantics) + the zero-length guard
        pass(entry, &seq_ptrs, &lens, &mut fenv, &mut ints, &mut geps, false, &mut b)?;
        if let Term_::CondBr { cond, .. } = &entry.term {
            if !ints.contains(cond.as_str()) {
                return Err(refuse("entry guard is not an integer length test"));
            }
        }
        if let Some(ph) = preheader {
            pass(ph, &seq_ptrs, &lens, &mut fenv, &mut ints, &mut geps, false, &mut b)?;
        }

        // ---- loop phis: ONE double accumulator; i64 induction machinery
        let mut acc_phi: Option<&Phi> = None;
        for p in &lp.phis {
            match p.ty.as_str() {
                "double" => {
                    if acc_phi.replace(p).is_some() {
                        return Err(refuse(
                            "two f64 accumulator phis -- multi-accumulator folds are \
                             on the roadmap (Sigma v1.2 is single-accumulator)"));
                    }
                }
                "i64" => {
                    let ok = p.inc.iter().all(|(v, l)|
                        *l == lp.label || v == "0" || v == "1"
                        || ints.contains(v.trim_start_matches('%')));
                    if !ok {
                        return Err(refuse(format!(
                            "i64 phi `%{}` does not start at 0/1 -- non-zero range \
                             starts are on the roadmap", p.dst)));
                    }
                    ints.insert(p.dst.clone());
                }
                t => return Err(refuse(format!("phi of type `{t}` in the loop"))),
            }
        }
        let acc_phi = acc_phi.ok_or_else(|| refuse(
            "no f64 accumulator phi -- the loop computes nothing Sigma can fold"))?;
        let init_of = |p: &Phi| p.inc.iter()
            .find(|(_, l)| *l != lp.label).map(|(v, _)| v.clone());
        let next_of = |p: &Phi| p.inc.iter()
            .find(|(_, l)| *l == lp.label).map(|(v, _)| v.clone());
        let (Some(init_v), Some(next_v)) = (init_of(acc_phi), next_of(acc_phi)) else {
            return Err(LiftError::Parse("accumulator phi is not 2-armed entry/loop".into()));
        };

        // init strictly precedes the fold body in the arena (children-first)
        let init_id = operand(&init_v, &fenv, &mut b)?;
        let acc_id = b.acc();
        fenv.insert(acc_phi.dst.clone(), acc_id);

        // ---- loop body
        pass(lp, &seq_ptrs, &lens, &mut fenv, &mut ints, &mut geps, true, &mut b)?;
        if let Term_::CondBr { cond, .. } = &lp.term {
            if !ints.contains(cond.as_str()) {
                return Err(refuse(
                    "loop condition depends on f64 data -- not a counted 0..n loop; \
                     Term_p folds iterate a runtime LENGTH (`while x > lim` has no \
                     Sigma reading)"));
            }
        }
        let body_root = *fenv.get(next_v.trim_start_matches('%')).ok_or_else(|| refuse(
            "the accumulator's next value was not recognized as f64 dataflow"))?;

        // ---- the fold exists now; everything after the loop sees the FOLD
        // node wherever it names the body result (LCSSA rebinding in reverse)
        let fold_id = b.fold(init_id, body_root);
        fenv.insert(next_v.trim_start_matches('%').to_string(), fold_id);

        // ---- optional LCSSA tail: post-loop f64 applied to the fold result.
        // Any reference it makes to OTHER in-body values (a last-iteration
        // live-out) wires Elem/Acc outside the fold -- the fold_owners
        // validator below turns that into a refusal.
        let mut tail_result = next_v.clone();
        if let Some(t) = tail {
            pass(t, &seq_ptrs, &lens, &mut fenv, &mut ints, &mut geps, false, &mut b)?;
            tail_result = t.insts.last().and_then(|l| l.split_once('='))
                .map(|(d, _)| d.trim().to_string())
                .ok_or_else(|| refuse("empty LCSSA tail block"))?;
        }

        // ---- merge: the phi's loop-side arm must be the (post-processed)
        // accumulator. Its entry-side arm is LLVM's constant-folded value of
        // the tail applied to init -- we deliberately do NOT verify that
        // folding (the lifter is untrusted); mu' samples L = 0, so the gate
        // arbitrates the empty-sequence reading directly.
        if exit.phis.len() != 1 || exit.phis[0].ty != "double" {
            return Err(refuse(
                "exit must merge exactly one f64 value -- multiple live-outs are \
                 on the roadmap"));
        }
        let lcssa = &exit.phis[0];
        // the merge's "from the computation" predecessor is the tail block
        // when one exists, the loop block otherwise
        let comp_label = tail.map(|t| t.label.as_str()).unwrap_or(lp.label.as_str());
        let comp_arm = lcssa.inc.iter()
            .find(|(_, l)| l == comp_label).map(|(v, _)| v.clone());
        let entry_arm = lcssa.inc.iter()
            .find(|(_, l)| l != comp_label).map(|(v, _)| v.clone());
        let loop_side_ok = comp_arm.as_deref() == Some(tail_result.as_str());
        let entry_side_ok = if tail.is_some() {
            entry_arm.is_some()
        } else {
            entry_arm.as_ref().map(|v| *v == init_v
                || matches!((parse_fconst(v), parse_fconst(&init_v)),
                            (Ok(x), Ok(y)) if x.to_bits() == y.to_bits()))
                .unwrap_or(false)
        };
        if !loop_side_ok || !entry_side_ok {
            return Err(refuse(
                "exit phi does not merge {accumulator init, accumulator next} -- \
                 the loop's live-out is not the accumulator"));
        }
        let merged = *fenv.get(tail_result.trim_start_matches('%'))
            .ok_or_else(|| refuse("tail result is not f64 dataflow"))?;
        fenv.insert(lcssa.dst.clone(), merged);

        // optional straight-line f64 post-processing, then ret
        pass(exit, &seq_ptrs, &lens, &mut fenv, &mut ints, &mut geps, false, &mut b)?;
        let Term_::Ret(r) = &exit.term else { unreachable!("exit chosen by Ret") };
        let root = operand(r.trim().strip_prefix("double").map(str::trim)
            .ok_or_else(|| refuse("non-double return"))?, &fenv, &mut b)?;

        let t = b.finish(root);
        // the Sigma v1.2 binding discipline is a free internal validator here:
        // if recognition wired Acc/Elem outside their fold, fail loudly now
        t.fold_owners().map_err(|e| refuse(format!(
            "a body value escapes its fold ({e}) -- last-iteration live-outs \
             are on the roadmap")))?;
        Ok(t)
    }

    // ================================================== acyclic CFGs ====
    // Trial item 2 (FIELD-TRIAL №1): an `if/else` whose arms both bind lets
    // lowers at -O1 to a branch DIAMOND (or triangle), not a `select` -- all
    // 10 of easer's P2+ refusals were this one shape. A phi at a 2-pred
    // merge in an acyclic CFG IS Select(cond, vTrue, vFalse); Σ's eager
    // evaluation of both arms is sound because every op in the alphabet is
    // total (no traps, no effects -- the unpicked arm's value simply drops).
    //
    // v1 scope, positive-only like everything else here:
    //   * all edges forward (guaranteed acyclic; textual order = topo order
    //     by SSA dominance), exactly one ret block, all-f64 params
    //   * merges of exactly 2 preds, shaped as a diamond (both preds `br M`,
    //     a unique decider cond-branching to exactly {P, Q}) or a triangle
    //     (one pred IS the decider, branching {other, M})
    //   * conditions must live in the f64 world (fcmp): an icmp condition is
    //     integer-dependent data with no Σ reading
    // Chained else-if beyond one level, n-way merges, and cross-block
    // deciders refuse with this vocabulary so the next trial can count them.
    fn lift_acyclic(sig: &str, blocks: &[Block_]) -> Result<Term, LiftError> {
        let params = super::parse_params(sig).map_err(|e| match e {
            LiftError::Refused(m) => refuse(format!(
                "acyclic CFG with non-scalar params ({m}) -- sequence-bearing \
                 branches are on the roadmap")),
            other => other,
        })?;
        let index: HashMap<&str, usize> = blocks.iter().enumerate()
            .map(|(i, bl)| (bl.label.as_str(), i)).collect();
        let succs = |bl: &Block_| -> Vec<usize> {
            match &bl.term {
                Term_::Br(l) => vec![index[l.as_str()]],
                Term_::CondBr { a, b, .. } => vec![index[a.as_str()], index[b.as_str()]],
                Term_::Ret(_) => vec![],
            }
        };
        // real topological order (MEASURED: LLVM's textual block order is a
        // LAYOUT order -- the merge can print before its own predecessors)
        let n = blocks.len();
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, bl) in blocks.iter().enumerate() {
            for t in succs(bl) {
                if t == i { return Err(refuse("self-loop in acyclic path")); }
                preds[t].push(i);
            }
        }
        let mut indeg: Vec<usize> = preds.iter().map(Vec::len).collect();
        let mut topo = Vec::with_capacity(n);
        let mut q: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        while let Some(i) = q.pop() {
            topo.push(i);
            for t in succs(&blocks[i]) {
                indeg[t] -= 1;
                if indeg[t] == 0 { q.push(t); }
            }
        }
        if topo.len() != n {
            return Err(refuse(
                "cyclic control flow beyond the canonical counted loop -- P3"));
        }
        let rets = blocks.iter().filter(|b| matches!(b.term, Term_::Ret(_))).count();
        if rets != 1 {
            return Err(refuse(format!(
                "{rets} return blocks -- early returns / multi-exit CFGs are on \
                 the roadmap (v1 recovers single-exit branch trees)")));
        }

        let mut b = TermBuilder::new();
        let mut fenv: HashMap<String, u32> = HashMap::new();
        let mut ints: std::collections::HashSet<String> = Default::default();
        for (i, p) in params.iter().enumerate() {
            fenv.insert(p.clone(), b.var(i as u32));
        }
        let seq_ptrs: HashMap<String, u32> = HashMap::new(); // scalars only in v1
        let lens: Vec<String> = Vec::new();
        let mut geps: HashMap<String, u32> = HashMap::new();

        // Recursive branch-tree resolution: a phi at merge M with incoming
        // map {pred label -> value} is resolved by walking the decider TREE
        // from the entry -- Select(cond, resolve(true subtree), resolve(false
        // subtree)); a subtree that IS a direct edge into M contributes that
        // edge's incoming value. This composes for free: nested if/else,
        // which LLVM collapses into ONE n-way phi (MEASURED), unfolds back
        // into nested Selects. Soundness of eagerly materializing every
        // arm's value: the alphabet is total, so the unpicked arm's value
        // simply drops -- and the gate checks it over full mu' anyway.
        //
        // Tree guard: every block upstream of M must have exactly one
        // predecessor (a shared arm or a sequential earlier merge breaks the
        // unique-attribution property and refuses with roadmap vocabulary).
        fn resolve(bi: usize, m: usize, blocks: &[Block_], preds: &[Vec<usize>],
                   index: &HashMap<&str, usize>, inc: &HashMap<&str, &str>,
                   fenv: &HashMap<String, u32>,
                   ints: &std::collections::HashSet<String>,
                   b: &mut TermBuilder) -> Result<u32, LiftError>
        {
            let bl = &blocks[bi];
            let arm = |target: &str, from: usize, blocks: &[Block_],
                       index: &HashMap<&str, usize>, inc: &HashMap<&str, &str>,
                       fenv: &HashMap<String, u32>,
                       ints: &std::collections::HashSet<String>,
                       preds: &[Vec<usize>], b: &mut TermBuilder|
                       -> Result<u32, LiftError> {
                if index[target] == m {
                    // direct edge into the merge: this pred's incoming value
                    let v = inc.get(blocks[from].label.as_str()).ok_or_else(||
                        refuse(format!("edge %{} -> merge carries no phi value",
                            blocks[from].label)))?;
                    operand(v, fenv, b)
                } else {
                    let ti = index[target];
                    if preds[ti].len() != 1 {
                        return Err(refuse(
                            "shared arm / sequential merge upstream of this \
                             merge -- non-tree acyclic regions are on the \
                             roadmap"));
                    }
                    resolve(ti, m, blocks, preds, index, inc, fenv, ints, b)
                }
            };
            match &bl.term {
                Term_::Br(l) if index[l.as_str()] == m => {
                    let v = inc.get(bl.label.as_str()).ok_or_else(|| refuse(
                        format!("edge %{} -> merge carries no phi value", bl.label)))?;
                    operand(v, fenv, b)
                }
                Term_::Br(l) => {
                    let ti = index[l.as_str()];
                    if preds[ti].len() != 1 {
                        return Err(refuse(
                            "shared arm / sequential merge upstream of this \
                             merge -- non-tree acyclic regions are on the roadmap"));
                    }
                    resolve(ti, m, blocks, preds, index, inc, fenv, ints, b)
                }
                Term_::CondBr { cond, a, b: bb } => {
                    if ints.contains(cond.as_str()) {
                        return Err(refuse(
                            "branch condition is integer data -- no Σ reading"));
                    }
                    let c = *fenv.get(cond.as_str()).ok_or_else(|| LiftError::Parse(
                        format!("unbound condition %{cond}")))?;
                    let tv = arm(a, bi, blocks, index, inc, fenv, ints, preds, b)?;
                    let fv = arm(bb, bi, blocks, index, inc, fenv, ints, preds, b)?;
                    Ok(b.ternary(Op::Select, c, tv, fv))
                }
                Term_::Ret(_) => Err(refuse(
                    "a return block upstream of a merge -- multi-exit regions \
                     are on the roadmap")),
            }
        }

        let entry = topo[0];
        let mut ret_val: Option<u32> = None;
        for &mi in &topo {
            let m = &blocks[mi];
            for p in &m.phis {
                if p.ty != "double" {
                    return Err(refuse(format!("phi of type `{}` at a merge", p.ty)));
                }
                let inc: HashMap<&str, &str> = p.inc.iter()
                    .map(|(v, l)| (l.as_str(), v.as_str())).collect();
                let id = resolve(entry, mi, blocks, &preds, &index, &inc,
                                 &fenv, &ints, &mut b)?;
                fenv.insert(p.dst.clone(), id);
            }
            pass(m, &seq_ptrs, &lens, &mut fenv, &mut ints, &mut geps, false, &mut b)?;
            if let Term_::Ret(r) = &m.term {
                ret_val = Some(operand(
                    r.trim().strip_prefix("double").map(str::trim)
                        .ok_or_else(|| refuse("non-double return"))?,
                    &fenv, &mut b)?);
            }
        }
        Ok(b.finish(ret_val.expect("single ret verified")))
    }
}

// ----------------------------------------------------------------- helpers --

fn strip_comment(l: &str) -> &str {
    // ';' cannot appear inside our accepted subset's tokens (no strings)
    match l.find(';') { Some(i) => &l[..i], None => l }
}

/// Split on top-level commas (never inside parens — call arg lists nest none
/// in our subset, but `nofpclass(nan)` style attrs do).
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => { out.push(&s[start..i]); start = i + 1; }
            _ => {}
        }
    }
    if start < s.len() || out.is_empty() { out.push(&s[start..]); }
    out
}

// ------------------------------------------------------------ rustc driver --

/// Emit LLVM IR for a `.rs` file with the review's recommended flags:
/// `-C opt-level=1` (mem2reg done, math not buried in stack traffic;
/// rustc never fp-contracts by default, so no pre-fused fma appears).
///
/// MEASURED rustc behavior (this box, 1.97): at opt-level ≥ 1 a small pub fn
/// in a lib crate is CROSS-CRATE-INLINE deferred — only MIR reaches the
/// metadata, no `define` reaches the IR. `#[no_mangle]` forces codegen (and
/// as a bonus removes symbol-mangling ambiguity), so the driver injects it
/// into a TEMP COPY of the source before the target fn. This is build
/// tooling on the untrusted side of the gate: if the injection changed
/// behavior in any way, the extraction gate is what would catch it.
pub fn rustc_emit_ir(rs_path: &std::path::Path, fn_name: &str) -> Result<String, String> {
    let src = std::fs::read_to_string(rs_path).map_err(|e| e.to_string())?;
    let src = inject_no_mangle(&src, fn_name)?;
    let dir = std::env::temp_dir().join(format!("dge_lift_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp_rs = dir.join("lift_input.rs");
    std::fs::write(&tmp_rs, src).map_err(|e| e.to_string())?;
    let out = dir.join("lift_input.ll");
    let st = std::process::Command::new(
            std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .args(["--emit=llvm-ir", "--crate-type=lib", "--edition=2021",
               "-C", "opt-level=1", "-C", "debuginfo=0",
               // P2: keep loops in canonical single-natural-loop form. rustc
               // 1.97's -O1 pipeline runtime-unrolls by 4 with an epilogue
               // loop (MEASURED — 7 blocks for a plain slice sum); compile-
               // time FULL unrolling of known trip counts still happens,
               // which is exactly what P1 wants for fixed-window iterators.
               "-C", "llvm-args=--unroll-runtime=false", "-o"])
        .arg(&out).arg(&tmp_rs)
        .output().map_err(|e| format!("rustc: {e}"))?;
    if !st.status.success() {
        return Err(format!("rustc failed:\n{}", String::from_utf8_lossy(&st.stderr)));
    }
    std::fs::read_to_string(&out).map_err(|e| e.to_string())
}

/// Prepend `#[no_mangle]` to the line defining `fn <name>` (word-bounded).
fn inject_no_mangle(src: &str, fn_name: &str) -> Result<String, String> {
    let needle = format!("fn {fn_name}");
    let mut pos = 0usize;
    while let Some(i) = src[pos..].find(&needle) {
        let i = pos + i;
        let after = src[i + needle.len()..].chars().next().unwrap_or(' ');
        if after == '(' || after == '<' || after.is_whitespace() {
            let line_start = src[..i].rfind('\n').map(|j| j + 1).unwrap_or(0);
            if src[..line_start].trim_end().ends_with("#[no_mangle]") {
                return Ok(src.to_string()); // already annotated
            }
            let mut out = String::with_capacity(src.len() + 16);
            out.push_str(&src[..line_start]);
            out.push_str("#[no_mangle]\n");
            out.push_str(&src[line_start..]);
            return Ok(out);
        }
        pos = i + needle.len();
    }
    Err(format!("`fn {fn_name}` not found in source"))
}

/// `dge lift <file.ll|file.rs> <fn_name> [--out t.sexpr]`
pub fn run(args: &[String]) {
    let (Some(file), Some(name)) = (args.first(), args.get(1)) else {
        eprintln!("usage: dge lift <file.ll|file.rs> <fn_name> [--out <t.sexpr>]");
        return;
    };
    let ir = if file.ends_with(".rs") {
        match rustc_emit_ir(std::path::Path::new(file), name) {
            Ok(s) => s, Err(e) => { eprintln!("{e}"); return; }
        }
    } else {
        match std::fs::read_to_string(file) {
            Ok(s) => s, Err(e) => { eprintln!("read {file}: {e}"); return; }
        }
    };
    match lift_ll(&ir, name) {
        Ok(t) => {
            let s = term::sexpr::print(&t);
            eprintln!("lifted `{name}` from IR: {} nodes, arity {}   \
                       [UNTRUSTED — run the extraction gate]", t.len(), t.arity());
            match args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)) {
                Some(p) => { std::fs::write(p, &s).ok(); eprintln!("-> {p}"); }
                None => println!("{s}"),
            }
        }
        Err(e) => eprintln!("lift failed: {e}"),
    }
}
