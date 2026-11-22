//! A hash map for the scheduler's sequential ids, read on every step under
//! the kernel lock.
//!
//! The ids are handed out by a counter, so they need no defence against
//! chosen keys: one multiplication spreads them over the buckets, where the
//! standard library's SipHash costs several times as much per lookup. A
//! `BTreeMap` costs a comparison per tree level instead (step 4 PR 4d-2 in
//! docs/architecture/parallel-scheduler.md).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// A map from an id to its record, iterated in no particular order.
pub(crate) type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

/// Hashes one integer id by a multiplication by an odd constant (the
/// golden-ratio constant, as in rustc's FxHash): a bijection, so distinct
/// ids never collide in the full hash, and the low bits that pick a bucket
/// differ for consecutive ids.
#[derive(Default)]
pub(crate) struct IdHasher(u64);

const SPREAD: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(byte)).wrapping_mul(SPREAD);
        }
    }

    fn write_u64(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SPREAD);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::hash::BuildHasher;

    use crate::scheduler::TaskId;

    /// hashbrown picks a bucket from the low bits of the hash: a table of
    /// 1024 buckets must give 1024 consecutive ids 1024 distinct buckets.
    #[test]
    fn consecutive_ids_land_in_distinct_buckets() {
        let build = BuildHasherDefault::<IdHasher>::default();
        let buckets: BTreeSet<u64> =
            (1..=1024).map(|raw| build.hash_one(TaskId::from_raw(raw)) & 1023).collect();
        assert_eq!(buckets.len(), 1024);
    }

    /// The top seven bits are hashbrown's tag, compared before the key: ids
    /// that differ only in their low bits must not all share one tag.
    #[test]
    fn consecutive_ids_spread_their_tags() {
        let build = BuildHasherDefault::<IdHasher>::default();
        let tags: BTreeSet<u64> =
            (1..=1024).map(|raw| build.hash_one(TaskId::from_raw(raw)) >> 57).collect();
        assert!(tags.len() > 100, "{} tags", tags.len());
    }

    #[test]
    fn bytes_in_another_order_hash_differently() {
        let mut one = IdHasher::default();
        one.write(&[1, 2]);
        let mut other = IdHasher::default();
        other.write(&[2, 1]);
        assert_ne!(one.finish(), other.finish());
    }
}
