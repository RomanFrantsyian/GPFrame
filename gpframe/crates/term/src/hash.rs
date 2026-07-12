//! Structural FNV-1a hash, post-order. [O6]
//!
//! Status: with full-key compare on every hit (memo crate, ast::structurally_eq)
//! this hash is demoted from assumption to index — O6 is DERIVED (v2.1 §3).
//! Collision ⇒ a compare miss ⇒ recompute; never a wrong value.

use crate::ast::Term;
use crate::sig::Op;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Post-order structural hash of the whole term (= hash of root's subtree).
/// Single pass is valid because the arena is topologically ordered.
/// Constants hash by *bit pattern* (−0.0 ≠ +0.0; NaNs by payload) — matches
/// the bitwise-key discipline of memo (T6) and the gate metric policy.
pub fn structural_hash(t: &Term) -> u64 {
    let mut sub: Vec<u64> = Vec::with_capacity(t.nodes.len());
    for n in &t.nodes {
        let mut h = FNV_OFFSET;
        h = fnv1a(h, n.op.name().as_bytes());
        match n.op {
            Op::Const => h = fnv1a(h, &t.consts[n.a as usize].to_bits().to_le_bytes()),
            Op::Var | Op::Elem => h = fnv1a(h, &n.a.to_le_bytes()),
            _ => {
                let ar = n.op.arity();
                if ar >= 1 { h = fnv1a(h, &sub[n.a as usize].to_le_bytes()); }
                if ar >= 2 { h = fnv1a(h, &sub[n.b as usize].to_le_bytes()); }
                if ar >= 3 { h = fnv1a(h, &sub[n.c as usize].to_le_bytes()); }
            }
        }
        sub.push(h);
    }
    sub[t.root as usize]
}
