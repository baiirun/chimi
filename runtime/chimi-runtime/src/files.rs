//! The files `std/fs` has open, shared by the interpreter (which keeps
//! `std::fs::File`s) and the native runtime (which keeps descriptors, through
//! `files_abi`).
//!
//! A handle is `slot | generation << 32`, so a handle used after its file was
//! closed names a slot whose generation moved on and fails as closed instead
//! of reaching another file. A resource may be lent out while open (`take`,
//! `give`): a close during a loan leaves the resource in its slot until the
//! last loan ends, so a descriptor's number is not reused under a system call
//! that still uses it. Only a slot holding no resource is reused.

use alloc::vec::Vec;

struct Slot<T> {
    generation: u32,
    open: bool,
    loans: u32,
    resource: Option<T>,
}

/// Open files by handle.
pub struct FileTable<T> {
    slots: Vec<Slot<T>>,
}

impl<T> Default for FileTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// What a close did.
#[derive(Debug, PartialEq, Eq)]
pub enum Closed<T> {
    /// The handle named no open file.
    NotOpen,
    /// The file is closed; a loan still holds the resource, which its
    /// `give` hands back.
    Lent,
    /// The file is closed and this is its resource, to be released.
    Released(T),
}

fn split(handle: i64) -> Option<(usize, u32)> {
    if handle < 0 {
        return None;
    }
    let slot = usize::try_from(handle & 0xffff_ffff).ok()?;
    let generation = u32::try_from(handle >> 32).ok()?;
    Some((slot, generation))
}

impl<T> FileTable<T> {
    pub const fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Opens `resource` and returns its handle.
    pub fn insert(&mut self, resource: T) -> i64 {
        let slot = match self.slots.iter().position(|slot| slot.resource.is_none()) {
            Some(slot) => slot,
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    open: false,
                    loans: 0,
                    resource: None,
                });
                self.slots.len() - 1
            }
        };
        let entry = &mut self.slots[slot];
        entry.open = true;
        entry.resource = Some(resource);
        slot as i64 | (i64::from(entry.generation) << 32)
    }

    fn open_slot(&mut self, handle: i64) -> Option<&mut Slot<T>> {
        let (slot, generation) = split(handle)?;
        let entry = self.slots.get_mut(slot)?;
        (entry.open && entry.generation == generation).then_some(entry)
    }

    /// The resource of open `handle`.
    pub fn get_mut(&mut self, handle: i64) -> Option<&mut T> {
        self.open_slot(handle)?.resource.as_mut()
    }

    /// Lends out the resource of open `handle` until `give(handle)`.
    pub fn take(&mut self, handle: i64) -> Option<&T> {
        let entry = self.open_slot(handle)?;
        entry.loans += 1;
        entry.resource.as_ref()
    }

    /// Ends a loan `take(handle)` began. Returns the resource when this was
    /// the last loan of a file closed meanwhile, to be released.
    pub fn give(&mut self, handle: i64) -> Option<T> {
        let (slot, _) = split(handle)?;
        let entry = self.slots.get_mut(slot)?;
        entry.loans = entry.loans.checked_sub(1)?;
        if entry.loans == 0 && !entry.open {
            entry.resource.take()
        } else {
            None
        }
    }

    /// Closes `handle`.
    pub fn close(&mut self, handle: i64) -> Closed<T> {
        let Some(entry) = self.open_slot(handle) else {
            return Closed::NotOpen;
        };
        entry.open = false;
        entry.generation = entry.generation.wrapping_add(1);
        if entry.loans != 0 {
            return Closed::Lent;
        }
        match entry.resource.take() {
            Some(resource) => Closed::Released(resource),
            None => Closed::Lent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_handle_names_nothing_even_after_its_slot_is_reused() {
        let mut table = FileTable::new();
        let first = table.insert(10);
        assert_eq!(table.close(first), Closed::Released(10));
        let second = table.insert(11);
        assert_eq!(second & 0xffff_ffff, first & 0xffff_ffff, "the slot is reused");
        assert_ne!(second, first, "with a new generation");
        assert_eq!(table.get_mut(first), None);
        assert_eq!(table.take(first), None);
        assert_eq!(table.close(first), Closed::NotOpen);
        assert_eq!(table.get_mut(second), Some(&mut 11));
    }

    #[test]
    fn a_close_during_a_loan_releases_at_the_last_give() {
        let mut table = FileTable::new();
        let handle = table.insert(7);
        assert_eq!(table.take(handle), Some(&7));
        assert_eq!(table.take(handle), Some(&7));
        assert_eq!(table.close(handle), Closed::Lent);
        assert_eq!(table.take(handle), None, "closed to new loans");
        let other = table.insert(8);
        assert_ne!(
            other & 0xffff_ffff,
            handle & 0xffff_ffff,
            "a lent slot is not reused"
        );
        assert_eq!(table.give(handle), None, "one loan is still out");
        assert_eq!(table.give(handle), Some(7));
        assert_eq!(table.give(handle), None, "no loan is left to end");
        assert_eq!(table.insert(9) & 0xffff_ffff, handle & 0xffff_ffff);
    }

    #[test]
    fn a_give_of_an_open_file_keeps_it() {
        let mut table = FileTable::new();
        let handle = table.insert(3);
        assert_eq!(table.take(handle), Some(&3));
        assert_eq!(table.give(handle), None);
        assert_eq!(table.close(handle), Closed::Released(3));
    }

    #[test]
    fn malformed_handles_name_nothing() {
        let mut table = FileTable::<i32>::new();
        let handle = table.insert(1);
        for bad in [-1, i64::MIN, handle + 1, handle | (1 << 32)] {
            assert_eq!(table.get_mut(bad), None, "{bad}");
            assert_eq!(table.give(bad), None, "{bad}");
        }
    }
}
