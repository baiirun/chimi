//! `CHIMI_REQUEST_IO_WAIT`: a task waits until a file descriptor is ready
//! without blocking the other tasks, and an idle scheduler waits on every
//! such descriptor, the next timer deadline, and the signal pipe at once.
//!
//! The wait is level-triggered and consumes nothing, like `Select.first`:
//! the request answers OK with the ready conditions when the descriptor is
//! ready now, and otherwise registers the task and parks it. Readiness only
//! wakes the task; the host then asks again, and the fresh check answers.
//! The host does the read or write itself afterwards, so a descriptor that
//! turns out not to be ready (another reader drained it first) just waits
//! again.
//!
//! Invariants:
//! - A task has at most one registration, and only while it is parked on
//!   the request. Waking, cancelling, or withdrawing the task removes it.
//! - A registration is a host operation in flight: virtual time does not
//!   advance while one exists.
//! - Ready tasks wake in registration order.
//! - A ready reply wakes the task at the front of the run queue, so the
//!   host's read or write follows it before any other task runs. Another OS
//!   process sharing the descriptor can still take the data first; the host
//!   then waits again (or blocks, for a descriptor it does not own).
//! - A wait may name the resource its descriptor belongs to (`channel`, a
//!   nonzero key in the form below). `CHIMI_REQUEST_IO_WITHDRAW` wakes every wait
//!   on a key, so a host closing a resource wakes its waiters before the
//!   descriptor closes and its number can be reused. Closing a descriptor
//!   a task waits on without withdrawing first is a host bug: poll(2) only
//!   reports it while the number stays unused.
//! - A withdraw is sticky. A key is `slot << 32 | generation`, as the
//!   native runtime's `binding_resource_key` builds it, and a slot's
//!   generation only grows. Once a key is withdrawn, a wait (IO or child)
//!   on that slot at that generation or an older one is answered as a
//!   withdrawn parked wait would be, PENDING and woken at once, without
//!   registering. With several carriers a close on another carrier can land
//!   between a host's "is the handle open" check and its wait; the host
//!   then looks again and sees the handle closed. The answer comes before
//!   the descriptor is checked, so it holds even when the descriptor is
//!   ready (its number may already belong to another resource). The table
//!   keeps one entry per slot ever withdrawn, which the native runtime's
//!   handle limit bounds.
//! - A failed poll(2) (not an interruption) wakes every waiter with its
//!   parked wait answered ERROR, so a reactor that cannot wait never spins.
//!
//! Backend: poll(2) on macOS and Linux, one code path. It costs O(n) in the
//! registrations per idle wait, which is fine until servers hold thousands
//! of connections; `Reactor::wait` is the one place an epoll or kqueue
//! backend would go.

use super::{RESPONSE_ERROR, RESPONSE_OK, RESPONSE_PENDING, Scheduler, SchedulerRequest, TaskId};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

/// Interest and ready bit: reading will not block.
pub const IO_READABLE: u32 = 1;
/// Interest and ready bit: writing will not block.
pub const IO_WRITABLE: u32 = 2;
/// Ready bit: the peer hung up. Reading returns what is left, then end of
/// input.
pub const IO_HANGUP: u32 = 4;
/// Ready bit: the descriptor has a pending error; the next read or write
/// reports it.
pub const IO_ERROR: u32 = 8;

const INTEREST: u32 = IO_READABLE | IO_WRITABLE;

/// Identity of one registration, in registration order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct IoWaitId(u64);

#[derive(Clone, Copy, Debug)]
struct IoWait {
    task: TaskId,
    fd: i32,
    interest: u32,
    /// The host's resource key; 0 for none.
    resource: u64,
}

#[derive(Default)]
pub(super) struct Reactor {
    next: u64,
    waits: BTreeMap<IoWaitId, IoWait>,
    /// Per slot of a resource key, the newest generation withdrawn.
    withdrawn: HashMap<u32, u32>,
}

/// A resource key's slot and generation (see the module's invariants).
fn key_parts(resource: u64) -> (u32, u32) {
    ((resource >> 32) as u32, resource as u32)
}

/// What an idle or non-blocking wait found.
struct Woken {
    tasks: Vec<TaskId>,
    /// poll(2) failed: every registered task is in `tasks`.
    failed: bool,
}

impl Reactor {
    pub(super) fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    fn register(&mut self, task: TaskId, fd: i32, interest: u32, resource: u64) -> IoWaitId {
        self.next += 1;
        let id = IoWaitId(self.next);
        self.waits.insert(
            id,
            IoWait {
                task,
                fd,
                interest,
                resource,
            },
        );
        id
    }

    /// Remembers that `resource` (nonzero) was withdrawn.
    pub(super) fn record_withdraw(&mut self, resource: u64) {
        let (slot, generation) = key_parts(resource);
        let newest = self.withdrawn.entry(slot).or_insert(generation);
        *newest = (*newest).max(generation);
    }

    /// True when `resource` (nonzero) or a newer key of its slot was
    /// withdrawn.
    pub(super) fn is_withdrawn(&self, resource: u64) -> bool {
        let (slot, generation) = key_parts(resource);
        self.withdrawn.get(&slot).is_some_and(|newest| generation <= *newest)
    }

    /// The tasks waiting on `resource`, in registration order.
    fn waiting_on(&self, resource: u64) -> Vec<TaskId> {
        self.waits
            .values()
            .filter(|wait| wait.resource == resource)
            .map(|wait| wait.task)
            .collect()
    }

    fn remove(&mut self, id: IoWaitId) {
        self.waits.remove(&id);
    }

    /// The tasks whose descriptors are ready now, without blocking.
    fn ready_now(&self) -> Woken {
        self.wait(Some(Duration::ZERO), None)
    }

    /// Blocks until a registered descriptor is ready, `wake_fd` is readable,
    /// a signal interrupts the wait, or `timeout` passes (never when none).
    /// Returns the tasks whose descriptors are ready, in registration order,
    /// or every registered task when poll(2) failed.
    fn wait(&self, timeout: Option<Duration>, wake_fd: Option<i32>) -> Woken {
        let watched: Vec<(i32, u32)> = self
            .waits
            .values()
            .map(|wait| (wait.fd, wait.interest))
            .collect();
        let Some(ready) = sys::wait(&watched, timeout, wake_fd) else {
            return Woken {
                tasks: self.waits.values().map(|wait| wait.task).collect(),
                failed: true,
            };
        };
        Woken {
            tasks: self
                .waits
                .values()
                .zip(ready)
                .filter(|(_, ready)| *ready)
                .map(|(wait, _)| wait.task)
                .collect(),
            failed: false,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Makes the next idle or non-blocking wait fail as poll(2) would.
    pub(super) static FAIL_NEXT_POLL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What a zero-timeout check of one descriptor found.
enum Check {
    Ready(u32),
    Waiting,
    Invalid,
}

#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod sys {
    use super::{Check, IO_ERROR, IO_HANGUP, IO_READABLE, IO_WRITABLE};
    use std::time::Duration;

    pub(super) fn check(fd: i32, interest: u32) -> Check {
        let mut polls = [pollfd(fd, interest)];
        if poll(&mut polls, Some(Duration::ZERO)) <= 0 {
            return Check::Waiting;
        }
        let revents = polls[0].revents;
        if revents & libc::POLLNVAL != 0 {
            // macOS has answered POLLNVAL for some open devices; an open
            // descriptor poll cannot watch is treated as always ready.
            // SAFETY: F_GETFD only reads the descriptor table.
            return if unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0 {
                Check::Ready(interest)
            } else {
                Check::Invalid
            };
        }
        let mut ready = 0;
        for (event, bit) in [
            (libc::POLLIN, IO_READABLE),
            (libc::POLLOUT, IO_WRITABLE),
            (libc::POLLHUP, IO_HANGUP),
            (libc::POLLERR, IO_ERROR),
        ] {
            if revents & event != 0 {
                ready |= bit;
            }
        }
        Check::Ready(ready)
    }

    /// One flag per `watched` entry: true when it is ready (or invalid, so
    /// its waiter wakes and learns that from its reissue). `None` when
    /// poll(2) failed for a reason other than an interruption.
    pub(super) fn wait(
        watched: &[(i32, u32)],
        timeout: Option<Duration>,
        wake_fd: Option<i32>,
    ) -> Option<Vec<bool>> {
        let mut polls: Vec<libc::pollfd> = watched
            .iter()
            .map(|&(fd, interest)| pollfd(fd, interest))
            .collect();
        if let Some(fd) = wake_fd {
            polls.push(libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            });
        }
        #[cfg(test)]
        if super::FAIL_NEXT_POLL.with(|fail| fail.replace(false)) {
            return None;
        }
        // A timeout or EINTR leaves every revents zero: the caller fires due
        // timers and delivers the signal, then waits again.
        // EAGAIN is transient (macOS documents it for poll); retry as EINTR.
        if poll(&mut polls, timeout) < 0
            && !matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EINTR | libc::EAGAIN)
            )
        {
            return None;
        }
        if let Some(fd) = wake_fd
            && polls.last().is_some_and(|poll| poll.revents != 0)
        {
            crate::scheduler::signal::drain(fd);
        }
        Some(
            polls[..watched.len()]
                .iter()
                .map(|poll| poll.revents != 0)
                .collect(),
        )
    }

    fn pollfd(fd: i32, interest: u32) -> libc::pollfd {
        let mut events = 0;
        if interest & IO_READABLE != 0 {
            events |= libc::POLLIN;
        }
        if interest & IO_WRITABLE != 0 {
            events |= libc::POLLOUT;
        }
        libc::pollfd {
            fd,
            events,
            revents: 0,
        }
    }

    /// poll(2) with the timeout rounded up to whole milliseconds, so a wait
    /// never ends before its deadline.
    fn poll(polls: &mut [libc::pollfd], timeout: Option<Duration>) -> i32 {
        let milliseconds = timeout.map_or(-1, |timeout| {
            i32::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(i32::MAX)
        });
        // SAFETY: `polls` is a valid, exclusively borrowed array for the call.
        unsafe {
            libc::poll(
                polls.as_mut_ptr(),
                polls.len() as libc::nfds_t,
                milliseconds,
            )
        }
    }
}

#[cfg(not(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
)))]
mod sys {
    use super::Check;
    use std::time::Duration;

    /// Without poll(2) every descriptor counts as ready, so the host's read
    /// or write blocks as it did before the reactor, and nothing registers.
    pub(super) fn check(_fd: i32, interest: u32) -> Check {
        Check::Ready(interest)
    }

    pub(super) fn wait(
        watched: &[(i32, u32)],
        _timeout: Option<Duration>,
        _wake_fd: Option<i32>,
    ) -> Option<Vec<bool>> {
        Some(vec![true; watched.len()])
    }
}

impl Scheduler {
    /// Serves an IO wait: `value` is the descriptor, `token` the interest,
    /// `channel` the resource key or 0. Ensures: OK with the ready
    /// conditions in `value`; PENDING with the task registered and parked;
    /// ERROR for a closed or negative descriptor or an interest outside
    /// `IO_READABLE | IO_WRITABLE`; a parked wait a failed poll ends answers
    /// ERROR too; PENDING, woken at once and not registered, for a withdrawn
    /// key.
    pub(super) fn handle_io_wait(&mut self, owner: TaskId, request: &mut SchedulerRequest) {
        let fd = i32::try_from(request.value).unwrap_or(-1);
        let interest = u32::try_from(request.token).unwrap_or(0);
        if fd < 0
            || Some(fd) == self.signals.wake_fd()
            || interest == 0
            || interest & !INTEREST != 0
        {
            request.status = RESPONSE_ERROR;
            let _ = self.wake_front(owner);
            return;
        }
        if request.channel != 0 && self.reactor.is_withdrawn(request.channel) {
            request.status = RESPONSE_PENDING;
            let _ = self.wake_front(owner);
            return;
        }
        match sys::check(fd, interest) {
            Check::Ready(ready) => {
                request.value = i64::from(ready);
                request.status = RESPONSE_OK;
                let _ = self.wake_front(owner);
            }
            Check::Invalid => {
                request.status = RESPONSE_ERROR;
                let _ = self.wake_front(owner);
            }
            Check::Waiting => {
                let id = self.reactor.register(owner, fd, interest, request.channel);
                self.tasks.get_mut(&owner).expect("waiting task").io_wait = Some(id);
                request.status = RESPONSE_PENDING;
            }
        }
    }

    /// Serves an IO withdraw: wakes every task waiting on the resource key in
    /// `channel` (nonzero), so each reissues its wait, and makes the
    /// withdraw sticky for that key and older ones of its slot. Ensures: OK, with the
    /// number of tasks woken in `value`; ERROR for key 0. Never parks.
    pub(super) fn handle_io_withdraw(&mut self, owner: TaskId, request: &mut SchedulerRequest) {
        if request.channel == 0 {
            request.status = RESPONSE_ERROR;
        } else {
            self.reactor.record_withdraw(request.channel);
            let waiting = self.reactor.waiting_on(request.channel);
            let children = self.take_child_waits_on(request.channel);
            request.value = i64::try_from(waiting.len() + children.len()).unwrap_or(i64::MAX);
            for task in waiting {
                self.clear_io_wait(task);
                let _ = self.wake(task);
            }
            for task in children {
                let _ = self.wake(task);
            }
            request.status = RESPONSE_OK;
        }
        let _ = self.wake_front(owner);
    }

    /// Wakes the tasks `woken` names; after a failed poll each one's parked
    /// wait answers ERROR instead of PENDING, so the host stops waiting.
    fn wake_io(&mut self, woken: Woken) {
        for task in woken.tasks {
            if woken.failed
                && let Some(record) = self.tasks.get(&task)
                && !record.request.is_null()
            {
                // SAFETY: a registration exists only while its task is parked
                // in an IO wait, whose request outlives the park.
                unsafe { (*record.request.cast::<super::RequestHeader>()).status = RESPONSE_ERROR };
            }
            self.clear_io_wait(task);
            let _ = self.wake(task);
        }
    }

    /// Removes the task's registration, if it has one.
    pub(super) fn clear_io_wait(&mut self, task: TaskId) {
        if let Some(id) = self
            .tasks
            .get_mut(&task)
            .and_then(|record| record.io_wait.take())
        {
            self.reactor.remove(id);
        }
    }

    /// The idle wait while a registration or a child wait exists, prepared
    /// under the kernel lock so the wait itself can run without it. Under
    /// the real clock it ends by the earliest timer deadline or after
    /// `MAX_REAL_WAIT`; under the virtual clock time is held, so it waits for
    /// a descriptor or a signal only. Either way it ends by the child
    /// waiters' `recheck_interval`. `finish_io_wait` then wakes the ready
    /// tasks and the child waiters whose child exited, and fires the timers
    /// due by now, which under the virtual clock are only those at the
    /// current instant.
    pub(super) fn prepare_io_wait(&self) -> IoWaitPlan {
        let timeout = match self.clock.mode() {
            // A timer already due fires without moving time, so it must not
            // wait behind the descriptors.
            super::ClockMode::Virtual => self
                .timers
                .earliest()
                .filter(|deadline| *deadline <= self.clock.now())
                .map(|_| Duration::ZERO),
            super::ClockMode::Real => self
                .timers
                .earliest()
                .map(|deadline| super::clock::real_wait(self.clock.now(), deadline)),
        };
        let timeout = match (timeout, self.recheck_interval()) {
            (Some(timeout), Some(recheck)) => Some(timeout.min(recheck)),
            (timeout, recheck) => timeout.or(recheck),
        };
        let (ids, watched) = self
            .reactor
            .waits
            .iter()
            .map(|(id, wait)| (*id, (wait.fd, wait.interest)))
            .unzip();
        IoWaitPlan {
            ids,
            watched,
            timeout,
            wake_fd: self.signals.wake_fd(),
        }
    }

    /// Applies what `plan`'s wait found. A registration withdrawn while the
    /// carrier waited is skipped: its task is no longer waiting on it.
    pub(super) fn finish_io_wait(&mut self, plan: IoWaitPlan, ready: Option<Vec<bool>>) {
        let still = |reactor: &Reactor, id: &IoWaitId| reactor.waits.contains_key(id);
        let woken = match ready {
            None => Woken {
                tasks: plan
                    .ids
                    .iter()
                    .filter(|id| still(&self.reactor, id))
                    .map(|id| self.reactor.waits[id].task)
                    .collect(),
                failed: true,
            },
            Some(ready) => Woken {
                tasks: plan
                    .ids
                    .iter()
                    .zip(ready)
                    .filter(|(id, ready)| *ready && still(&self.reactor, id))
                    .map(|(id, _)| self.reactor.waits[id].task)
                    .collect(),
                failed: false,
            },
        };
        self.wake_io(woken);
        self.recheck_children_after_idle();
        self.fire_due_timers();
    }

    /// Wakes the tasks whose descriptors are ready now, so a program that
    /// always has runnable work still serves its IO. `drive` calls it every
    /// `IO_POLL_INTERVAL` resumes while a registration exists.
    pub(super) fn poll_io_now(&mut self) {
        let woken = self.reactor.ready_now();
        self.wake_io(woken);
    }
}

/// A descriptor wait prepared under the kernel lock: the registrations it
/// watches, in registration order, and how long it may block.
pub(super) struct IoWaitPlan {
    ids: Vec<IoWaitId>,
    watched: Vec<(i32, u32)>,
    timeout: Option<Duration>,
    wake_fd: Option<i32>,
}

impl IoWaitPlan {
    /// Blocks in poll(2) without the kernel. Returns which watched
    /// descriptors are ready, or None when poll(2) failed.
    pub(super) fn block(&self) -> Option<Vec<bool>> {
        sys::wait(&self.watched, self.timeout, self.wake_fd)
    }
}

/// Resumes between two non-blocking reactor polls while some task waits on
/// IO and others keep running.
pub(super) const IO_POLL_INTERVAL: u32 = 64;
