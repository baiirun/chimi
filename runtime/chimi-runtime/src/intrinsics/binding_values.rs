//! The value half of the C binding API (`chimi_binding.h`): reading a
//! binding call's arguments, building the values it returns, and its
//! results (CHI-305 step 5, phase 6). A `ChimiBindingValue` is a [`Value`]
//! and a `ChimiBindingResult` a [`Control`], so these hand the runtime's own
//! pointers across. Handles, waits and cancellation checks stay in
//! `runtime.c` with the handle lock and the scheduler requests they make.
//! Every value is allocated in the order `runtime.c` allocated it, so memory
//! reports do not change.

use core::ffi::{CStr, c_char, c_int};
use core::ptr;

use super::core::lasting_message;
use super::host::tuple;
use super::standard::{boolean_value, defect, defect_with, standard_value, success, void_value};
use crate::heap::layout::{BindingCall, Control, ControlKind, StandardConstructor, Value, ValueKind};
use crate::heap::list;
use crate::heap::values::{integer_value, new_control, text_value};

/// Argument `index` of `call` when it is a `kind`, else null.
///
/// # Safety
/// `call` is the running binding call.
unsafe fn argument(call: *const BindingCall, index: u32, kind: ValueKind) -> *const Value {
    // SAFETY: the caller's contract: the call holds `argument_count` live values.
    unsafe {
        if index >= (*call).argument_count {
            return ptr::null();
        }
        let value = *(*call).arguments.add(index as usize);
        if (*value).kind == kind { value } else { ptr::null() }
    }
}

/// `length` bytes at `bytes` as a slice; `bytes` may be null when
/// `length` is 0.
///
/// # Safety
/// `bytes` holds `length` readable bytes.
unsafe fn slice<'a>(bytes: *const u8, length: usize) -> &'a [u8] {
    if length == 0 {
        &[]
    } else {
        // SAFETY: the caller's contract.
        unsafe { core::slice::from_raw_parts(bytes, length) }
    }
}

/// The `length` values at `items` when none is null: a builder returns
/// null for a value it refused, and a value built from one is refused too.
///
/// # Safety
/// `items` holds `length` pointers, or `length` is 0.
unsafe fn present<'a>(items: *const *mut Value, length: u32) -> Option<&'a [*mut Value]> {
    let items = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller's contract.
        unsafe { core::slice::from_raw_parts(items, length as usize) }
    };
    items.iter().all(|item| !item.is_null()).then_some(items)
}

/// The defect a call's result reports for a value its builder refused.
///
/// # Safety
/// `call` is the running binding call, on a fiber that may allocate.
unsafe fn invalid_value(call: *const BindingCall) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { defect(c"a C binding built an invalid value", i64::from((*call).call_node)) }
}

/// # Safety
/// `call` is the running binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_argument_count(call: *const BindingCall) -> u32 {
    // SAFETY: the caller's contract.
    unsafe { (*call).argument_count }
}

/// # Safety
/// `call` is the running binding call; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_integer(call: *const BindingCall, index: u32, out: *mut i64) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        let value = argument(call, index, ValueKind::INTEGER);
        if !value.is_null() {
            *out = (*value).payload.integer;
        }
        c_int::from(!value.is_null())
    }
}

/// # Safety
/// `call` is the running binding call; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_boolean(call: *const BindingCall, index: u32, out: *mut c_int) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        let value = argument(call, index, ValueKind::BOOLEAN);
        if !value.is_null() {
            *out = c_int::from((*value).payload.boolean);
        }
        c_int::from(!value.is_null())
    }
}

/// The bytes of argument `index` when it is a `kind` (Text or Bytes).
///
/// # Safety
/// `call` is the running binding call; `bytes` and `length` are writable.
unsafe fn read_bytes(
    call: *const BindingCall,
    index: u32,
    kind: ValueKind,
    bytes: *mut *const c_char,
    length: *mut usize,
) -> c_int {
    // SAFETY: the caller's contract.
    unsafe {
        let value = argument(call, index, kind);
        if !value.is_null() {
            *bytes = (*value).payload.text.bytes;
            *length = (*value).payload.text.length;
        }
        c_int::from(!value.is_null())
    }
}

/// # Safety
/// `call` is the running binding call; `bytes` and `length` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_text(
    call: *const BindingCall,
    index: u32,
    bytes: *mut *const c_char,
    length: *mut usize,
) -> c_int {
    // SAFETY: the caller's contract.
    unsafe { read_bytes(call, index, ValueKind::TEXT, bytes, length) }
}

/// # Safety
/// `call` is the running binding call; `bytes` and `length` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_bytes(
    call: *const BindingCall,
    index: u32,
    bytes: *mut *const u8,
    length: *mut usize,
) -> c_int {
    // SAFETY: the caller's contract.
    unsafe { read_bytes(call, index, ValueKind::BYTES, bytes.cast(), length) }
}

/// # Safety
/// Runs inside a binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_void(_call: *mut BindingCall) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { void_value() }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_integer(_call: *mut BindingCall, value: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { integer_value(ValueKind::INTEGER, value) }
}

/// # Safety
/// Runs inside a binding call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_boolean(_call: *mut BindingCall, value: c_int) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { boolean_value(value != 0) }
}

/// A Text of `bytes`, or null when `bytes` is null or not UTF-8: the
/// result functions report a null as a defect, so a binding never hands
/// Chimi invalid Text. Text may hold U+0000.
///
/// # Safety
/// Runs inside a binding call, on a fiber that may allocate; `bytes` is
/// null or holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_text(_call: *mut BindingCall, bytes: *const c_char, length: usize) -> *mut Value {
    if bytes.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller's contract.
    unsafe {
        if core::str::from_utf8(slice(bytes.cast(), length)).is_err() {
            return ptr::null_mut();
        }
        text_value(bytes, length)
    }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate; `bytes` holds
/// `length` bytes, or `length` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_bytes(_call: *mut BindingCall, bytes: *const u8, length: usize) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        let value = text_value(bytes.cast(), length);
        (*value).kind = ValueKind::BYTES;
        value
    }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_none(_call: *mut BindingCall) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { standard_value(StandardConstructor::OPTION_NONE, &[]) }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate; `value` is
/// null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_some(_call: *mut BindingCall, value: *mut Value) -> *mut Value {
    if value.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller's contract.
    unsafe { standard_value(StandardConstructor::OPTION_SOME, &[value]) }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate; `items` holds
/// `length` values, each null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_tuple(_call: *mut BindingCall, items: *const *mut Value, length: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { present(items, length).map_or(ptr::null_mut(), |items| tuple(items)) }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate; `items` holds
/// `length` values, each null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_make_list(_call: *mut BindingCall, items: *const *mut Value, length: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe {
        match present(items, length) {
            Some(items) => list::list_value(list::from_values(items.as_ptr(), length), 0, length),
            None => ptr::null_mut(),
        }
    }
}

/// # Safety
/// `call` is the running binding call, on a fiber that may allocate;
/// `value` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_success(call: *mut BindingCall, value: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { if value.is_null() { invalid_value(call) } else { success(value) } }
}

/// # Safety
/// `call` is the running binding call, on a fiber that may allocate;
/// `value` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_ok(call: *mut BindingCall, value: *mut Value) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if value.is_null() {
            return invalid_value(call);
        }
        success(standard_value(StandardConstructor::RESULT_OK, &[value]))
    }
}

/// `Success(Err((code, message)))`; a message that is null or not UTF-8 is
/// a defect.
///
/// # Safety
/// `call` is the running binding call, on a fiber that may allocate;
/// `message` is null or a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_failure(call: *mut BindingCall, code: i64, message: *const c_char) -> *mut Control {
    // SAFETY: the caller's contract. The Int is made before the Text, as in C.
    unsafe {
        let code = chimi_binding_make_integer(call, code);
        let length = if message.is_null() { 0 } else { CStr::from_ptr(message).count_bytes() };
        let message = chimi_binding_make_text(call, message, length);
        if message.is_null() {
            return defect(c"a C binding failed with a message that is not UTF-8", i64::from((*call).call_node));
        }
        success(standard_value(StandardConstructor::RESULT_ERR, &[tuple(&[code, message])]))
    }
}

/// A defect with a copy of `message`, which must be UTF-8. The copy lives
/// outside every heap, as every defect message does: a control's message
/// is not traced, and the heap the call ran on may be freed while the
/// defect is still read.
///
/// # Safety
/// `call` is the running binding call, on a fiber that may allocate;
/// `message` is null or a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_defect(call: *mut BindingCall, message: *const c_char) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        let node = i64::from((*call).call_node);
        if message.is_null() {
            return defect(c"a C binding defected with a message that is not UTF-8", node);
        }
        let message = CStr::from_ptr(message).to_bytes();
        if core::str::from_utf8(message).is_err() {
            return defect(c"a C binding defected with a message that is not UTF-8", node);
        }
        defect_with(lasting_message(message), node)
    }
}

/// # Safety
/// Runs inside a binding call, on a fiber that may allocate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_binding_cancelled(_call: *mut BindingCall) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { new_control(ControlKind::CANCEL, ptr::null_mut(), ptr::null(), -1) }
}
