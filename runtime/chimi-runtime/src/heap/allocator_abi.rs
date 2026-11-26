//! The C view of [`super::allocator`]: `runtime.c`'s `allocate`,
//! `release_allocation` and footprint helpers call these with the running
//! carrier.

use super::allocator;
use super::layout::{AllocationHeader, Carrier};

/// [`allocator::allocate`].
///
/// # Safety
/// As for `allocate`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_allocate_v1(here: *mut Carrier, count: usize, size: usize) -> *mut u8 {
    // SAFETY: the caller's contract.
    unsafe { allocator::allocate(here, count, size) }
}

/// [`allocator::release`].
///
/// # Safety
/// As for `release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_release_allocation_v1(here: *mut Carrier, header: *mut AllocationHeader) {
    // SAFETY: the caller's contract.
    unsafe { allocator::release(here, header) }
}

/// [`allocator::footprint_add`].
///
/// # Safety
/// `here` is the running carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_footprint_add_v1(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe { allocator::footprint_add(here, bytes) }
}

/// [`allocator::footprint_sub`].
///
/// # Safety
/// `here` is the running carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_footprint_sub_v1(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe { allocator::footprint_sub(here, bytes) }
}

/// [`allocator::fragment_footprint_add`].
///
/// # Safety
/// `here` is the running carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_footprint_add_v1(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe { allocator::fragment_footprint_add(here, bytes) }
}

/// [`allocator::fragment_footprint_sub`].
///
/// # Safety
/// `here` is the running carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_fragment_footprint_sub_v1(here: *mut Carrier, bytes: usize) {
    // SAFETY: the caller's contract.
    unsafe { allocator::fragment_footprint_sub(here, bytes) }
}

/// [`allocator::seed_process_footprint`].
///
/// # Safety
/// As for `seed_process_footprint`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_footprint_seed_process_v1(first: *mut Carrier) {
    // SAFETY: the caller's contract.
    unsafe { allocator::seed_process_footprint(first) }
}
