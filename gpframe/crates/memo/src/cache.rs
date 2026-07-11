//! Key = (term structural hash, env FNV); value entry stores the FULL env
//! bits and term for the authority compare.  [T6/D8]
//!
//! Correctness invariants (in code order in `eval`):
//!   1. lookup by (term hash, env hash)      — index only, NO allocation
//!   2. FULL-KEY COMPARE: stored env bits AND stored term — authority
//!      (O6 DERIVED for the term; bitwise env equality for the args)
//!   3. compare failed ⇒ miss                — collision costs time, never truth
//!   4. eviction drops entries               — recompute of a pure fn (T6)
//!
//! Concurrency: Mutex<HashMap> reference impl; DashMap swap (v1 M4) is a
//! lock-granularity/perf-only change.

use crate::evict::Lru;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use term::{eval, Term};

type Key = (u64, u64); // (term structural hash, env bits FNV)

#[inline]
fn env_fnv(env: &[f64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for x in env {
        for b in x.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

#[inline]
fn env_matches(stored: &[u64], env: &[f64]) -> bool {
    stored.len() == env.len()
        && stored.iter().zip(env).all(|(s, x)| *s == x.to_bits())
}

struct Entry {
    val: f64,
    env_bits: Vec<u64>,
    term: Term,
}

fn entry_bytes(e: &Entry) -> usize {
    64 + e.env_bits.len() * 8 + e.term.len() * 16 + e.term.consts.len() * 8
}

struct Inner {
    map: HashMap<Key, Entry>,
    lru: Lru<Key>,
    bytes: usize,
}

pub struct MemoCache {
    inner: Mutex<Inner>,
    cap_bytes: usize,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions: AtomicU64,
}

impl Default for MemoCache {
    fn default() -> Self { Self::with_capacity(64 * 1024 * 1024) }
}

impl MemoCache {
    pub fn new() -> Self { Self::default() }

    pub fn with_capacity(cap_bytes: usize) -> Self {
        MemoCache {
            inner: Mutex::new(Inner { map: HashMap::new(), lru: Lru::new(), bytes: 0 }),
            cap_bytes,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize { self.inner.lock().unwrap().map.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// eval-through-cache; see module invariants. The hit path performs
    /// zero heap allocation.
    pub fn eval(&self, t: &Term, env: &[f64]) -> f64 {
        let key: Key = (t.hash, env_fnv(env));
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(e) = g.map.get(&key) {
                if env_matches(&e.env_bits, env) && e.term.structurally_eq(t) {
                    let v = e.val;
                    g.lru.touch(&key);
                    self.hits.fetch_add(1, Relaxed);
                    return v;
                }
                // double-hash collision: authority says miss
            }
        }
        self.misses.fetch_add(1, Relaxed);
        let v = eval(t, env);
        let entry = Entry {
            val: v,
            env_bits: env.iter().map(|x| x.to_bits()).collect(),
            term: t.clone(),
        };
        let sz = entry_bytes(&entry);
        let mut g = self.inner.lock().unwrap();
        if g.map.insert(key, entry).is_none() {
            g.bytes += sz;
        }
        g.lru.touch(&key);
        while g.bytes > self.cap_bytes {
            let Some(victim) = g.lru.pop_lru() else { break };
            if let Some(e) = g.map.remove(&victim) {
                g.bytes -= entry_bytes(&e);
                self.evictions.fetch_add(1, Relaxed);
            }
        }
        v
    }
}
