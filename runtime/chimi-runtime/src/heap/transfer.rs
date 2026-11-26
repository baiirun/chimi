//! The capability walk (CHI-305 step 5, phase 3): visits every part of a
//! value graph once and hands each exclusive capability in it to
//! `runtime.c`, which transfers it to a child task or records it for a send.
//!
//! The walk keeps its own stack, so a graph of any nesting depth costs heap,
//! not the fiber's 64 KiB stack. A part reachable twice (shared after a
//! copy) is visited where a recursive pre-order walk would first reach it:
//! `Visited` is an open-addressing set of the parts already walked, so the
//! walk is linear in the references it follows, and each probe of it counts
//! in the carrier's `capture_transfer_probes` so a test can bound that work.
//! Map keys are not walked: typing admits no capability in a key.

use core::ffi::c_void;
use core::ptr;

use alloc::vec::Vec;

use super::layout::{Callable, Environment, MapNode, Value, ValueKind};
use super::list;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// Transfers or records one Receiver or Completion, or counts one
    /// managed process or supervisor handle, for the walk `transfer`
    /// (`CapabilityTransfer`). False when the transfer failed or the task
    /// was cancelled meanwhile; the walk then stops.
    fn chimi_runtime_transfer_capability_v1(transfer: *mut c_void, value: *mut Value) -> bool;
    /// The number of requirement slots of an environment, written by the
    /// compiler into each program.
    static CHIMI_REQUIREMENT_SLOT_COUNT: u32;
}

/// The kind of a part on the walk's stack.
#[derive(Clone, Copy)]
enum Part {
    Value,
    Environment,
    Callable,
    MapNode,
}

/// The parts already walked: open addressing with linear probing over a
/// power-of-two table at most half full.
struct Visited {
    slots: Vec<*const c_void>,
    count: usize,
}

/// Fibonacci hashing: the top bits of the product, for a power-of-two
/// `capacity`.
fn slot_of(pointer: *const c_void, capacity: usize) -> usize {
    let hash = (pointer as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash >> (64 - capacity.trailing_zeros())) as usize
}

/// A zeroed table of `capacity` slots, or the out-of-memory exit.
fn empty_table(capacity: usize) -> Vec<*const c_void> {
    let mut slots = Vec::new();
    if slots.try_reserve_exact(capacity).is_err() {
        // SAFETY: a function of runtime.c that does not return.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    slots.resize(capacity, ptr::null());
    slots
}

impl Visited {
    /// Adds `pointer`; false when it was already there. Each probe counts
    /// in `probes`.
    fn insert(&mut self, pointer: *const c_void, probes: &mut u64) -> bool {
        if 2 * (self.count + 1) > self.slots.len() {
            let capacity = if self.slots.is_empty() { 64 } else { self.slots.len() * 2 };
            let mut slots = empty_table(capacity);
            for &entry in self.slots.iter().filter(|entry| !entry.is_null()) {
                let mut slot = slot_of(entry, capacity);
                while !slots[slot].is_null() {
                    slot = (slot + 1) & (capacity - 1);
                }
                slots[slot] = entry;
            }
            self.slots = slots;
        }
        let mask = self.slots.len() - 1;
        let mut slot = slot_of(pointer, self.slots.len());
        loop {
            *probes = probes.wrapping_add(1);
            if self.slots[slot] == pointer {
                return false;
            }
            if self.slots[slot].is_null() {
                break;
            }
            slot = (slot + 1) & mask;
        }
        self.slots[slot] = pointer;
        self.count += 1;
        true
    }
}

/// The walk's stack of parts still to visit. Parts pop last-pushed first,
/// so a container pushes its parts last to first and the walk visits them
/// in order.
struct Pending(Vec<(Part, *mut c_void)>);

impl Pending {
    fn push<T>(&mut self, part: Part, pointer: *mut T) {
        if pointer.is_null() {
            return;
        }
        if self.0.len() == self.0.capacity() {
            let more = if self.0.capacity() == 0 { 64 } else { self.0.capacity() };
            if self.0.try_reserve_exact(more).is_err() {
                // SAFETY: a function of runtime.c that does not return.
                unsafe { chimi_runtime_exhausted_memory() }
            }
        }
        self.0.push((part, pointer.cast()));
    }
}

/// Queues the parts of `value`, or hands it to `runtime.c` when it is a
/// capability; false when that hand-off fails.
///
/// # Safety
/// `value` is a live value; `transfer` is the caller's `CapabilityTransfer`.
unsafe fn walk_value(transfer: *mut c_void, value: *mut Value, pending: &mut Pending) -> bool {
    // SAFETY: the caller's contract; each payload is read as the kind the
    // value names.
    unsafe {
        let payload = &raw const (*value).payload;
        match (*value).kind {
            ValueKind::CHANNEL_RECEIVER
            | ValueKind::CHANNEL_COMPLETION
            | ValueKind::MANAGED_PROCESS
            | ValueKind::MANAGED_SUPERVISOR => chimi_runtime_transfer_capability_v1(transfer, value),
            ValueKind::LIST => {
                for index in (0..(*payload).list.length).rev() {
                    pending.push(Part::Value, list::get(value, index));
                }
                true
            }
            ValueKind::RECORD | ValueKind::CAUSE | ValueKind::TUPLE => {
                let record = (*payload).record;
                for index in (0..record.length as usize).rev() {
                    pending.push(Part::Value, (*record.fields.add(index)).value);
                }
                true
            }
            ValueKind::CONSTRUCTED => {
                let constructed = (*payload).constructed;
                for index in (0..constructed.length as usize).rev() {
                    pending.push(Part::Value, (*constructed.fields.add(index)).value);
                }
                true
            }
            ValueKind::CALLABLE => {
                pending.push(Part::Callable, (*payload).callable);
                true
            }
            ValueKind::MAP | ValueKind::SET => {
                pending.push(Part::MapNode, (*payload).map);
                true
            }
            ValueKind::VOID
            | ValueKind::INTEGER
            | ValueKind::FLOAT
            | ValueKind::TEXT
            | ValueKind::BYTES
            | ValueKind::BOOLEAN
            | ValueKind::RANGE
            | ValueKind::DURATION
            | ValueKind::INSTANT
            | ValueKind::TIMESTAMP
            | ValueKind::DECIMAL
            | ValueKind::JSON_NUMBER
            | ValueKind::TASK
            | ValueKind::CHANNEL_SENDER
            | ValueKind::PROCESS_ADDRESS
            | ValueKind::PROCESS_INBOX
            | ValueKind::SUPERVISOR_RUNNING
            | ValueKind::PROCESS_MONITOR
            | ValueKind::TEST_RECORDER
            | ValueKind::TEST_SCRIPT
            | ValueKind::TEST_MAILBOX
            | ValueKind::NATIVE_HANDLE => true,
            _ => false,
        }
    }
}

/// Queues the parts of an environment, a callable or a map node.
///
/// # Safety
/// `pointer` is a live part of the kind `part` names.
unsafe fn walk_part(part: Part, pointer: *mut c_void, pending: &mut Pending) {
    // SAFETY: the caller's contract.
    unsafe {
        match part {
            Part::Value => {}
            Part::Environment => {
                let environment = pointer.cast::<Environment>();
                let requirements = (*environment).requirements;
                if !requirements.is_null() {
                    for slot in (0..CHIMI_REQUIREMENT_SLOT_COUNT as usize).rev() {
                        pending.push(Part::Value, *requirements.add(slot));
                    }
                }
                let locals = (*environment).locals;
                if !locals.is_null() {
                    for slot in (0..(*environment).local_count as usize).rev() {
                        pending.push(Part::Value, *locals.add(slot));
                    }
                }
            }
            Part::Callable => {
                let callable = pointer.cast::<Callable>();
                pending.push(Part::Callable, (*callable).inner);
                pending.push(Part::Environment, (*callable).environment);
            }
            Part::MapNode => {
                let node = pointer.cast::<MapNode>();
                pending.push(Part::MapNode, (*node).right);
                pending.push(Part::MapNode, (*node).left);
                pending.push(Part::Value, (*node).value);
            }
        }
    }
}

/// Hands every capability reachable from `root` to `runtime.c` for
/// `transfer`, stopping at the first hand-off that fails. False when one
/// failed. Adds the visited set's probes to `probes`, which the caller
/// folds into the carrier it ends on: a hand-off can switch fibers, so the
/// carrier the walk started on may not be the one it ends on.
///
/// # Safety
/// `root` is null or a live value whose graph the current task may read;
/// `transfer` is the caller's `CapabilityTransfer`.
pub unsafe fn captures(transfer: *mut c_void, root: *mut Value, probes: &mut u64) -> bool {
    let mut visited = Visited { slots: Vec::new(), count: 0 };
    let mut pending = Pending(Vec::new());
    pending.push(Part::Value, root);
    let mut transferred = true;
    while transferred {
        let Some((part, pointer)) = pending.0.pop() else { break };
        if !visited.insert(pointer, probes) {
            continue;
        }
        // SAFETY: the caller's contract; each part was pushed with its kind.
        unsafe {
            match part {
                Part::Value => transferred = walk_value(transfer, pointer.cast(), &mut pending),
                _ => walk_part(part, pointer, &mut pending),
            }
        }
    }
    transferred
}
