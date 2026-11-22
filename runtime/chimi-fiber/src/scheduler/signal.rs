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
//!
//! `std/signal`'s signals (SIGHUP, SIGUSR1, SIGUSR2) take a separate path
//! and never stop the program. The runner's handler for them sets a bit in a
//! second word and writes to the same pipe. Between resumes the scheduler
//! moves those bits into its latch, one pending bit per signal, so repeats
//! coalesce as the kernel's own do. A task waits with `REQUEST_SIGNAL_WAIT`;
//! the latch wakes the oldest waiter that wants a pending signal, one waiter
//! per signal, and the bit is cleared only when a waiter's reissued request
//! takes it, so a waiter cancelled before it runs again loses nothing. The
//! wait is not a host operation, so it never holds virtual time.

use std::sync::atomic::{AtomicU32, Ordering};

use super::clock::{Instant, TimerId, TimerTarget};
use super::{
    ClockEvent, ClockEventKind, RESPONSE_ERROR, RESPONSE_OK, RESPONSE_PENDING, Scheduler,
    SchedulerRequest, TaskId, TaskOutcome,
};

/// The `std/signal` signal codes, 1 to `USER_SIGNAL_CODES`; code `n` is
/// latch bit `n - 1`.
const USER_SIGNAL_CODES: u32 = 3;
/// The latch bits of the `std/signal` codes.
const USER_SIGNAL_MASK: u32 = (1 << USER_SIGNAL_CODES) - 1;

/// A task parked in `REQUEST_SIGNAL_WAIT`.
struct SignalWaiter {
    task: TaskId,
    /// The latch bits it wants.
    mask: u32,
    /// The pending bit it was woken for and has not yet taken. That bit is
    /// reserved for it: no other waiter is woken for it and no fresh wait
    /// takes it, until this task's reissued wait takes a bit or it leaves.
    woken_for: Option<u32>,
}

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
    /// The runner's `std/signal` word: bit `code - 1` set by its handler.
    user_word: Option<*mut u32>,
    /// The pending `std/signal` signals, one bit per code.
    latch: u32,
    /// Parked waiters, oldest first.
    waiters: Vec<SignalWaiter>,
    /// Injected `std/signal` signals' timers, while pending.
    user_timers: Vec<TimerId>,
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
        self.signals.timer = Some(self.insert_timer(at, TimerTarget::Signal(signal)));
    }

    /// Connects the runner's `std/signal` handler: it sets bit `code - 1` of
    /// `word` and writes to the wake pipe. Null for none.
    pub fn set_user_signal_source(&mut self, word: *mut u32) {
        self.signals.user_word = (!word.is_null()).then_some(word);
    }

    /// Latches the `std/signal` signal `code` (1 SIGHUP, 2 SIGUSR1, 3
    /// SIGUSR2) when the execution clock reaches `at`. Other codes are
    /// ignored.
    pub fn inject_user_signal_at(&mut self, at: Instant, code: u32) {
        if (1..=USER_SIGNAL_CODES).contains(&code) {
            let timer = self.insert_timer(at, TimerTarget::UserSignal(code));
            self.signals.user_timers.push(timer);
        }
    }

    /// A fired timer's id stays in `user_timers` until the root ends;
    /// cancelling it then does nothing.
    pub(super) fn fire_user_signal(&mut self, code: u32) {
        self.latch_user_signals(1 << (code - 1));
    }

    /// Moves the bits the runner's handler set into the latch. Called
    /// between resumes.
    pub(super) fn collect_user_signals(&mut self) {
        let Some(word) = self.signals.user_word else {
            return;
        };
        // SAFETY: the runner keeps the word valid and 4-byte aligned while
        // the scheduler runs; its handler sets bits with an atomic OR.
        let bits = unsafe { AtomicU32::from_ptr(word) }.swap(0, Ordering::SeqCst);
        if bits != 0 {
            self.latch_user_signals(bits);
        }
    }

    fn latch_user_signals(&mut self, bits: u32) {
        let bits = bits & USER_SIGNAL_MASK;
        let now = self.clock.now();
        for code in 1..=USER_SIGNAL_CODES {
            if bits & (1 << (code - 1)) != 0 {
                self.emit_clock(ClockEvent {
                    sequence: 0,
                    kind: ClockEventKind::SignalReceived,
                    task: None,
                    time: now,
                    request_kind: signal_number(code),
                });
            }
        }
        self.signals.latch |= bits;
        self.dispatch_user_signals();
    }

    /// Wakes, for each pending bit no woken waiter holds, the oldest waiter
    /// that wants it.
    fn dispatch_user_signals(&mut self) {
        let mut woken = Vec::new();
        for bit in (0..USER_SIGNAL_CODES).map(|index| 1_u32 << index) {
            if self.signals.latch & bit == 0 || self.reserved_bits(None) & bit != 0 {
                continue;
            }
            if let Some(waiter) = self
                .signals
                .waiters
                .iter_mut()
                .find(|waiter| waiter.woken_for.is_none() && waiter.mask & bit != 0)
            {
                waiter.woken_for = Some(bit);
                woken.push(waiter.task);
            }
        }
        for task in woken {
            let _ = self.wake(task);
        }
    }

    /// The bits reserved for woken waiters other than `except`.
    fn reserved_bits(&self, except: Option<TaskId>) -> u32 {
        self.signals
            .waiters
            .iter()
            .filter(|waiter| Some(waiter.task) != except)
            .filter_map(|waiter| waiter.woken_for)
            .fold(0, |bits, bit| bits | bit)
    }

    /// Serves a signal wait: takes the first listed code that is pending and
    /// not reserved for another woken waiter, or parks the task. A task
    /// reissuing after a wake keeps its place among the waiters; a new one
    /// is the newest. See `REQUEST_SIGNAL_WAIT`.
    pub(super) fn handle_signal_wait(&mut self, owner: TaskId, request: &mut SchedulerRequest) {
        self.collect_user_signals();
        let Some(codes) = wait_codes(request.value) else {
            request.status = RESPONSE_ERROR;
            let _ = self.wake_front(owner);
            self.withdraw_signal_wait(owner);
            return;
        };
        let available = self.signals.latch & !self.reserved_bits(Some(owner));
        if let Some(code) = codes
            .iter()
            .copied()
            .find(|code| available & (1 << (code - 1)) != 0)
        {
            self.signals.latch &= !(1 << (code - 1));
            request.value = i64::from(code);
            request.status = RESPONSE_OK;
            let _ = self.wake_front(owner);
            // A bit this task was woken for but did not take is free for
            // the next waiter.
            self.withdraw_signal_wait(owner);
            return;
        }
        let mask = codes.iter().fold(0, |mask, code| mask | 1 << (code - 1));
        if let Some(waiter) = self
            .signals
            .waiters
            .iter_mut()
            .find(|waiter| waiter.task == owner)
        {
            waiter.mask = mask;
            waiter.woken_for = None;
        } else {
            // Carrier 0 watches the signal pipe.
            self.wake_carrier(super::ready::Carrier(0));
            self.signals.waiters.push(SignalWaiter {
                task: owner,
                mask,
                woken_for: None,
            });
        }
        request.status = RESPONSE_PENDING;
        self.dispatch_user_signals();
    }

    /// Removes the wait of a task that leaves it: it asks for something else,
    /// is refused, is cancelled, or ends. A bit it was woken for goes to the
    /// next waiter.
    pub(super) fn withdraw_signal_wait(&mut self, task: TaskId) {
        let before = self.signals.waiters.len();
        self.signals.waiters.retain(|waiter| waiter.task != task);
        if self.signals.waiters.len() != before {
            self.dispatch_user_signals();
        }
    }

    /// True when a task is parked waiting for a `std/signal` signal.
    pub(super) fn has_signal_waiters(&self) -> bool {
        !self.signals.waiters.is_empty()
    }

    /// The task a delivered signal cancels.
    pub(super) fn set_signal_root(&mut self, root: TaskId) {
        self.signals.root = Some(root);
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
        if self.retire_signals_after_root() {
            return;
        }
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
        // Cancelled like any task (scope-owned-resources.md): a `main` inside
        // `Resource.scoped` runs its releases masked before it ends. The
        // runner ends the program at once on a second signal.
        self.cancel_task(root, TaskOutcome::Cancelled);
    }

    /// True when a stalled real-clock run should wait for a signal instead
    /// of reporting a deadlock: a signal can still arrive and be delivered,
    /// and a running supervisor makes the program a service waiting for
    /// work rather than one stuck by mistake.
    /// A task waiting in `REQUEST_SIGNAL_WAIT` is waiting for one too.
    pub(super) fn stall_waits_for_signal(&self) -> bool {
        self.signals.wake_fd.is_some()
            && self.signals.root.is_some()
            && ((!self.signals.delivered && self.has_running_supervisor())
                || (self.signals.user_word.is_some() && self.has_signal_waiters()))
    }

    /// Once the root has ended, no signal is delivered: a pending injected
    /// signal is retired so it never moves virtual time, and the runner
    /// reports the root's own outcome. Returns true when signals are over.
    fn retire_signals_after_root(&mut self) -> bool {
        let Some(root) = self.signals.root else {
            return self.signals.delivered;
        };
        if self
            .tasks
            .get(&root)
            .is_some_and(|record| record.phase.is_live())
        {
            return self.signals.delivered;
        }
        for timer in std::mem::take(&mut self.signals.user_timers) {
            self.timers.cancel(timer);
        }
        if self.signals.delivered {
            return true;
        }
        self.signals.delivered = true;
        if let Some(timer) = self.signals.timer.take() {
            self.timers.cancel(timer);
        }
        true
    }
}

/// The codes of a signal wait's `value`, or none when it lists none or a
/// code outside 1 to `USER_SIGNAL_CODES`.
fn wait_codes(value: i64) -> Option<Vec<u32>> {
    let bytes = value.to_le_bytes();
    let count = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let (codes, rest) = bytes.split_at(count);
    let valid = |code: &u8| (1..=USER_SIGNAL_CODES).contains(&u32::from(*code));
    if codes.is_empty() || rest.iter().any(|byte| *byte != 0) || !codes.iter().all(valid) {
        return None;
    }
    Some(codes.iter().map(|code| u32::from(*code)).collect())
}

/// The operating system's number for `std/signal` code `code`, for traces.
#[cfg(unix)]
fn signal_number(code: u32) -> u32 {
    let number = match code {
        1 => libc::SIGHUP,
        2 => libc::SIGUSR1,
        _ => libc::SIGUSR2,
    };
    u32::try_from(number).unwrap_or(0)
}

#[cfg(not(unix))]
fn signal_number(code: u32) -> u32 {
    code
}

/// Waits until one of `fds` is readable or `timeout` passes (forever when
/// none), then empties the readable ones. Returns early on any signal.
#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
pub(super) fn wait_readable(fds: &[i32], timeout: Option<std::time::Duration>) {
    let milliseconds = timeout.map_or(-1, |timeout| {
        // Round up so a wait never ends before its deadline.
        i32::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(i32::MAX)
    });
    let mut polls = [libc::pollfd {
        fd: -1,
        events: libc::POLLIN,
        revents: 0,
    }; 2];
    assert!(
        fds.len() <= polls.len(),
        "at most the signal and wake pipes"
    );
    let polls = &mut polls[..fds.len()];
    for (poll, &fd) in polls.iter_mut().zip(fds) {
        poll.fd = fd;
    }
    // SAFETY: `polls` is a valid, exclusively borrowed array for the call.
    let ready = unsafe {
        libc::poll(
            polls.as_mut_ptr(),
            polls.len() as libc::nfds_t,
            milliseconds,
        )
    };
    if ready > 0 {
        for poll in polls.iter().filter(|poll| poll.revents != 0) {
            drain(poll.fd);
        }
    }
}

/// Empties a wake pipe, so the next wait blocks until a new byte.
#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
pub(super) fn drain(fd: i32) {
    let mut buffer = [0_u8; 64];
    // SAFETY: the runner opens the pipe non-blocking; this only drains it.
    while unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
}

#[cfg(not(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
)))]
pub(super) fn wait_readable(_fds: &[i32], timeout: Option<std::time::Duration>) {
    if let Some(timeout) = timeout {
        std::thread::sleep(timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::wait_codes;

    #[test]
    fn a_wait_lists_codes_one_per_byte_up_to_a_zero() {
        assert_eq!(wait_codes(0x01_02_03), Some(vec![3, 2, 1]));
        assert_eq!(wait_codes(0x02), Some(vec![2]));
        assert_eq!(wait_codes(0), None, "an empty list");
        assert_eq!(wait_codes(0x04), None, "a code past SIGUSR2");
        assert_eq!(wait_codes(0x01_00_01), None, "a code after the terminator");
    }
}
