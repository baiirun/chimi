//! The execution time source and the scheduler's timer queue.
//!
//! Invariants:
//! - `Clock::now` never decreases.
//! - Virtual time moves only through `Clock::wait_until`, which the scheduler
//!   calls only when no fiber is runnable and no host operation is in flight.
//! - Timers with the same deadline fire in registration order.

use super::TaskId;
use super::process::IncarnationId;
use std::collections::BTreeMap;
use std::time::Duration;

/// A point on the execution's monotonic time source: signed nanoseconds since
/// the origin chosen when the scheduler was created.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Instant(i64);

impl Instant {
    pub const ORIGIN: Self = Self(0);
    /// The end of the range. A deadline here is never reached, so a scope
    /// entered with it has no timer (a saturated deadline means the same).
    pub const NEVER: Self = Self(i64::MAX);

    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// Deadlines saturate at the end of the representable range, which no
    /// execution reaches.
    pub fn saturating_add(self, duration: Duration) -> Self {
        let nanos = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
        Self(self.0.saturating_add(nanos))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockMode {
    /// The host monotonic clock. An idle scheduler sleeps until its earliest
    /// timer.
    Real,
    /// Deterministic time owned by the scheduler. It jumps to the earliest
    /// timer when nothing can run.
    Virtual,
}

pub(super) enum Clock {
    Real { origin: std::time::Instant },
    Virtual { now: Instant },
}

impl Clock {
    pub(super) fn new(mode: ClockMode) -> Self {
        match mode {
            ClockMode::Real => Self::Real {
                origin: std::time::Instant::now(),
            },
            ClockMode::Virtual => Self::Virtual {
                now: Instant::ORIGIN,
            },
        }
    }

    pub(super) fn mode(&self) -> ClockMode {
        match self {
            Self::Real { .. } => ClockMode::Real,
            Self::Virtual { .. } => ClockMode::Virtual,
        }
    }

    pub(super) fn now(&self) -> Instant {
        match self {
            Self::Real { origin } => {
                Instant(i64::try_from(origin.elapsed().as_nanos()).unwrap_or(i64::MAX))
            }
            Self::Virtual { now } => *now,
        }
    }

    /// Waits until `deadline`: virtual time jumps there (never backwards) and
    /// real time sleeps the thread. Returns true when virtual time moved.
    /// Under the real clock a signal written to `wake_fd` ends the wait
    /// early.
    pub(super) fn wait_until(&mut self, deadline: Instant, wake_fd: Option<i32>) -> bool {
        match self {
            Self::Real { .. } => {
                let now = self.now();
                if deadline > now {
                    let wait = Duration::from_nanos((deadline.0 - now.0) as u64);
                    match wake_fd {
                        Some(fd) => super::signal::wait_readable(fd, Some(wait)),
                        None => std::thread::sleep(wait),
                    }
                }
                false
            }
            Self::Virtual { now } => {
                if deadline > *now {
                    *now = deadline;
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// Identity of one registered timer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TimerId(u64);

impl TimerId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What a timer does when it fires.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TimerTarget {
    /// Wakes a task parked in a sleep.
    Wake(TaskId),
    /// Forces an incarnation whose cleanup has not finished by its deadline.
    ShutdownDeadline(IncarnationId),
    /// Delivers an injected signal (`Scheduler::inject_signal_at`).
    Signal(i32),
    /// Fires a task's cancellation scope.
    ScopeDeadline(TaskId, super::ScopeId),
}

#[derive(Default)]
pub(super) struct TimerQueue {
    next: u64,
    by_deadline: BTreeMap<(Instant, TimerId), TimerTarget>,
    deadlines: BTreeMap<TimerId, Instant>,
}

impl TimerQueue {
    pub(super) fn insert(&mut self, deadline: Instant, target: TimerTarget) -> TimerId {
        self.next += 1;
        let id = TimerId(self.next);
        self.by_deadline.insert((deadline, id), target);
        self.deadlines.insert(id, deadline);
        id
    }

    pub(super) fn cancel(&mut self, id: TimerId) -> Option<TimerTarget> {
        let deadline = self.deadlines.remove(&id)?;
        self.by_deadline.remove(&(deadline, id))
    }

    pub(super) fn earliest(&self) -> Option<Instant> {
        self.by_deadline
            .keys()
            .next()
            .map(|(deadline, _)| *deadline)
    }

    /// Removes and returns the earliest timer whose deadline is at or before
    /// `now`.
    pub(super) fn pop_due(&mut self, now: Instant) -> Option<(TimerId, Instant, TimerTarget)> {
        let (&(deadline, id), _) = self.by_deadline.first_key_value()?;
        if deadline > now {
            return None;
        }
        let target = self.by_deadline.remove(&(deadline, id))?;
        self.deadlines.remove(&id);
        Some((id, deadline, target))
    }

    pub(super) fn len(&self) -> usize {
        self.deadlines.len()
    }
}
