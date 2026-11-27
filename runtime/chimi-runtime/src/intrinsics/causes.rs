//! Failure causes (time-and-outcomes.md, "Failure context") and the native
//! core of std/cause; the interpreter's `execution/causes.rs` is the
//! reference, and the language tests run every cause program on both
//! backends.
//!
//! A cause is a `Cause` value, immutable once built: adding a frame or a link
//! builds a new one, so a cause shared by a stored Err and a live failure
//! never changes under either. Its unnamed fields are the [`Slot`]s; a frame
//! is a tuple of its text and its node (-1 when unknown), and `caused_by` is a
//! cause or Void. A live outcome holds its cause in `Control.cause`; null
//! means the fresh cause of the outcome itself, built only when read.
//! `Cause.context` calls a function, so it stays with `runtime.c`'s dispatch
//! and builds its frame here. Values are allocated in the order `runtime.c`
//! allocated them, so memory reports do not change.

use core::ffi::{CStr, c_char};
use core::ptr;

use super::debug::render;
use super::message::integer_spelling;
use super::standard::{shape_defect, standard_value, success, void_value};
use super::text::HeapText;
use crate::heap::layout::{
    AllocationHeader, AllocationKind, Control, ControlKind, Field, Intrinsic, StandardConstructor, Value, ValueKind,
};
use crate::heap::list;
use crate::heap::values::{allocate_kind, integer_value, new_control, new_value, new_value_slots, text_value};

unsafe extern "C" {
    /// The source location of the program's node `node` (`runtime.c`, which
    /// keeps the node tables until dispatch moves).
    fn chimi_runtime_node_location_v1(node: i64) -> *const c_char;
}

/// The cause's fields, in order.
struct Slot;

impl Slot {
    const KIND: usize = 0;
    const PAYLOAD: usize = 1;
    const ORIGIN: usize = 2;
    const FRAMES: usize = 3;
    const DROPPED_FRAMES: usize = 4;
    const CAUSED_BY: usize = 5;
    const LINKS: usize = 6;
    const DROPPED_LINKS: usize = 7;
    const SUPPRESSED: usize = 8;
    const DROPPED_SUPPRESSED: usize = 9;
    const REASON: usize = 10;
    const COUNT: usize = 11;
}

const EXPECTED_FAILURE: i64 = 0;
const DEFECT: i64 = 1;
const CANCELLATION: i64 = 2;
const FRAME_LIMIT: u32 = 32;
const SUPPRESSED_LIMIT: u32 = 16;
const LINK_LIMIT: i64 = 64;
/// A suppressed cause's reason: a sibling's failure (`Task.all` and friends).
const REASON_SIBLING: i64 = 1;
/// As `runtime.c` writes it in a cancellation's outcome record.
const CANCELLATION_REASON: &CStr = c"requested before the next semantic operation";

/// The name of the hidden field after a stored Err's own fields that holds
/// its cause, recognised by its address. Equality, order, rendering and
/// patterns read only `length` fields.
static STORED_CAUSE_FIELD: [u8; 6] = *b"cause\0";

fn stored_cause_name() -> *const c_char {
    STORED_CAUSE_FIELD.as_ptr().cast()
}

/// # Safety
/// `cause` is a live Cause.
unsafe fn field(cause: *const Value, slot: usize) -> *mut Value {
    // SAFETY: the caller's contract; a cause has every slot.
    unsafe { (*(*cause).payload.record.fields.add(slot)).value }
}

/// # Safety
/// `cause` is a live Cause; the slot holds an Int.
unsafe fn integer(cause: *const Value, slot: usize) -> i64 {
    // SAFETY: the caller's contract.
    unsafe { (*field(cause, slot)).payload.integer }
}

/// # Safety
/// `cause` is a Cause this code just built, not yet shared.
unsafe fn set(cause: *mut Value, slot: usize, value: *mut Value) {
    // SAFETY: the caller's contract.
    unsafe { (*(*cause).payload.record.fields.add(slot)).value = value };
}

/// # Safety
/// Runs on a fiber that may allocate.
#[inline(never)]
unsafe fn int(raw: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::INTEGER, raw) }
}

/// # Safety
/// Runs on a fiber that may allocate; `values` holds `length` values.
#[inline(never)]
unsafe fn list_of(values: *const *mut Value, length: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { list::list_value(list::from_values(values, length), 0, length) }
}

/// A fresh cause of `kind`: no frames, links or suppressed causes.
///
/// # Safety
/// Runs on a fiber that may allocate; `payload` is live or null.
#[inline(never)]
unsafe fn cause_new(kind: i64, payload: *mut Value, origin: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let cause = new_value(ValueKind::CAUSE);
        (*cause).payload.record.length = Slot::COUNT as u32;
        let fields = allocate_kind::<Field>(Slot::COUNT, AllocationKind::FIELDS);
        (*cause).payload.record.fields = fields;
        let zero = int(0);
        let empty = list_of(ptr::null(), 0);
        let kind = int(kind);
        let origin = int(origin);
        let reason = int(-1);
        let values = [kind, payload, origin, empty, zero, void_value(), zero, zero, empty, zero, reason];
        for (slot, value) in values.into_iter().enumerate() {
            *fields.add(slot) = Field { name: ptr::null(), value };
        }
        cause
    }
}

/// A new cause with `cause`'s slots, to change before it is shared.
///
/// # Safety
/// Runs on a fiber that may allocate; `cause` is a live Cause.
#[inline(never)]
unsafe fn cause_copy(cause: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let copy = cause_new(0, ptr::null_mut(), 0);
        for slot in 0..Slot::COUNT {
            set(copy, slot, field(cause, slot));
        }
        copy
    }
}

/// The outcome's cause, built from the outcome while it is still implicit;
/// null for a success or a break.
///
/// # Safety
/// Runs on a fiber that may allocate; `control` is live.
pub unsafe fn control_cause(control: *const Control) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        if !(*control).cause.is_null() {
            return (*control).cause;
        }
        match (*control).kind {
            ControlKind::FAILURE => cause_new(EXPECTED_FAILURE, (*control).value, (*control).node),
            ControlKind::DEFECT => {
                let message = CStr::from_ptr((*control).message).to_bytes();
                cause_new(DEFECT, text_value(message.as_ptr().cast(), message.len()), (*control).node)
            }
            ControlKind::CANCEL => {
                let reason = CANCELLATION_REASON.to_bytes();
                cause_new(CANCELLATION, text_value(reason.as_ptr().cast(), reason.len()), -1)
            }
            _ => ptr::null_mut(),
        }
    }
}

/// A copy of the outcome with `cause` as its cause; a failure takes the
/// cause's origin as its node.
///
/// # Safety
/// Runs on a fiber that may allocate; `control` is live and `cause` a live
/// Cause.
pub unsafe fn control_with_cause(control: *const Control, cause: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let copy = new_control((*control).kind, (*control).value, (*control).message, (*control).node);
        (*copy).cause = cause;
        if (*control).kind == ControlKind::FAILURE {
            (*copy).node = integer(cause, Slot::ORIGIN);
        }
        copy
    }
}

/// `items` with `item` pushed, and whether one was dropped: at `limit` it
/// first drops the entry just past the kept oldest half.
///
/// # Safety
/// Runs on a fiber that may allocate; `items` is a live List.
#[inline(never)]
unsafe fn push_bounded(items: *const Value, item: *mut Value, limit: u32) -> (*mut Value, bool) {
    // SAFETY: the caller's contract.
    unsafe {
        let length = (*items).payload.list.length;
        if length != limit {
            return (list::list_push(items, item), false);
        }
        let kept = new_value_slots(limit);
        let mut count = 0;
        for index in (0..length).filter(|&index| index != limit / 2) {
            *kept.add(count) = list::get(items, index);
            count += 1;
        }
        *kept.add(count) = item;
        (list_of(kept, count as u32 + 1), true)
    }
}

/// `cause` with the context frame `text` at `node` added.
///
/// # Safety
/// Runs on a fiber that may allocate; `cause` is a live Cause and `text` a
/// live Text.
pub unsafe fn cause_with_frame(cause: *const Value, text: *mut Value, node: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let frame = new_value(ValueKind::TUPLE);
        (*frame).payload.record.length = 2;
        let fields = allocate_kind::<Field>(2, AllocationKind::FIELDS);
        (*frame).payload.record.fields = fields;
        *fields = Field { name: ptr::null(), value: text };
        *fields.add(1) = Field { name: ptr::null(), value: int(node) };
        let copy = cause_copy(cause);
        let (frames, dropped) = push_bounded(field(cause, Slot::FRAMES), frame, FRAME_LIMIT);
        set(copy, Slot::FRAMES, frames);
        if dropped {
            set(copy, Slot::DROPPED_FRAMES, int(integer(cause, Slot::DROPPED_FRAMES) + 1));
        }
        copy
    }
}

/// `cause` with `sibling` appended to its suppressed causes, marked Sibling.
///
/// # Safety
/// Runs on a fiber that may allocate; both are live Causes.
pub unsafe fn cause_with_sibling(cause: *const Value, sibling: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let marked = cause_copy(sibling);
        set(marked, Slot::REASON, int(REASON_SIBLING));
        let copy = cause_copy(cause);
        let (suppressed, dropped) = push_bounded(field(cause, Slot::SUPPRESSED), marked, SUPPRESSED_LIMIT);
        set(copy, Slot::SUPPRESSED, suppressed);
        if dropped {
            set(copy, Slot::DROPPED_SUPPRESSED, int(integer(cause, Slot::DROPPED_SUPPRESSED) + 1));
        }
        copy
    }
}

/// The cause `cause` was caused by, or null.
///
/// # Safety
/// `cause` is a live Cause.
unsafe fn below(cause: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let below = field(cause, Slot::CAUSED_BY);
        if (*below).kind == ValueKind::CAUSE { below } else { ptr::null_mut() }
    }
}

/// # Safety
/// Runs on a fiber that may allocate; `cause` is unshared and `below` null
/// or a live Cause.
unsafe fn link(cause: *mut Value, below: *mut Value) {
    // SAFETY: the caller's contract.
    unsafe {
        set(cause, Slot::CAUSED_BY, if below.is_null() { void_value() } else { below });
        set(cause, Slot::LINKS, int(if below.is_null() { 0 } else { integer(below, Slot::LINKS) + 1 }));
    }
}

/// `cause` (fresh, unshared) linked to `below`. A chain longer than the
/// limit keeps its newest and oldest halves: the link at depth LIMIT / 2 is
/// dropped and the newer links above it are copied onto the rest of the
/// chain.
///
/// # Safety
/// Runs on a fiber that may allocate; `cause` is unshared and `below` a live
/// Cause.
#[inline(never)]
unsafe fn linked(cause: *mut Value, below_cause: *mut Value) -> *mut Value {
    const KEEP_NEWEST: usize = (LINK_LIMIT / 2) as usize;
    // SAFETY: the caller's contract.
    unsafe {
        link(cause, below_cause);
        if integer(cause, Slot::LINKS) <= LINK_LIMIT {
            return cause;
        }
        let mut newer = [ptr::null_mut::<Value>(); KEEP_NEWEST];
        let mut count = 0;
        let mut current = below_cause;
        for _ in 1..KEEP_NEWEST {
            newer[count] = current;
            count += 1;
            current = below(current);
        }
        let dropped = integer(current, Slot::DROPPED_LINKS) + 1;
        let mut resumed = below(current);
        if count == 0 {
            set(cause, Slot::DROPPED_LINKS, int(integer(cause, Slot::DROPPED_LINKS) + dropped));
        }
        for index in (0..count).rev() {
            let copy = cause_copy(newer[index]);
            if index == count - 1 {
                set(copy, Slot::DROPPED_LINKS, int(integer(copy, Slot::DROPPED_LINKS) + dropped));
            }
            link(copy, resumed);
            resumed = copy;
        }
        link(cause, resumed);
        cause
    }
}

/// The cause a `Failure(...)` or `Result.Err(...)` built in a cause arm gets
/// from `matched` (a cause, or null): the same one when it keeps it,
/// otherwise a new cause of `error` started at `node`, linked to it.
///
/// # Safety
/// Runs on a fiber that may allocate; `matched` is null or a live Cause and
/// `error` live.
pub unsafe fn cause_derived(matched: *mut Value, keep: bool, error: *mut Value, node: i64) -> *mut Value {
    if keep {
        return matched;
    }
    // SAFETY: the caller's contract.
    unsafe {
        let fresh = cause_new(EXPECTED_FAILURE, error, node);
        if matched.is_null() { fresh } else { linked(fresh, matched) }
    }
}

/// The cause a stored Err keeps in its hidden field, or null.
///
/// # Safety
/// `value` is live.
pub unsafe fn stored_cause(value: *const Value) -> *mut Value {
    // SAFETY: the caller's contract; a constructed value's fields are one
    // heap allocation, whose header says how many slots it holds.
    unsafe {
        if (*value).kind != ValueKind::CONSTRUCTED {
            return ptr::null_mut();
        }
        let constructed = (*value).payload.constructed;
        let length = constructed.length as usize;
        let header = constructed.fields.cast::<AllocationHeader>().wrapping_sub(1);
        let slots = (*header).bytes / size_of::<Field>();
        if slots <= length || (*constructed.fields.add(length)).name != stored_cause_name() {
            return ptr::null_mut();
        }
        (*constructed.fields.add(length)).value
    }
}

/// A copy of the constructed `value` whose hidden field holds `cause`.
///
/// # Safety
/// Runs on a fiber that may allocate; `value` is a live constructed value
/// and `cause` a live Cause.
pub unsafe fn with_stored_cause(value: *const Value, cause: *mut Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let length = (*value).payload.constructed.length as usize;
        let copy = new_value(ValueKind::CONSTRUCTED);
        (*copy).payload.constructed = (*value).payload.constructed;
        let fields = allocate_kind::<Field>(length + 1, AllocationKind::FIELDS);
        (*copy).payload.constructed.fields = fields;
        ptr::copy_nonoverlapping((*value).payload.constructed.fields, fields, length);
        *fields.add(length) = Field { name: stored_cause_name(), value: cause };
        copy
    }
}

/// The cause a cause arm binds for a matched subject: the live failure's,
/// or the one a stored Err keeps, or Void.
///
/// # Safety
/// Runs on a fiber that may allocate; `subject` is live.
pub unsafe fn matched_cause_value(subject: *const Control) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let cause = match (*subject).kind {
            ControlKind::FAILURE => control_cause(subject),
            ControlKind::SUCCESS => stored_cause((*subject).value),
            _ => ptr::null_mut(),
        };
        if cause.is_null() { void_value() } else { cause }
    }
}

/// `Cause.to_text`'s writer, as causes.rs `render` writes it. Bounded: at
/// most 64 links, each with at most 32 frames and 16 suppressed causes.
struct Writer(HeapText);

impl Writer {
    /// # Safety
    /// Runs on a fiber that may allocate.
    #[inline(never)]
    unsafe fn text(&mut self, bytes: &[u8]) {
        // SAFETY: the caller's contract.
        unsafe { self.0.push_bytes(bytes) };
    }

    /// # Safety
    /// As for [`Writer::text`]; `text` is a live Text.
    unsafe fn value(&mut self, text: *const Value) {
        // SAFETY: the caller's contract.
        unsafe {
            let payload = (*text).payload.text;
            self.text(core::slice::from_raw_parts(payload.bytes.cast(), payload.length));
        }
    }

    /// # Safety
    /// As for [`Writer::text`].
    #[inline(never)]
    unsafe fn location(&mut self, node: i64) {
        if node < 0 {
            return;
        }
        // SAFETY: the caller's contract; `node` is a node of the program.
        unsafe {
            self.text(b" (");
            self.text(CStr::from_ptr(chimi_runtime_node_location_v1(node)).to_bytes());
            self.text(b")");
        }
    }

    /// # Safety
    /// As for [`Writer::text`].
    #[inline(never)]
    unsafe fn indent(&mut self, depth: u32) {
        // SAFETY: the caller's contract.
        unsafe {
            self.text(b"\n");
            for _ in 0..=depth {
                self.text(b"  ");
            }
        }
    }

    /// "(N <what>s dropped)" on its own line, as one write.
    ///
    /// # Safety
    /// As for [`Writer::text`].
    unsafe fn dropped(&mut self, depth: u32, count: i64, what: &[u8]) {
        let mut line = [0u8; 96];
        let mut length = 0;
        let mut spelling = [0; 20];
        let plural: &[u8] = if count == 1 { b"" } else { b"s" };
        for part in [b"(" as &[u8], integer_spelling(count, &mut spelling), b" ", what, plural, b" dropped)"] {
            line[length..length + part.len()].copy_from_slice(part);
            length += part.len();
        }
        // SAFETY: the caller's contract.
        unsafe {
            self.indent(depth);
            self.text(&line[..length]);
        }
    }

    /// # Safety
    /// As for [`Writer::text`]; `cause` is a live Cause.
    unsafe fn cause(&mut self, cause: *const Value, depth: u32) {
        const KINDS: [&[u8]; 3] = [b"expected failure", b"defect", b"cancellation"];
        const SUPPRESSION_LABELS: [&[u8]; 3] = [b"cleanup", b"sibling", b"displaced"];
        // SAFETY: the caller's contract; frames, suppressed causes and links
        // are the shapes the builders above make.
        unsafe {
            let kind = integer(cause, Slot::KIND);
            self.text(KINDS[kind as usize]);
            self.text(b" ");
            let payload = field(cause, Slot::PAYLOAD);
            if kind == EXPECTED_FAILURE {
                let summary = render(payload, false);
                self.value(summary);
            } else {
                self.value(payload);
            }
            self.location(integer(cause, Slot::ORIGIN));
            let frames = field(cause, Slot::FRAMES);
            let dropped_frames = integer(cause, Slot::DROPPED_FRAMES);
            for index in 0..(*frames).payload.list.length {
                if index == FRAME_LIMIT / 2 && dropped_frames > 0 {
                    self.dropped(depth, dropped_frames, b"context frame");
                }
                let frame = (*list::get(frames, index)).payload.record.fields;
                self.indent(depth);
                self.text(b"context: ");
                self.value((*frame).value);
                self.location((*(*frame.add(1)).value).payload.integer);
            }
            let suppressed = field(cause, Slot::SUPPRESSED);
            let dropped_suppressed = integer(cause, Slot::DROPPED_SUPPRESSED);
            for index in 0..(*suppressed).payload.list.length {
                if index == SUPPRESSED_LIMIT / 2 && dropped_suppressed > 0 {
                    self.dropped(depth, dropped_suppressed, b"suppressed cause");
                }
                let child = list::get(suppressed, index);
                let reason = integer(child, Slot::REASON);
                self.indent(depth);
                self.text(b"suppressed (");
                let label = usize::try_from(reason).ok().and_then(|reason| SUPPRESSION_LABELS.get(reason));
                self.text(label.map_or(b"unknown", |label| label));
                self.text(b"): ");
                self.cause(child, depth + 1);
            }
            let below = below(cause);
            if !below.is_null() {
                let dropped_links = integer(cause, Slot::DROPPED_LINKS);
                if dropped_links > 0 {
                    self.dropped(depth, dropped_links, b"cause");
                }
                self.indent(depth);
                self.text(b"caused by: ");
                self.cause(below, depth + 1);
            }
        }
    }
}

/// `Cause.to_text` of `cause`, as a Text.
///
/// # Safety
/// Runs on a fiber that may allocate; `cause` is a live Cause.
pub unsafe fn cause_text(cause: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let mut writer = Writer(HeapText::new());
        // The first block before any byte, as the C's `buffer_reserve(0)`.
        writer.0.push_bytes(b"");
        writer.cause(cause, 0);
        text_value(writer.0.data().cast(), writer.0.len())
    }
}

/// Whether a root outcome's record shows its cause: only when the cause
/// says more than the record already does.
///
/// # Safety
/// `cause` is a live Cause.
pub unsafe fn cause_has_detail(cause: *const Value) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        (*field(cause, Slot::FRAMES)).payload.list.length > 0
            || !below(cause).is_null()
            || (*field(cause, Slot::SUPPRESSED)).payload.list.length > 0
    }
}

/// `Some(cause)`, or `None` for null.
///
/// # Safety
/// Runs on a fiber that may allocate.
unsafe fn cause_option(cause: *mut Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        if cause.is_null() {
            standard_value(StandardConstructor::OPTION_NONE, &[])
        } else {
            standard_value(StandardConstructor::OPTION_SOME, &[cause])
        }
    }
}

/// The std/cause intrinsics but `Cause.context`, which `runtime.c` keeps
/// because it calls a function.
///
/// # Safety
/// Runs on a fiber that may allocate; `arguments` are live.
pub unsafe fn cause_operation(intrinsic: u32, arguments: &[*mut Value], call_node: u32) -> *mut Control {
    // SAFETY: the caller's contract; payloads are read as their kinds say.
    unsafe {
        if intrinsic == Intrinsic::CAUSE_OF {
            if arguments.len() != 1 || (*arguments[0]).kind != ValueKind::CONSTRUCTED {
                return shape_defect(call_node);
            }
            return success(cause_option(stored_cause(arguments[0])));
        }
        if arguments.len() != 1 || (*arguments[0]).kind != ValueKind::CAUSE {
            return shape_defect(call_node);
        }
        let cause = arguments[0];
        match intrinsic {
            Intrinsic::CAUSE_KIND_CODE => success(field(cause, Slot::KIND)),
            Intrinsic::CAUSE_SUMMARY => {
                let payload = field(cause, Slot::PAYLOAD);
                success(if integer(cause, Slot::KIND) == EXPECTED_FAILURE { render(payload, false) } else { payload })
            }
            Intrinsic::CAUSE_FRAMES => {
                let frames = field(cause, Slot::FRAMES);
                let length = (*frames).payload.list.length;
                let texts = new_value_slots(length);
                for index in 0..length {
                    *texts.add(index as usize) = (*(*list::get(frames, index)).payload.record.fields).value;
                }
                success(list_of(texts, length))
            }
            Intrinsic::CAUSE_CAUSED_BY => success(cause_option(below(cause))),
            Intrinsic::CAUSE_SUPPRESSED => success(field(cause, Slot::SUPPRESSED)),
            Intrinsic::CAUSE_REASON_CODE => success(field(cause, Slot::REASON)),
            Intrinsic::CAUSE_TO_TEXT => success(cause_text(cause)),
            _ => shape_defect(call_node),
        }
    }
}
