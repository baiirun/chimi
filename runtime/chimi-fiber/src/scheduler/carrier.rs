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
    /// How long this carrier retries the lock before it blocks (`lock_spin`).
    spin: std::time::Duration,
}

impl<'kernel> Locked<'kernel> {
    /// Requires: `carrier` is one of the kernel's carriers, and no other
    /// thread reaches the kernel as `carrier`.
    pub(crate) fn new(kernel: &'kernel Mutex<Scheduler>, carrier: Carrier) -> Self {
        Self {
            kernel,
            carrier,
            spin: lock_spin(),
        }
    }
}

impl KernelAccess for Locked<'_> {
    fn with<R>(&mut self, operation: impl FnOnce(&mut Scheduler) -> R) -> R {
        let (result, wakes) = {
            let mut kernel = lock_spinning(self.kernel, self.spin);
            #[cfg(test)]
            let _held = HeldMark::set();
            debug_assert!(
                !kernel.wakes.any_marked(),
                "every access that marks a carrier writes its wake on unlock"
            );
            kernel.enter_carrier(self.carrier);
            let result = operation(&mut kernel);
            (result, kernel.take_wakes())
        };
        wakes.write();
        result
    }
}

/// How long a carrier retries the kernel lock before it blocks in it.
const LOCK_SPIN: std::time::Duration = std::time::Duration::from_micros(20);

/// `LOCK_SPIN`, or none on a host with one core: there a carrier that
/// spins holds the only core the lock's holder needs to let go (measured
/// 8% slower with 8 or 32 carriers on one core). With two cores or more it
/// helped at every carrier count measured, up to 32 on four cores.
fn lock_spin() -> std::time::Duration {
    if std::thread::available_parallelism().map_or(1, std::num::NonZero::get) > 1 {
        LOCK_SPIN
    } else {
        std::time::Duration::ZERO
    }
}

/// Takes the kernel lock, first retrying for up to `spin` without
/// blocking. A kernel hold lasts about a microsecond, while a thread that
/// blocks in the lock sleeps until the holder wakes it, which with several
/// carriers sending at once cost more than the holds themselves
/// (multi-carrier-baseline.md, "Spinning on the kernel lock"). A poisoned
/// lock is taken over, as everywhere the kernel is reached.
fn lock_spinning<T>(lock: &Mutex<T>, spin: std::time::Duration) -> std::sync::MutexGuard<'_, T> {
    // The clock is read only once the lock was found held.
    let mut spin_until = None;
    loop {
        match lock.try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let now = std::time::Instant::now();
        if now >= *spin_until.get_or_insert(now + spin) {
            return lock.lock().unwrap_or_else(PoisonError::into_inner);
        }
        for _ in 0..32 {
            std::hint::spin_loop();
        }
    }
}

#[cfg(test)]
thread_local! {
    static KERNEL_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// True while this thread holds the kernel lock: tests check that a fiber
/// the carrier loop resumes runs without it. Never inlined, as for any
/// thread-local a fiber reads: a fiber may resume on another thread, and an
/// inlined read could reuse the address of the thread it parked on
/// ("Runtime state by owner", rules; seen on macOS).
#[cfg(test)]
#[inline(never)]
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
    Sleep(Duration, super::wake::WakeFds),
    /// Block on the carrier's wake pipe (and, on carrier 0, the signal
    /// pipe) until another carrier wakes it; then look again.
    Park(super::wake::WakeFds),
    /// The virtual clock moved under the kernel, or a due timer gave this
    /// carrier work; look again.
    Continue,
    /// The run has ended: a carrier other than 0 leaves its loop.
    Exit,
}

/// How a run ended for the carriers (`Scheduler::stop_carriers`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RunEnd {
    /// The root finished and no process is left: no carrier but 0 holds a
    /// fiber any more (`Scheduler::retire_carrier` checks it).
    Finished,
    /// The root waits on the host: the fibers stay for the next run.
    Parked,
}

/// What a carrier does after one idle step.
#[derive(Debug, PartialEq)]
pub(super) enum Waited {
    /// It waited, or has work; drive again.
    Again,
    /// Carrier 0's verdict.
    Idle(Idle),
    /// The run has ended for this carrier.
    Exit,
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
        match wait_idle(kernel) {
            Waited::Again => {}
            Waited::Idle(idle) => return idle,
            Waited::Exit => unreachable!("only a carrier other than 0 exits, in run_carrier"),
        }
    }
}

/// The loop of a carrier other than 0: drives its tasks and parks when it
/// has none, until the run ends.
pub(super) fn run_carrier(kernel: &mut impl KernelAccess) {
    loop {
        drive(kernel);
        match wait_idle(kernel) {
            Waited::Again => {}
            Waited::Exit => return,
            Waited::Idle(_) => unreachable!("only carrier 0 gives a verdict"),
        }
    }
}

/// What a carrier `drive` left with nothing runnable does: gives carrier
/// 0's verdict, leaves once the run has ended, or waits (for descriptors,
/// a timer, its wake pipe, or a virtual-time jump) and drives again.
pub(super) fn wait_idle(kernel: &mut impl KernelAccess) -> Waited {
    match kernel.with(Scheduler::idle_step) {
        IdleStep::Done(idle) => return Waited::Idle(idle),
        IdleStep::Exit => return Waited::Exit,
        IdleStep::Io(plan) => {
            let ready = plan.block();
            kernel.with(|kernel| kernel.finish_io_wait(plan, ready));
        }
        IdleStep::Sleep(wait, wake_fds) => {
            // A spin that a wake ends skips the block; a due timer is fired
            // either way, so a spin as long as the wait loses no time.
            let spin = wake_fds.spin_within(wait);
            if !wake_fds.spin(spin) {
                clock::block_real(wait.saturating_sub(spin), wake_fds.as_slice());
                wake_fds.settle();
            }
            kernel.with(Scheduler::fire_due_timers);
        }
        IdleStep::Park(wake_fds) => {
            assert!(
                !wake_fds.as_slice().is_empty(),
                "only a carrier with a wake pipe parks"
            );
            if !wake_fds.spin(wake_fds.spin_within(Duration::MAX)) {
                super::signal::wait_readable(wake_fds.as_slice(), None);
                wake_fds.settle();
            }
        }
        IdleStep::Continue => {}
    }
    Waited::Again
}

/// Runs every carrier of a kernel in turn on this thread, as a test stands
/// in for their threads, until carrier 0 gives a verdict. A carrier that
/// parked runs again only once one of its pipes is readable (the driver
/// empties it). A wait carrier 0 would block in while another carrier still
/// has turns to take is skipped, since those turns would have run alongside
/// it. Panics if every carrier is parked with no pipe readable and no
/// verdict: a wake was lost.
#[cfg(test)]
pub(super) fn round_robin(kernel: &mut Scheduler) -> Idle {
    let carriers = kernel.ready.carriers();
    let mut parked: Vec<Option<super::wake::WakeFds>> = vec![None; carriers as usize];
    loop {
        let mut ran = false;
        for index in 0..carriers {
            if let Some(fds) = parked[index as usize] {
                if !fds.as_slice().iter().any(|&fd| test_readable(fd)) {
                    continue;
                }
                for &fd in fds.as_slice() {
                    super::signal::drain(fd);
                }
                parked[index as usize] = None;
            }
            ran = true;
            kernel.enter_carrier(Carrier(index));
            drive(kernel);
            let others_parked = parked
                .iter()
                .enumerate()
                .all(|(other, fds)| other == index as usize || fds.is_some());
            match kernel.with(Scheduler::idle_step) {
                IdleStep::Done(idle) => return idle,
                IdleStep::Park(fds) => parked[index as usize] = Some(fds),
                IdleStep::Exit => panic!("carrier {index} left a run that gave no verdict"),
                IdleStep::Io(plan) if others_parked => {
                    let ready = plan.block();
                    kernel.with(|kernel| kernel.finish_io_wait(plan, ready));
                }
                IdleStep::Sleep(wait, fds) if others_parked => {
                    clock::block_real(wait, fds.as_slice());
                    kernel.with(Scheduler::fire_due_timers);
                }
                IdleStep::Io(_) | IdleStep::Sleep(..) | IdleStep::Continue => {}
            }
        }
        assert!(ran, "every carrier parked with no wake pending and no verdict");
    }
}

#[cfg(test)]
fn test_readable(fd: i32) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    unsafe { libc::poll(&mut poll, 1, 0) > 0 }
}

#[cfg(test)]
mod lock_tests {
    use super::{LOCK_SPIN, lock_spinning};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A lock held past the spin is still taken once its holder lets go.
    #[test]
    fn a_lock_held_past_the_spin_is_taken_when_released() {
        let lock = Mutex::new(0);
        std::thread::scope(|scope| {
            let held = lock.lock().expect("lock");
            let (taken, waiter) = std::sync::mpsc::channel();
            let lock = &lock;
            scope.spawn(move || {
                *lock_spinning(lock, LOCK_SPIN) += 1;
                taken.send(()).expect("send");
            });
            std::thread::sleep(LOCK_SPIN * 50);
            assert!(waiter.try_recv().is_err(), "not taken while held");
            drop(held);
            waiter.recv_timeout(Duration::from_secs(30)).expect("taken once released");
        });
        assert_eq!(*lock.lock().expect("lock"), 1);
    }

    /// A holder that panics after the waiter stopped spinning poisons the
    /// lock under the waiter's blocking take, which takes it over too.
    #[test]
    fn a_lock_poisoned_while_waiting_is_taken_over() {
        let lock = Mutex::new(7);
        let (held, holding) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let lock = &lock;
            let holder = scope.spawn(move || {
                let _held = lock.lock().expect("lock");
                held.send(()).expect("send");
                std::thread::sleep(LOCK_SPIN * 100);
                panic!("poison it");
            });
            holding.recv().expect("held");
            assert_eq!(*lock_spinning(lock, LOCK_SPIN), 7);
            assert!(holder.join().is_err());
        });
    }

    /// A host with one core does not spin; any other spins `LOCK_SPIN`.
    #[test]
    fn the_spin_is_off_on_one_core() {
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let expected = if cores > 1 { LOCK_SPIN } else { Duration::ZERO };
        assert_eq!(super::lock_spin(), expected);
    }

    /// With no spin a held lock is still taken once released.
    #[test]
    fn with_no_spin_a_held_lock_is_taken_when_released() {
        let lock = Mutex::new(0);
        std::thread::scope(|scope| {
            let held = lock.lock().expect("lock");
            let lock = &lock;
            let taker = scope.spawn(move || *lock_spinning(lock, Duration::ZERO) += 1);
            std::thread::sleep(Duration::from_millis(5));
            drop(held);
            taker.join().expect("taken");
        });
        assert_eq!(*lock.lock().expect("lock"), 1);
    }

    /// A lock a panic already poisoned is taken over at once.
    #[test]
    fn a_poisoned_lock_is_taken_over() {
        let lock = Mutex::new(7);
        let _ = std::panic::catch_unwind(|| {
            let _held = lock.lock().expect("lock");
            panic!("poison it");
        });
        assert!(lock.is_poisoned());
        assert_eq!(*lock_spinning(&lock, LOCK_SPIN), 7);
    }
}
