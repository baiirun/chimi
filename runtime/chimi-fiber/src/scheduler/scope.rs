//! Cancellation scopes: the part of a task that `Task.timeout` or
//! `Task.deadline` bounds (`docs/specification/time-and-outcomes.md`,
//! "Timeouts and deadlines").
//!
//! A scope runs in its task, not in a child task. Each task keeps a stack of
//! the scopes it is inside, innermost last, and every child task records the
//! innermost scope it was started in.
//!
//! When a scope's deadline timer fires, the scope is *fired*:
//! - every live child started in it or in a scope nested in it is cancelled;
//! - if the task is parked on an operation that can be withdrawn (a sleep, a
//!   task result, a channel send or receive, a process receive), the operation
//!   is withdrawn as if it had never been issued, and the task resumes with a
//!   cancellation that is *delivered* for the outermost fired scope;
//! - otherwise the cancellation waits for the task's next cancellation point,
//!   a request in [`is_cancellation_point`], which is answered with the
//!   cancellation instead of being served. A channel operation whose value
//!   already linearized is still collected first, so no value is lost.
//!
//! The task unwinds to the scope boundary, which issues `REQUEST_SCOPE_EXIT`.
//! The reply says whether this scope's cancellation was the one delivered; only
//! then does the boundary convert the cancellation into `Timeout.TimedOut`.
//! Task cancellation, an enclosing scope's cancellation, and process shutdown
//! are never delivered for an inner scope, so they pass through it unchanged.
//!
//! A task-level cancellation (`begin_terminal`) or a forced process body
//! resumes the fiber with [`ResumeContext::terminal`] set, which tells the
//! boundary to issue no request: a terminal fiber must finish without parking.
//!
//! Hook for masked cancellation (G12 scope-owned resources): a masked region
//! would be a scope that [`Scheduler::deliverable_scope`] skips until it exits.

use super::clock::{Instant, TimerTarget};
use super::{
    ClockEvent, ClockEventKind, RequestHeader, Scheduler, TaskId, TaskOutcome, TaskPhase,
};
use crate::ChimiFiberCommand;
use std::cell::Cell;

/// Identity of one cancellation scope, unique within the scheduler.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScopeId(u64);

impl ScopeId {
    pub fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// One entry of a task's scope stack.
#[derive(Clone, Copy, Debug)]
pub(super) struct Scope {
    pub(super) id: ScopeId,
    pub(super) timer: Option<super::TimerId>,
    pub(super) deadline: Instant,
    pub(super) fired: bool,
    /// The task was resumed with this scope's cancellation.
    pub(super) delivered: bool,
}

/// What `REQUEST_SCOPE_EXIT` answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeExit {
    /// Children cancelled by the scope are still draining.
    Pending,
    /// The scope is gone; `delivered` says whether its own cancellation is the
    /// one unwinding the task.
    Exited { delivered: bool },
    /// The task is not inside that scope.
    Unknown,
}

thread_local! {
    static RESUME_TERMINAL: Cell<bool> = const { Cell::new(false) };
    static RESUME_SCOPE_PENDING: Cell<bool> = const { Cell::new(false) };
    static RESUME_POLLS: Cell<bool> = const { Cell::new(false) };
}

/// What the fiber about to resume may read about its own cancellation state.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ResumeContext {
    /// The cancellation ends the whole task (`begin_terminal`, or a forced
    /// process body): unwind without any request that can park.
    pub(super) terminal: bool,
    /// A fired scope waits for the task's next cancellation point.
    pub(super) scope_pending: bool,
    /// Loop passes must poll the scheduler for scope deadlines.
    pub(super) polls: bool,
}

impl ResumeContext {
    pub(super) const TERMINAL: Self = Self {
        terminal: true,
        scope_pending: false,
        polls: false,
    };
}

/// Records `context` for the fiber about to resume. Every resume sets it.
pub(super) fn set_resume_context(context: ResumeContext) {
    RESUME_TERMINAL.with(|cell| cell.set(context.terminal));
    RESUME_SCOPE_PENDING.with(|cell| cell.set(context.scope_pending));
    RESUME_POLLS.with(|cell| cell.set(context.polls));
}

#[unsafe(no_mangle)]
/// 1 when the running fiber's latest resume was a task-level cancellation:
/// the fiber must unwind without issuing any request that can park.
pub extern "C" fn chimi_fiber_cancel_is_terminal() -> u32 {
    u32::from(RESUME_TERMINAL.with(Cell::get))
}

#[unsafe(no_mangle)]
/// 1 when a fired scope of the running task waits for its next cancellation
/// point. Loop passes check it and then issue `CHIMI_REQUEST_SCOPE_CHECK`.
pub extern "C" fn chimi_fiber_scope_cancel_pending() -> u32 {
    u32::from(RESUME_SCOPE_PENDING.with(Cell::get))
}

#[unsafe(no_mangle)]
/// 1 when the running task is inside a timeout scope under the real clock.
/// A fiber that never parks never lets the scheduler fire timers, so its loop
/// passes issue `CHIMI_REQUEST_SCOPE_CHECK` now and then, and every request
/// first fires the task's due scope deadlines. Under the virtual clock
/// computation takes no time, so this is always 0.
pub extern "C" fn chimi_fiber_scope_polls() -> u32 {
    u32::from(RESUME_POLLS.with(Cell::get))
}

/// Requests a pending scope cancellation answers instead of serving.
pub(super) fn is_cancellation_point(kind: u32) -> bool {
    matches!(
        kind,
        super::REQUEST_TIMER_NOW
            | super::REQUEST_TIMER_SLEEP_UNTIL
            | super::REQUEST_TASK_START
            | super::REQUEST_TASK_RESULT
            | super::REQUEST_TASK_ANY
            | super::REQUEST_CHANNEL_SEND
            | super::REQUEST_CHANNEL_RECEIVE
            | super::REQUEST_PROCESS_RECEIVE
            | super::REQUEST_SCOPE_ENTER
            | super::REQUEST_SCOPE_CHECK
    )
}

/// Parks a firing scope withdraws: they block on something that can be
/// removed without side effects. `0` is a bare park.
fn is_withdrawable(kind: u32) -> bool {
    matches!(
        kind,
        0 | super::REQUEST_TIMER_SLEEP_UNTIL
            | super::REQUEST_TASK_RESULT
            | super::REQUEST_TASK_ANY
            | super::REQUEST_CHANNEL_SEND
            | super::REQUEST_CHANNEL_RECEIVE
            | super::REQUEST_PROCESS_RECEIVE
    )
}

impl Scheduler {
    /// Pushes a scope with a deadline timer onto `task`'s stack.
    pub(super) fn enter_scope(&mut self, task: TaskId, deadline: Instant) -> ScopeId {
        self.next_scope += 1;
        let id = ScopeId(self.next_scope);
        let timer = self
            .timers
            .insert(deadline, TimerTarget::ScopeDeadline(task, id));
        self.emit_timer(ClockEventKind::TimerStarted, Some(task), deadline);
        self.tasks
            .get_mut(&task)
            .expect("a task entering a scope is live")
            .scopes
            .push(Scope {
                id,
                timer: Some(timer),
                deadline,
                fired: false,
                delivered: false,
            });
        id
    }

    /// Leaves `scope` and every scope nested in it that unwinding skipped.
    pub(super) fn exit_scope(&mut self, task: TaskId, scope: ScopeId) -> ScopeExit {
        let Some(record) = self.tasks.get(&task) else {
            return ScopeExit::Unknown;
        };
        let Some(index) = record.scopes.iter().position(|entry| entry.id == scope) else {
            return ScopeExit::Unknown;
        };
        let leaving: Vec<ScopeId> = record.scopes[index..].iter().map(|entry| entry.id).collect();
        let fired = record.scopes[index..].iter().any(|entry| entry.fired);
        let children = self.scope_children(task, &leaving);
        if fired && !children.is_empty() {
            return ScopeExit::Pending;
        }
        let record = self.tasks.get_mut(&task).expect("task inside the scope");
        let delivered = record.scopes[index].delivered;
        let removed: Vec<_> = record.scopes.drain(index..).collect();
        let parent_scope = record.scopes.last().map(|entry| entry.id);
        for entry in removed {
            if let Some(timer) = entry.timer {
                self.cancel_timer(timer);
            }
        }
        for child in children {
            self.tasks.get_mut(&child).expect("live child").started_in_scope = parent_scope;
        }
        ScopeExit::Exited { delivered }
    }

    /// The scope deadline timer for `scope` of `task` fired.
    /// `withdraw` is false when `task` is issuing a request right now: that
    /// request has parked nothing yet and meets the gate instead.
    pub(super) fn fire_scope(&mut self, task: TaskId, scope: ScopeId, withdraw: bool) {
        let Some(record) = self.tasks.get(&task) else {
            return;
        };
        let live = matches!(record.phase, TaskPhase::Runnable | TaskPhase::Parked);
        let stopping = self.is_stopping(task);
        let record = self.tasks.get_mut(&task).expect("live task");
        let Some(index) = record.scopes.iter().position(|entry| entry.id == scope) else {
            return;
        };
        let entry = &mut record.scopes[index];
        // The timer has fired either way; a stopping task keeps shutdown's
        // own cancellation.
        entry.timer = None;
        if !live || stopping {
            return;
        }
        if entry.fired {
            return;
        }
        entry.fired = true;
        let nested: Vec<ScopeId> = record.scopes[index..].iter().map(|entry| entry.id).collect();
        let now = self.clock.now();
        self.emit_clock(ClockEvent {
            sequence: 0,
            kind: ClockEventKind::ScopeFired,
            task: Some(task),
            time: now,
            request_kind: 0,
        });
        // Withdraw the park first: a parent waiting on a child of this scope
        // must not be cancelled by that child's terminal below.
        if withdraw
            && self.tasks[&task].phase == TaskPhase::Parked
            && is_withdrawable(self.parked_kind(task))
            && self.deliverable_scope(task).is_some()
        {
            self.disarm_wake_sources(task);
            self.deliver_scope_cancel(task);
        }
        for child in self.scope_children(task, &nested) {
            if self.tasks.get(&child).is_some_and(|record| {
                matches!(record.phase, TaskPhase::Runnable | TaskPhase::Parked)
            }) {
                self.begin_terminal(child, TaskOutcome::Cancelled, false);
            }
        }
    }

    /// True when `task`'s loop passes must poll for scope deadlines: it is
    /// inside a scope whose timer is armed, under the real clock.
    pub(super) fn polls_scope_deadlines(&self, task: TaskId) -> bool {
        self.clock.mode() == super::clock::ClockMode::Real
            && self
                .tasks
                .get(&task)
                .is_some_and(|record| record.scopes.iter().any(|entry| entry.timer.is_some()))
    }

    /// Fires `task`'s scopes whose deadline has passed, outermost first, while
    /// it issues a request. Under the real clock a task that never parks
    /// would otherwise run past its deadline: the scheduler fires timers only
    /// when it regains control between tasks.
    pub(super) fn poll_scope_deadlines(&mut self, task: TaskId) {
        if !self.polls_scope_deadlines(task) {
            return;
        }
        let now = self.clock.now();
        let due: Vec<_> = self.tasks[&task]
            .scopes
            .iter()
            .filter(|entry| entry.deadline <= now)
            .filter_map(|entry| entry.timer.map(|timer| (entry.id, timer, entry.deadline)))
            .collect();
        for (scope, timer, deadline) in due {
            if self.timers.cancel(timer).is_some() {
                self.emit_timer(ClockEventKind::TimerFired, Some(task), deadline);
                self.fire_scope(task, scope, false);
            }
        }
    }

    /// Answers a cancellation-point request with a pending scope cancellation
    /// instead of serving it. Returns true when it did.
    pub(super) fn refuse_for_scope(&mut self, task: TaskId, kind: u32) -> bool {
        if !is_cancellation_point(kind)
            || self.deliverable_scope(task).is_none()
            || self.is_stopping(task)
        {
            return false;
        }
        // A send or receive the task re-issues to collect a linearized result
        // is served: the value belongs to the operation.
        if matches!(
            kind,
            super::REQUEST_CHANNEL_SEND | super::REQUEST_CHANNEL_RECEIVE
        ) && self.channels.has_operation(task)
        {
            return false;
        }
        self.deliver_scope_cancel(task);
        true
    }

    /// True when a fired scope of `task` waits for its next cancellation point.
    pub(super) fn scope_cancel_pending(&self, task: TaskId) -> bool {
        self.deliverable_scope(task).is_some()
    }

    /// The outermost fired scope whose cancellation has not been delivered, if
    /// no cancellation is already unwinding the task.
    fn deliverable_scope(&self, task: TaskId) -> Option<usize> {
        let scopes = &self.tasks.get(&task)?.scopes;
        if scopes.iter().any(|entry| entry.delivered) {
            return None;
        }
        scopes.iter().position(|entry| entry.fired)
    }

    /// Resumes `task` with the outermost fired scope's cancellation. Its
    /// parked request keeps its status, so a withdrawn send's value is
    /// released once, by the scheduler.
    fn deliver_scope_cancel(&mut self, task: TaskId) {
        let Some(index) = self.deliverable_scope(task) else {
            return;
        };
        let record = self.tasks.get_mut(&task).expect("live task");
        record.scopes[index].delivered = true;
        record.command = ChimiFiberCommand::Cancel;
        let _ = self.wake_front(task);
    }

    /// Clears a task's scopes when it ends.
    pub(super) fn clear_scopes(&mut self, task: TaskId) {
        let Some(record) = self.tasks.get_mut(&task) else {
            return;
        };
        let scopes = std::mem::take(&mut record.scopes);
        for entry in scopes {
            if let Some(timer) = entry.timer {
                self.cancel_timer(timer);
            }
        }
    }

    /// Wakes `task` if it is parked leaving a scope, so it asks again whether
    /// the scope's children have drained.
    pub(super) fn wake_scope_exit(&mut self, task: TaskId) {
        if self
            .tasks
            .get(&task)
            .is_some_and(|record| record.phase == TaskPhase::Parked)
            && self.parked_kind(task) == super::REQUEST_SCOPE_EXIT
        {
            let _ = self.wake(task);
        }
    }

    /// The live children of `task` started inside one of `scopes`.
    fn scope_children(&self, task: TaskId, scopes: &[ScopeId]) -> Vec<TaskId> {
        self.tasks[&task]
            .children
            .iter()
            .copied()
            .filter(|child| {
                self.tasks
                    .get(child)
                    .and_then(|record| record.started_in_scope)
                    .is_some_and(|scope| scopes.contains(&scope))
            })
            .collect()
    }

    /// The request kind `task` is parked on, or 0 for a bare park.
    fn parked_kind(&self, task: TaskId) -> u32 {
        let request = self.tasks[&task].request;
        if request.is_null() {
            0
        } else {
            // SAFETY: a recorded request stays valid while its fiber is parked,
            // and every request record starts with `RequestHeader`.
            unsafe { (*request.cast::<RequestHeader>()).kind }
        }
    }
}
