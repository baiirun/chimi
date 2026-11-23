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
//! The safe readers (`get`, `iter`, `Index`) may be called on the local
//! path only for records homed on the caller's carrier: another carrier's
//! records may be written meanwhile, through `homed_mut`.

use super::id_map::IdMap;
use super::ready::Carrier;
use std::cell::UnsafeCell;
use std::hash::Hash;

/// Records by id, iterated in no particular order (as `IdMap`).
pub(crate) struct Homed<K, R> {
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
    pub(crate) fn insert(&mut self, id: K, home: Carrier, record: R) {
        let previous = self.slots.insert(id, Slot { home, record: UnsafeCell::new(record) });
        assert!(previous.is_none(), "a record id is filed once");
    }

    pub(crate) fn remove(&mut self, id: &K) -> Option<R> {
        self.slots.remove(id).map(|slot| slot.record.into_inner())
    }

    #[inline]
    pub(crate) fn get(&self, id: &K) -> Option<&R> {
        // SAFETY: a record is written through `&mut self`, which cannot
        // coexist with this borrow, or by the local path of its home
        // carrier (`homed_mut`). This runs on the write side, where no local
        // path runs, or on a local path for a record homed on its own
        // carrier, which no other thread writes.
        self.slots.get(id).map(|slot| unsafe { &*slot.record.get() })
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, id: &K) -> Option<&mut R> {
        self.slots.get_mut(id).map(|slot| slot.record.get_mut())
    }

    /// The record `id`, if it is filed and homed on `carrier`; None
    /// otherwise, which the local path takes as "not local".
    ///
    /// # Safety
    ///
    /// The caller may write records homed on `carrier`: it has exclusive
    /// access to the kernel, or holds the kernel lock's read side as the
    /// one thread acting as `carrier` (`carrier::Local`). No other
    /// reference to this record is live, and none is made while the
    /// returned one is.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn homed_mut(&self, id: &K, carrier: Carrier) -> Option<&mut R> {
        let slot = self.slots.get(id).filter(|slot| slot.home == carrier)?;
        // SAFETY: the caller's contract; a record homed on `carrier` is
        // written by no other thread while the caller acts as `carrier`.
        Some(unsafe { &mut *slot.record.get() })
    }

    /// The record and its home, both writable, in one lookup.
    #[inline]
    pub(crate) fn get_mut_homed(&mut self, id: &K) -> Option<(&mut Carrier, &mut R)> {
        self.slots
            .get_mut(id)
            .map(|slot| (&mut slot.home, slot.record.get_mut()))
    }

    pub(crate) fn contains_key(&self, id: &K) -> bool {
        self.slots.contains_key(id)
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// The carrier `id` is homed on, if it is filed.
    #[inline]
    pub(crate) fn home(&self, id: &K) -> Option<Carrier> {
        self.slots.get(id).map(|slot| slot.home)
    }

    /// Makes `home` the home of `id`. Panics if `id` is not filed.
    pub(crate) fn set_home(&mut self, id: &K, home: Carrier) {
        self.slots.get_mut(id).expect("a filed record").home = home;
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &R)> {
        self.iter_homed().map(|(id, _, record)| (id, record))
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &R> {
        self.iter().map(|(_, record)| record)
    }

    /// Every id, in order.
    pub(crate) fn ids_in_order(&self) -> Vec<K>
    where
        K: Ord,
    {
        let mut ids: Vec<K> = self.slots.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Every record with its id and home.
    pub(crate) fn iter_homed(&self) -> impl Iterator<Item = (&K, Carrier, &R)> {
        // SAFETY: as for `get`.
        self.slots
            .iter()
            .map(|(id, slot)| (id, slot.home, unsafe { &*slot.record.get() }))
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut R)> {
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
