//! The native handles binding packages open (sockets, children, pipes, TLS
//! sessions, databases): a slot per live resource, through `handles_abi`.
//!
//! A value names its slot and the slot's generation, so a copy of a closed
//! handle stays closed when the slot is reused. A close moves the generation
//! on, and a generation only grows: the scheduler treats a withdraw of a
//! descriptor wait as final for that generation and older ones of the slot. A
//! slot whose generation would wrap retires instead of being reused. Slot 0 is
//! never used, so a zeroed value is never open.
//!
//! A binding call holds every open handle among its arguments for its whole
//! length (`hold`, `let_go`), so a close meanwhile only marks the handle
//! closing: its state stays in the slot until the last holder lets go, which
//! hands the state back for release. One call at a time runs binding code on a
//! handle (`runner`); waiting for a busy handle is the caller's.
//!
//! The table only keeps the books. It never runs a release: every state it
//! hands back is the caller's to release once it has let go of its lock.

use alloc::vec::Vec;

/// At most this many slots, slot 0 included.
pub const LIMIT: u32 = 1 << 20;

/// A handle's state, handed back to be released by its kind's release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Release<K, S> {
    pub kind: K,
    pub state: S,
}

#[derive(Clone, Copy)]
enum Slot<K, S, R> {
    /// Unused; `next` is the next free slot, 0 at the end of the list.
    Free { generation: u32, next: u32 },
    /// Never used again: slot 0, or a slot whose generation reached the end.
    Retired,
    Open(Live<K, S, R>),
    /// Closed while held: the state waits for the last holder to let go. The
    /// generation has already moved on, so no value names it.
    Closing(Live<K, S, R>),
}

#[derive(Clone, Copy)]
struct Live<K, S, R> {
    kind: K,
    state: S,
    generation: u32,
    /// Binding calls holding the handle now.
    users: u32,
    /// The call running binding code on the handle.
    runner: Option<R>,
}

/// What a close did.
#[derive(Debug, PartialEq, Eq)]
pub enum Closed<K, S> {
    /// The handle named no open handle (of the asked kind).
    NotOpen,
    /// Closed; a holder keeps the state until it lets go.
    Held,
    /// Closed, and this state is the caller's to release.
    Released(Release<K, S>),
}

/// The open native handles. `K` is a handle's kind, `S` its state and `R`
/// the token of a running call.
pub struct Handles<K, S, R> {
    slots: Vec<Slot<K, S, R>>,
    /// The first free slot, 0 when none is.
    free: u32,
}

impl<K: Copy + Eq, S: Copy, R: Copy + Eq> Default for Handles<K, S, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy + Eq, S: Copy, R: Copy + Eq> Handles<K, S, R> {
    pub const fn new() -> Self {
        Self { slots: Vec::new(), free: 0 }
    }

    /// Slots made so far, slot 0 included; at least 1.
    pub fn len(&self) -> u32 {
        u32::try_from(self.slots.len()).unwrap_or(LIMIT).max(1)
    }

    /// True before the first handle is opened.
    pub fn is_empty(&self) -> bool {
        self.slots.len() <= 1
    }

    /// Opens a handle of `kind` holding `state`: its slot and generation, or
    /// `None` when the table is full (the state is then still the caller's).
    /// Running out of memory to grow it ends the program, as every runtime
    /// allocation does (the C table returned `None` then too).
    pub fn open(&mut self, kind: K, state: S) -> Option<(u32, u32)> {
        let slot = match self.free {
            0 => self.grow()?,
            free => free,
        };
        let entry = self.slots.get_mut(index(slot))?;
        let Slot::Free { generation, next } = *entry else {
            return None;
        };
        *entry = Slot::Open(Live { kind, state, generation, users: 0, runner: None });
        if slot == self.free {
            self.free = next;
        }
        Some((slot, generation))
    }

    /// A new free slot at the end, not yet on the free list.
    fn grow(&mut self) -> Option<u32> {
        if self.slots.is_empty() {
            self.slots.try_reserve(64).ok()?;
            self.slots.push(Slot::Retired);
        }
        let slot = u32::try_from(self.slots.len()).ok().filter(|slot| *slot < LIMIT)?;
        self.slots.try_reserve(1).ok()?;
        self.slots.push(Slot::Free { generation: 0, next: 0 });
        Some(slot)
    }

    fn open_entry(&mut self, slot: u32, generation: u32) -> Option<&mut Live<K, S, R>> {
        match self.slots.get_mut(index(slot)) {
            Some(Slot::Open(live)) if live.generation == generation => Some(live),
            _ => None,
        }
    }

    fn live(&mut self, slot: u32) -> Option<&mut Live<K, S, R>> {
        match self.slots.get_mut(index(slot)) {
            Some(Slot::Open(live) | Slot::Closing(live)) => Some(live),
            _ => None,
        }
    }

    /// The kind and state of the open handle (slot, generation).
    pub fn get(&mut self, slot: u32, generation: u32) -> Option<(K, S)> {
        self.open_entry(slot, generation).map(|live| (live.kind, live.state))
    }

    /// Puts a slot whose state is released, or about to be, on the free list,
    /// or retires it when its generation is at the end.
    fn free(&mut self, slot: u32, generation: u32) {
        let next = self.free;
        let Some(entry) = self.slots.get_mut(index(slot)) else { return };
        if generation == u32::MAX {
            *entry = Slot::Retired;
        } else {
            *entry = Slot::Free { generation, next };
            self.free = slot;
        }
    }

    /// Closes the open handle (slot, generation), of `kind` unless `None`.
    pub fn close(&mut self, slot: u32, generation: u32, kind: Option<K>) -> Closed<K, S> {
        let Some(live) = self.open_entry(slot, generation) else {
            return Closed::NotOpen;
        };
        if kind.is_some_and(|kind| kind != live.kind) {
            return Closed::NotOpen;
        }
        // An open slot's generation is below the end: reaching it retires.
        live.generation = generation.saturating_add(1);
        let live = *live;
        if live.users > 0 {
            if let Some(entry) = self.slots.get_mut(index(slot)) {
                *entry = Slot::Closing(live);
            }
            return Closed::Held;
        }
        self.free(slot, live.generation);
        Closed::Released(Release { kind: live.kind, state: live.state })
    }

    /// Counts one more call holding the open handle (slot, generation); false
    /// when it is not open.
    pub fn hold(&mut self, slot: u32, generation: u32) -> bool {
        match self.open_entry(slot, generation) {
            Some(live) => {
                live.users = live.users.saturating_add(1);
                true
            }
            None => false,
        }
    }

    /// Counts one call holding `slot` fewer. The last holder of a handle
    /// closed meanwhile gets its state to release.
    pub fn let_go(&mut self, slot: u32) -> Option<Release<K, S>> {
        let entry = self.slots.get_mut(index(slot))?;
        match entry {
            Slot::Open(live) => {
                live.users = live.users.saturating_sub(1);
                None
            }
            Slot::Closing(live) => {
                live.users = live.users.saturating_sub(1);
                if live.users > 0 {
                    return None;
                }
                let live = *live;
                self.free(slot, live.generation);
                Some(Release { kind: live.kind, state: live.state })
            }
            Slot::Free { .. } | Slot::Retired => None,
        }
    }

    /// The call running binding code on `slot`, if any.
    pub fn runner(&mut self, slot: u32) -> Option<R> {
        self.live(slot).and_then(|live| live.runner)
    }

    /// Starts (`Some`) or stops (`None`) a call running on `slot`.
    pub fn set_runner(&mut self, slot: u32, runner: Option<R>) {
        if let Some(live) = self.live(slot) {
            live.runner = runner;
        }
    }

    /// At the program's end, when nothing runs: hands back the state of
    /// `slot` if it is open or closing, ignoring holders (a task abandoned
    /// inside a call still counts as one).
    pub fn end(&mut self, slot: u32) -> Option<Release<K, S>> {
        let (live, generation) = match self.slots.get(index(slot))? {
            Slot::Open(live) => (*live, live.generation.saturating_add(1)),
            Slot::Closing(live) => (*live, live.generation),
            Slot::Free { .. } | Slot::Retired => return None,
        };
        self.free(slot, generation);
        Some(Release { kind: live.kind, state: live.state })
    }
}

fn index(slot: u32) -> usize {
    usize::try_from(slot).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Table = Handles<u8, u32, u8>;

    #[test]
    fn slot_zero_is_never_used_and_a_new_handle_starts_at_generation_zero() {
        let mut table = Table::new();
        assert!(table.is_empty());
        assert_eq!(table.len(), 1);
        assert_eq!(table.open(1, 10), Some((1, 0)));
        assert_eq!(table.open(1, 11), Some((2, 0)));
        assert_eq!(table.len(), 3);
        assert_eq!(table.get(0, 0), None);
        assert_eq!(table.get(1, 0), Some((1, 10)));
    }

    #[test]
    fn a_closed_handle_names_nothing_even_after_its_slot_is_reused() {
        let mut table = Table::new();
        let (slot, generation) = table.open(1, 10).unwrap();
        assert_eq!(table.close(slot, generation, None), Closed::Released(Release { kind: 1, state: 10 }));
        assert_eq!(table.get(slot, generation), None);
        assert_eq!(table.close(slot, generation, None), Closed::NotOpen);
        assert_eq!(table.open(2, 20), Some((slot, generation + 1)));
        assert_eq!(table.get(slot, generation), None);
        assert!(!table.hold(slot, generation));
        assert_eq!(table.get(slot, generation + 1), Some((2, 20)));
    }

    #[test]
    fn freed_slots_are_reused_last_freed_first() {
        let mut table = Table::new();
        let a = table.open(1, 1).unwrap();
        let b = table.open(1, 2).unwrap();
        let c = table.open(1, 3).unwrap();
        table.close(a.0, a.1, None);
        table.close(c.0, c.1, None);
        assert_eq!(table.open(1, 4), Some((c.0, 1)));
        assert_eq!(table.open(1, 5), Some((a.0, 1)));
        assert_eq!(table.open(1, 6), Some((4, 0)));
        assert_eq!(table.get(b.0, b.1), Some((1, 2)));
    }

    #[test]
    fn a_close_of_another_kind_leaves_the_handle_open() {
        let mut table = Table::new();
        let (slot, generation) = table.open(1, 10).unwrap();
        assert_eq!(table.close(slot, generation, Some(2)), Closed::NotOpen);
        assert_eq!(table.get(slot, generation), Some((1, 10)));
        assert_eq!(table.close(slot, generation, Some(1)), Closed::Released(Release { kind: 1, state: 10 }));
    }

    #[test]
    fn a_handle_closed_while_held_is_released_by_its_last_holder_and_only_then_reused() {
        let mut table = Table::new();
        let (slot, generation) = table.open(1, 10).unwrap();
        assert!(table.hold(slot, generation));
        assert!(table.hold(slot, generation));
        assert_eq!(table.close(slot, generation, None), Closed::Held);
        assert_eq!(table.get(slot, generation), None);
        assert!(!table.hold(slot, generation));
        // Not reusable while held.
        assert_eq!(table.open(2, 20), Some((2, 0)));
        assert_eq!(table.let_go(slot), None);
        assert_eq!(table.let_go(slot), Some(Release { kind: 1, state: 10 }));
        assert_eq!(table.let_go(slot), None);
        assert_eq!(table.open(3, 30), Some((slot, generation + 1)));
    }

    #[test]
    fn letting_go_of_an_open_handle_keeps_it_open() {
        let mut table = Table::new();
        let (slot, generation) = table.open(1, 10).unwrap();
        assert!(table.hold(slot, generation));
        assert_eq!(table.let_go(slot), None);
        assert_eq!(table.close(slot, generation, None), Closed::Released(Release { kind: 1, state: 10 }));
    }

    #[test]
    fn a_slot_at_the_last_generation_retires() {
        let mut table = Table::new();
        let (slot, _) = table.open(1, 10).unwrap();
        if let Some(Slot::Open(live)) = table.slots.get_mut(slot as usize) {
            live.generation = u32::MAX - 1;
        }
        assert_eq!(table.close(slot, u32::MAX - 1, None), Closed::Released(Release { kind: 1, state: 10 }));
        assert_eq!(table.open(2, 20), Some((slot + 1, 0)));
        assert_eq!(table.get(slot, u32::MAX), None);
        assert_eq!(table.end(slot), None);
    }

    #[test]
    fn the_runner_is_kept_per_slot_and_survives_a_close_while_held() {
        let mut table = Table::new();
        let (slot, generation) = table.open(1, 10).unwrap();
        assert_eq!(table.runner(slot), None);
        table.hold(slot, generation);
        table.set_runner(slot, Some(7));
        assert_eq!(table.runner(slot), Some(7));
        assert_eq!(table.close(slot, generation, None), Closed::Held);
        assert_eq!(table.runner(slot), Some(7));
        table.set_runner(slot, None);
        assert_eq!(table.runner(slot), None);
        assert_eq!(table.let_go(slot), Some(Release { kind: 1, state: 10 }));
        assert_eq!(table.runner(slot), None);
        table.set_runner(slot, Some(7));
        assert_eq!(table.runner(slot), None);
    }

    #[test]
    fn the_end_releases_open_and_closing_handles_once_and_moves_open_ones_on() {
        let mut table = Table::new();
        let open = table.open(1, 10).unwrap();
        let held = table.open(2, 20).unwrap();
        let closed = table.open(3, 30).unwrap();
        table.hold(held.0, held.1);
        assert_eq!(table.close(held.0, held.1, None), Closed::Held);
        table.close(closed.0, closed.1, None);
        assert_eq!(table.end(0), None);
        assert_eq!(table.end(open.0), Some(Release { kind: 1, state: 10 }));
        assert_eq!(table.end(held.0), Some(Release { kind: 2, state: 20 }));
        assert_eq!(table.end(closed.0), None);
        assert_eq!(table.end(open.0), None);
        assert_eq!(table.get(open.0, open.1), None);
        // Each freed slot keeps its generation moving on.
        assert_eq!(table.open(4, 40), Some((held.0, 1)));
        assert_eq!(table.open(4, 41), Some((open.0, 1)));
    }

    #[test]
    fn the_table_stops_at_its_limit() {
        let mut table = Table::new();
        for expected in 1..LIMIT {
            assert_eq!(table.open(1, expected), Some((expected, 0)));
        }
        assert_eq!(table.open(1, 0), None);
        assert_eq!(table.close(5, 0, None), Closed::Released(Release { kind: 1, state: 5 }));
        assert_eq!(table.open(1, 0), Some((5, 1)));
    }

    #[test]
    fn calls_on_slots_out_of_range_do_nothing() {
        let mut table = Table::new();
        assert_eq!(table.get(9, 0), None);
        assert_eq!(table.close(9, 0, None), Closed::NotOpen);
        assert!(!table.hold(9, 0));
        assert_eq!(table.let_go(9), None);
        assert_eq!(table.runner(9), None);
        table.set_runner(9, Some(1));
        assert_eq!(table.end(9), None);
    }
}
