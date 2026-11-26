//! Structural order (CHI-305 step 5, phase 4, third PR): the one comparison
//! behind `==`, `<`, `Order.compare` and Map keys. For ordered types it is
//! the source-level order; for types that are only equatable (records,
//! maps, `Json.Value`) it is an internal total order consistent with
//! equality. It mirrors the interpreter's `execution/structural.rs`.
//!
//! The walk keeps its own work stack, so nesting depth never costs host
//! stack: the first [`INLINE_STEPS`] steps live in the caller's frame, which
//! covers comparing scalars and small values without a heap allocation, and
//! deeper walks move to the heap.

use core::cmp::Ordering;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::MaybeUninit;
use core::ptr;

use alloc::vec::Vec;

use super::layout::{Field, MapNode, Value, ValueKind};
use super::list;
use super::map;
use crate::decimal_abi::{ChimiDecimal, chimi_decimal_compare_v1};

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
    /// The declaration-order ordinal of the constructor named by the
    /// program symbol `symbol`.
    fn chimi_runtime_constructor_ordinal_v1(symbol: u32) -> u32;
}

const INLINE_STEPS: usize = 32;

/// One pending comparison: two values, or, with both null, a decision that
/// stands when every step above it was equal (such as the length rule after
/// the shared prefix of two lists).
#[derive(Clone, Copy)]
struct Step {
    left: *const Value,
    right: *const Value,
    decided: i32,
}

/// The walk's stack: inline until it outgrows [`INLINE_STEPS`], then all
/// in `spilled`. The inline steps are left uninitialized, as each compare
/// of two scalars would otherwise clear them.
struct Stack<'a> {
    inline: &'a mut MaybeUninit<[Step; INLINE_STEPS]>,
    length: usize,
    spilled: Vec<Step>,
}

impl Stack<'_> {
    #[inline(always)]
    fn push(&mut self, left: *const Value, right: *const Value, decided: i32) {
        let step = Step { left, right, decided };
        if self.length < INLINE_STEPS && self.spilled.capacity() == 0 {
            // SAFETY: `length` is within the inline steps.
            unsafe { self.inline.as_mut_ptr().cast::<Step>().add(self.length).write(step) };
            self.length += 1;
        } else {
            self.spill(step);
        }
    }

    #[cold]
    #[inline(never)]
    fn spill(&mut self, step: Step) {
        if self.spilled.capacity() == 0 {
            if self.spilled.try_reserve(INLINE_STEPS * 2).is_err() {
                // SAFETY: a function of runtime.c that does not return.
                unsafe { chimi_runtime_exhausted_memory() }
            }
            // SAFETY: the first `length` inline steps were written.
            let written = unsafe { core::slice::from_raw_parts(self.inline.as_ptr().cast::<Step>(), self.length) };
            self.spilled.extend_from_slice(written);
        } else if self.spilled.len() == self.spilled.capacity() && self.spilled.try_reserve(self.spilled.len()).is_err() {
            // SAFETY: as above.
            unsafe { chimi_runtime_exhausted_memory() }
        }
        self.spilled.push(step);
        self.length += 1;
    }

    fn decide(&mut self, decided: i32) {
        self.push(ptr::null(), ptr::null(), decided);
    }

    #[inline(always)]
    fn pop(&mut self) -> Option<Step> {
        if self.length == 0 {
            return None;
        }
        self.length -= 1;
        if self.spilled.capacity() == 0 {
            // SAFETY: steps below `length + 1` were written.
            Some(unsafe { self.inline.as_ptr().cast::<Step>().add(self.length).read() })
        } else {
            self.spilled.pop()
        }
    }
}

fn order_of<T: Ord>(left: T, right: T) -> i32 {
    match left.cmp(&right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// The bytes of a C string.
///
/// # Safety
/// `text` is a C string.
unsafe fn c_bytes<'a>(text: *const c_char) -> &'a [u8] {
    // SAFETY: the caller's contract.
    unsafe { CStr::from_ptr(text).to_bytes() }
}

/// `strcmp` of two fields' names, for `qsort`.
///
/// # Safety
/// Both point to field pointers whose names are C strings.
unsafe extern "C" fn field_name_order(left: *const c_void, right: *const c_void) -> c_int {
    // SAFETY: the caller's contract.
    unsafe { libc::strcmp((**left.cast::<*const Field>()).name, (**right.cast::<*const Field>()).name) }
}

/// The first `length` fields of `fields` in name order: records have no
/// declaration order, so name order is a fixed order both sides share.
/// libc's `qsort` sorts them, as a Rust sort would add 10 KiB to every
/// program.
///
/// # Safety
/// `fields` holds `length` fields with C-string names.
unsafe fn fields_by_name(fields: *const Field, length: u32) -> Vec<*const Field> {
    let mut sorted = Vec::new();
    if sorted.try_reserve_exact(length as usize).is_err() {
        // SAFETY: a function of runtime.c that does not return.
        unsafe { chimi_runtime_exhausted_memory() }
    }
    // SAFETY: the caller's contract; `sorted` holds `length` pointers.
    unsafe {
        sorted.extend((0..length as usize).map(|index| fields.add(index)));
        libc::qsort(sorted.as_mut_ptr().cast(), sorted.len(), size_of::<*const Field>(), Some(field_name_order));
    }
    sorted
}

/// -1, 0 or 1 as `left` orders before, with or after `right`.
///
/// # Safety
/// Both values are live and of the same equatable type.
pub unsafe fn compare(left_root: *const Value, right_root: *const Value) -> i32 {
    // A local of its own: in one struct with the length, the optimizer
    // merges the length's first store with the undefined steps into a
    // memset of all of them.
    let mut inline = MaybeUninit::uninit();
    let mut stack = Stack { inline: &mut inline, length: 0, spilled: Vec::new() };
    // The roots stay out of the stack, so nothing is written to it before
    // the walk needs it (an initial push lets the optimizer clear it all).
    let mut roots = Some(Step { left: left_root, right: right_root, decided: 0 });
    let mut order = 0;
    while order == 0 {
        let Some(step) = roots.take().or_else(|| stack.pop()) else { break };
        if step.left.is_null() {
            order = step.decided;
            continue;
        }
        let (left, right) = (step.left, step.right);
        if left == right {
            continue;
        }
        // SAFETY: the caller's contract; each payload is read as the kind
        // both values have.
        unsafe {
            if (*left).kind != (*right).kind {
                order = order_of((*left).kind.0, (*right).kind.0);
                continue;
            }
            let (l, r) = (&(*left).payload, &(*right).payload);
            match (*left).kind {
                ValueKind::VOID => {}
                ValueKind::INTEGER | ValueKind::DURATION | ValueKind::INSTANT | ValueKind::TIMESTAMP => {
                    order = order_of(l.integer, r.integer);
                }
                ValueKind::DECIMAL => {
                    let decimal = |d: super::layout::Decimal| ChimiDecimal {
                        high: d.high,
                        low: d.low,
                        scale: d.scale,
                        negative: d.negative,
                    };
                    order = chimi_decimal_compare_v1(decimal(l.decimal), decimal(r.decimal));
                }
                // Finite, so total; -0.0 and 0.0 are equal.
                ValueKind::FLOAT => {
                    order = i32::from(l.floating > r.floating) - i32::from(l.floating < r.floating);
                }
                ValueKind::BOOLEAN => order = order_of(l.boolean, r.boolean),
                // Lexicographic by unsigned byte.
                ValueKind::TEXT | ValueKind::BYTES => {
                    let bytes = |text: super::layout::TextPayload| {
                        if text.length == 0 {
                            &[][..]
                        } else {
                            core::slice::from_raw_parts(text.bytes.cast::<u8>(), text.length)
                        }
                    };
                    order = order_of(bytes(l.text), bytes(r.text));
                }
                ValueKind::JSON_NUMBER => {
                    order = super::json_number::compare(c_bytes(l.json_number), c_bytes(r.json_number));
                }
                ValueKind::LIST => {
                    let (left_length, right_length) = (l.list.length, r.list.length);
                    stack.decide(order_of(left_length, right_length));
                    for index in (0..left_length.min(right_length)).rev() {
                        stack.push(list::get(left, index), list::get(right, index), 0);
                    }
                }
                ValueKind::RECORD => {
                    let length = l.record.length.min(r.record.length);
                    let left_fields = fields_by_name(l.record.fields, length);
                    let right_fields = fields_by_name(r.record.fields, length);
                    for index in (0..length as usize).rev() {
                        stack.push((*left_fields[index]).value, (*right_fields[index]).value, 0);
                    }
                }
                // Normalized bounds, so 0..3 == 0..=2; empty ranges keep
                // their bounds.
                ValueKind::RANGE => {
                    order = order_of(l.range.start, r.range.start);
                    if order == 0 {
                        order = order_of(l.range.end, r.range.end);
                    }
                }
                // Typing gives both tuples one arity; elements compare left
                // to right.
                ValueKind::TUPLE => {
                    for index in (0..l.record.length as usize).rev() {
                        stack.push((*l.record.fields.add(index)).value, (*r.record.fields.add(index)).value, 0);
                    }
                }
                ValueKind::CONSTRUCTED => {
                    order = order_of(
                        chimi_runtime_constructor_ordinal_v1(l.constructed.constructor),
                        chimi_runtime_constructor_ordinal_v1(r.constructed.constructor),
                    );
                    if order == 0 {
                        for index in (0..l.constructed.length as usize).rev() {
                            stack.push(
                                (*l.constructed.fields.add(index)).value,
                                (*r.constructed.fields.add(index)).value,
                                0,
                            );
                        }
                    }
                }
                // A set's values are all Void, so the walk compares its items.
                ValueKind::MAP | ValueKind::SET => {
                    let left_entries = map::entries(l.map);
                    let right_entries = map::entries(r.map);
                    stack.decide(order_of(left_entries.len(), right_entries.len()));
                    for index in (0..left_entries.len().min(right_entries.len())).rev() {
                        let (left_node, right_node): (*const MapNode, *const MapNode) =
                            (left_entries[index], right_entries[index]);
                        stack.push((*left_node).value, (*right_node).value, 0);
                        stack.push((*left_node).key, (*right_node).key, 0);
                    }
                }
                // Typing admits no comparison of these; identity keeps the
                // order total.
                ValueKind::CALLABLE
                | ValueKind::TASK
                | ValueKind::CHANNEL_SENDER
                | ValueKind::CHANNEL_RECEIVER
                | ValueKind::CHANNEL_COMPLETION
                | ValueKind::PROCESS_INBOX
                | ValueKind::SUPERVISOR_RUNNING
                | ValueKind::MANAGED_PROCESS
                | ValueKind::MANAGED_SUPERVISOR
                | ValueKind::CAUSE => order = order_of(left as usize as i64, right as usize as i64),
                // Routing kind, then the name or incarnation it routes to.
                ValueKind::PROCESS_MONITOR
                | ValueKind::TEST_RECORDER
                | ValueKind::TEST_SCRIPT
                | ValueKind::TEST_MAILBOX => order = order_of(l.address.target as i64, r.address.target as i64),
                ValueKind::NATIVE_HANDLE => {
                    order = order_of(l.handle.slot, r.handle.slot);
                    if order == 0 {
                        order = order_of(l.handle.generation, r.handle.generation);
                    }
                }
                ValueKind::PROCESS_ADDRESS => {
                    order = order_of(l.address.kind, r.address.kind);
                    if order == 0 {
                        order = order_of(l.address.target as i64, r.address.target as i64);
                    }
                }
                _ => {}
            }
        }
    }
    order
}
