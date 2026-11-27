//! The value cells (CHI-305 step 5, phase 4, first PR): a value, a control,
//! a table of value pointers, an environment, and the scalar and text
//! values, each allocated in the running fiber's area with its allocation
//! kind set, as `runtime.c`'s builders did. The list vector and the map
//! follow in the next PRs of the phase.
//!
//! Every builder allocates, so each may run the young path's refill but
//! never a collection: values stay where they are until a pass.

use core::ffi::{c_char, c_void};
use core::ptr;

use super::allocator::allocate;
use super::fragment::set_kind;
use super::layout::{
    AllocationKind, Callable, Control, ControlKind, Decimal, Environment, MapNode, Value, ValueKind,
};

unsafe extern "C" {
    /// The number of requirement slots of an environment, written by the
    /// compiler into each program.
    static CHIMI_REQUIREMENT_SLOT_COUNT: u32;
}

/// `count` items of `T`, of allocation kind `kind`.
///
/// # Safety
/// The running carrier's fiber has a live heap.
#[inline(always)]
pub(crate) unsafe fn allocate_kind<T>(count: usize, kind: AllocationKind) -> *mut T {
    // SAFETY: the caller's contract.
    unsafe {
        let payload = allocate(count, size_of::<T>());
        set_kind(payload.cast::<c_void>(), kind);
        payload.cast()
    }
}

/// A zeroed value of `kind`; the caller writes its payload.
///
/// # Safety
/// The running carrier's fiber has a live heap.
#[inline(always)]
pub unsafe fn new_value(kind: ValueKind) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = allocate_kind::<Value>(1, AllocationKind::VALUE);
        (*value).kind = kind;
        value
    }
}

/// A control of `kind` with no cause.
///
/// # Safety
/// The running carrier's fiber has a live heap; `message` is null or
/// outlives the control.
pub unsafe fn new_control(kind: ControlKind, value: *mut Value, message: *const c_char, node: i64) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let control = allocate_kind::<Control>(1, AllocationKind::CONTROL);
        (*control).kind = kind;
        (*control).value = value;
        (*control).message = message;
        (*control).node = node;
        (*control).cause = ptr::null_mut();
        control
    }
}

/// A table of `count` null value pointers, or null for none.
///
/// # Safety
/// The running carrier's fiber has a live heap.
#[inline(always)]
pub unsafe fn new_value_slots(count: u32) -> *mut *mut Value {
    if count == 0 {
        return ptr::null_mut();
    }
    // SAFETY: the caller's contract.
    unsafe { allocate_kind::<*mut Value>(count as usize, AllocationKind::VALUE_POINTERS) }
}

/// A copy of the `count` value pointers at `source`.
///
/// # Safety
/// The running carrier's fiber has a live heap; `source` holds `count`
/// pointers when `count` is not 0.
#[inline(always)]
pub unsafe fn clone_value_slots(source: *const *mut Value, count: u32) -> *mut *mut Value {
    // SAFETY: the caller's contract; the new table holds `count` pointers.
    unsafe {
        let slots = new_value_slots(count);
        if count != 0 {
            ptr::copy_nonoverlapping(source, slots, count as usize);
        }
        slots
    }
}

/// The requirement slot count of the running program.
pub(crate) fn requirement_slots() -> u32 {
    // SAFETY: a constant the program defines.
    unsafe { CHIMI_REQUIREMENT_SLOT_COUNT }
}

/// An environment of `kind` with `local_count` locals and the given tables.
///
/// # Safety
/// The running carrier's fiber has a live heap.
#[inline(always)]
pub(crate) unsafe fn environment(
    kind: AllocationKind,
    local_count: u32,
    locals: *mut *mut Value,
    requirements: *mut *mut Value,
) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe {
        let environment = allocate_kind::<Environment>(1, kind);
        (*environment).local_count = local_count;
        (*environment).locals = locals;
        (*environment).requirements = requirements;
        environment
    }
}

/// A call frame's environment: `local_count` empty locals and empty
/// requirements. The environment is allocated before its tables.
///
/// # Safety
/// The running carrier's fiber has a live heap.
pub unsafe fn new_environment(local_count: u32) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe {
        let frame = environment(AllocationKind::FRAME_ENVIRONMENT, local_count, ptr::null_mut(), ptr::null_mut());
        (*frame).locals = new_value_slots(local_count);
        (*frame).requirements = new_value_slots(requirement_slots());
        frame
    }
}

/// A frame copy of `source` that copies the tables `locals` and
/// `requirements` say to and shares the others. Locals and requirements
/// are each immutable outside the boundaries that bind them, so a boundary
/// copies only the table it changes; precise marking keeps a shared table
/// alive through the environment pointer.
///
/// # Safety
/// The running carrier's fiber has a live heap; `source` is a live
/// environment.
pub unsafe fn clone_environment(source: *const Environment, locals: bool, requirements: bool) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe {
        let count = (*source).local_count;
        let frame = environment(AllocationKind::FRAME_ENVIRONMENT, count, ptr::null_mut(), ptr::null_mut());
        (*frame).locals = if locals { clone_value_slots((*source).locals, count) } else { (*source).locals };
        (*frame).requirements = if requirements {
            clone_value_slots((*source).requirements, requirement_slots())
        } else {
            (*source).requirements
        };
        frame
    }
}

/// A closure's environment holding only a copy of `source`'s requirements.
///
/// # Safety
/// As for [`clone_environment`].
pub unsafe fn capture_requirements(source: *const Environment) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe {
        let captured = environment(AllocationKind::ENVIRONMENT, 0, ptr::null_mut(), ptr::null_mut());
        (*captured).requirements = clone_value_slots((*source).requirements, requirement_slots());
        captured
    }
}

/// A value of `kind` whose payload is the 64-bit `raw`: an Int, Duration,
/// Instant or Timestamp.
///
/// # Safety
/// The running carrier's fiber has a live heap.
pub unsafe fn integer_value(kind: ValueKind, raw: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(kind);
        (*value).payload.integer = raw;
        value
    }
}

/// A Float.
///
/// # Safety
/// The running carrier's fiber has a live heap.
pub unsafe fn float_value(raw: f64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::FLOAT);
        (*value).payload.floating = raw;
        value
    }
}

/// A Range.
///
/// # Safety
/// The running carrier's fiber has a live heap.
pub unsafe fn range_value(start: i64, end: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::RANGE);
        (*value).payload.range.start = start;
        (*value).payload.range.end = end;
        value
    }
}

/// A Decimal.
///
/// # Safety
/// The running carrier's fiber has a live heap.
pub unsafe fn decimal_value(decimal: Decimal) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(ValueKind::DECIMAL);
        (*value).payload.decimal = decimal;
        value
    }
}

/// A value of `kind` whose payload is the pointer `raw`: a callable for
/// CALLABLE, a map node for MAP and SET.
///
/// # Safety
/// The running carrier's fiber has a live heap; `raw` is null or a live
/// part of the kind `kind` holds.
pub unsafe fn pointer_value(kind: ValueKind, raw: *mut c_void) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = new_value(kind);
        match kind {
            ValueKind::CALLABLE => (*value).payload.callable = raw.cast::<Callable>(),
            _ => (*value).payload.map = raw.cast::<MapNode>(),
        }
        value
    }
}

/// A NUL-terminated copy of the `length` bytes at `bytes` (which may be
/// null when `length` is 0), as BYTES.
///
/// # Safety
/// The running carrier's fiber has a live heap; `bytes` holds `length`
/// bytes.
pub unsafe fn copy_bytes(bytes: *const c_char, length: usize) -> *mut c_char {
    // SAFETY: the caller's contract; the copy has room for the terminator.
    unsafe {
        let copy = allocate(length + 1, 1).cast::<c_char>();
        if length != 0 {
            ptr::copy_nonoverlapping(bytes, copy, length);
        }
        *copy.add(length) = 0;
        copy
    }
}

/// A Text of the `length` bytes at `bytes`.
///
/// # Safety
/// As for [`copy_bytes`].
pub unsafe fn text_value(bytes: *const c_char, length: usize) -> *mut Value {
    // SAFETY: the caller's contract. The bytes are allocated before the
    // value, as the C did, so memory reports do not change.
    unsafe {
        let raw = copy_bytes(bytes, length);
        let value = new_value(ValueKind::TEXT);
        (*value).payload.text.bytes = raw;
        (*value).payload.text.length = length;
        value
    }
}

/// A JSON number holding a copy of the C string `raw`.
///
/// # Safety
/// The running carrier's fiber has a live heap; `raw` is a C string.
pub unsafe fn json_number_value(raw: *const c_char) -> *mut Value {
    // SAFETY: the caller's contract. The value is allocated before its
    // bytes, as the C did.
    unsafe {
        let value = new_value(ValueKind::JSON_NUMBER);
        (*value).payload.json_number = copy_bytes(raw, core::ffi::CStr::from_ptr(raw).count_bytes());
        value
    }
}
