//! The running fiber's requests to the scheduler (chimi-fiber), for the
//! Rust intrinsics. A request goes through `runtime.c`, which restores the
//! fiber's heap and allocation target and moves to the carrier the fiber
//! resumed on, so code here must read the carrier again after each request.

use crate::heap::layout::{Scheduler, SchedulerRequest};

unsafe extern "C" {
    /// Whether the program starts the scheduler (program data).
    static CHIMI_REQUIRES_SCHEDULER: u32;
    /// Serves `request` and returns the fiber's command (`runtime.c`).
    fn chimi_runtime_scheduler_request_v1(request: *mut SchedulerRequest) -> u32;
    /// Whether the running task's cancellation ends it, rather than a
    /// scope inside it; `runtime.c` defines a weak stand-in returning 1
    /// for a program linked without the scheduler.
    fn chimi_fiber_cancel_is_terminal() -> u32;
}

/// Whether the program starts the scheduler; without it, a request has no
/// one to answer it.
#[inline(always)]
pub fn requires_scheduler() -> bool {
    // SAFETY: program data, constant for the run.
    unsafe { CHIMI_REQUIRES_SCHEDULER != 0 }
}

/// Serves `request` on the running fiber; true when the fiber resumed to
/// be cancelled.
///
/// # Safety
/// Runs on a fiber of a program that starts the scheduler.
pub unsafe fn request(request: &mut SchedulerRequest) -> bool {
    // SAFETY: the caller's contract.
    unsafe { chimi_runtime_scheduler_request_v1(request) == Scheduler::FIBER_CANCEL }
}

/// Whether the running task's cancellation is its own end.
pub fn cancel_is_terminal() -> bool {
    // SAFETY: reads the running fiber's state.
    unsafe { chimi_fiber_cancel_is_terminal() != 0 }
}
