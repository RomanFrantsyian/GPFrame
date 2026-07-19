//! S-expression parse/print. Round-trip property (R0 exit test):
//! for tree-shaped t, parse(print(t)) is structurally equal; for DAG-shaped
//! t (shared subtrees), print duplicates sharing, so the round trip preserves
//! semantics ([[.]]-equal) but not the arena — both are asserted in tests.
//!
//! Surface syntax:  (+ (var 0) (* 2.5 (var 1)))   bare numbers = constants.

use crate::ast::{NodeId, Term, TermBuilder};
use crate::sig::Op;

pub fn print(t: &Term) -> String {
    fn go(t: &Term, id: NodeId, out: &mut String) {
        let n = t.node(id);
        match n.op {
            Op::Const => {
                let v = t.consts[n.a as usize];
                if v.is_nan() { out.push_str("NaN"); }
                else { out.push_str(&format!("{v:?}")); }
            }
            Op::Var => out.push_str(&format!("(var {})", n.a)),
            Op::Acc => out.push_str("acc"),
            Op::Elem => out.push_str(&format!("(elem {})", n.a)),
            Op::Len => out.push_str(&format!("(len {})", n.a)),
            Op::Ext1 => {
                out.push_str(&format!("(ext:{} ", t.exts[n.b as usize]));
                go(t, n.a, out);
                out.push(')');
            }
            Op::Ext2 => {
                out.push_str(&format!("(ext:{} ", t.exts[n.c as usize]));
                go(t, n.a, out);
                out.push(' ');
                go(t, n.b, out);
                out.push(')');
            }
            _ => {
                out.push('(');
                out.push_str(n.op.name());
                let kids = [n.a, n.b, n.c];
                for &k in kids.iter().take(n.op.arity()) {
                    out.push(' ');
                    go(t, k, out);
                }
                out.push(')');
            }
        }
    }
    let mut s = String::new();
    go(t, t.root, &mut s);
    s
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    UnexpectedEof,
    UnexpectedToken(String),
    UnknownOp(String),
    BadNumber(String),
    TrailingInput,
}

fn lex(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in src.chars() {
        match c {
            '(' | ')' => {
                if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
                out.push(c.to_string());
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

fn parse_number(tok: &str) -> Result<f64, ParseError> {
    match tok {
        "NaN" => Ok(f64::NAN),
        "inf" => Ok(f64::INFINITY),
        "-inf" => Ok(f64::NEG_INFINITY),
        _ => tok.parse::<f64>().map_err(|_| ParseError::BadNumber(tok.to_string())),
    }
}

struct P<'a> {
    toks: &'a [String],
    pos: usize,
}

impl<'a> P<'a> {
    fn next(&mut self) -> Result<&'a str, ParseError> {
        let t = self.toks.get(self.pos).ok_or(ParseError::UnexpectedEof)?;
        self.pos += 1;
        Ok(t)
    }

    fn expect(&mut self, s: &str) -> Result<(), ParseError> {
        let t = self.next()?;
        if t == s { Ok(()) } else { Err(ParseError::UnexpectedToken(t.to_string())) }
    }

    fn peek_is(&self, s: &str) -> bool {
        self.toks.get(self.pos).map(|t| *t == s).unwrap_or(false)
    }

    fn expr(&mut self, b: &mut TermBuilder) -> Result<NodeId, ParseError> {
        let t = self.next()?;
        if t != "(" {
            if t == "acc" {
                return Ok(b.acc());
            }
            // bare atom = constant
            return Ok(b.constant(parse_number(t)?));
        }
        let head = self.next()?;
        if head == "elem" {
            let i = self.next()?;
            let idx: u32 = i.parse().map_err(|_| ParseError::BadNumber(i.to_string()))?;
            self.expect(")")?;
            return Ok(b.elem(idx));
        }
        if head == "len" {
            let i = self.next()?;
            let idx: u32 = i.parse().map_err(|_| ParseError::BadNumber(i.to_string()))?;
            self.expect(")")?;
            return Ok(b.len_of(idx));
        }
        if head == "fold" {
            let init = self.expr(b)?;
            let body = self.expr(b)?;
            self.expect(")")?;
            return Ok(b.fold(init, body));
        }
        if head == "var" {
            let i = self.next()?;
            let idx: u32 = i.parse().map_err(|_| ParseError::BadNumber(i.to_string()))?;
            self.expect(")")?;
            return Ok(b.var(idx));
        }
        if let Some(name) = head.strip_prefix("ext:") {
            let name = name.to_string();
            let a = self.expr(b)?;
            if self.peek_is(")") {
                self.expect(")")?;
                return Ok(b.ext1(&name, a));
            }
            let a2 = self.expr(b)?;
            self.expect(")")?;
            return Ok(b.ext2(&name, a, a2));
        }
        let op = Op::from_name(head).ok_or_else(|| ParseError::UnknownOp(head.to_string()))?;
        let mut kids = [0u32; 3];
        for slot in kids.iter_mut().take(op.arity()) {
            *slot = self.expr(b)?;
        }
        self.expect(")")?;
        Ok(match op.arity() {
            1 => b.unary(op, kids[0]),
            2 => b.binary(op, kids[0], kids[1]),
            _ => b.ternary(op, kids[0], kids[1], kids[2]),
        })
    }
}

pub fn parse(src: &str) -> Result<Term, ParseError> {
    let toks = lex(src);
    let mut p = P { toks: &toks, pos: 0 };
    let mut b = TermBuilder::new();
    let root = p.expr(&mut b)?;
    if p.pos != toks.len() {
        return Err(ParseError::TrailingInput);
    }
    Ok(b.finish(root))
}
