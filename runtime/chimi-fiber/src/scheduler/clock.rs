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

/// The longest single real-clock sleep. The host's sleep timers do not count
/// time it spends suspended, but `Clock::now` does, so the scheduler wakes at
/// least this often while a timer is pending and a deadline that passed
/// during a suspend fires at most this late after the host resumes.
pub(super) const MAX_REAL_WAIT: Duration = Duration::from_secs(1);

pub(super) enum Clock {
    Real { origin: i64 },
    Virtual { now: Instant },
}

impl Clock {
    pub(super) fn new(mode: ClockMode) -> Self {
        match mode {
            ClockMode::Real => Self::Real {
                origin: host_monotonic_nanos(),
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
            Self::Real { origin } => Instant(host_monotonic_nanos().saturating_sub(*origin)),
            Self::Virtual { now } => *now,
        }
    }

    /// Waits until `deadline`: virtual time jumps there (never backwards) and
    /// real time sleeps the thread for at most `MAX_REAL_WAIT`, so callers
    /// loop until the deadline is due. Returns true when virtual time moved.
    /// Under the real clock a signal written to `wake_fd` ends the wait
    /// early.
    pub(super) fn wait_until(&mut self, deadline: Instant, wake_fd: Option<i32>) -> bool {
        match self {
            Self::Real { .. } => {
                let wait = real_wait(self.now(), deadline);
                if !wait.is_zero() {
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

/// How long one real-clock sleep toward `deadline` lasts: the time left,
/// capped at `MAX_REAL_WAIT`, and zero once it is due.
pub(super) fn real_wait(now: Instant, deadline: Instant) -> Duration {
    let left = u64::try_from(deadline.0.saturating_sub(now.0)).unwrap_or(0);
    Duration::from_nanos(left).min(MAX_REAL_WAIT)
}

/// Nanoseconds on a host clock that never decreases and keeps counting while
/// the host is suspended: `CLOCK_BOOTTIME` on Linux, and on macOS
/// `CLOCK_MONOTONIC`, which reads `mach_continuous_time`.
#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
fn host_monotonic_nanos() -> i64 {
    #[cfg(target_os = "linux")]
    const SOURCE: libc::clockid_t = libc::CLOCK_BOOTTIME;
    #[cfg(target_os = "macos")]
    const SOURCE: libc::clockid_t = libc::CLOCK_MONOTONIC;
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid timespec for the duration of the call.
    let status = unsafe { libc::clock_gettime(SOURCE, &mut time) };
    assert_eq!(status, 0, "the host monotonic clock is unavailable");
    timespec_nanos(time.tv_sec, time.tv_nsec)
}

fn timespec_nanos(seconds: i64, nanoseconds: i64) -> i64 {
    seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanoseconds)
}

/// Targets without the suspend-counting clocks fall back to the standard
/// monotonic clock, measured from the first reading in the process.
#[cfg(not(all(
    unix,
    any(target_os = "macos", target_os = "linux"),
    any(target_arch = "aarch64", target_arch = "x86_64")
)))]
fn host_monotonic_nanos() -> i64 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let base = BASE.get_or_init(std::time::Instant::now);
    i64::try_from(base.elapsed().as_nanos()).unwrap_or(i64::MAX)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_wait_is_capped_and_ends_once_due() {
        let now = Instant::from_nanos(1_000);
        assert_eq!(
            real_wait(now, Instant::from_nanos(1_500)),
            Duration::from_nanos(500)
        );
        assert_eq!(real_wait(now, Instant::NEVER), MAX_REAL_WAIT);
        assert_eq!(real_wait(now, now), Duration::ZERO);
        assert_eq!(real_wait(now, Instant::from_nanos(-5)), Duration::ZERO);
    }

    #[test]
    fn a_host_reading_counts_whole_seconds_and_nanoseconds() {
        assert_eq!(timespec_nanos(3, 250), 3_000_000_250);
        assert_eq!(timespec_nanos(i64::MAX, 1), i64::MAX);
    }

    #[test]
    fn the_real_clock_advances_at_least_as_far_as_a_sleep() {
        let clock = Clock::new(ClockMode::Real);
        let before = clock.now();
        std::thread::sleep(Duration::from_millis(20));
        let after = clock.now();
        assert!(before >= Instant::ORIGIN);
        assert!(after.as_nanos() - before.as_nanos() >= 20_000_000);
    }
}
