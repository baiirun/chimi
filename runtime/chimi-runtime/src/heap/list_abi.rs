//! The C view of [`super::list`]: `runtime.c`'s list builders call these
//! on the running fiber.

use super::layout::{Carrier, ListVector, Value};
use super::list;

/// [`list::from_values`].
///
/// # Safety
/// As for `from_values`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_list_vector_from_values_v1(values: *const *mut Value, length: u32) -> ListVector {
    // SAFETY: the caller's contract.
    unsafe { list::from_values(values, length) }
}

/// [`list::list_value`].
///
/// # Safety
/// As for `list_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_list_value_v1(vector: ListVector, start: u32, length: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { list::list_value(vector, start, length) }
}

/// [`list::list_push`].
///
/// # Safety
/// As for `list_push`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_list_push_v1(list: *const Value, item: *mut Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { list::list_push(list, item) }
}

/// [`list::slice`].
///
/// # Safety
/// As for `slice`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_list_slice_v1(here: *mut Carrier, list: *mut Value, from: u32, to: u32) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { list::slice(here, list, from, to) }
}
