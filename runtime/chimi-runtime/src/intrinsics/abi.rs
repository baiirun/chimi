//! The C view of the intrinsic families: `runtime.c`'s `invoke_intrinsic`
//! calls these for the std/map, std/set, std/json and std/decimal intrinsics.

use core::ffi::c_void;

use super::collections::{Call, map_operation, set_operation};
use super::decimals::decimal_operation;
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
