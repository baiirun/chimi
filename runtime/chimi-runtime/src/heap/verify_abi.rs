//! The C view of [`super::verify`]: `runtime.c` calls it after each pass in
//! a checking build (`CHIMI_HEAP_VERIFY`).

use core::ffi::c_char;

use super::layout::{AllocationScope, Control, Value};
use alloc::vec::Vec;

use super::verify::{Failure, Pass, verify};

/// 0 when `scope` passes the check, or the [`Failure`] code.
///
/// # Safety
/// As for [`verify`]: `scope` was just collected with mark epoch `epoch`,
/// keeping `value` and `control`, and its ancestors and heap are live.
/// `freed` holds `freed_count` payload addresses the pass freed, and
/// `chunks` holds `chunk_bounds` addresses, each emptied young chunk's
/// start then end; either may be null when its count is 0.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn chimi_heap_verify_v1(
    scope: *const AllocationScope,
    value: *const Value,
    control: *const Control,
    epoch: u64,
    full: bool,
    promoted: usize,
    freed: *const *const u8,
    freed_count: usize,
    chunks: *const *const u8,
    chunk_bounds: usize,
) -> u32 {
    // SAFETY: the caller's contract.
    let (freed, chunks) = unsafe { (addresses(freed, freed_count), addresses(chunks, chunk_bounds)) };
    let mut freed = freed;
    freed.sort_unstable();
    let young_chunks: Vec<(usize, usize)> = chunks.chunks_exact(2).map(|bounds| (bounds[0], bounds[1])).collect();
    let pass = Pass { epoch, full, promoted, freed: &freed, young_chunks: &young_chunks };
    // SAFETY: the caller's contract.
    match unsafe { verify(scope, value, control, &pass) } {
        Ok(()) => 0,
        Err(failure) => failure as u32,
    }
}

/// The `count` addresses at `items`, which may be null when `count` is 0.
///
/// # Safety
/// `items` points to `count` readable pointers.
unsafe fn addresses(items: *const *const u8, count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    // SAFETY: the caller's contract.
    unsafe { core::slice::from_raw_parts(items, count) }.iter().map(|item| *item as usize).collect()
}

/// The text of a [`Failure`] code, or of an unknown one.
#[unsafe(no_mangle)]
pub extern "C" fn chimi_heap_verify_message_v1(code: u32) -> *const c_char {
    match Failure::ALL.into_iter().find(|failure| *failure as u32 == code) {
        Some(failure) => failure.message().as_ptr(),
        None => c"an unknown failure".as_ptr(),
    }
}
