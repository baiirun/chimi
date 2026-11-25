//! `CHIMI_REQUEST_SELECT`: one task waits on several sources and commits to
//! the first that is ready, in list order (time-and-outcomes.md, "Semantics of
//! `Wait.first`").
//!
//! The request answers with the index of the first ready source, and the
//! host then performs that source's ordinary operation (receive, process
//! receive, task result), which answers at once. A receive source's value is
//! taken in the select step itself and kept for that receive
//! (`commit_receive`), so a rival receiver of the same queue can never take
//! it in between. An inbox and a task handle have the selecting task as their
//! only consumer, so they need nothing kept. A losing source is untouched.
//!
//! When nothing is ready, the task registers as a watcher on every source and
//! parks. A watcher only wakes the task; the task then asks again, the
//! request clears every registration, and polls in list order once more. So a
//! cancelled or withdrawn select leaves every source as it was.

use super::clock::Instant;
use super::{
    ChannelId, RESPONSE_ERROR, RESPONSE_OK, RESPONSE_PENDING, ReceiveResult, Receiver, Scheduler,
    SchedulerRequest, TaskId, TaskPhase,
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
    /// ERROR for an empty or malformed list. Consumes only a committed
    /// receive source's value, which it keeps for the task.
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
                    if source.kind == SELECT_RECEIVE {
                        self.commit_receive(owner, source);
                    }
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

    /// Takes the value of a committed receive source in the same step as the
    /// answer, keeping it for the task's follow-up receive
    /// (`Operation::ReceiveReady`, which `REQUEST_CHANNEL_RECEIVE` reads
    /// first). Another receiver of the same queue can then never take the
    /// value between the answer and that receive, whichever carrier it runs
    /// on; a value kept for a task that ends first is released with its other
    /// operations. Requires: the source answered ready in this step.
    fn commit_receive(&mut self, owner: TaskId, source: &ChimiSelectSource) {
        let receiver = Receiver::from_raw(source.channel, source.owner, source.token);
        match self.channels.receive(owner, receiver) {
            Ok((result, transition)) => {
                debug_assert!(!matches!(result, ReceiveResult::Pending), "a ready source");
                self.apply_channel_transition(transition);
            }
            Err(error) => {
                let _ = self.channel_violation::<()>(owner, error, super::REQUEST_SELECT, None);
            }
        }
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
                        let _ =
                            self.channel_violation::<()>(owner, error, super::REQUEST_SELECT, None);
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
                if let Err(error) = self.require_owner(Some(owner), task) {
                    self.task_handle_violation(Some(owner), error, super::REQUEST_SELECT);
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
        if self.config.trace_mode == super::TraceMode::Off {
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
