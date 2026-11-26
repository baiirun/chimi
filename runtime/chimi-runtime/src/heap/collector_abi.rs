//! The C view of [`super::collector`]: `runtime.c`'s loops and regions call
//! these at a pass boundary and at a region's end, with the running carrier.

use super::collector;
use super::layout::{AllocationScope, Carrier, Control, HeapStatus, Value};

/// [`collector::collect`].
///
/// # Safety
/// As for `collect`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_scope_collect_v1(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe { collector::collect(here, scope, value, control) }
}

/// [`collector::end`].
///
/// # Safety
/// As for `end`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_scope_finish_v1(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe { collector::end(here, scope, value, control) }
}

/// [`collector::sweep`], for the C test harnesses that run one pass of a
/// chosen kind.
///
/// # Safety
/// As for `sweep`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_scope_sweep_v1(
    here: *mut Carrier,
    scope: *mut AllocationScope,
    value: *mut *mut Value,
    control: *mut *mut Control,
    full: bool,
    ending: bool,
) -> HeapStatus {
    // SAFETY: the caller's contract.
    unsafe { collector::sweep(here, scope, value, control, full, ending) }
}

/// [`collector::take_epoch`], for the C test harness that checks epochs
/// stay unique across carriers.
///
/// # Safety
/// `here` is the running carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_take_mark_epoch_v1(here: *mut Carrier) -> u64 {
    // SAFETY: the caller's contract.
    unsafe { collector::take_epoch(here) }
}
