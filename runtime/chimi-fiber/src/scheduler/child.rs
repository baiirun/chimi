//! `CHIMI_REQUEST_CHILD_WAIT`: a task waits until a child process the host
//! started has exited, without blocking the other tasks.
//!
//! The wait is level-triggered and consumes nothing, like an IO wait: the
//! scheduler only looks (`waitid` with `WNOWAIT`), so the child stays a
//! zombie until the host reaps it with its own `waitpid`. A task cancelled
//! between the wake and its reissue therefore loses no exit status, and
//! another task can still reap the child later.
//!
//! The runner's SIGCHLD handler sets `CHILD_EXITED_SIGNAL_BIT` in the
//! `std/signal` word and writes to the wake pipe. Between resumes the
//! scheduler takes that bit and checks every waiter; it wakes the ones whose
//! child has exited (or is not a child at all), and those reissue to get the
//! answer. SIGCHLD carries no pid and coalesces, so one bit checks them all.
//!
//! Invariants:
//! - A task has at most one child wait, and only while it is parked on the
//!   request. Any other request, a cancellation, or its end removes it.
//! - A child wait is a host operation in flight: virtual time does not
//!   advance while one exists, because a child runs in real time.
//! - The idle wait rechecks the waiters at least every `recheck_interval`,
//!   so a SIGCHLD that never arrives (no handler installed, or another
//!   library reaped first) delays a wake but never loses it.

use super::{RESPONSE_ERROR, RESPONSE_OK, RESPONSE_PENDING, Scheduler, SchedulerRequest, TaskId};
use std::time::Duration;

/// The bit the runner's SIGCHLD handler sets in the `std/signal` word. It is
/// above the `std/signal` codes, so it never reaches their latch.
pub const CHILD_EXITED_SIGNAL_BIT: u32 = 1 << 8;

/// The longest an idle scheduler waits before rechecking child waiters when
/// no SIGCHLD source is connected.
const UNSIGNALED_RECHECK: Duration = Duration::from_millis(10);

/// A task parked in `REQUEST_CHILD_WAIT`.
struct ChildWaiter {
    task: TaskId,
    pid: i32,
}

#[derive(Default)]
pub(super) struct Children {
    /// Parked waiters, oldest first.
    waiters: Vec<ChildWaiter>,
}

impl Children {
    pub(super) fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }
}

/// What a check of one child found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Check {
    /// It has exited and waits to be reaped.
    Exited,
    Running,
    /// It is not a child of this process (or was already reaped).
    Unknown,
}

impl Scheduler {
    /// Serves a child wait: `value` is the pid. Ensures: OK when the child
    /// has exited, left unreaped for the host; PENDING with the task parked
    /// while it runs; ERROR for a pid that is not positive or not an
    /// unreaped child of this process.
    pub(super) fn handle_child_wait(&mut self, owner: TaskId, request: &mut SchedulerRequest) {
        let pid = i32::try_from(request.value).unwrap_or(-1);
        let check = if pid > 0 {
            sys::check(pid)
        } else {
            Check::Unknown
        };
        match check {
            Check::Exited => {
                request.status = RESPONSE_OK;
                let _ = self.wake_front(owner);
            }
            Check::Unknown => {
                request.status = RESPONSE_ERROR;
                let _ = self.wake_front(owner);
            }
            Check::Running => {
                self.children.waiters.push(ChildWaiter { task: owner, pid });
                request.status = RESPONSE_PENDING;
            }
        }
    }

    /// Removes the task's child wait, if it has one.
    pub(super) fn clear_child_wait(&mut self, task: TaskId) {
        self.children.waiters.retain(|waiter| waiter.task != task);
    }

    /// Wakes, oldest first, every waiter whose child is no longer running;
    /// each reissues its wait for the answer.
    pub(super) fn recheck_children(&mut self) {
        if self.children.is_empty() {
            return;
        }
        let mut woken = Vec::new();
        self.children.waiters.retain(|waiter| {
            let done = sys::check(waiter.pid) != Check::Running;
            if done {
                woken.push(waiter.task);
            }
            !done
        });
        for task in woken {
            let _ = self.wake(task);
        }
    }

    /// How long an idle wait may block before rechecking child waiters:
    /// with a SIGCHLD source the handler's pipe write ends the wait, so the
    /// recheck is only a backstop.
    pub(super) fn recheck_interval(&self) -> Option<Duration> {
        if self.children.is_empty() {
            None
        } else if self.signals.has_user_source() {
            Some(super::clock::MAX_REAL_WAIT)
        } else {
            Some(UNSIGNALED_RECHECK)
        }
    }
}

#[cfg(unix)]
mod sys {
    use super::Check;

    /// Looks at `pid` without reaping it.
    pub(super) fn check(pid: i32) -> Check {
        loop {
            // SAFETY: an all-zero siginfo_t is valid; waitid fills it.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is valid and exclusively borrowed for the call.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Check::Unknown;
            }
            // With WNOHANG and no exited child, waitid leaves si_pid zero.
            // SAFETY: waitid succeeded, so `info` holds a SIGCHLD record.
            return if unsafe { info.si_pid() } == 0 {
                Check::Running
            } else {
                Check::Exited
            };
        }
    }
}

#[cfg(not(unix))]
mod sys {
    use super::Check;

    pub(super) fn check(_pid: i32) -> Check {
        Check::Unknown
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Check, sys};
    use std::process::Command;

    #[test]
    fn a_check_sees_a_running_child_then_its_exit_without_reaping_it() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let pid = i32::try_from(child.id()).expect("pid");
        assert_eq!(sys::check(pid), Check::Running);
        drop(child.stdin.take());
        while sys::check(pid) == Check::Running {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(sys::check(pid), Check::Exited, "a check does not reap");
        assert!(child.wait().expect("reap").code().is_some());
        assert_eq!(sys::check(pid), Check::Unknown, "a reaped child is gone");
    }
}
