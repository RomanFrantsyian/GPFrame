//! crate `memo` — [D8 T6 A6 | R6] — cache. Deps: term ONLY.
//!
//! T6: Term_p purity ⇒ [[T]] is a function ⇒ caching is an identity on
//! semantics. The cache can be arbitrarily aggressive; it cannot be wrong.
//! O6 → DERIVED: hash is an index; FULL-KEY COMPARE on hit is the authority.

pub mod cache;
pub mod evict;
