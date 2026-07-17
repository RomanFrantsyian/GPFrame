//! egg::Language mirror of Σ + arena↔RecExpr bridges + rule-pattern
//! translation. [R2] The e-graph library is NOT trusted base: whatever it
//! extracts goes back through the certificate machinery (T1 + gate).

use egg::{define_language, Id, Language, RecExpr};
use std::fmt;
use std::str::FromStr;
use term::{Op, Term, TermBuilder};

/// f64 constant carried as raw bits — Ord/Hash for egg, and it keeps the
/// bitwise discipline (−0.0 ≠ +0.0, NaN payloads distinct) inside the graph.
/// Token form: `c<16 hex digits>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConstBits(pub u64);

impl fmt::Display for ConstBits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{:016x}", self.0)
    }
}

impl FromStr for ConstBits {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let hex = s.strip_prefix('c').ok_or("no c prefix")?;
        if hex.len() != 16 {
            return Err("need 16 hex digits".into());
        }
        u64::from_str_radix(hex, 16).map(ConstBits).map_err(|e| e.to_string())
    }
}

/// Fold element terminal. Token form: `e<idx>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemIdx(pub u32);

impl fmt::Display for ElemIdx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

impl FromStr for ElemIdx {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let d = s.strip_prefix('e').ok_or("no e prefix")?;
        if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
            return Err("not an elem index".into());
        }
        d.parse().map(ElemIdx).map_err(|e: std::num::ParseIntError| e.to_string())
    }
}

/// Sequence-length terminal (Σ v1.3). Token form: `L<idx>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LenIdx(pub u32);

impl fmt::Display for LenIdx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.0)
    }
}

impl FromStr for LenIdx {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let d = s.strip_prefix('L').ok_or("no L prefix")?;
        if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
            return Err("not a len index".into());
        }
        d.parse().map(LenIdx).map_err(|e: std::num::ParseIntError| e.to_string())
    }
}

/// Variable terminal. Token form: `v<idx>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VarIdx(pub u32);

impl fmt::Display for VarIdx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl FromStr for VarIdx {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let d = s.strip_prefix('v').ok_or("no v prefix")?;
        if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
            return Err("not a var index".into());
        }
        d.parse().map(VarIdx).map_err(|e: std::num::ParseIntError| e.to_string())
    }
}

define_language! {
    pub enum SigLang {
        "+" = Add([Id; 2]),
        "-" = Sub([Id; 2]),
        "*" = Mul([Id; 2]),
        "/" = Div([Id; 2]),
        "min" = Min([Id; 2]),
        "max" = Max([Id; 2]),
        "pow" = Pow([Id; 2]),
        "lt" = Lt([Id; 2]),
        "gt" = Gt([Id; 2]),
        "le" = Le([Id; 2]),
        "ge" = Ge([Id; 2]),
        "eq" = Eq([Id; 2]),
        "ne" = Ne([Id; 2]),
        "neg" = Neg([Id; 1]),
        "abs" = Abs([Id; 1]),
        "sqrt" = Sqrt([Id; 1]),
        "floor" = Floor([Id; 1]),
        "ceil" = Ceil([Id; 1]),
        "sin" = Sin([Id; 1]),
        "cos" = Cos([Id; 1]),
        "tan" = Tan([Id; 1]),
        "exp" = Exp([Id; 1]),
        "exp2" = Exp2([Id; 1]),
        "ln" = Ln([Id; 1]),
        "fma" = Fma([Id; 3]),
        "fold" = Fold([Id; 2]),
        "acc" = Acc([Id; 0]),
        Elem(ElemIdx),
        Len(LenIdx),
        "select" = Select([Id; 3]),
        Const(ConstBits),
        Var(VarIdx),
    }
}

/// term::Op of an e-node (for cost bridging).
pub fn op_of(n: &SigLang) -> Op {
    use SigLang::*;
    match n {
        Add(_) => Op::Add, Sub(_) => Op::Sub, Mul(_) => Op::Mul, Div(_) => Op::Div,
        Min(_) => Op::Min, Max(_) => Op::Max, Pow(_) => Op::Pow,
        Lt(_) => Op::Lt, Gt(_) => Op::Gt, Le(_) => Op::Le, Ge(_) => Op::Ge,
        Eq(_) => Op::Eq, Ne(_) => Op::Ne, Exp2(_) => Op::Exp2,
        Neg(_) => Op::Neg, Abs(_) => Op::Abs, Sqrt(_) => Op::Sqrt,
        Floor(_) => Op::Floor, Ceil(_) => Op::Ceil,
        Sin(_) => Op::Sin, Cos(_) => Op::Cos, Tan(_) => Op::Tan,
        Exp(_) => Op::Exp, Ln(_) => Op::Ln,
        Fma(_) => Op::Fma, Select(_) => Op::Select,
        Fold(_) => Op::Fold, Acc(_) => Op::Acc, Elem(_) => Op::Elem,
        Len(_) => Op::Len,
        Const(_) => Op::Const, Var(_) => Op::Var,
    }
}

/// Term → RecExpr, recursive from the root so the RecExpr root is last
/// (egg's convention). Tree-ifies sharing — semantics unaffected.
pub fn to_egg(t: &Term) -> RecExpr<SigLang> {
    fn go(t: &Term, id: u32, out: &mut RecExpr<SigLang>) -> Id {
        let n = t.node(id);
        use SigLang::*;
        match n.op {
            Op::Const => out.add(Const(ConstBits(t.consts[n.a as usize].to_bits()))),
            Op::Var => out.add(Var(VarIdx(n.a))),
            Op::Acc => out.add(Acc([])),
            Op::Elem => out.add(Elem(ElemIdx(n.a))),
            Op::Len => out.add(Len(LenIdx(n.a))),
            Op::Fold => {
                let init = go(t, n.a, out);
                let body = go(t, n.b, out);
                out.add(Fold([init, body]))
            }
            op => {
                let a = go(t, n.a, out);
                let node = match op.arity() {
                    1 => match op {
                        Op::Neg => Neg([a]), Op::Abs => Abs([a]), Op::Sqrt => Sqrt([a]),
                        Op::Floor => Floor([a]), Op::Ceil => Ceil([a]),
                        Op::Sin => Sin([a]), Op::Cos => Cos([a]), Op::Tan => Tan([a]),
                        Op::Exp => Exp([a]), Op::Exp2 => Exp2([a]), Op::Ln => Ln([a]),
                        _ => unreachable!(),
                    },
                    2 => {
                        let b = go(t, n.b, out);
                        match op {
                            Op::Add => Add([a, b]), Op::Sub => Sub([a, b]),
                            Op::Mul => Mul([a, b]), Op::Div => Div([a, b]),
                            Op::Min => Min([a, b]), Op::Max => Max([a, b]),
                            Op::Pow => Pow([a, b]),
                            Op::Lt => Lt([a, b]), Op::Gt => Gt([a, b]),
                            Op::Eq => Eq([a, b]), Op::Ne => Ne([a, b]),
                            Op::Le => Le([a, b]), Op::Ge => Ge([a, b]),
                            _ => unreachable!(),
                        }
                    }
                    _ => {
                        let b = go(t, n.b, out);
                        let c = go(t, n.c, out);
                        match op {
                            Op::Fma => Fma([a, b, c]),
                            Op::Select => Select([a, b, c]),
                            _ => unreachable!(),
                        }
                    }
                };
                out.add(node)
            }
        }
    }
    let mut out = RecExpr::default();
    go(t, t.root, &mut out);
    out
}

/// RecExpr → Term. RecExpr is child-before-parent, so a single pass through
/// TermBuilder preserves the topological invariant; root = last node.
pub fn from_egg(e: &RecExpr<SigLang>) -> Term {
    let nodes = e.as_ref();
    let mut b = TermBuilder::new();
    let mut map: Vec<u32> = Vec::with_capacity(nodes.len());
    for n in nodes {
        use SigLang::*;
        let id = match n {
            Const(c) => b.constant(f64::from_bits(c.0)),
            Var(v) => b.var(v.0),
            Acc(_) => b.acc(),
            Elem(k) => b.elem(k.0),
            Len(k) => b.len_of(k.0),
            Fold([i, bo]) => {
                let init = map[usize::from(*i)];
                let body = map[usize::from(*bo)];
                b.fold(init, body)
            }
            _ => {
                let op = op_of(n);
                let kids: Vec<u32> = n.children().iter().map(|&c| map[usize::from(c)]).collect();
                match op.arity() {
                    1 => b.unary(op, kids[0]),
                    2 => b.binary(op, kids[0], kids[1]),
                    _ => b.ternary(op, kids[0], kids[1], kids[2]),
                }
            }
        };
        map.push(id);
    }
    let root = *map.last().expect("empty RecExpr");
    b.finish(root)
}

/// Translate a rule-table pattern (human syntax with f64 literals, e.g.
/// "(* ?a 1.0)") into egg pattern syntax ("(* ?a c3ff0000000000000)").
pub fn translate_pattern(src: &str) -> String {
    src.split_whitespace()
        .map(|raw| {
            // peel parens
            let open = raw.chars().take_while(|&c| c == '(').count();
            let close = raw.chars().rev().take_while(|&c| c == ')').count();
            let core = &raw[open..raw.len() - close];
            let mapped = if core.is_empty() || core.starts_with('?') {
                core.to_string()
            } else if let Ok(v) = core.parse::<f64>() {
                // numeric literal → bitwise const token (but not op names:
                // parse::<f64> rejects "+", "-" alone? "-" parses? no: err)
                if core == "-" || core == "+" { core.to_string() }
                else { format!("c{:016x}", v.to_bits()) }
            } else {
                core.to_string()
            };
            format!("{}{}{}", "(".repeat(open), mapped, ")".repeat(close))
        })
        .collect::<Vec<_>>()
        .join(" ")
}
