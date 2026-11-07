//! The in-memory half of a cache: values by key, bounded by their weight, with the
//! least recently used going first.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

struct Slot<V> {
    value: Arc<V>,
    weight: usize,
    /// When it was last used. Higher is newer.
    tick: u64,
}

pub struct Lru<V> {
    slots: HashMap<String, Slot<V>>,
    /// Keys by `tick`, so the oldest is the first entry.
    order: BTreeMap<u64, String>,
    clock: u64,
    used: usize,
    capacity: usize,
}

impl<V> Lru<V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
            used: 0,
            capacity,
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// The value for `key`, which then counts as the newest.
    pub fn get(&mut self, key: &str) -> Option<Arc<V>> {
        let tick = self.next_tick();
        let slot = self.slots.get_mut(key)?;
        self.order.remove(&slot.tick);
        slot.tick = tick;
        self.order.insert(tick, key.to_owned());
        Some(slot.value.clone())
    }

    /// Stores `value`, then drops the oldest until what is left fits. A value heavier
    /// than the whole cache isn't kept, because keeping it would push out everything.
    pub fn insert(&mut self, key: &str, value: Arc<V>, weight: usize) {
        self.remove(key);
        if weight > self.capacity {
            return;
        }
        let tick = self.next_tick();
        self.order.insert(tick, key.to_owned());
        self.slots.insert(
            key.to_owned(),
            Slot {
                value,
                weight,
                tick,
            },
        );
        self.used += weight;
        while self.used > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(slot) = self.slots.remove(&oldest) {
                self.used -= slot.weight;
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(slot) = self.slots.remove(key) {
            self.order.remove(&slot.tick);
            self.used -= slot.weight;
        }
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.order.clear();
        self.used = 0;
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(lru: &mut Lru<&'static str>, key: &str, weight: usize) {
        lru.insert(key, Arc::new("v"), weight);
    }

    #[test]
    fn the_least_recently_used_goes_first() {
        let mut lru = Lru::new(30);
        put(&mut lru, "a", 10);
        put(&mut lru, "b", 10);
        put(&mut lru, "c", 10);
        // Using `a` makes `b` the oldest.
        assert!(lru.get("a").is_some());
        put(&mut lru, "d", 10);
        assert!(lru.get("b").is_none());
        assert!(lru.get("a").is_some() && lru.get("c").is_some() && lru.get("d").is_some());
    }

    #[test]
    fn weight_not_count_is_what_is_bounded() {
        let mut lru = Lru::new(100);
        for key in ["a", "b", "c", "d"] {
            put(&mut lru, key, 20);
        }
        put(&mut lru, "big", 90);
        assert_eq!(lru.len(), 1);
        assert!(lru.get("big").is_some());
    }

    #[test]
    fn a_value_too_heavy_for_the_cache_is_not_kept_and_pushes_nothing_out() {
        let mut lru = Lru::new(50);
        put(&mut lru, "a", 10);
        put(&mut lru, "huge", 51);
        assert!(lru.get("huge").is_none());
        assert!(lru.get("a").is_some());
    }

    #[test]
    fn replacing_a_key_replaces_its_weight() {
        let mut lru = Lru::new(100);
        put(&mut lru, "a", 60);
        put(&mut lru, "a", 20);
        put(&mut lru, "b", 70);
        assert!(lru.get("a").is_some() && lru.get("b").is_some());
        lru.clear();
        assert_eq!(lru.len(), 0);
    }
}
