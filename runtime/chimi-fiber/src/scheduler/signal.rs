//! Delivering SIGINT and SIGTERM to a running program.
//!
//! The runner owns the signal handlers. Its handler stores the signal number
//! in a word the scheduler can read and writes one byte to a pipe, so a
//! real-clock wait wakes. The scheduler delivers the first signal by
//! cancelling the root task, the same cancellation `Task.cancel` performs:
//! the root's children are cancelled and every root supervisor it started
//! stops, each member draining within its `shutdown_within`. A later signal
//! is the runner's business (it ends the program at once).
//!
//! Under the virtual clock a test injects a signal at an instant instead;
//! it is a timer, so a program that would otherwise be idle jumps to it.

use super::clock::{Instant, TimerId, TimerTarget};
use super::{ClockEvent, ClockEventKind, Scheduler, TaskId, TaskOutcome, TaskPhase};

#[derive(Default)]
pub(super) struct Signals {
    /// The runner's received-signal word, or null.
    word: Option<*mut i32>,
    /// The read end of the runner's wake pipe, or none.
    wake_fd: Option<i32>,
    /// The task a signal cancels.
    root: Option<TaskId>,
    /// A signal has been delivered; later ones are not.
    delivered: bool,
    /// An injected signal's timer, while it is pending.
    timer: Option<TimerId>,
    /// An injected signal that fired.
    injected: i32,
}

impl Signals {
    /// The signal waiting to be delivered, if any.
    fn pending(&self) -> Option<i32> {
        if self.delivered {
            return None;
        }
        // SAFETY: the runner keeps the word valid while the scheduler runs;
        // its handler writes it asynchronously, so it is read volatile.
        let received = self
            .word
            .map_or(0, |word| unsafe { std::ptr::read_volatile(word) });
        [received, self.injected]
            .into_iter()
            .find(|signal| *signal != 0)
    }

    pub(super) fn wake_fd(&self) -> Option<i32> {
        self.wake_fd
    }
}

impl Scheduler {
    /// Connects the runner's signal handler: `word` holds the number of the
    /// first SIGINT or SIGTERM received (0 before), and its handler writes a
    /// byte to the pipe whose read end is `wake_fd` (0 or negative for
    /// none, since a zeroed C config must not name standard input).
    pub fn set_signal_source(&mut self, word: *mut i32, wake_fd: i32) {
        self.signals.word = (!word.is_null()).then_some(word);
        self.signals.wake_fd = (wake_fd > 0).then_some(wake_fd);
    }

    /// Delivers `signal` when the execution clock reaches `at`.
    pub fn inject_signal_at(&mut self, at: Instant, signal: i32) {
        if let Some(timer) = self.signals.timer.take() {
            self.timers.cancel(timer);
        }
        self.signals.timer = Some(self.timers.insert(at, TimerTarget::Signal(signal)));
    }

    /// The task a delivered signal cancels.
    pub(super) fn set_signal_root(&mut self, root: TaskId) {
        self.signals.root = Some(root);
    }

    /// The root has ended, so a signal still to come changes nothing; an
    /// injected one must not move virtual time.
    pub(super) fn retire_signal_timer(&mut self) {
        if let Some(timer) = self.signals.timer.take() {
            self.timers.cancel(timer);
        }
    }

    pub(super) fn fire_signal(&mut self, signal: i32) {
        self.signals.timer = None;
        self.signals.injected = signal;
        if let Some(word) = self.signals.word {
            // SAFETY: as in `Signals::pending`. The runner reads it after the
            // run to learn which signal ended the program.
            unsafe { std::ptr::write_volatile(word, signal) };
        }
        self.deliver_signal();
    }

    /// Cancels the root once, at the first signal. Called between resumes.
    pub(super) fn deliver_signal(&mut self) {
        let Some(signal) = self.signals.pending() else {
            return;
        };
        let Some(root) = self.signals.root else {
            return;
        };
        self.signals.delivered = true;
        if let Some(timer) = self.signals.timer.take() {
            self.timers.cancel(timer);
        }
        let now = self.clock.now();
        self.emit_clock(ClockEvent {
            sequence: 0,
            kind: ClockEventKind::SignalReceived,
            task: Some(root),
            time: now,
            request_kind: u32::try_from(signal).unwrap_or(0),
        });
        if matches!(
            self.tasks.get(&root).map(|record| record.phase),
            Some(TaskPhase::Runnable | TaskPhase::Parked)
        ) {
            self.begin_terminal(root, TaskOutcome::Cancelled, false);
        }
    }

    /// True when a stalled real-clock run should wait for a signal instead
    /// of reporting a deadlock: a signal can still arrive and be delivered,
    /// and a running supervisor makes the program a service waiting for
    /// work rather than one stuck by mistake.
    pub(super) fn stall_waits_for_signal(&self) -> bool {
        !self.signals.delivered
            && self.signals.wake_fd.is_some()
            && self.signals.root.is_some()
            && self.has_running_supervisor()
    }

    /// Blocks until the runner's handler writes to the wake pipe.
    pub(super) fn wait_for_signal(&self) {
        if let Some(fd) = self.signals.wake_fd {
            wait_readable(fd, None);
        }
    }
}

/// Waits until `fd` is readable or `timeout` passes (forever when none),
/// then empties it. Returns early on any signal.
#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
pub(super) fn wait_readable(fd: i32, timeout: Option<std::time::Duration>) {
    let milliseconds = timeout.map_or(-1, |timeout| {
        // Round up so a wait never ends before its deadline.
        i32::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(i32::MAX)
    });
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    let ready = unsafe { libc::poll(&mut poll, 1, milliseconds) };
    if ready > 0 {
        let mut buffer = [0_u8; 64];
        // SAFETY: the runner opens the pipe non-blocking; this only drains it.
        while unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
    }
}

#[cfg(not(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
)))]
pub(super) fn wait_readable(_fd: i32, timeout: Option<std::time::Duration>) {
    if let Some(timeout) = timeout {
        std::thread::sleep(timeout);
    }
}
