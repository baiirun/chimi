//! How many live values name each native handle, through `handle_counts_abi`.
//!
//! A copy of a native handle value retains its key and a freed one drops it;
//! the drop that removes a key's last reference reports it, and the caller
//! then closes the handle no value names any more. A key is a handle's slot
//! and generation, so the values of a closed handle's old generation keep
//! their own count until they are freed, and that count's zero finds the slot
//! moved on.
//!
//! An open-addressing map keyed by handle key, at most half full, with
//! backward-shift deletion so every probe sequence stays unbroken. Keys are
//! never 0, which marks an empty entry.

use alloc::vec::Vec;

#[derive(Clone, Copy, Default)]
struct Entry {
    key: u64,
    references: usize,
}

/// Live references per handle key.
pub struct HandleCounts {
    entries: Vec<Entry>,
    len: usize,
}

impl Default for HandleCounts {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleCounts {
    pub const fn new() -> Self {
        Self { entries: Vec::new(), len: 0 }
    }

    /// Keys with at least one reference.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entries the map has room for.
    pub fn capacity(&self) -> usize {
        self.entries.len()
    }

    fn mask(&self) -> usize {
        self.entries.len().wrapping_sub(1)
    }

    fn home(&self, key: u64) -> usize {
        // Fibonacci hashing, as the C table did; the high bits mix best.
        let mixed = key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32;
        usize::try_from(mixed).unwrap_or(usize::MAX) & self.mask()
    }

    /// The entry holding `key`, or the empty one where it would go.
    fn find(&self, key: u64) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let mut at = self.home(key);
        loop {
            let entry = self.entries.get(at)?;
            if entry.key == key || entry.key == 0 {
                return Some(at);
            }
            at = (at + 1) & self.mask();
        }
    }

    /// Doubles the room (64 entries at first); false, with the map as it
    /// was, when there is no memory for it.
    fn grow(&mut self) -> bool {
        let capacity = if self.entries.is_empty() { 64 } else { self.entries.len().saturating_mul(2) };
        let mut grown = Vec::new();
        if grown.try_reserve_exact(capacity).is_err() {
            return false;
        }
        grown.resize(capacity, Entry::default());
        let old = core::mem::replace(&mut self.entries, grown);
        for entry in old.into_iter().filter(|entry| entry.key != 0) {
            if let Some(at) = self.find(entry.key)
                && let Some(slot) = self.entries.get_mut(at)
            {
                *slot = entry;
            }
        }
        true
    }

    /// Counts one more reference to `key`; false, counting nothing, when the
    /// map has no memory to grow. Key 0 is never counted.
    pub fn retain(&mut self, key: u64) -> bool {
        if key == 0 {
            return true;
        }
        if self.len.saturating_add(1).saturating_mul(2) > self.entries.len() && !self.grow() {
            return false;
        }
        let Some(at) = self.find(key) else { return false };
        let Some(entry) = self.entries.get_mut(at) else { return false };
        if entry.key == 0 {
            entry.key = key;
            self.len += 1;
        }
        entry.references = entry.references.saturating_add(1);
        true
    }

    /// Removes one reference to `key`: true exactly when it was the last, so
    /// the caller owns the handle's close. A key the map does not hold (an
    /// earlier execution's, after a reset) is not counted.
    pub fn drop(&mut self, key: u64) -> bool {
        if key == 0 {
            return false;
        }
        let Some(at) = self.find(key) else { return false };
        let Some(entry) = self.entries.get_mut(at) else { return false };
        if entry.key != key {
            return false;
        }
        entry.references = entry.references.saturating_sub(1);
        if entry.references != 0 {
            return false;
        }
        self.remove(at);
        true
    }

    /// Empties entry `hole`, shifting later entries of its run back over it.
    fn remove(&mut self, mut hole: usize) {
        let mask = self.mask();
        let mut next = (hole + 1) & mask;
        while let Some(entry) = self.entries.get(next).copied().filter(|entry| entry.key != 0) {
            let home = self.home(entry.key);
            if (next.wrapping_sub(home) & mask) >= (next.wrapping_sub(hole) & mask) {
                if let Some(slot) = self.entries.get_mut(hole) {
                    *slot = entry;
                }
                hole = next;
            }
            next = (next + 1) & mask;
        }
        if let Some(slot) = self.entries.get_mut(hole) {
            *slot = Entry::default();
        }
        self.len = self.len.saturating_sub(1);
    }

    /// Forgets every count, keeping the room: each execution starts a new
    /// scheduler, whose ids start over.
    pub fn reset(&mut self) {
        self.entries.iter_mut().for_each(|entry| *entry = Entry::default());
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_drop_of_a_key_reports_it_once() {
        let mut counts = HandleCounts::new();
        assert!(counts.retain(7));
        assert!(counts.retain(7));
        assert_eq!(counts.len(), 1);
        assert!(!counts.drop(7));
        assert!(counts.drop(7));
        assert!(!counts.drop(7));
        assert!(counts.is_empty());
    }

    #[test]
    fn keys_count_apart_and_an_unknown_key_is_not_counted() {
        let mut counts = HandleCounts::new();
        assert!(!counts.drop(5));
        assert!(counts.retain(1));
        assert!(counts.retain(2));
        assert!(!counts.drop(3));
        assert!(counts.drop(2));
        assert_eq!(counts.len(), 1);
        assert!(counts.drop(1));
    }

    #[test]
    fn key_zero_is_never_counted() {
        let mut counts = HandleCounts::new();
        assert!(counts.retain(0));
        assert!(counts.is_empty());
        assert!(!counts.drop(0));
    }

    #[test]
    fn the_map_grows_and_keeps_every_count() {
        let mut counts = HandleCounts::new();
        for key in 1..=10_000u64 {
            assert!(counts.retain(key << 32 | 3));
            assert!(counts.retain(key << 32 | 3));
        }
        assert_eq!(counts.len(), 10_000);
        assert!(counts.capacity() >= 20_000);
        for key in 1..=10_000u64 {
            assert!(!counts.drop(key << 32 | 3), "key {key} lost a reference");
        }
        for key in (1..=10_000u64).rev() {
            assert!(counts.drop(key << 32 | 3), "key {key} kept a reference");
        }
        assert!(counts.is_empty());
    }

    #[test]
    fn removing_from_the_middle_of_a_run_keeps_the_rest_findable() {
        let mut counts = HandleCounts::new();
        assert!(counts.retain(1));
        // Keys that share key 1's home entry, found by search.
        let home = counts.home(1);
        let sharing: Vec<u64> = (2..u64::MAX).filter(|key| counts.home(*key) == home).take(4).collect();
        for key in &sharing {
            assert!(counts.retain(*key));
        }
        assert!(counts.drop(1));
        for key in &sharing {
            assert!(counts.drop(*key), "key {key} was lost");
        }
        assert!(counts.is_empty());
    }

    #[test]
    fn a_reset_forgets_every_count_and_keeps_the_room() {
        let mut counts = HandleCounts::new();
        for key in 1..=100 {
            assert!(counts.retain(key));
        }
        let capacity = counts.capacity();
        counts.reset();
        assert!(counts.is_empty());
        assert_eq!(counts.capacity(), capacity);
        assert!(!counts.drop(5));
        assert!(counts.retain(5));
        assert!(counts.drop(5));
    }
}
