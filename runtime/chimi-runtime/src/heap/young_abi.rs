//! The C view of [`super::young`]: `runtime.c`'s collector and region code
//! call these with the running carrier.

use super::layout::{AllocationScope, Carrier};
use super::young;

/// [`young::flush_counts`].
///
/// # Safety
/// As for `flush_counts`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_young_flush_counts_v1(here: *mut Carrier, scope: *mut AllocationScope) {
    // SAFETY: the caller's contract.
    unsafe { young::flush_counts(here, scope) }
}

/// [`young::chunks_used`].
///
/// # Safety
/// As for `chunks_used`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_young_chunks_used_v1(scope: *const AllocationScope) -> bool {
    // SAFETY: the caller's contract.
    unsafe { young::chunks_used(scope) }
}

/// [`young::reset_chunks`].
///
/// # Safety
/// As for `reset_chunks`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_young_reset_chunks_v1(here: *mut Carrier, scope: *mut AllocationScope, keep_first: bool) {
    // SAFETY: the caller's contract.
    unsafe { young::reset_chunks(here, scope, keep_first) }
}

/// [`young::scope_begin`].
///
/// # Safety
/// As for `scope_begin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_scope_begin_v1(here: *mut Carrier, scope: *mut AllocationScope) {
    // SAFETY: the caller's contract.
    unsafe { young::scope_begin(here, scope) }
}

/// [`young::scope_end`].
///
/// # Safety
/// As for `scope_end`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_scope_end_v1(scope: *mut AllocationScope) {
    // SAFETY: the caller's contract.
    unsafe { young::scope_end(scope) }
}
