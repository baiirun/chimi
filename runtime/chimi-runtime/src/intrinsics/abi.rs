//! The C view of the intrinsic families: `runtime.c`'s `invoke_intrinsic`
//! calls these for the std/map, std/set, std/json, std/decimal, std/bytes and
//! std/duration intrinsics, and `Debug.crash`, `Option.unwrap_or_fail`,
//! std/text and std/list, and std/test's assertions.

use core::ffi::c_void;

use super::assertions::assertion_operation;
use super::bytes::bytes_operation;
use super::collections::{Call, map_operation, set_operation};
use super::core::core_operation;
use super::debug::render;
use super::decimals::decimal_operation;
use super::durations::duration_operation;
use super::json::json_operation;
use crate::heap::layout::{Control, Environment, Value};

/// The arguments as a slice.
///
/// # Safety
/// `arguments` holds `count` values, or `count` is 0.
unsafe fn arguments<'a>(arguments: *mut *mut Value, count: u32) -> &'a [*mut Value] {
    if count == 0 {
        &[]
    } else {
        // SAFETY: the caller's contract.
        unsafe { core::slice::from_raw_parts(arguments, count as usize) }
    }
}

/// [`map_operation`].
///
/// # Safety
/// As for `map_operation`; `runtime` and `environment` are the call's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_map_v1(
    runtime: *mut c_void,
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    span: u64,
    call_node: u32,
    environment: *mut Environment,
) -> *mut Control {
    let call = Call { runtime, span, call_node, environment };
    // SAFETY: the caller's contract.
    unsafe { map_operation(&call, intrinsic, arguments(values, count)) }
}

/// [`set_operation`].
///
/// # Safety
/// As for `set_operation`; `runtime` and `environment` are the call's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_set_v1(
    runtime: *mut c_void,
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    span: u64,
    call_node: u32,
    environment: *mut Environment,
) -> *mut Control {
    let call = Call { runtime, span, call_node, environment };
    // SAFETY: the caller's contract.
    unsafe { set_operation(&call, intrinsic, arguments(values, count)) }
}

/// [`json_operation`].
///
/// # Safety
/// As for `json_operation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_json_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { json_operation(intrinsic, arguments(values, count), call_node) }
}

/// [`decimal_operation`].
///
/// # Safety
/// As for `decimal_operation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_decimal_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { decimal_operation(intrinsic, arguments(values, count), call_node) }
}

/// [`bytes_operation`].
///
/// # Safety
/// As for `bytes_operation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_bytes_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { bytes_operation(intrinsic, arguments(values, count), call_node) }
}

/// [`duration_operation`].
///
/// # Safety
/// As for `duration_operation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_duration_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { duration_operation(intrinsic, arguments(values, count), call_node) }
}

/// [`core_operation`].
///
/// # Safety
/// As for `core_operation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_core_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { core_operation(intrinsic, arguments(values, count), call_node) }
}

/// [`render`]: `value` in the debug format, as a Text.
///
/// # Safety
/// As for `render`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_runtime_debug_render_v1(value: *const Value, indented: bool) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { render(value, indented) }
}

/// [`assertion_operation`].
///
/// # Safety
/// As for `assertion_operation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_intrinsic_assertion_v1(
    intrinsic: u32,
    values: *mut *mut Value,
    count: u32,
    call_node: u32,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe { assertion_operation(intrinsic, arguments(values, count), call_node) }
}
