//! `CHIMI_REQUEST_CHILD_WAIT`: a task waits until a child process the host
//! started has exited, without blocking the other tasks.
//!
//! The wait is level-triggered and consumes nothing, like an IO wait: the
//! scheduler only looks (`waitid` with `WNOWAIT`), so the child stays a
//! zombie until the host reaps it with its own `waitpid`. A task cancelled
//! between the wake and its reissue therefore loses no exit status, and
//! another task can still reap the child later.
//!
//! The scheduler checks every waiter after each idle wait and every
//! `IO_POLL_INTERVAL` resumes of a busy run; it wakes the ones whose child
//! has exited (or is not a child at all), and those reissue to get the
//! answer. The runner's SIGCHLD handler writes a byte to the wake pipe, so an
//! idle wait ends when a child exits. SIGCHLD carries no pid and coalesces,
//! so one byte checks them all.
//!
//! Invariants:
//! - A task has at most one child wait, and only while it is parked on the
//!   request. Any other request, a cancellation, or its end removes it.
//! - A child wait is a host operation in flight: virtual time does not
//!   advance while one exists, because a child runs in real time.
//! - An idle wait also ends by `recheck_interval`, so a SIGCHLD that never
//!   arrives (no handler installed, or another library's handler) delays a
//!   wake but never loses it. The interval starts at `FIRST_RECHECK` when a
//!   task parks and doubles after each recheck that wakes nobody, up to
//!   `MAX_REAL_WAIT`: a missed exit is found within about the time already
//!   waited, and a long wait costs about one `waitid` a second.

use super::{RESPONSE_ERROR, RESPONSE_OK, RESPONSE_PENDING, Scheduler, SchedulerRequest, TaskId};
use std::time::Duration;

/// The first idle recheck's interval after a task parks.
const FIRST_RECHECK: Duration = Duration::from_millis(10);

/// A task parked in `REQUEST_CHILD_WAIT`.
struct ChildWaiter {
    task: TaskId,
    pid: i32,
    /// The host resource the child belongs to (the request's `channel`), or
    /// 0: `REQUEST_IO_WITHDRAW` of that key wakes the waiter too.
    resource: u64,
}

#[derive(Default)]
pub(super) struct Children {
    /// Parked waiters, oldest first.
    waiters: Vec<ChildWaiter>,
    /// The next idle recheck's interval.
    recheck: Duration,
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
    /// unreaped child of this process; PENDING, woken at once and not
    /// registered, for a withdrawn key in `channel` (see reactor.rs).
    pub(super) fn handle_child_wait(&mut self, owner: TaskId, request: &mut SchedulerRequest) {
        let pid = i32::try_from(request.value).unwrap_or(-1);
        if pid > 0 && request.channel != 0 && self.reactor.is_withdrawn(request.channel) {
            request.status = RESPONSE_PENDING;
            let _ = self.wake_front(owner);
            return;
        }
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
                // Only carrier 0 waits for children.
                self.wake_carrier(super::ready::Carrier(0));
                self.children.waiters.push(ChildWaiter {
                    task: owner,
                    pid,
                    resource: request.channel,
                });
                self.children.recheck = FIRST_RECHECK;
                request.status = RESPONSE_PENDING;
            }
        }
    }

    /// Removes and returns, oldest first, the waiters whose wait names
    /// `resource` (nonzero), for `REQUEST_IO_WITHDRAW`.
    pub(super) fn take_child_waits_on(&mut self, resource: u64) -> Vec<TaskId> {
        let mut taken = Vec::new();
        self.children.waiters.retain(|waiter| {
            let named = waiter.resource == resource;
            if named {
                taken.push(waiter.task);
            }
            !named
        });
        taken
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

    /// Rechecks after an idle wait, and backs the next one off when it found
    /// no exit.
    pub(super) fn recheck_children_after_idle(&mut self) {
        let before = self.children.waiters.len();
        self.recheck_children();
        if before != 0 && self.children.waiters.len() == before {
            self.children.recheck = (self.children.recheck * 2).min(super::clock::MAX_REAL_WAIT);
        }
    }

    /// How long an idle wait may block before rechecking child waiters, or
    /// none while there are none.
    pub(super) fn recheck_interval(&self) -> Option<Duration> {
        (!self.children.is_empty()).then_some(self.children.recheck)
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
