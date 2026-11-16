//! `CHIMI_REQUEST_SELECT`: one task waits on several sources and commits to
//! the first that is ready, in list order (time-and-outcomes.md, "Semantics of
//! `Select.first`").
//!
//! The request never consumes anything. It answers with the index of the
//! first ready source, and the host then performs that source's ordinary
//! operation (receive, process receive, task result), which answers at once.
//! That is atomic because every source belongs to the selecting task alone:
//! a receiver and an inbox have one consumer, a task handle one owner, and
//! the host issues the follow-up request before any other task runs (the
//! reply wakes the selecting task at the front of the run queue).
//!
//! When nothing is ready, the task registers as a watcher on every source and
//! parks. A watcher only wakes the task; the task then asks again, the
//! request clears every registration, and polls in list order once more. So a
//! cancelled or withdrawn select leaves every source as it was.

use super::clock::Instant;
use super::{
    ChannelId, Receiver, Scheduler, SchedulerRequest, TaskId, TaskPhase, RESPONSE_ERROR,
    RESPONSE_OK, RESPONSE_PENDING,
};

/// A receive event on a channel: `channel`, `owner` and `token` are the
/// receiver's.
pub const SELECT_RECEIVE: u32 = 1;
/// The calling process body's inbox.
pub const SELECT_INBOX: u32 = 2;
/// A child task's terminal: `task`.
pub const SELECT_TASK: u32 = 3;
/// A deadline Instant of the execution clock: `deadline`.
pub const SELECT_TIMER: u32 = 4;

/// One source of a `CHIMI_REQUEST_SELECT`, whose `context` points to `value`
/// of these.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiSelectSource {
    pub kind: u32,
    pub channel: u64,
    pub owner: u64,
    pub token: u64,
    pub task: u64,
    pub deadline: i64,
}

/// What a parked select registered, so it can be withdrawn.
#[derive(Debug, Default)]
pub(super) struct SelectWait {
    channels: Vec<ChannelId>,
    tasks: Vec<TaskId>,
    inbox: bool,
}

impl SelectWait {
    pub(super) fn watches_task(&self, task: TaskId) -> bool {
        self.tasks.contains(&task)
    }
}

enum Poll {
    Ready,
    Waiting,
    /// The source broke the contract; the task is already defected.
    Violation,
    Invalid,
}

impl Scheduler {
    /// Serves a select. Ensures: answers OK with the first ready source's
    /// index in `value`, or PENDING with the task registered on every source;
    /// ERROR for an empty or malformed list. Must not consume any source.
    pub(super) fn handle_select(&mut self, owner: TaskId, request: &mut SchedulerRequest) {
        self.clear_select(owner);
        let count = usize::try_from(request.value).unwrap_or(0);
        if count == 0 || request.context.is_null() {
            request.status = RESPONSE_ERROR;
            let _ = self.wake_front(owner);
            return;
        }
        // SAFETY: the host keeps `count` sources readable while it is
        // suspended on the request.
        let sources = unsafe {
            std::slice::from_raw_parts(request.context.cast::<ChimiSelectSource>(), count)
        }
        .to_vec();
        let now = self.clock.now();
        let mut wait = SelectWait::default();
        let mut earliest: Option<Instant> = None;
        for (index, source) in sources.iter().enumerate() {
            match self.poll_source(owner, source, now, &mut wait, &mut earliest) {
                Poll::Waiting => {}
                Poll::Ready => {
                    self.withdraw(owner, wait);
                    request.value = index as i64;
                    request.status = RESPONSE_OK;
                    self.emit_select(owner, super::ClockEventKind::SelectCommitted, index);
                    let _ = self.wake_front(owner);
                    return;
                }
                Poll::Violation => {
                    self.withdraw(owner, wait);
                    request.status = RESPONSE_ERROR;
                    return;
                }
                Poll::Invalid => {
                    self.withdraw(owner, wait);
                    request.status = RESPONSE_ERROR;
                    let _ = self.wake_front(owner);
                    return;
                }
            }
        }
        if let Some(deadline) = earliest {
            let _ = self.start_wake_timer(owner, deadline);
        }
        self.tasks.get_mut(&owner).expect("selecting task").select = Some(wait);
        request.status = RESPONSE_PENDING;
        self.emit_select(owner, super::ClockEventKind::SelectParked, 0);
    }

    fn poll_source(
        &mut self,
        owner: TaskId,
        source: &ChimiSelectSource,
        now: Instant,
        wait: &mut SelectWait,
        earliest: &mut Option<Instant>,
    ) -> Poll {
        match source.kind {
            SELECT_RECEIVE => {
                let receiver = Receiver::from_raw(source.channel, source.owner, source.token);
                match self.channels.select_receive(owner, receiver) {
                    Ok(true) => Poll::Ready,
                    Ok(false) => {
                        wait.channels.push(receiver.channel());
                        Poll::Waiting
                    }
                    Err(error) => {
                        let _ = self.channel_violation::<()>(
                            owner,
                            error,
                            super::REQUEST_SELECT,
                            None,
                        );
                        Poll::Violation
                    }
                }
            }
            SELECT_INBOX => match self.select_inbox(owner) {
                None => Poll::Invalid,
                Some(true) => Poll::Ready,
                Some(false) => {
                    wait.inbox = true;
                    Poll::Waiting
                }
            },
            SELECT_TASK => {
                let task = TaskId(source.task);
                if self.require_owner(Some(owner), task).is_err() {
                    self.defect_contract_violator(Some(owner));
                    return Poll::Violation;
                }
                if matches!(self.tasks[&task].phase, TaskPhase::Terminal(_)) {
                    Poll::Ready
                } else {
                    wait.tasks.push(task);
                    Poll::Waiting
                }
            }
            SELECT_TIMER => {
                let deadline = Instant::from_nanos(source.deadline);
                if deadline <= now {
                    Poll::Ready
                } else {
                    *earliest = Some(earliest.map_or(deadline, |at| at.min(deadline)));
                    Poll::Waiting
                }
            }
            _ => Poll::Invalid,
        }
    }

    /// Withdraws a select's registrations and its timer, if it has any.
    pub(super) fn clear_select(&mut self, task: TaskId) {
        let Some(record) = self.tasks.get_mut(&task) else {
            return;
        };
        let Some(wait) = record.select.take() else {
            return;
        };
        if let Some(timer) = record.timer.take() {
            self.cancel_timer(timer);
        }
        self.withdraw(task, wait);
    }

    fn withdraw(&mut self, task: TaskId, wait: SelectWait) {
        for channel in wait.channels {
            self.channels.unwatch(task, channel);
        }
        if wait.inbox {
            self.unwatch_inbox(task);
        }
    }

    /// Wakes `parent` when it is parked in a select that waits on `child`.
    pub(super) fn wake_selecting_parent(&mut self, parent: TaskId, child: TaskId) {
        if self.tasks.get(&parent).is_some_and(|record| {
            record.phase == TaskPhase::Parked
                && record
                    .select
                    .as_ref()
                    .is_some_and(|wait| wait.watches_task(child))
        }) {
            let _ = self.wake(parent);
        }
    }

    fn emit_select(&mut self, task: TaskId, kind: super::ClockEventKind, index: usize) {
        if self.trace_mode == super::TraceMode::Off {
            return;
        }
        let now = self.clock.now();
        self.emit_clock(super::ClockEvent {
            sequence: 0,
            kind,
            task: Some(task),
            time: now,
            request_kind: index as u32,
        });
    }
}
