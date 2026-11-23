//! The local path (parallel-scheduler.md, "Step 4 plan", 4e-2): one step
//! of `drive` (finish a resume, the housekeeping after it, enter the next
//! task) done under the kernel lock's read side, when everything it
//! touches is homed on the carrier doing it. Other carriers run their own
//! local steps at the same time, so this code never makes a `&mut
//! Scheduler`: it writes only records homed on its carrier, through their
//! cells (`Homed::homed_mut`, `Shards::own`, the channel operation slots),
//! and reads only what no reader writes (the configuration, the clock, the
//! tables' structure, homes, and the shared lists it checks are empty).
//!
//! Each phase first checks that it can run locally, changing nothing,
//! and returns `Escalate` if not; the carrier then runs that phase and the
//! rest of the step on the write side, with the ordinary code. A phase
//! that runs locally does what the ordinary code would, in the same order,
//! for the cases it accepts: with tracing off (no events), no scope,
//! select, descriptor, child or signal wait, no stop under way, and no
//! host hook to call.

use super::carrier::Runnable;
use super::ready::Carrier;
use super::{ChimiFiberCommand, ChimiFiberEvent, ChimiFiberState, Scheduler, Task, TaskId, TaskPhase};
use std::marker::PhantomData;
use std::ptr::NonNull;

/// The phase could not run locally; nothing was changed.
#[derive(Debug)]
pub(super) struct Escalate;

/// Where a step that left the local path goes on, on the write side.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Phase {
    Complete,
    AfterResume,
    Next,
}

/// The kernel as one carrier reaches it on the local path.
pub(super) struct Local<'kernel> {
    kernel: NonNull<Scheduler>,
    carrier: Carrier,
    _kernel: PhantomData<&'kernel Scheduler>,
}

impl<'kernel> Local<'kernel> {
    /// # Safety
    ///
    /// For `'kernel`, the caller may write what is homed on `carrier` and
    /// read everything that no local path writes: it holds the kernel
    /// lock's read side as the only thread acting as `carrier`, or has the
    /// scheduler to itself (from a `&mut Scheduler` it does not otherwise
    /// use meanwhile).
    pub(super) unsafe fn new(kernel: NonNull<Scheduler>, carrier: Carrier) -> Self {
        Self {
            kernel,
            carrier,
            _kernel: PhantomData,
        }
    }

    /// The shared view. Through it the local path reads only what no local
    /// path writes, and records homed on its own carrier.
    fn kernel(&self) -> &'kernel Scheduler {
        // SAFETY: `new`'s contract. No local path writes outside a cell,
        // so a shared reference to the scheduler aliases no write.
        unsafe { self.kernel.as_ref() }
    }

    /// `task`'s record, when it is homed on this carrier.
    pub(super) fn task(&mut self, task: TaskId) -> Option<&mut Task> {
        // SAFETY: `new`'s contract; the record is borrowed from `self`, so
        // no two are live at once.
        unsafe { self.kernel().tasks.homed_mut(&task, self.carrier) }
    }

    pub(super) fn carrier(&self) -> Carrier {
        self.carrier
    }

    /// True when `task` is homed on this carrier.
    pub(super) fn is_local(&self, task: TaskId) -> bool {
        self.kernel().tasks.home(&task) == Some(self.carrier)
    }

    /// Runs one `drive` step from finishing `runnable`'s resume. Returns
    /// the next task, or the phase the write side goes on from.
    pub(super) fn step(
        &mut self,
        runnable: Runnable,
        event: ChimiFiberEvent,
        resumes: &mut u32,
    ) -> Result<Option<Runnable>, Phase> {
        self.complete(runnable, event).map_err(|Escalate| Phase::Complete)?;
        self.after_resume(resumes).map_err(|Escalate| Phase::AfterResume)?;
        self.next_runnable().map_err(|Escalate| Phase::Next)
    }

    /// `Scheduler::complete` for a parked step whose request, if any, is a
    /// process send or receive or a channel send or receive.
    fn complete(&mut self, runnable: Runnable, event: ChimiFiberEvent) -> Result<(), Escalate> {
        let kernel = self.kernel();
        if !matches!(runnable.kind, super::carrier::RunKind::Step)
            || kernel.config.trace_mode != super::TraceMode::Off
            || event.state != ChimiFiberState::Parked as u32
            || !kernel.children.is_empty()
            || kernel.has_signal_waiters()
            || kernel.moves_tasks_for_test()
        {
            return Err(Escalate);
        }
        let task = runnable.task;
        let record = self.task(task).ok_or(Escalate)?;
        if record.pending_end.is_some()
            || record.select.is_some()
            || record.io_wait.is_some()
            || !record.scopes.is_empty()
        {
            return Err(Escalate);
        }
        if let Some(incarnation) = record.process
            && !self.incarnation_runs(incarnation)
        {
            return Err(Escalate);
        }
        // The park, as `apply_step` records it; undone if the request
        // cannot be served here.
        let record = self.task(task).expect("checked above");
        let (phase, request, park) = (record.phase, record.request, record.park);
        record.phase = TaskPhase::Parked;
        record.request = event.request;
        record.park += 1;
        if event.request.is_null() {
            return Ok(());
        }
        // SAFETY: every request record starts with `RequestHeader`, and
        // `chimi_fiber_request` keeps it alive until the fiber resumes.
        let kind = unsafe { (*event.request.cast::<super::RequestHeader>()).kind };
        let served = match kind {
            super::REQUEST_PROCESS_SEND => self.process_send(task, event.request.cast()),
            super::REQUEST_PROCESS_RECEIVE => self.process_receive(task, event.request.cast()),
            super::REQUEST_CHANNEL_SEND => self.channel_send(task, event.request.cast()),
            super::REQUEST_CHANNEL_RECEIVE => self.channel_receive(task, event.request.cast()),
            _ => Err(Escalate),
        };
        if served.is_err() {
            let record = self.task(task).expect("checked above");
            record.phase = phase;
            record.request = request;
            record.park = park;
        }
        served
    }

    /// `Scheduler::after_resume`, when its housekeeping has nothing to do.
    fn after_resume(&mut self, resumes: &mut u32) -> Result<(), Escalate> {
        let kernel = self.kernel();
        let next = resumes.wrapping_add(1);
        if !kernel.released_terminal.is_empty()
            || self.carrier == Carrier(0) && kernel.signals_have_work()
            || next.is_multiple_of(super::reactor::IO_POLL_INTERVAL)
                && (!kernel.reactor.is_empty() || !kernel.children.is_empty())
        {
            return Err(Escalate);
        }
        *resumes = next;
        Ok(())
    }

    /// `Scheduler::next_runnable` when this carrier has no job and its
    /// front ready task resumes plainly: it ran before, is resumed (not
    /// cancelled), and is inside no scope. An empty queue escalates too,
    /// since the write side then drains releases and may steal.
    fn next_runnable(&mut self) -> Result<Option<Runnable>, Escalate> {
        let kernel = self.kernel();
        // SAFETY: `new`'s contract; this carrier's own shard, borrowed for
        // this function only, and never alongside another reference to it.
        let shard = unsafe { kernel.shards.own(self.carrier) };
        shard.unpark();
        if shard.has_jobs() {
            return Err(Escalate);
        }
        let task = shard.front_ready().ok_or(Escalate)?;
        let record = self.task(task).ok_or(Escalate)?;
        if record.phase != TaskPhase::Runnable
            || !record.ran
            || record.command != ChimiFiberCommand::Resume
            || !record.scopes.is_empty()
        {
            return Err(Escalate);
        }
        shard.pop_ready();
        record.phase = TaskPhase::Running;
        record.request = std::ptr::null_mut();
        record.entry_context = std::ptr::null_mut();
        // With no scope there is no pending cancellation and no deadline
        // to poll (`scope_cancel_pending`, `polls_scope_deadlines`).
        let context = super::scope::ResumeContext {
            terminal: false,
            scope_pending: false,
            polls: false,
        };
        let fiber = record
            .fiber
            .as_mut()
            .expect("runnable task owns a fiber")
            .enter(context, kernel.clock.reading());
        Ok(Some(Runnable {
            task,
            fiber,
            command: ChimiFiberCommand::Resume,
            kind: super::carrier::RunKind::Step,
        }))
    }

    /// `Scheduler::wake_at` for a task homed here: a parked task becomes
    /// ready on this carrier's queue.
    pub(super) fn wake(&mut self, task: TaskId, front: bool) {
        let record = self.task(task).expect("a local wake's task is homed here");
        if record.phase == TaskPhase::Parked {
            record.phase = TaskPhase::Runnable;
            // SAFETY: as in `next_runnable`.
            unsafe { self.kernel().shards.own(self.carrier) }.push_ready(task, front);
        }
    }

    /// The channel registry, for its local operations.
    pub(super) fn channels(&self) -> &'kernel crate::channel::ChannelRegistry {
        &self.kernel().channels
    }

    /// The process kernel's tables, for the local process operations.
    pub(super) fn processes(&self) -> &'kernel super::process::ProcessKernel {
        &self.kernel().kernel
    }
}

impl Local<'_> {
    /// A channel send of a value that carries no capability.
    fn channel_send(&mut self, owner: TaskId, record: *mut super::SchedulerRequest) -> Result<(), Escalate> {
        // SAFETY: the parked owner's request record stays valid until it
        // resumes; it is copied and written back whole, as
        // `handle_task_request` does.
        let mut reply = unsafe { record.read() };
        // SAFETY: as above, for the capabilities the record names.
        if !unsafe { super::carried_capabilities(reply.context) }.is_empty() {
            return Err(Escalate);
        }
        let carrier = self.carrier;
        // SAFETY: `new`'s contract, for this carrier.
        let (result, woken) = unsafe {
            self.channels().send_local(
                carrier,
                owner,
                crate::channel::Sender::from_raw(reply.channel),
                reply.value as usize,
                |task| self.is_local(task),
            )
        }
        .ok_or(Escalate)?;
        if let Some(receiver) = woken {
            self.wake(receiver, false);
        }
        use crate::channel::SendResult;
        reply.status = match result {
            SendResult::Pending => super::RESPONSE_PENDING,
            SendResult::Accepted => super::RESPONSE_OK,
            SendResult::Published(reached) => {
                reply.value = reached as i64;
                super::RESPONSE_OK
            }
            SendResult::Closed(value) => {
                reply.value = value as i64;
                super::RESPONSE_CLOSED
            }
            SendResult::Aborted(value) => {
                reply.value = value as i64;
                super::RESPONSE_ABORTED
            }
        };
        if result != SendResult::Pending {
            self.wake(owner, false);
        }
        // SAFETY: the owner is still parked on this record.
        unsafe { record.write(reply) };
        Ok(())
    }

    /// A channel receive by its receiver's owner.
    fn channel_receive(&mut self, owner: TaskId, record: *mut super::SchedulerRequest) -> Result<(), Escalate> {
        // SAFETY: as in `channel_send`.
        let mut reply = unsafe { record.read() };
        let receiver = crate::channel::Receiver::from_raw(reply.channel, reply.owner, reply.token);
        let carrier = self.carrier;
        // SAFETY: `new`'s contract, for this carrier.
        let (result, woken) = unsafe {
            self.channels()
                .receive_local(carrier, owner, receiver, |task| self.is_local(task))
        }
        .ok_or(Escalate)?;
        if let Some(sender) = woken {
            self.wake(sender, false);
        }
        use crate::channel::ReceiveResult;
        reply.status = match result {
            ReceiveResult::Pending => super::RESPONSE_PENDING,
            ReceiveResult::Received(value) => {
                reply.value = value as i64;
                super::RESPONSE_SUCCEEDED
            }
            ReceiveResult::Drained => super::RESPONSE_DRAINED,
            ReceiveResult::Aborted => super::RESPONSE_ABORTED,
        };
        if result != ReceiveResult::Pending {
            self.wake(owner, false);
        }
        // SAFETY: the owner is still parked on this record.
        unsafe { record.write(reply) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::carrier::{RunKind, Runnable};
    use super::super::jobs::Job;
    use super::super::scope::{Scope, ScopeId, ScopeKind};
    use super::super::*;
    use super::{Carrier, Local};
    use crate::FIXTURE_LOCK;
    use std::ptr::NonNull;

    unsafe extern "C" {
        fn chimi_fixture_generated_many_parks(context: *mut c_void) -> crate::ChimiFiberResult;
    }

    /// A kernel on the real clock with tracing off, and a task that has run
    /// once and parked with no request.
    fn parked() -> (Scheduler, TaskId) {
        let mut scheduler = Scheduler::new(64 * 1024, TraceMode::Off);
        let task = scheduler
            .start_root(chimi_fixture_generated_many_parks)
            .expect("task");
        scheduler.drive();
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Parked);
        (scheduler, task)
    }

    fn local(scheduler: &mut Scheduler) -> Local<'_> {
        // SAFETY: the test has the scheduler to itself.
        unsafe { Local::new(NonNull::from(scheduler), Carrier(0)) }
    }

    /// `task`'s step returning parked on `request`.
    fn park(local: &mut Local<'_>, task: TaskId, request: *mut c_void) -> Result<(), super::Escalate> {
        let runnable = Runnable {
            task,
            fiber: NonNull::dangling(),
            command: ChimiFiberCommand::Resume,
            kind: RunKind::Step,
        };
        let event = ChimiFiberEvent {
            state: ChimiFiberState::Parked as u32,
            value: 0,
            defect_code: 0,
            roots: std::ptr::null(),
            request,
        };
        local.complete(runnable, event)
    }

    fn scope() -> Scope {
        Scope {
            id: ScopeId::from_raw(1),
            kind: ScopeKind::Timeout,
            entered_at: 0,
            timer: None,
            deadline: super::super::clock::Instant::from_nanos(0),
            fired: false,
            draining: false,
            delivered: false,
        }
    }

    fn request(kind: u32) -> SchedulerRequest {
        SchedulerRequest {
            kind,
            status: 0,
            task: 0,
            channel: 0,
            owner: 0,
            token: 0,
            value: 0,
            entry: None,
            context: std::ptr::null_mut(),
        }
    }

    /// The park number counts parks the write side recorded, so a request
    /// the local path cannot serve leaves it, the phase and the request as
    /// they were.
    #[test]
    fn an_escalated_park_changes_nothing() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (mut scheduler, task) = parked();
        let before = {
            let record = &scheduler.tasks[&task];
            (record.phase, record.request, record.park)
        };
        let mut request = request(REQUEST_TIMER_NOW);
        let escalated = park(&mut local(&mut scheduler), task, (&raw mut request).cast());
        assert!(escalated.is_err());
        let record = &scheduler.tasks[&task];
        assert_eq!((record.phase, record.request, record.park), before);
    }

    /// Scopes need the write side, which polls their deadlines and
    /// delivers their cancellations.
    #[test]
    fn a_task_inside_a_scope_escalates() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (mut scheduler, task) = parked();
        scheduler.tasks.get_mut(&task).expect("task").scopes.push(scope());
        let park_before = scheduler.tasks[&task].park;
        assert!(park(&mut local(&mut scheduler), task, std::ptr::null_mut()).is_err());
        assert_eq!(scheduler.tasks[&task].park, park_before);

        scheduler.wake(task).expect("woken");
        assert!(local(&mut scheduler).next_runnable().is_err());
        scheduler.tasks.get_mut(&task).expect("task").scopes.clear();
    }

    /// Released tasks and queued jobs are the write side's housekeeping, so
    /// a step finding either goes there.
    #[test]
    fn housekeeping_escalates() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (mut scheduler, task) = parked();
        scheduler.released_terminal.push(task);
        let mut resumes = 0;
        assert!(local(&mut scheduler).after_resume(&mut resumes).is_err());
        assert_eq!(resumes, 0);
        scheduler.released_terminal.clear();

        scheduler.wake(task).expect("woken");
        scheduler.shards.push_job(Carrier(0), Job::Unwind { task, forced: false });
        assert!(local(&mut scheduler).next_runnable().is_err());
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Runnable);
        scheduler.shards.pop_job(Carrier(0));
    }

    /// A send into a full queue waits: the reply says pending and the
    /// sender stays parked.
    #[test]
    fn a_send_into_a_full_queue_is_pending() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (mut scheduler, task) = parked();
        let channel = scheduler.create_channel(task, 0).expect("channel");
        let receiver = scheduler
            .start_root(chimi_fixture_generated_many_parks)
            .expect("receiver");
        scheduler.drive();
        let receiving = scheduler
            .transfer_receiver(task, channel.receiver, receiver)
            .expect("handed over");
        // A send the write side served gives the task its operation slot.
        assert_eq!(scheduler.channel_send(task, channel.sender, 1), Ok(SendResult::Pending));
        assert_eq!(
            scheduler.channel_receive(receiver, receiving),
            Ok(ReceiveResult::Received(1))
        );
        assert_eq!(scheduler.take_send_result(task), Some(SendResult::Accepted));
        scheduler.drive();
        let mut request = request(REQUEST_CHANNEL_SEND);
        request.channel = channel.sender.channel().get();
        request.value = 2;
        assert!(park(&mut local(&mut scheduler), task, (&raw mut request).cast()).is_ok());
        assert_eq!(request.status, RESPONSE_PENDING);
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Parked);
    }
}
