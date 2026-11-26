//! The C view of [`super::verify`]: `runtime.c` calls it after each pass in
//! a checking build (`CHIMI_HEAP_VERIFY`).

use core::ffi::c_char;

use super::layout::{AllocationScope, Control, Value};
use super::verify::{Failure, verify};

/// 0 when `scope` passes the check, or the [`Failure`] code.
///
/// # Safety
/// As for [`verify`]: `scope` was just collected with mark epoch `epoch`,
/// keeping `value` and `control`, and its ancestors and heap are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_heap_verify_v1(
    scope: *const AllocationScope,
    value: *const Value,
    control: *const Control,
    epoch: u64,
    full: bool,
) -> u32 {
    // SAFETY: the caller's contract.
    match unsafe { verify(scope, value, control, epoch, full) } {
        Ok(()) => 0,
        Err(failure) => failure as u32,
    }
}

/// The text of a [`Failure`] code, or of an unknown one.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_heap_verify_message_v1(code: u32) -> *const c_char {
    let failures = [
        Failure::YoungNotEmpty,
        Failure::BadSurvivorHeader,
        Failure::SurvivorCountsWrong,
        Failure::WrongKind,
        Failure::PointsIntoYoung,
        Failure::PointsIntoEndedRegion,
        Failure::PointsToFreed,
        Failure::StillInChunk,
    ];
    match failures.into_iter().find(|failure| *failure as u32 == code) {
        Some(failure) => failure.message().as_ptr(),
        None => c"an unknown failure".as_ptr(),
    }
}
