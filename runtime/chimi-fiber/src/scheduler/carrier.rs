//! The carrier loop: what one OS thread does to run the scheduler's tasks.
//! The kernel (`Scheduler`) holds all shared state; the loop reaches it
//! through `KernelAccess` and gives it up while a fiber runs and while the
//! carrier waits, which is where other carriers will take it (step 3 PR 5 of
//! docs/architecture/parallel-scheduler.md). Lock order is the kernel first,
//! then the native runtime's handle table.

use super::reactor::IoWaitPlan;
use super::ready::Carrier;
use super::{ChimiFiberCommand, Idle, Scheduler, TaskId, clock, resume_fiber};
use crate::ChimiFiber;
use std::ptr::NonNull;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How the loop reaches the kernel: directly, for the scheduler's own
/// `&mut self` methods and its unit tests, or through the kernel lock.
pub(crate) trait KernelAccess {
    fn with<R>(&mut self, operation: impl FnOnce(&mut Scheduler) -> R) -> R;
}

impl KernelAccess for Scheduler {
    fn with<R>(&mut self, operation: impl FnOnce(&mut Scheduler) -> R) -> R {
        let result = operation(self);
        self.take_wakes().write();
        result
    }
}

/// The kernel behind its lock, as one carrier reaches it. Each access tells
/// the kernel which carrier holds it, so ready work and wakes are routed by
/// carrier rather than by whoever ran last, and wakes the carriers it handed
/// work to once the lock is released. Access cannot fail: a panic
/// while the lock is held aborts in `chimi_scheduler_run` (an `extern "C"`
/// function), so a poisoned lock is only taken over, never reported.
pub(crate) struct Locked<'kernel> {
    kernel: &'kernel Mutex<Scheduler>,
    carrier: Carrier,
}

impl<'kernel> Locked<'kernel> {
    /// Requires: `carrier` is one of the kernel's carriers, and no other
    /// thread reaches the kernel as `carrier`.
    pub(crate) fn new(kernel: &'kernel Mutex<Scheduler>, carrier: Carrier) -> Self {
        Self { kernel, carrier }
    }
}

impl KernelAccess for Locked<'_> {
    fn with<R>(&mut self, operation: impl FnOnce(&mut Scheduler) -> R) -> R {
        let (result, wakes) = {
            let mut kernel = self.kernel.lock().unwrap_or_else(PoisonError::into_inner);
            #[cfg(test)]
            let _held = HeldMark::set();
            kernel.enter_carrier(self.carrier);
            let result = operation(&mut kernel);
            (result, kernel.take_wakes())
        };
        wakes.write();
        result
    }
}

#[cfg(test)]
thread_local! {
    static KERNEL_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// True while this thread holds the kernel lock: tests check that a fiber
/// the carrier loop resumes runs without it.
#[cfg(test)]
pub(super) fn kernel_held() -> bool {
    KERNEL_HELD.with(std::cell::Cell::get)
}

#[cfg(test)]
struct HeldMark(bool);

#[cfg(test)]
impl HeldMark {
    fn set() -> Self {
        Self(KERNEL_HELD.with(|held| held.replace(true)))
    }
}

#[cfg(test)]
impl Drop for HeldMark {
    fn drop(&mut self) {
        let previous = self.0;
        KERNEL_HELD.with(|held| held.set(previous));
    }
}

/// A task marked running whose fiber was entered on this thread, to be
/// resumed without the kernel.
#[derive(Clone, Copy)]
pub(super) struct Runnable {
    pub(super) task: TaskId,
    pub(super) fiber: NonNull<ChimiFiber>,
    pub(super) command: ChimiFiberCommand,
    pub(super) kind: RunKind,
}

/// Why the carrier resumes a fiber, which decides how its return applies.
#[derive(Clone, Copy)]
pub(super) enum RunKind {
    /// A ready task's next step (`Scheduler::complete_run`).
    Step,
    /// A job: the cancellation unwind of a task another carrier ended.
    Unwind { forced: bool },
}

/// What an idle carrier does once `drive` found nothing runnable.
pub(super) enum IdleStep {
    Done(Idle),
    /// Wait on descriptors without the kernel, then apply what was found.
    Io(IoWaitPlan),
    /// Sleep on the real clock without the kernel (or until the signal pipe
    /// or the carrier's wake pipe is readable), then fire due timers.
    Sleep(Duration, Vec<i32>),
    /// The virtual clock moved under the kernel; look again.
    Continue,
}

/// Runs ready tasks until this carrier's ready queue is empty and no queued
/// release refills it. The kernel is taken once per resume: to finish the
/// last task's step, do the housekeeping between resumes, and enter the next.
pub(super) fn drive(kernel: &mut impl KernelAccess) {
    let mut resumes = 0_u32;
    let mut next = kernel.with(|kernel| {
        kernel.between_resumes();
        kernel.next_runnable(&mut resumes)
    });
    while let Some(runnable) = next {
        let event = resume_fiber(runnable.fiber, runnable.command);
        next = kernel.with(|kernel| {
            kernel.complete(runnable, event);
            kernel.after_resume(&mut resumes);
            kernel.next_runnable(&mut resumes)
        });
    }
}

/// Drives runnable work, and whenever nothing is runnable and no host
/// operation is in flight, waits: on descriptors, or for the earliest timer
/// (virtual time jumps to it, real time sleeps until it). Returns when
/// nothing more can happen without the host.
pub(super) fn run_until_idle(kernel: &mut impl KernelAccess) -> Idle {
    loop {
        drive(kernel);
        match kernel.with(Scheduler::idle_step) {
            IdleStep::Done(idle) => return idle,
            IdleStep::Io(plan) => {
                let ready = plan.block();
                kernel.with(|kernel| kernel.finish_io_wait(plan, ready));
            }
            IdleStep::Sleep(wait, wake_fds) => {
                clock::block_real(wait, &wake_fds);
                kernel.with(Scheduler::fire_due_timers);
            }
            IdleStep::Continue => {}
        }
    }
}
