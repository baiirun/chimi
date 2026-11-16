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
//! A cancelled scope exit also aborts the exclusive channel capabilities the
//! task gained inside the scope: the operation's values are discarded, and
//! without mutation they were the only way to reach them.
//!
//! `Resource.scoped` (`std/resource.chimi`) puts a resource entry on the same
//! stack: `REQUEST_RESOURCE_ENTER` before acquire, `REQUEST_RESOURCE_RELEASE`
//! before release, which *masks* the entry, and `REQUEST_RESOURCE_EXIT` after
//! release. Only scopes above the innermost masked entry are deliverable, so a
//! scope that fires outside a release waits until the release returns, and a
//! timeout written inside the release still works. Children started while
//! masked are tagged with the masked entry, so a firing outer scope leaves
//! them alone.
//!
//! Cancelling a task that holds a resource (`Task.cancel`, its parent ending,
//! a child's defect) fires a *root* scope beneath every other entry instead of
//! tearing the fiber down: the cancellation is delivered like a timeout's, at
//! the next cancellation point outside every release, and the task ends with
//! the recorded outcome once its fiber finishes ([`Scheduler::cancel_task`]).
//! Children started outside every entry stay alive until then, because a
//! release may still use them (the task that serves a pool, a log writer);
//! the task's terminal cancels them.
//!
//! A forced end (`begin_terminal` from a forced path, a forced process body)
//! resumes the fiber with [`ResumeContext::terminal`] set, which tells the
//! boundary to issue no request: a terminal fiber must finish without parking,
//! and releases it has not started do not run.

use super::clock::{Instant, TimerTarget};
use super::{
    ClockEvent, ClockEventKind, RequestHeader, Scheduler, TaskEventKind, TaskId, TaskOutcome,
    TaskPhase, cause,
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
    pub(super) kind: ScopeKind,
    /// The channel registry's arrival stamp when the entry was pushed.
    pub(super) entered_at: u64,
    pub(super) timer: Option<super::TimerId>,
    pub(super) deadline: Instant,
    pub(super) fired: bool,
    /// The task was resumed with this scope's cancellation.
    pub(super) delivered: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScopeKind {
    /// `Task.timeout` or `Task.deadline`, with a deadline timer.
    Timeout,
    /// A `Resource.scoped` call; `masked` once its release has begun.
    Resource { masked: bool },
    /// The task's own cancellation while it holds a resource; always fired,
    /// always the bottom entry.
    Root,
}

impl Scope {
    fn is_mask(&self) -> bool {
        self.kind == ScopeKind::Resource { masked: true }
    }
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
            | super::REQUEST_CHANNEL_SEND
            | super::REQUEST_CHANNEL_RECEIVE
            | super::REQUEST_PROCESS_RECEIVE
            | super::REQUEST_SELECT
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
            | super::REQUEST_CHANNEL_SEND
            | super::REQUEST_CHANNEL_RECEIVE
            | super::REQUEST_PROCESS_RECEIVE
            | super::REQUEST_SELECT
    )
}

impl Scheduler {
    fn push_entry(&mut self, task: TaskId, kind: ScopeKind, deadline: Instant, timer: Option<super::TimerId>) -> ScopeId {
        self.next_scope += 1;
        let id = ScopeId(self.next_scope);
        let entered_at = self.channels.tick();
        self.tasks
            .get_mut(&task)
            .expect("a task entering a scope is live")
            .scopes
            .push(Scope {
                id,
                kind,
                entered_at,
                timer,
                deadline,
                fired: false,
                delivered: false,
            });
        id
    }

    /// Pushes a scope with a deadline timer onto `task`'s stack.
    pub(super) fn enter_scope(&mut self, task: TaskId, deadline: Instant) -> ScopeId {
        let id = self.push_entry(task, ScopeKind::Timeout, deadline, None);
        let timer = self.timers.insert(deadline, TimerTarget::ScopeDeadline(task, id));
        self.emit_timer(ClockEventKind::TimerStarted, Some(task), deadline);
        let record = self.tasks.get_mut(&task).expect("task inside the scope");
        record.scopes.last_mut().expect("pushed entry").timer = Some(timer);
        id
    }

    /// Pushes an unmasked resource entry: `Resource.scoped` is about to
    /// acquire.
    pub(super) fn enter_resource(&mut self, task: TaskId) -> ScopeId {
        let now = self.clock.now();
        self.push_entry(task, ScopeKind::Resource { masked: false }, now, None)
    }

    /// Masks the resource entry `scope`: its release begins. Entries above it
    /// are stale (unwinding a forced end skipped their exits) and go.
    pub(super) fn mask_resource(&mut self, task: TaskId, scope: ScopeId) -> bool {
        let Some(index) = self.entry_index(task, scope, true) else {
            return false;
        };
        self.pop_entries(task, index + 1);
        self.tasks.get_mut(&task).expect("live task").scopes[index].kind =
            ScopeKind::Resource { masked: true };
        true
    }

    /// Leaves the resource entry `scope`. Returns `None` when the task holds no
    /// such entry, and otherwise whether a cancellation it held off is now
    /// delivered.
    pub(super) fn exit_resource(&mut self, task: TaskId, scope: ScopeId) -> Option<bool> {
        let index = self.entry_index(task, scope, true)?;
        self.pop_entries(task, index);
        if self.is_stopping(task) {
            return Some(!self.holds_mask(task) && self.tell_deferred_shutdown(task));
        }
        if self.deliverable_scope(task).is_some() {
            let record = self.tasks.get_mut(&task).expect("live task");
            let index = Self::deliverable_in(&record.scopes).expect("checked");
            record.scopes[index].delivered = true;
            return Some(true);
        }
        Some(false)
    }

    /// True while `task` runs a release: some resource entry is masked.
    pub(super) fn holds_mask(&self, task: TaskId) -> bool {
        self.tasks
            .get(&task)
            .is_some_and(|record| record.scopes.iter().any(Scope::is_mask))
    }

    /// True when `child` of `task` was started while one of `task`'s
    /// releases ran, in that masked entry or in a scope inside it.
    pub(super) fn started_in_release(&self, task: TaskId, child: TaskId) -> bool {
        let (Some(record), Some(started)) = (
            self.tasks.get(&task),
            self.tasks.get(&child).and_then(|record| record.started_in_scope),
        ) else {
            return false;
        };
        let Some(mask) = record.scopes.iter().position(Scope::is_mask) else {
            return false;
        };
        record.scopes[mask..].iter().any(|entry| entry.id == started)
    }

    fn entry_index(&self, task: TaskId, scope: ScopeId, resource: bool) -> Option<usize> {
        let scopes = &self.tasks.get(&task)?.scopes;
        let index = scopes.iter().position(|entry| entry.id == scope)?;
        let is_resource = matches!(scopes[index].kind, ScopeKind::Resource { .. });
        (is_resource == resource).then_some(index)
    }

    /// Removes the entries from `index` up, cancels their timers, and moves
    /// the children started in them to the entry below. A child moved under a
    /// scope that has fired is cancelled, as the firing would have done had
    /// it started there.
    fn pop_entries(&mut self, task: TaskId, index: usize) {
        let record = self.tasks.get_mut(&task).expect("live task");
        if index >= record.scopes.len() {
            return;
        }
        let removed: Vec<Scope> = record.scopes.drain(index..).collect();
        let below = record.scopes.last().map(|entry| entry.id);
        let below_fired = Self::fired_unmasked(&record.scopes);
        for entry in &removed {
            if let Some(timer) = entry.timer {
                self.cancel_timer(timer);
            }
        }
        let leaving: Vec<ScopeId> = removed.iter().map(|entry| entry.id).collect();
        let children = self.scope_children(task, &leaving);
        for child in children {
            self.tasks.get_mut(&child).expect("live child").started_in_scope = below;
            if below_fired {
                self.cancel_task(child, TaskOutcome::Cancelled);
            }
        }
    }

    /// True when a fired entry lies at or below the top of `scopes` with no
    /// masked entry in between: a child started at the top would be cancelled.
    fn fired_unmasked(scopes: &[Scope]) -> bool {
        for entry in scopes.iter().rev() {
            if entry.fired {
                return true;
            }
            if entry.is_mask() {
                return false;
            }
        }
        false
    }

    /// Leaves `scope` and every scope nested in it that unwinding skipped.
    /// When `cancelled`, the operation's values are being discarded, so the
    /// capabilities the task gained inside the scope are aborted.
    pub(super) fn exit_scope(&mut self, task: TaskId, scope: ScopeId, cancelled: bool) -> ScopeExit {
        if !self.tasks.contains_key(&task) {
            return ScopeExit::Unknown;
        }
        let Some(index) = self.entry_index(task, scope, false) else {
            return ScopeExit::Unknown;
        };
        let record = &self.tasks[&task];
        let leaving: Vec<ScopeId> = record.scopes[index..].iter().map(|entry| entry.id).collect();
        let fired = record.scopes[index..].iter().any(|entry| entry.fired);
        let children = self.scope_children(task, &leaving);
        if fired && !children.is_empty() {
            return ScopeExit::Pending;
        }
        let entry = record.scopes[index];
        self.pop_entries(task, index);
        if cancelled {
            let transition = self.channels.abort_arrivals(task, entry.entered_at);
            self.apply_channel_transition(transition);
        }
        ScopeExit::Exited {
            delivered: entry.delivered,
        }
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
        if !live || stopping || entry.fired {
            return;
        }
        let now = self.clock.now();
        self.emit_clock(ClockEvent {
            sequence: 0,
            kind: ClockEventKind::ScopeFired,
            task: Some(task),
            time: now,
            request_kind: 0,
        });
        self.fire_entry(task, index, withdraw);
    }

    /// Marks entry `index` fired, withdraws a withdrawable park when the
    /// cancellation can be delivered now, and cancels the children of the
    /// entry and of the entries above it up to the first masked one.
    fn fire_entry(&mut self, task: TaskId, index: usize, withdraw: bool) {
        let record = self.tasks.get_mut(&task).expect("live task");
        record.scopes[index].fired = true;
        let end = record.scopes[index + 1..]
            .iter()
            .position(Scope::is_mask)
            .map_or(record.scopes.len(), |offset| index + 1 + offset);
        let nested: Vec<ScopeId> = record.scopes[index..end].iter().map(|entry| entry.id).collect();
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
        let children: Vec<TaskId> = self.tasks[&task]
            .children
            .iter()
            .copied()
            .filter(|child| {
                let started_in = self.tasks.get(child).and_then(|record| record.started_in_scope);
                match started_in {
                    Some(scope) => nested.contains(&scope),
                    // A child started outside every entry outlives the
                    // releases, which may still use it (a pool task, a log
                    // writer); the task's terminal cancels it.
                    None => false,
                }
            })
            .collect();
        for child in children {
            if self.tasks.get(&child).is_some_and(|record| {
                matches!(record.phase, TaskPhase::Runnable | TaskPhase::Parked)
            }) {
                self.cancel_task(child, TaskOutcome::Cancelled);
            }
        }
    }

    /// Cancels `task` with `outcome`. A task that holds a `Resource.scoped`
    /// entry keeps running: a root scope fires beneath its other entries, its
    /// releases run, and it ends with `outcome` once its fiber finishes
    /// ([`Self::begin_terminal`]). A process body under shutdown is already
    /// being cancelled by shutdown, so it only records a defect. Any other
    /// task, and every task while a forced end is under way (`forcing`), ends
    /// now.
    pub(super) fn cancel_task(&mut self, task: TaskId, outcome: TaskOutcome) {
        let Some(record) = self.tasks.get_mut(&task) else {
            return;
        };
        if !matches!(record.phase, TaskPhase::Runnable | TaskPhase::Parked) {
            return;
        }
        if let Some(recorded) = record.terminating.as_mut() {
            if outcome.is_defect() && !recorded.is_defect() {
                *recorded = outcome;
            }
            if self.forcing > 0 {
                self.begin_terminal(task, outcome, false);
            }
            return;
        }
        let holds_resource = record
            .scopes
            .iter()
            .any(|entry| matches!(entry.kind, ScopeKind::Resource { .. }));
        let process = record.process.is_some();
        let no_fiber = record.fiber.is_none();
        let stopping = process && self.is_stopping(task);
        if stopping && !self.is_forced(task) && holds_resource {
            // Shutdown already cancels this body, holding off while it
            // releases (`Scheduler::begin_shutdown`); a defect only decides
            // how it ends.
            if outcome.is_defect() {
                self.absorbs_child_defect(task);
            }
            return;
        }
        if self.forcing > 0 || no_fiber || !holds_resource || stopping {
            self.begin_terminal(task, outcome, false);
            return;
        }
        self.tasks.get_mut(&task).expect("live task").terminating = Some(outcome);
        self.next_scope += 1;
        let id = ScopeId(self.next_scope);
        let entered_at = self.channels.tick();
        let now = self.clock.now();
        let record = self.tasks.get_mut(&task).expect("live task");
        record.scopes.insert(
            0,
            Scope {
                id,
                kind: ScopeKind::Root,
                entered_at,
                timer: None,
                deadline: now,
                fired: false,
                delivered: false,
            },
        );
        self.emit(task, TaskEventKind::CancellationRequested, cause(outcome), false);
        self.fire_entry(task, 0, true);
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
    /// no cancellation is already unwinding the task. Only entries above the
    /// innermost masked resource entry count: a release holds the rest off.
    fn deliverable_scope(&self, task: TaskId) -> Option<usize> {
        Self::deliverable_in(&self.tasks.get(&task)?.scopes)
    }

    fn deliverable_in(scopes: &[Scope]) -> Option<usize> {
        let base = scopes.iter().rposition(Scope::is_mask).map_or(0, |index| index + 1);
        let open = &scopes[base..];
        if open.iter().any(|entry| entry.delivered) {
            return None;
        }
        open.iter().position(|entry| entry.fired).map(|index| base + index)
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
