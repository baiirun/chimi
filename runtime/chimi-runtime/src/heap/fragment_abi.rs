//! The C view of [`super::fragment`]: `runtime.c`'s transfers, tasks and
//! processes call these with the running carrier.

use core::ffi::c_void;

use super::fragment;
use super::layout::{AllocationArea, AllocationKind, Carrier, HeapStatus, RuntimeFragment, RuntimeHeap};

/// [`fragment::copy`].
///
/// # Safety
/// As for `copy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_copy_v1(
    here: *mut Carrier,
    roots: *const *mut c_void,
    kinds: *const AllocationKind,
    count: u32,
    out: *mut *mut RuntimeFragment,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe { fragment::copy(here, roots, kinds, count, out) }
}

/// [`fragment::adopt`].
///
/// # Safety
/// As for `adopt`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_adopt_v1(
    here: *mut Carrier,
    fragment: *mut RuntimeFragment,
    roots: *mut *mut c_void,
) {
    // SAFETY: the caller's contract.
    unsafe { fragment::adopt(here, fragment, roots) }
}

/// [`fragment::copy_into_current`].
///
/// # Safety
/// As for `copy_into_current`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_copy_into_current_v1(
    here: *mut Carrier,
    fragment: *const RuntimeFragment,
    roots: *mut *mut c_void,
) {
    // SAFETY: the caller's contract.
    unsafe { fragment::copy_into_current(here, fragment, roots) }
}

/// [`fragment::duplicate`].
///
/// # Safety
/// As for `duplicate`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_duplicate_v1(
    here: *mut Carrier,
    source: *const RuntimeFragment,
) -> *mut RuntimeFragment {
    // SAFETY: the caller's contract.
    unsafe { fragment::duplicate(here, source) }
}

/// [`fragment::free`].
///
/// # Safety
/// As for `free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_free_v1(here: *mut Carrier, fragment: *mut RuntimeFragment) {
    // SAFETY: the caller's contract.
    unsafe { fragment::free(here, fragment) }
}

/// [`fragment::area_release_all`].
///
/// # Safety
/// As for `area_release_all`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_area_release_all_v1(here: *mut Carrier, area: *mut AllocationArea) {
    // SAFETY: the caller's contract.
    unsafe { fragment::area_release_all(here, area) }
}

/// [`fragment::area_free`].
///
/// # Safety
/// As for `area_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_area_free_v1(here: *mut Carrier, area: *mut AllocationArea) {
    // SAFETY: the caller's contract.
    unsafe { fragment::area_free(here, area) }
}

/// [`fragment::heap_create`].
///
/// # Safety
/// As for `heap_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_heap_create_v1(here: *mut Carrier) -> *mut RuntimeHeap {
    // SAFETY: the caller's contract.
    unsafe { fragment::heap_create(here) }
}

/// [`fragment::heap_destroy`].
///
/// # Safety
/// As for `heap_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_heap_destroy_v1(here: *mut Carrier, heap: *mut RuntimeHeap) {
    // SAFETY: the caller's contract.
    unsafe { fragment::heap_destroy(here, heap) }
}
