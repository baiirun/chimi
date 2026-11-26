//! The C view of [`super::map`] and [`super::compare`]: `runtime.c`'s Map
//! and Set intrinsics, `==`, `<` and `Order.compare` call these.

use core::ffi::c_char;
use core::ptr;

use super::compare::compare;
use super::json_number;
use super::layout::{MapNode, Value};
use super::map;

unsafe extern "C" {
    /// Prints "Chimi runtime exhausted memory" and exits 70.
    fn chimi_runtime_exhausted_memory() -> !;
}

/// [`compare`].
///
/// # Safety
/// As for `compare`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_value_compare_v1(left: *const Value, right: *const Value) -> i32 {
    // SAFETY: the caller's contract.
    unsafe { compare(left, right) }
}

/// [`json_number::compare`].
///
/// # Safety
/// Both are C strings holding JSON numbers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_json_number_compare_v1(left: *const c_char, right: *const c_char) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        json_number::compare(core::ffi::CStr::from_ptr(left).to_bytes(), core::ffi::CStr::from_ptr(right).to_bytes())
    }
}

/// [`json_number::canonical_text`] in a `malloc` block the caller frees,
/// with its length in `length`.
///
/// # Safety
/// `text` is a C string holding a JSON number; `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_json_number_canonical_text_v1(text: *const c_char, length: *mut usize) -> *mut c_char {
    // SAFETY: the caller's contract; the block holds the whole text.
    unsafe {
        let canonical = json_number::canonical_text(core::ffi::CStr::from_ptr(text).to_bytes());
        let bytes = libc::malloc(canonical.len().max(1)).cast::<u8>();
        if bytes.is_null() {
            chimi_runtime_exhausted_memory()
        }
        ptr::copy_nonoverlapping(canonical.as_ptr(), bytes, canonical.len());
        *length = canonical.len();
        bytes.cast()
    }
}

/// [`map::put`].
///
/// # Safety
/// As for `put`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_map_put_v1(node: *mut MapNode, key: *mut Value, value: *mut Value) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe { map::put(node, key, value) }
}

/// [`map::remove`].
///
/// # Safety
/// As for `remove`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_map_remove_v1(node: *mut MapNode, key: *const Value) -> *mut MapNode {
    // SAFETY: the caller's contract.
    unsafe { map::remove(node, key) }
}

/// [`map::get`].
///
/// # Safety
/// As for `get`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_map_get_v1(node: *const MapNode, key: *const Value) -> *mut Value {
    // SAFETY: the caller's contract.
    unsafe { map::get(node, key) }
}

/// [`map::size`].
///
/// # Safety
/// As for `size`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_map_size_v1(node: *const MapNode) -> i64 {
    // SAFETY: the caller's contract.
    unsafe { map::size(node) }
}

/// [`map::entries`] in a `malloc` block the caller frees, with their
/// number in `length`.
///
/// # Safety
/// As for `entries`; `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_map_entries_v1(root: *const MapNode, length: *mut usize) -> *mut *const MapNode {
    // SAFETY: the caller's contract; the block holds every entry.
    unsafe {
        let entries = map::entries(root);
        let nodes = libc::malloc(entries.len().max(1) * size_of::<*const MapNode>()).cast::<*const MapNode>();
        if nodes.is_null() {
            chimi_runtime_exhausted_memory()
        }
        ptr::copy_nonoverlapping(entries.as_ptr(), nodes, entries.len());
        *length = entries.len();
        nodes
    }
}
