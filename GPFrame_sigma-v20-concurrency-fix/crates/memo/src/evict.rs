//! Lazy LRU over hashed keys (v1 M4). Dropping an entry only forces a
//! recompute of a pure function (T6) — eviction is trivially sound.
//!
//! Lazy scheme: every touch pushes (tick, key) onto a queue and bumps the
//! key's current tick; pop_lru discards stale queue entries (tick mismatch)
//! until it finds a live minimum. Amortized O(1) per operation.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub struct Lru<K: Eq + Hash + Clone> {
    tick: u64,
    current: HashMap<K, u64>,
    queue: VecDeque<(u64, K)>,
}

impl<K: Eq + Hash + Clone> Default for Lru<K> {
    fn default() -> Self { Self::new() }
}

impl<K: Eq + Hash + Clone> Lru<K> {
    pub fn new() -> Self {
        Lru { tick: 0, current: HashMap::new(), queue: VecDeque::new() }
    }

    pub fn touch(&mut self, k: &K) {
        self.tick += 1;
        self.current.insert(k.clone(), self.tick);
        self.queue.push_back((self.tick, k.clone()));
    }

    /// Least-recently-used live key, removing it from tracking.
    pub fn pop_lru(&mut self) -> Option<K> {
        while let Some((t, k)) = self.queue.pop_front() {
            if self.current.get(&k) == Some(&t) {
                self.current.remove(&k);
                return Some(k);
            } // else: stale entry for a since-touched key — skip
        }
        None
    }
}
