//! [D1] Arena representation: `Node{op,a,b,c}`, `Term{nodes,root,consts,hash}`.
//!
//! INVARIANT (load-bearing for the O(n) interpreter and hasher):
//!   children strictly precede parents — `nodes[i]` may only reference ids < i.
//!   `TermBuilder` is the sole way to construct a `Term` and enforces this,
//!   so a single left-to-right pass over `nodes` is a valid post-order.

use crate::hash::structural_hash;
use crate::sig::Op;

/// Arena index. u32: 4 G nodes is beyond any depth-capped GP population.
pub type NodeId = u32;

/// One arena slot. Unused child slots are 0 (never read: arity-gated).
/// For `Const`/`Var`, `a` holds the payload index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub op: Op,
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

/// An immutable term of Term_p. `hash` is a cached structural FNV (an index,
/// not an authority — see O6): equality goes through `structurally_eq`.
#[derive(Debug, Clone)]
pub struct Term {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    /// Constant pool; `Op::Const` payloads index here. Kept out-of-line so
    /// gp::refine (Nelder-Mead) can perturb constants without touching shape.
    pub consts: Vec<f64>,
    pub hash: u64,
}

impl Term {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Number of distinct `Var` payloads = required env width.
    pub fn arity(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.op == Op::Var)
            .map(|n| n.a as usize + 1)
            .max()
            .unwrap_or(0)
    }

    /// Tree depth (root = 1). One topological pass.
    pub fn depth(&self) -> u32 {
        let mut d: Vec<u32> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let kid = match n.op.arity() {
                0 => 0,
                1 => d[n.a as usize],
                2 => d[n.a as usize].max(d[n.b as usize]),
                _ => d[n.a as usize].max(d[n.b as usize]).max(d[n.c as usize]),
            };
            d.push(kid + 1);
        }
        d[self.root as usize]
    }

    /// FULL-KEY COMPARE — the authority behind every hash lookup (O6→DERIVED).
    /// Bitwise on constants: -0.0 ≠ +0.0, NaN payloads distinguished.
    pub fn structurally_eq(&self, other: &Term) -> bool {
        self.root == other.root
            && self.nodes == other.nodes
            && self.consts.len() == other.consts.len()
            && self
                .consts
                .iter()
                .zip(&other.consts)
                .all(|(x, y)| x.to_bits() == y.to_bits())
    }
}

/// Sole constructor path for `Term`; enforces the topological invariant
/// because every child id handed back was produced by an earlier push.
#[derive(Default)]
pub struct TermBuilder {
    nodes: Vec<Node>,
    consts: Vec<f64>,
}

impl TermBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, n: Node) -> NodeId {
        // Invariant check: children precede parent.
        let id = self.nodes.len() as NodeId;
        debug_assert!(n.op.arity() < 1 || n.a < id);
        debug_assert!(n.op.arity() < 2 || n.b < id);
        debug_assert!(n.op.arity() < 3 || n.c < id);
        self.nodes.push(n);
        id
    }

    pub fn constant(&mut self, v: f64) -> NodeId {
        let k = self.consts.len() as u32;
        self.consts.push(v);
        self.push(Node { op: Op::Const, a: k, b: 0, c: 0 })
    }

    pub fn var(&mut self, index: u32) -> NodeId {
        self.push(Node { op: Op::Var, a: index, b: 0, c: 0 })
    }

    pub fn unary(&mut self, op: Op, a: NodeId) -> NodeId {
        assert_eq!(op.arity(), 1);
        self.push(Node { op, a, b: 0, c: 0 })
    }

    pub fn binary(&mut self, op: Op, a: NodeId, b: NodeId) -> NodeId {
        assert_eq!(op.arity(), 2);
        self.push(Node { op, a, b, c: 0 })
    }

    pub fn ternary(&mut self, op: Op, a: NodeId, b: NodeId, c: NodeId) -> NodeId {
        assert_eq!(op.arity(), 3);
        self.push(Node { op, a, b, c })
    }

    /// Recursively copy the subtree of `src` rooted at `id` into this builder,
    /// returning the new root id. Tree-ifies shared subtrees (correct: sharing
    /// is a space optimization, not semantics). Invariant holds: children are
    /// pushed before parents by recursion order.
    pub fn copy_subtree(&mut self, src: &Term, id: NodeId) -> NodeId {
        let n = *src.node(id);
        match n.op {
            Op::Const => self.constant(src.consts[n.a as usize]),
            Op::Var => self.var(n.a),
            _ => {
                let ar = n.op.arity();
                let a = self.copy_subtree(src, n.a);
                if ar == 1 { return self.unary(n.op, a); }
                let b = self.copy_subtree(src, n.b);
                if ar == 2 { return self.binary(n.op, a, b); }
                let c = self.copy_subtree(src, n.c);
                self.ternary(n.op, a, b, c)
            }
        }
    }

    /// Copy `host`'s subtree at `host_root`, but at node `at` splice in a copy
    /// of `donor`'s subtree at `donor_id` instead. Foundation of GP crossover
    /// and mutation-by-rebuild.
    pub fn graft(
        &mut self,
        host: &Term,
        host_root: NodeId,
        at: NodeId,
        donor: &Term,
        donor_id: NodeId,
    ) -> NodeId {
        if host_root == at {
            return self.copy_subtree(donor, donor_id);
        }
        let n = *host.node(host_root);
        match n.op {
            Op::Const => self.constant(host.consts[n.a as usize]),
            Op::Var => self.var(n.a),
            _ => {
                let ar = n.op.arity();
                let a = self.graft(host, n.a, at, donor, donor_id);
                if ar == 1 { return self.unary(n.op, a); }
                let b = self.graft(host, n.b, at, donor, donor_id);
                if ar == 2 { return self.binary(n.op, a, b); }
                let c = self.graft(host, n.c, at, donor, donor_id);
                self.ternary(n.op, a, b, c)
            }
        }
    }

    /// Finish, fixing `root` and caching the structural hash.
    pub fn finish(self, root: NodeId) -> Term {
        assert!((root as usize) < self.nodes.len(), "root out of arena");
        let mut t = Term { nodes: self.nodes, root, consts: self.consts, hash: 0 };
        t.hash = structural_hash(&t);
        t
    }
}
