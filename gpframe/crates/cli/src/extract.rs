//! `dge extract <file.rs> <fn_name>` — the FRONT DOOR: Rust fn → Term_p.
//!
//! Covers the audit's EXTRACT class (plus manually-monomorphized generics —
//! the with-effort step the audit predicts): f64 params, arithmetic, Σ math
//! methods, let bindings with shadowing, if/else with comparison conditions.
//!
//! EXTRACTION IS A LOWERING (L1 applies in reverse): the translation below
//! is NOT trusted. Its output must pass the extraction gate — a differential
//! check of interp(term) against the rustc-compiled original over μ' —
//! before anything downstream may call the term "the function".
//!
//! Comparison encodings (cond ≠ 0 ⇒ then-branch, matching Σ's select):
//!   a <  b  →  (- b (min a b))     exact for all inputs unless b is NaN
//!   a >  b  →  (- a (min b a))     exact for all inputs unless a is NaN
//!   a <= b  →  (select [a > b] 0 1)
//!   a >= b  →  (select [a < b] 0 1)
//! The NaN caveats are precisely why the extraction gate exists: if a
//! condition's vulnerable side can be NaN under μ', the gate REFUTES and
//! extraction honestly fails rather than shipping a semantic drift.

use std::collections::HashMap;
use term::{Op, Term, TermBuilder};

#[derive(Debug)]
pub enum ExtractError {
    Parse(String),
    FnNotFound(String),
    Unsupported(String),
}

type Scopes = Vec<HashMap<String, u32>>;

fn lookup(scopes: &Scopes, name: &str) -> Option<u32> {
    scopes.iter().rev().find_map(|s| s.get(name).copied())
}

const METHODS: &[(&str, Op)] = &[
    ("sin", Op::Sin), ("cos", Op::Cos), ("tan", Op::Tan),
    ("exp", Op::Exp), ("ln", Op::Ln), ("sqrt", Op::Sqrt),
    ("abs", Op::Abs), ("floor", Op::Floor), ("ceil", Op::Ceil),
];

struct Cx<'a> {
    b: &'a mut TermBuilder,
}

impl Cx<'_> {
    fn expr(&mut self, e: &syn::Expr, sc: &mut Scopes) -> Result<u32, ExtractError> {
        use syn::Expr as E;
        let unsup = |s: String| ExtractError::Unsupported(s);
        match e {
            E::Lit(l) => match &l.lit {
                syn::Lit::Float(f) => Ok(self.b.constant(
                    f.base10_parse::<f64>().map_err(|e| unsup(e.to_string()))?)),
                syn::Lit::Int(i) => Ok(self.b.constant(
                    i.base10_parse::<i64>().map_err(|e| unsup(e.to_string()))? as f64)),
                _ => Err(unsup("non-numeric literal".into())),
            },
            E::Path(p) => {
                let name = p.path.get_ident()
                    .map(|i| i.to_string())
                    .ok_or_else(|| unsup("qualified path".into()))?;
                lookup(sc, &name).ok_or(ExtractError::Unsupported(format!("unbound `{name}`")))
                    .map(Ok).unwrap_or_else(Err)
            }
            E::Paren(p) => self.expr(&p.expr, sc),
            E::Group(g) => self.expr(&g.expr, sc),
            E::Unary(u) => match u.op {
                syn::UnOp::Neg(_) => {
                    let a = self.expr(&u.expr, sc)?;
                    Ok(self.b.unary(Op::Neg, a))
                }
                _ => Err(unsup("unary op".into())),
            },
            E::Binary(bin) => {
                use syn::BinOp::*;
                let op = match bin.op {
                    Add(_) => Op::Add, Sub(_) => Op::Sub,
                    Mul(_) => Op::Mul, Div(_) => Op::Div,
                    Lt(_) | Gt(_) | Le(_) | Ge(_) =>
                        return self.comparison(bin, sc),
                    _ => return Err(unsup("unsupported binary op".into())),
                };
                let a = self.expr(&bin.left, sc)?;
                let c = self.expr(&bin.right, sc)?;
                Ok(self.b.binary(op, a, c))
            }
            E::MethodCall(m) => {
                let name = m.method.to_string();
                let recv = self.expr(&m.receiver, sc)?;
                if let Some((_, op)) = METHODS.iter().find(|(n, _)| *n == name) {
                    return Ok(self.b.unary(*op, recv));
                }
                match name.as_str() {
                    "powf" | "min" | "max" => {
                        let arg = self.expr(&m.args[0], sc)?;
                        let op = match name.as_str() {
                            "powf" => Op::Pow, "min" => Op::Min, _ => Op::Max,
                        };
                        Ok(self.b.binary(op, recv, arg))
                    }
                    "powi" => {
                        let arg = self.expr(&m.args[0], sc)?;
                        Ok(self.b.binary(Op::Pow, recv, arg))
                    }
                    "mul_add" => {
                        let a1 = self.expr(&m.args[0], sc)?;
                        let a2 = self.expr(&m.args[1], sc)?;
                        Ok(self.b.ternary(Op::Fma, recv, a1, a2))
                    }
                    "recip" => {
                        let one = self.b.constant(1.0);
                        Ok(self.b.binary(Op::Div, one, recv))
                    }
                    _ => Err(unsup(format!("method .{name}() outside Σ"))),
                }
            }
            E::Call(c) => {
                // const-lift helpers like easer's `f(1.0)`: single-arg call
                // whose argument is itself extractable — treat as identity.
                if let syn::Expr::Path(p) = &*c.func {
                    let name = p.path.segments.last()
                        .map(|s| s.ident.to_string()).unwrap_or_default();
                    if c.args.len() == 1 && (name == "f" || name == "from" || name == "cast") {
                        return self.expr(&c.args[0], sc);
                    }
                    return Err(unsup(format!("call `{name}` outside Σ")));
                }
                Err(unsup("indirect call".into()))
            }
            E::If(i) => {
                let cond = self.expr(&i.cond, sc)?;
                let then_v = self.block(&i.then_branch, sc)?;
                let else_v = match &i.else_branch {
                    Some((_, eb)) => self.expr(eb, sc)?,
                    None => return Err(unsup("if without else (no value)".into())),
                };
                Ok(self.b.ternary(Op::Select, cond, then_v, else_v))
            }
            E::Block(b) => self.block(&b.block, sc),
            E::Return(r) => match &r.expr {
                Some(e) => self.expr(e, sc),
                None => Err(unsup("bare return".into())),
            },
            other => Err(unsup(format!("expression form {:?}", std::mem::discriminant(other)))),
        }
    }

    /// comparisons → nonzero-iff-true encodings (see module doc)
    fn comparison(&mut self, bin: &syn::ExprBinary, sc: &mut Scopes) -> Result<u32, ExtractError> {
        use syn::BinOp::*;
        let a = self.expr(&bin.left, sc)?;
        let b = self.expr(&bin.right, sc)?;
        let lt = |cx: &mut Self, a: u32, b: u32| {
            let m = cx.b.binary(Op::Min, a, b);
            cx.b.binary(Op::Sub, b, m) // b - min(a,b)
        };
        Ok(match bin.op {
            Lt(_) => lt(self, a, b),
            Gt(_) => lt(self, b, a),
            Le(_) => {
                let gt = lt(self, b, a);
                let z = self.b.constant(0.0);
                let one = self.b.constant(1.0);
                self.b.ternary(Op::Select, gt, z, one)
            }
            Ge(_) => {
                let ltv = lt(self, a, b);
                let z = self.b.constant(0.0);
                let one = self.b.constant(1.0);
                self.b.ternary(Op::Select, ltv, z, one)
            }
            _ => unreachable!(),
        })
    }

    fn block(&mut self, blk: &syn::Block, sc: &mut Scopes) -> Result<u32, ExtractError> {
        sc.push(HashMap::new());
        let mut last: Option<u32> = None;
        for stmt in &blk.stmts {
            match stmt {
                syn::Stmt::Local(l) => {
                    let name = match &l.pat {
                        syn::Pat::Ident(pi) => pi.ident.to_string(),
                        syn::Pat::Type(pt) => match &*pt.pat {
                            syn::Pat::Ident(pi) => pi.ident.to_string(),
                            _ => return Err(ExtractError::Unsupported("pattern let".into())),
                        },
                        _ => return Err(ExtractError::Unsupported("pattern let".into())),
                    };
                    let init = l.init.as_ref()
                        .ok_or(ExtractError::Unsupported("let without init".into()))?;
                    let v = self.expr(&init.expr, sc)?;
                    sc.last_mut().unwrap().insert(name, v);
                }
                syn::Stmt::Expr(e, _) => last = Some(self.expr(e, sc)?),
                _ => return Err(ExtractError::Unsupported("statement form".into())),
            }
        }
        sc.pop();
        last.ok_or(ExtractError::Unsupported("empty block".into()))
    }
}

/// Extract `fn_name` from Rust source. Searches free fns and impl methods.
pub fn extract_fn(src: &str, fn_name: &str) -> Result<Term, ExtractError> {
    let ast = syn::parse_file(src).map_err(|e| ExtractError::Parse(e.to_string()))?;

    fn find<'a>(items: &'a [syn::Item], name: &str)
        -> Option<(&'a syn::Signature, &'a syn::Block)>
    {
        for item in items {
            match item {
                syn::Item::Fn(f) if f.sig.ident == name => return Some((&f.sig, &f.block)),
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        if let Some(hit) = find(inner, name) { return Some(hit); }
                    }
                }
                syn::Item::Impl(i) => {
                    for ii in &i.items {
                        if let syn::ImplItem::Fn(f) = ii {
                            if f.sig.ident == name { return Some((&f.sig, &f.block)); }
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    let (sig, block) = find(&ast.items, fn_name)
        .ok_or_else(|| ExtractError::FnNotFound(fn_name.into()))?;

    let mut b = TermBuilder::new();
    let mut scopes: Scopes = vec![HashMap::new()];
    for (i, arg) in sig.inputs.iter().enumerate() {
        match arg {
            syn::FnArg::Typed(t) => {
                let name = match &*t.pat {
                    syn::Pat::Ident(pi) => pi.ident.to_string(),
                    _ => return Err(ExtractError::Unsupported("pattern param".into())),
                };
                let id = b.var(i as u32);
                scopes[0].insert(name, id);
            }
            syn::FnArg::Receiver(_) =>
                return Err(ExtractError::Unsupported("method receiver (flatten self first)".into())),
        }
    }

    let root = Cx { b: &mut b }.block(block, &mut scopes)?;
    Ok(b.finish(root))
}

pub fn run(args: &[String]) {
    let (Some(file), Some(name)) = (args.first(), args.get(1)) else {
        eprintln!("usage: dge extract <file.rs> <fn_name> [--out <t.sexpr>]");
        return;
    };
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {file}: {e}"); return; }
    };
    match extract_fn(&src, name) {
        Ok(t) => {
            let sexpr = term::sexpr::print(&t);
            println!("{sexpr}");
            if let Some(i) = args.iter().position(|a| a == "--out") {
                if let Some(out) = args.get(i + 1) {
                    std::fs::write(out, format!("{sexpr}\n")).ok();
                    eprintln!("({} nodes, arity {}) -> {out}", t.len(), t.arity());
                }
            }
            eprintln!("NOTE: extraction is a lowering — gate this term against \
                       the compiled original before trusting it.");
        }
        Err(e) => eprintln!("extraction failed: {e:?}"),
    }
}
