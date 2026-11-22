//! A table of records that each have a home carrier: the task table today.
//!
//! Each record sits in its own `UnsafeCell`, with its home beside the cell
//! rather than inside it. That is the shape the local path of step 4e
//! needs (docs/architecture/parallel-scheduler.md, "Step 4 plan", the
//! access model): a carrier holding only its own lock reads the table's
//! structure and any record's home through shared references, and writes
//! only records homed on it, through their cells, while other carriers do
//! the same with theirs. Inserting or removing a record, or changing its
//! home, needs `&mut Homed`, which only every lock gives.
//!
//! Until the local path exists every access goes through `&Homed` or
//! `&mut Homed`, and a record is written only through `&mut Homed`, so the
//! table behaves as the map it wraps.

use super::id_map::IdMap;
use super::ready::Carrier;
use std::cell::UnsafeCell;
use std::hash::Hash;

/// Records by id, iterated in no particular order (as `IdMap`).
pub(super) struct Homed<K, R> {
    slots: IdMap<K, Slot<R>>,
}

struct Slot<R> {
    home: Carrier,
    record: UnsafeCell<R>,
}

impl<K: Copy + Eq + Hash, R> Default for Homed<K, R> {
    fn default() -> Self {
        Self { slots: IdMap::default() }
    }
}

impl<K: Copy + Eq + Hash, R> Homed<K, R> {
    /// Files `record` under `id`, homed on `home`. Requires: `id` is not
    /// filed.
    pub(super) fn insert(&mut self, id: K, home: Carrier, record: R) {
        let previous = self.slots.insert(id, Slot { home, record: UnsafeCell::new(record) });
        assert!(previous.is_none(), "a record id is filed once");
    }

    pub(super) fn remove(&mut self, id: &K) -> Option<R> {
        self.slots.remove(id).map(|slot| slot.record.into_inner())
    }

    #[inline]
    pub(super) fn get(&self, id: &K) -> Option<&R> {
        // SAFETY: a record is written only through `&mut self` (`get_mut`,
        // `iter_mut`), which cannot coexist with this shared borrow.
        self.slots.get(id).map(|slot| unsafe { &*slot.record.get() })
    }

    #[inline]
    pub(super) fn get_mut(&mut self, id: &K) -> Option<&mut R> {
        self.slots.get_mut(id).map(|slot| slot.record.get_mut())
    }

    pub(super) fn contains_key(&self, id: &K) -> bool {
        self.slots.contains_key(id)
    }

    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    /// The carrier `id` is homed on, if it is filed.
    #[inline]
    pub(super) fn home(&self, id: &K) -> Option<Carrier> {
        self.slots.get(id).map(|slot| slot.home)
    }

    /// Makes `home` the home of `id`. Panics if `id` is not filed.
    pub(super) fn set_home(&mut self, id: &K, home: Carrier) {
        self.slots.get_mut(id).expect("a filed record").home = home;
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&K, &R)> {
        self.iter_homed().map(|(id, _, record)| (id, record))
    }

    /// Every record with its id and home.
    pub(super) fn iter_homed(&self) -> impl Iterator<Item = (&K, Carrier, &R)> {
        // SAFETY: as for `get`.
        self.slots
            .iter()
            .map(|(id, slot)| (id, slot.home, unsafe { &*slot.record.get() }))
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut R)> {
        self.slots
            .iter_mut()
            .map(|(id, slot)| (id, slot.record.get_mut()))
    }
}

impl<K: Copy + Eq + Hash + std::fmt::Debug, R> std::ops::Index<&K> for Homed<K, R> {
    type Output = R;

    #[inline]
    fn index(&self, id: &K) -> &R {
        match self.get(id) {
            Some(record) => record,
            None => missing(id),
        }
    }
}

#[cold]
#[inline(never)]
fn missing<K: std::fmt::Debug>(id: &K) -> ! {
    panic!("no record for {id:?}")
}

#[cfg(test)]
mod tests {
    use super::{Carrier, Homed};

    /// A home changes without touching the record, and a removed record
    /// comes back whole.
    #[test]
    fn a_record_keeps_its_value_when_its_home_changes() {
        let mut table: Homed<u64, Vec<u8>> = Homed::default();
        table.insert(7, Carrier(1), vec![1, 2]);
        table.get_mut(&7).expect("filed").push(3);
        table.set_home(&7, Carrier(0));
        assert_eq!(table.home(&7), Some(Carrier(0)));
        assert_eq!(table[&7], vec![1, 2, 3]);
        assert_eq!(table.iter_homed().map(|(id, home, _)| (*id, home)).collect::<Vec<_>>(), [(7, Carrier(0))]);
        assert_eq!(table.remove(&7), Some(vec![1, 2, 3]));
        assert_eq!(table.home(&7), None);
        assert!(table.get(&7).is_none());
    }

    #[test]
    #[should_panic(expected = "a record id is filed once")]
    fn an_id_is_filed_once() {
        let mut table: Homed<u64, u8> = Homed::default();
        table.insert(1, Carrier(0), 1);
        table.insert(1, Carrier(1), 2);
    }
}
