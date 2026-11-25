//! The C view of [`crate::console`]: the queues of standard output, standard
//! error and `--log-file`, by index 0, 1 and 2. Any carrier thread writes
//! lines, so `runtime.c` makes every call under `console_lock` (or with one
//! carrier, where nothing else runs). No call makes a system call.

use crate::console::{ConsoleQueue, LineState};
use crate::serialized::Serialized;

static QUEUES: Serialized<[ConsoleQueue; 3]> =
    Serialized::new([ConsoleQueue::new(), ConsoleQueue::new(), ConsoleQueue::new()]);

/// Runs `body` on queue `index`, or returns `missing` for an index naming no
/// queue.
///
/// # Safety
/// The caller holds `console_lock`.
unsafe fn with_queue<R>(index: usize, missing: R, body: impl FnOnce(&mut ConsoleQueue) -> R) -> R {
    // SAFETY: the caller holds the lock.
    unsafe { QUEUES.with(|queues| queues.get_mut(index).map_or(missing, body)) }
}

/// Queues `length` bytes at `bytes`. False when the queue cannot grow.
/// # Safety
/// The caller holds `console_lock`, and `bytes` points to `length` readable
/// bytes (or `length` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_push_v1(queue: usize, bytes: *const u8, length: usize) -> bool {
    let bytes = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller passes `length` readable bytes.
        unsafe { core::slice::from_raw_parts(bytes, length) }
    };
    // SAFETY: the caller holds the lock.
    unsafe { with_queue(queue, false, |queue| queue.push(bytes)) }
}

/// The number of bytes queued and not yet written.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_queued_v1(queue: usize) -> usize {
    // SAFETY: the caller holds the lock.
    unsafe { with_queue(queue, 0, |queue| queue.queued()) }
}

/// The first unwritten byte; `chimi_console_queued_v1` bytes follow it. The
/// pointer is valid until the next call that changes the queue.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_front_v1(queue: usize) -> *const u8 {
    // SAFETY: the caller holds the lock.
    unsafe { with_queue(queue, core::ptr::null(), |queue| queue.front().as_ptr()) }
}

/// Marks the first `count` unwritten bytes written.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_advance_v1(queue: usize, count: usize) {
    // SAFETY: the caller holds the lock.
    unsafe { with_queue(queue, (), |queue| queue.advance(count)) }
}

/// Drops every queued byte and fails each line not yet fully written with
/// errno `error`.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_fail_v1(queue: usize, error: i32) {
    // SAFETY: the caller holds the lock.
    unsafe { with_queue(queue, (), |queue| queue.fail(error)) }
}

/// Starts waiting on the line that ends with the last byte queued and
/// returns its id, or -1 when its record cannot be allocated.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_begin_line_v1(queue: usize) -> i64 {
    // SAFETY: the caller holds the lock.
    let id = unsafe { with_queue(queue, None, |queue| queue.begin_line()) };
    id.and_then(|id| i64::try_from(id).ok()).unwrap_or(-1)
}

/// Line `id`'s state: -1 while pending, 0 once written, else the errno of
/// the write that dropped it.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_line_state_v1(queue: usize, id: i64) -> i32 {
    let Ok(id) = u64::try_from(id) else { return 0 };
    // SAFETY: the caller holds the lock.
    match unsafe { with_queue(queue, LineState::Written, |queue| queue.line_state(id)) } {
        LineState::Pending => -1,
        LineState::Written => 0,
        LineState::Failed(error) => error,
    }
}

/// Stops waiting on line `id`.
/// # Safety
/// The caller holds `console_lock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chimi_console_end_line_v1(queue: usize, id: i64) {
    let Ok(id) = u64::try_from(id) else { return };
    // SAFETY: the caller holds the lock.
    unsafe { with_queue(queue, (), |queue| queue.end_line(id)) }
}
