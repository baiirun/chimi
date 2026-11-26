//! The C view of [`super::values`]: `runtime.c`'s builders call these on
//! the running fiber.

use core::ffi::{c_char, c_void};

use super::layout::{Control, ControlKind, Decimal, Environment, Value, ValueKind};
use super::values;

/// [`values::new_value`].
///
/// # Safety
/// As for `new_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_new_v1(kind: ValueKind) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::new_value(kind) }
}

/// [`values::new_control`].
///
/// # Safety
/// As for `new_control`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_control_new_v1(
    kind: ControlKind,
    value: *mut Value,
    message: *const c_char,
    node: i64,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { values::new_control(kind, value, message, node) }
}

/// [`values::new_value_slots`].
///
/// # Safety
/// As for `new_value_slots`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_slots_new_v1(count: u32) -> *mut *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::new_value_slots(count) }
}

/// [`values::clone_value_slots`].
///
/// # Safety
/// As for `clone_value_slots`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_slots_clone_v1(source: *const *mut Value, count: u32) -> *mut *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::clone_value_slots(source, count) }
}

/// [`values::new_environment`].
///
/// # Safety
/// As for `new_environment`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_environment_new_v1(local_count: u32) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe { values::new_environment(local_count) }
}

/// [`values::clone_environment`].
///
/// # Safety
/// As for `clone_environment`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_environment_clone_v1(
    source: *const Environment,
    locals: bool,
    requirements: bool,
) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe { values::clone_environment(source, locals, requirements) }
}

/// [`values::capture_requirements`].
///
/// # Safety
/// As for `capture_requirements`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_environment_capture_requirements_v1(source: *const Environment) -> *mut Environment {
    // SAFETY: the caller's contract.
    unsafe { values::capture_requirements(source) }
}

/// [`values::integer_value`].
///
/// # Safety
/// As for `integer_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_integer_v1(kind: ValueKind, raw: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::integer_value(kind, raw) }
}

/// [`values::float_value`].
///
/// # Safety
/// As for `float_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_float_v1(raw: f64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::float_value(raw) }
}

/// [`values::range_value`].
///
/// # Safety
/// As for `range_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_range_v1(start: i64, end: i64) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::range_value(start, end) }
}

/// [`values::decimal_value`].
///
/// # Safety
/// As for `decimal_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_decimal_v1(decimal: Decimal) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::decimal_value(decimal) }
}

/// [`values::pointer_value`].
///
/// # Safety
/// As for `pointer_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_pointer_v1(kind: ValueKind, raw: *mut c_void) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::pointer_value(kind, raw) }
}

/// [`values::copy_bytes`].
///
/// # Safety
/// As for `copy_bytes`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_bytes_copy_v1(bytes: *const c_char, length: usize) -> *mut c_char {
    // SAFETY: the caller's contract.
    unsafe { values::copy_bytes(bytes, length) }
}

/// [`values::text_value`].
///
/// # Safety
/// As for `text_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_text_v1(bytes: *const c_char, length: usize) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::text_value(bytes, length) }
}

/// [`values::json_number_value`].
///
/// # Safety
/// As for `json_number_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_json_number_v1(raw: *const c_char) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { values::json_number_value(raw) }
}
