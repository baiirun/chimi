//! Deterministic structured lifecycle for finite Chimi tasks.

pub use crate::channel::{
    ChannelCapabilities, ChannelError, ChannelEventKind, ChannelId, ChannelMetrics, Completion,
    ReceiveResult, Receiver, SendResult, Sender,
};
use crate::channel::{ChannelRegistry, Observation, Transition};
use crate::{
    ChimiFiber, ChimiFiberCommand, ChimiFiberConfig, ChimiFiberEntry, ChimiFiberEvent,
    ChimiFiberState, chimi_fiber_create, chimi_fiber_destroy, chimi_fiber_resume,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::c_void;
use std::ptr::NonNull;

const SCHEDULER_DEFECT_CODE: i64 = -2;
const CAPABILITY_VIOLATION_DEFECT_CODE: i64 = -3;
const CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE: i64 = -4;

pub const REQUEST_TASK_START: u32 = 1;
pub const REQUEST_TASK_RESULT: u32 = 2;
pub const REQUEST_TASK_CANCEL: u32 = 3;
pub const REQUEST_CHANNEL_BOUNDED: u32 = 4;
pub const REQUEST_CHANNEL_SEND: u32 = 5;
pub const REQUEST_CHANNEL_RECEIVE: u32 = 6;
pub const REQUEST_CHANNEL_CLOSE: u32 = 7;
pub const REQUEST_CHANNEL_ABORT: u32 = 8;
pub const REQUEST_CHANNEL_TRANSFER_RECEIVER: u32 = 9;
pub const REQUEST_CHANNEL_TRANSFER_COMPLETION: u32 = 10;
pub const RESPONSE_UNSET: u32 = 0;
pub const RESPONSE_OK: u32 = 1;
pub const RESPONSE_PENDING: u32 = 2;
pub const RESPONSE_SUCCEEDED: u32 = 3;
pub const RESPONSE_CANCELLED: u32 = 4;
pub const RESPONSE_DEFECTED: u32 = 5;
pub const RESPONSE_STACK_OVERFLOW: u32 = 6;
pub const RESPONSE_ERROR: u32 = 7;
pub const RESPONSE_CLOSED: u32 = 8;
pub const RESPONSE_ABORTED: u32 = 9;
pub const RESPONSE_DRAINED: u32 = 10;

#[repr(C)]
pub struct SchedulerRequest {
    pub kind: u32,
    pub status: u32,
    pub task: u64,
    pub channel: u64,
    pub owner: u64,
    pub token: u64,
    pub value: i64,
    pub entry: Option<ChimiFiberEntry>,
    pub context: *mut c_void,
}

#[repr(C)]
pub struct SchedulerOutcome {
    pub state: u32,
    pub value: i64,
    pub origin: u64,
    pub defect_code: i64,
}

pub const SCHEDULER_TRACE_TASK: u32 = 1;
pub const SCHEDULER_TRACE_CHANNEL: u32 = 2;

#[repr(C)]
pub struct SchedulerTraceEvent {
    pub family: u32,
    pub kind: u32,
    pub sequence: u64,
    pub task: u64,
    pub parent: u64,
    pub cause: u64,
    pub channel: u64,
    pub capacity: u64,
    pub queue_depth: u64,
    pub waiter_depth: u64,
    pub discard_count: u64,
}

pub type SchedulerTraceSink = unsafe extern "C" fn(*mut c_void, *const SchedulerTraceEvent);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TaskId(u64);

impl TaskId {
    pub fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceMode {
    Off,
    Boundaries,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Succeeded(i64),
    Cancelled,
    Defected { origin: TaskId, code: i64 },
    StackOverflow { origin: TaskId },
}

impl TaskOutcome {
    fn is_defect(self) -> bool {
        matches!(self, Self::Defected { .. } | Self::StackOverflow { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskEventKind {
    Started,
    Parked,
    Woken,
    CancellationRequested,
    CleanupCompleted,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskEvent {
    pub sequence: u64,
    pub task: TaskId,
    pub parent: Option<TaskId>,
    pub kind: TaskEventKind,
    pub cause: Option<TaskId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelEvent {
    pub sequence: u64,
    pub channel: ChannelId,
    pub task: Option<TaskId>,
    pub kind: ChannelEventKind,
    pub cause: Option<TaskId>,
    pub capacity: usize,
    pub queue_depth: usize,
    pub waiter_depth: usize,
    pub discard_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerMetrics {
    pub tasks: usize,
    pub channels: usize,
    pub task_events: usize,
    pub channel_events: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskResult {
    Pending,
    Ready(TaskOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    UnknownTask(TaskId),
    WrongOwner {
        task: TaskId,
        expected: Option<TaskId>,
        actual: Option<TaskId>,
    },
    OwnerIsTerminal(TaskId),
    TaskMustBeParked(TaskId),
    TaskStillLive(TaskId),
    Channel(ChannelError),
    FiberCreationFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskPhase {
    Runnable,
    Parked,
    Draining(TaskOutcome),
    Terminal(TaskOutcome),
}

struct OwnedFiber(NonNull<ChimiFiber>);

impl OwnedFiber {
    fn new(
        entry: ChimiFiberEntry,
        context: *mut c_void,
        config: ChimiFiberConfig,
    ) -> Result<Self, SchedulerError> {
        let pointer = NonNull::new(unsafe { chimi_fiber_create(Some(entry), context, config) })
            .ok_or(SchedulerError::FiberCreationFailed)?;
        Ok(Self(pointer))
    }

    fn resume(&mut self, command: ChimiFiberCommand) -> ChimiFiberEvent {
        unsafe { chimi_fiber_resume(self.0.as_ptr(), command as u32) }
    }
}

impl Drop for OwnedFiber {
    fn drop(&mut self) {
        unsafe {
            chimi_fiber_destroy(self.0.as_ptr());
        }
    }
}

struct Task {
    parent: Option<TaskId>,
    children: BTreeSet<TaskId>,
    waiting_on: Option<TaskId>,
    fiber: Option<OwnedFiber>,
    phase: TaskPhase,
    observed: bool,
}

pub struct Scheduler {
    tasks: BTreeMap<TaskId, Task>,
    runnable: VecDeque<TaskId>,
    next_task: u64,
    stack_bytes: usize,
    trace_mode: TraceMode,
    events: Vec<TaskEvent>,
    channels: ChannelRegistry,
    channel_events: Vec<ChannelEvent>,
    constructed_events: u64,
    trace_sink: Option<SchedulerTraceSink>,
    trace_context: *mut c_void,
}

impl Scheduler {
    pub fn new(stack_bytes: usize, trace_mode: TraceMode) -> Self {
        Self {
            tasks: BTreeMap::new(),
            runnable: VecDeque::new(),
            next_task: 1,
            stack_bytes,
            trace_mode,
            events: Vec::new(),
            channels: ChannelRegistry::new(
                trace_mode != TraceMode::Off,
                trace_mode == TraceMode::Full,
            ),
            channel_events: Vec::new(),
            constructed_events: 0,
            trace_sink: None,
            trace_context: std::ptr::null_mut(),
        }
    }

    pub fn start_root(&mut self, entry: ChimiFiberEntry) -> Result<TaskId, SchedulerError> {
        self.start(None, entry, std::ptr::null_mut())
    }

    pub fn start_root_with_context(
        &mut self,
        entry: ChimiFiberEntry,
        context: *mut c_void,
    ) -> Result<TaskId, SchedulerError> {
        self.start(None, entry, context)
    }

    pub fn start_child(
        &mut self,
        parent: TaskId,
        entry: ChimiFiberEntry,
    ) -> Result<TaskId, SchedulerError> {
        let parent_task = self
            .tasks
            .get(&parent)
            .ok_or(SchedulerError::UnknownTask(parent))?;
        if matches!(
            parent_task.phase,
            TaskPhase::Draining(_) | TaskPhase::Terminal(_)
        ) {
            return Err(SchedulerError::OwnerIsTerminal(parent));
        }
        self.start(Some(parent), entry, std::ptr::null_mut())
    }

    pub fn drive(&mut self) {
        while let Some(task) = self.runnable.pop_front() {
            self.run(task);
        }
    }

    pub fn wake(&mut self, task: TaskId) -> Result<(), SchedulerError> {
        self.wake_at(task, false)
    }

    fn wake_front(&mut self, task: TaskId) -> Result<(), SchedulerError> {
        self.wake_at(task, true)
    }

    fn wake_at(&mut self, task: TaskId, front: bool) -> Result<(), SchedulerError> {
        let record = self
            .tasks
            .get_mut(&task)
            .ok_or(SchedulerError::UnknownTask(task))?;
        if record.phase == TaskPhase::Parked {
            record.phase = TaskPhase::Runnable;
            if front {
                self.runnable.push_front(task);
            } else {
                self.runnable.push_back(task);
            }
            self.emit(task, TaskEventKind::Woken, None, true);
        }
        Ok(())
    }

    pub fn result(
        &mut self,
        owner: Option<TaskId>,
        task: TaskId,
    ) -> Result<TaskResult, SchedulerError> {
        if let Err(error) = self.require_owner(owner, task) {
            self.defect_contract_violator(owner);
            return Err(error);
        }
        let phase = self
            .tasks
            .get(&task)
            .expect("ownership validation requires task")
            .phase;
        match phase {
            TaskPhase::Terminal(outcome) => {
                self.tasks.get_mut(&task).expect("terminal task").observed = true;
                Ok(TaskResult::Ready(outcome))
            }
            _ => {
                if let Some(owner) = owner {
                    let owner_record = self.tasks.get_mut(&owner).expect("validated owner");
                    if owner_record.phase == TaskPhase::Parked {
                        owner_record.waiting_on = Some(task);
                    }
                }
                Ok(TaskResult::Pending)
            }
        }
    }

    pub fn cancel(&mut self, owner: Option<TaskId>, task: TaskId) -> Result<(), SchedulerError> {
        if let Err(error) = self.require_owner(owner, task) {
            self.defect_contract_violator(owner);
            return Err(error);
        }
        if matches!(
            self.tasks.get(&task).map(|record| record.phase),
            Some(TaskPhase::Draining(_) | TaskPhase::Terminal(_))
        ) {
            return Ok(());
        }
        self.begin_terminal(task, TaskOutcome::Cancelled, false);
        Ok(())
    }

    pub fn events(&self) -> &[TaskEvent] {
        &self.events
    }

    pub fn channel_events(&self) -> &[ChannelEvent] {
        &self.channel_events
    }

    /// Exports the current trace batch without retaining it in the scheduler.
    pub fn drain_events(&mut self) -> (Vec<TaskEvent>, Vec<ChannelEvent>) {
        (
            std::mem::take(&mut self.events),
            std::mem::take(&mut self.channel_events),
        )
    }

    pub fn metrics(&self) -> SchedulerMetrics {
        SchedulerMetrics {
            tasks: self.tasks.len(),
            channels: self.channels.len(),
            task_events: self.events.len(),
            channel_events: self.channel_events.len(),
        }
    }

    pub fn create_channel(
        &mut self,
        owner: TaskId,
        capacity: usize,
    ) -> Result<ChannelCapabilities, SchedulerError> {
        self.require_live_task(owner)?;
        let (capabilities, transition) = self.channels.create(owner, capacity);
        self.apply_channel_transition(transition);
        Ok(capabilities)
    }

    pub fn channel_send(
        &mut self,
        task: TaskId,
        sender: Sender,
        value: usize,
    ) -> Result<SendResult, SchedulerError> {
        self.require_parked_task(task)?;
        match self.channels.send(task, sender, value) {
            Ok((result, transition)) => {
                self.apply_channel_transition(transition);
                Ok(result)
            }
            Err(error) => self.channel_violation(task, error),
        }
    }

    pub fn channel_receive(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<ReceiveResult, SchedulerError> {
        self.require_parked_task(task)?;
        match self.channels.receive(task, receiver) {
            Ok((result, transition)) => {
                self.apply_channel_transition(transition);
                Ok(result)
            }
            Err(error) => self.channel_violation(task, error),
        }
    }

    pub fn channel_close(
        &mut self,
        task: TaskId,
        completion: Completion,
    ) -> Result<(), SchedulerError> {
        self.require_parked_task(task)?;
        match self.channels.close(task, completion) {
            Ok(transition) => {
                self.apply_channel_transition(transition);
                self.wake(task)?;
                Ok(())
            }
            Err(error) => self.channel_violation(task, error),
        }
    }

    pub fn channel_abort(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<(), SchedulerError> {
        self.require_parked_task(task)?;
        match self.channels.abort(task, receiver) {
            Ok(transition) => {
                self.apply_channel_transition(transition);
                self.wake(task)?;
                Ok(())
            }
            Err(error) => self.channel_violation(task, error),
        }
    }

    pub fn transfer_receiver(
        &mut self,
        task: TaskId,
        receiver: Receiver,
        new_owner: TaskId,
    ) -> Result<Receiver, SchedulerError> {
        self.require_live_task(task)?;
        self.require_live_task(new_owner)?;
        match self.channels.transfer_receiver(task, receiver, new_owner) {
            Ok((receiver, transition)) => {
                self.apply_channel_transition(transition);
                Ok(receiver)
            }
            Err(error) => self.channel_violation(task, error),
        }
    }

    pub fn transfer_completion(
        &mut self,
        task: TaskId,
        completion: Completion,
        new_owner: TaskId,
    ) -> Result<Completion, SchedulerError> {
        self.require_live_task(task)?;
        self.require_live_task(new_owner)?;
        match self
            .channels
            .transfer_completion(task, completion, new_owner)
        {
            Ok((completion, transition)) => {
                self.apply_channel_transition(transition);
                Ok(completion)
            }
            Err(error) => self.channel_violation(task, error),
        }
    }

    pub fn take_send_result(&mut self, task: TaskId) -> Option<SendResult> {
        self.channels.take_send_result(task)
    }

    pub fn take_receive_result(&mut self, task: TaskId) -> Option<ReceiveResult> {
        self.channels.take_receive_result(task)
    }

    pub fn channel_metrics(&self, channel: ChannelId) -> Result<ChannelMetrics, SchedulerError> {
        self.channels
            .metrics(channel)
            .map_err(SchedulerError::Channel)
    }

    /// Releases a channel after the language runtime proves that no capability
    /// value can reach it. Lifecycle checks keep an early release explicit.
    pub fn release_channel(&mut self, channel: ChannelId) -> Result<(), SchedulerError> {
        self.channels
            .release(channel)
            .map_err(SchedulerError::Channel)
    }

    /// Releases an observed terminal task after its handle becomes unreachable.
    pub fn release_task(&mut self, task: TaskId) -> Result<(), SchedulerError> {
        let releasable = self.tasks.get(&task).is_some_and(|record| {
            matches!(record.phase, TaskPhase::Terminal(_))
                && record.observed
                && record.children.is_empty()
        }) && !self
            .tasks
            .values()
            .any(|record| record.parent == Some(task));
        if !releasable {
            return if self.tasks.contains_key(&task) {
                Err(SchedulerError::TaskStillLive(task))
            } else {
                Err(SchedulerError::UnknownTask(task))
            };
        }
        self.tasks.remove(&task);
        Ok(())
    }

    pub fn constructed_events(&self) -> u64 {
        self.constructed_events
    }

    pub fn unobserved_terminal_tasks(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.tasks.iter().filter_map(|(task, record)| {
            matches!(record.phase, TaskPhase::Terminal(_))
                .then_some(*task)
                .filter(|_| !record.observed)
        })
    }

    fn start(
        &mut self,
        parent: Option<TaskId>,
        entry: ChimiFiberEntry,
        context: *mut c_void,
    ) -> Result<TaskId, SchedulerError> {
        let id = TaskId(self.next_task);
        self.next_task = self
            .next_task
            .checked_add(1)
            .expect("task identity space exhausted");
        let fiber = OwnedFiber::new(
            entry,
            context,
            ChimiFiberConfig {
                stack_bytes: self.stack_bytes,
                // Task/channel tracing records semantic scheduler events. A
                // native parked-stack snapshot is a separate diagnostic and
                // is not part of TaskEvent, so capturing one here would add
                // unused symbolization work to every full-trace park.
                capture_backtraces: 0,
            },
        )?;
        self.tasks.insert(
            id,
            Task {
                parent,
                children: BTreeSet::new(),
                waiting_on: None,
                fiber: Some(fiber),
                phase: TaskPhase::Runnable,
                observed: false,
            },
        );
        if let Some(parent) = parent {
            self.tasks
                .get_mut(&parent)
                .expect("validated parent remains live")
                .children
                .insert(id);
        }
        self.runnable.push_back(id);
        self.emit(id, TaskEventKind::Started, None, false);
        Ok(id)
    }

    fn run(&mut self, task: TaskId) {
        if self.tasks.get(&task).map(|record| record.phase) != Some(TaskPhase::Runnable) {
            return;
        }
        let event = self
            .tasks
            .get_mut(&task)
            .and_then(|record| record.fiber.as_mut())
            .expect("runnable task owns a fiber")
            .resume(ChimiFiberCommand::Resume);
        if event.state == ChimiFiberState::Parked as u32 {
            self.tasks.get_mut(&task).expect("running task").phase = TaskPhase::Parked;
            self.emit(task, TaskEventKind::Parked, None, true);
            if !event.request.is_null() {
                self.handle_request(task, event.request.cast());
            }
            return;
        }
        let outcome = self.outcome(task, event);
        self.begin_terminal(task, outcome, true);
    }

    fn outcome(&self, task: TaskId, event: ChimiFiberEvent) -> TaskOutcome {
        match event.state {
            value if value == ChimiFiberState::Succeeded as u32 => {
                TaskOutcome::Succeeded(event.value)
            }
            value if value == ChimiFiberState::Cancelled as u32 => TaskOutcome::Cancelled,
            value if value == ChimiFiberState::Defected as u32 => TaskOutcome::Defected {
                origin: task,
                code: event.defect_code,
            },
            value if value == ChimiFiberState::StackOverflow as u32 => {
                TaskOutcome::StackOverflow { origin: task }
            }
            _ => TaskOutcome::Defected {
                origin: task,
                code: SCHEDULER_DEFECT_CODE,
            },
        }
    }

    fn handle_request(&mut self, owner: TaskId, request: *mut SchedulerRequest) {
        // SAFETY: `chimi_fiber_request` keeps the request on the parked fiber's
        // stack until this scheduler resumes that fiber.
        let request = unsafe { &mut *request };
        request.status = RESPONSE_UNSET;
        match request.kind {
            REQUEST_TASK_START => {
                let Some(entry) = request.entry else {
                    request.status = RESPONSE_ERROR;
                    let _ = self.wake(owner);
                    return;
                };
                match self.start(Some(owner), entry, request.context) {
                    Ok(task) => {
                        request.task = task.get();
                        request.status = RESPONSE_OK;
                    }
                    Err(_) => request.status = RESPONSE_ERROR,
                }
                // The parent must publish any transferred capture tokens
                // before the newly runnable child can observe its closure.
                let _ = self.wake_front(owner);
            }
            REQUEST_TASK_RESULT => match self.result(Some(owner), TaskId(request.task)) {
                Ok(TaskResult::Pending) => request.status = RESPONSE_PENDING,
                Ok(TaskResult::Ready(outcome)) => {
                    match outcome {
                        TaskOutcome::Succeeded(value) => {
                            request.status = RESPONSE_SUCCEEDED;
                            request.value = value;
                        }
                        TaskOutcome::Cancelled => request.status = RESPONSE_CANCELLED,
                        TaskOutcome::Defected { origin, code } => {
                            request.status = RESPONSE_DEFECTED;
                            request.owner = origin.get();
                            request.value = code;
                        }
                        TaskOutcome::StackOverflow { origin } => {
                            request.status = RESPONSE_STACK_OVERFLOW;
                            request.owner = origin.get();
                        }
                    }
                    let _ = self.wake(owner);
                }
                Err(_) => request.status = RESPONSE_ERROR,
            },
            REQUEST_TASK_CANCEL => {
                request.status = if self.cancel(Some(owner), TaskId(request.task)).is_ok() {
                    RESPONSE_OK
                } else {
                    RESPONSE_ERROR
                };
                let _ = self.wake(owner);
            }
            REQUEST_CHANNEL_BOUNDED => {
                match usize::try_from(request.value)
                    .ok()
                    .and_then(|capacity| self.create_channel(owner, capacity).ok())
                {
                    Some(capabilities) => {
                        request.channel = capabilities.sender.channel().get();
                        request.owner = owner.get();
                        request.token = 1;
                        request.status = RESPONSE_OK;
                    }
                    None => request.status = RESPONSE_ERROR,
                }
                let _ = self.wake(owner);
            }
            REQUEST_CHANNEL_SEND => {
                let result = self.take_send_result(owner).or_else(|| {
                    self.channel_send(
                        owner,
                        Sender::from_raw(request.channel),
                        request.value as usize,
                    )
                    .ok()
                });
                match result {
                    Some(SendResult::Pending) => request.status = RESPONSE_PENDING,
                    Some(SendResult::Accepted) => {
                        let _ = self.take_send_result(owner);
                        request.status = RESPONSE_OK;
                        let _ = self.wake(owner);
                    }
                    Some(SendResult::Closed(value)) => {
                        let _ = self.take_send_result(owner);
                        request.value = value as i64;
                        request.status = RESPONSE_CLOSED;
                        let _ = self.wake(owner);
                    }
                    Some(SendResult::Aborted(value)) => {
                        let _ = self.take_send_result(owner);
                        request.value = value as i64;
                        request.status = RESPONSE_ABORTED;
                        let _ = self.wake(owner);
                    }
                    None => request.status = RESPONSE_ERROR,
                }
            }
            REQUEST_CHANNEL_RECEIVE => {
                let receiver = Receiver::from_raw(request.channel, request.owner, request.token);
                let result = self
                    .take_receive_result(owner)
                    .or_else(|| self.channel_receive(owner, receiver).ok());
                match result {
                    Some(ReceiveResult::Pending) => request.status = RESPONSE_PENDING,
                    Some(ReceiveResult::Received(value)) => {
                        let _ = self.take_receive_result(owner);
                        request.value = value as i64;
                        request.status = RESPONSE_SUCCEEDED;
                        let _ = self.wake(owner);
                    }
                    Some(ReceiveResult::Drained) => {
                        let _ = self.take_receive_result(owner);
                        request.status = RESPONSE_DRAINED;
                        let _ = self.wake(owner);
                    }
                    Some(ReceiveResult::Aborted) => {
                        let _ = self.take_receive_result(owner);
                        request.status = RESPONSE_ABORTED;
                        let _ = self.wake(owner);
                    }
                    None => request.status = RESPONSE_ERROR,
                }
            }
            REQUEST_CHANNEL_CLOSE => {
                let completion =
                    Completion::from_raw(request.channel, request.owner, request.token);
                request.status = if self.channel_close(owner, completion).is_ok() {
                    RESPONSE_OK
                } else {
                    RESPONSE_ERROR
                };
                let _ = self.wake(owner);
            }
            REQUEST_CHANNEL_ABORT => {
                let receiver = Receiver::from_raw(request.channel, request.owner, request.token);
                request.status = if self.channel_abort(owner, receiver).is_ok() {
                    RESPONSE_OK
                } else {
                    RESPONSE_ERROR
                };
                let _ = self.wake(owner);
            }
            REQUEST_CHANNEL_TRANSFER_RECEIVER => {
                let receiver = Receiver::from_raw(request.channel, request.owner, request.token);
                match self.transfer_receiver(owner, receiver, TaskId(request.task)) {
                    Ok(receiver) => {
                        request.owner = receiver.owner().get();
                        request.token = receiver.token();
                        request.status = RESPONSE_OK;
                    }
                    Err(_) => request.status = RESPONSE_ERROR,
                }
                let _ = self.wake_front(owner);
            }
            REQUEST_CHANNEL_TRANSFER_COMPLETION => {
                let completion =
                    Completion::from_raw(request.channel, request.owner, request.token);
                match self.transfer_completion(owner, completion, TaskId(request.task)) {
                    Ok(completion) => {
                        request.owner = completion.owner().get();
                        request.token = completion.token();
                        request.status = RESPONSE_OK;
                    }
                    Err(_) => request.status = RESPONSE_ERROR,
                }
                let _ = self.wake_front(owner);
            }
            _ => {
                request.status = RESPONSE_ERROR;
                let _ = self.wake(owner);
            }
        }
    }

    fn begin_terminal(&mut self, task: TaskId, mut outcome: TaskOutcome, fiber_is_terminal: bool) {
        let phase = self.tasks.get(&task).map(|record| record.phase);
        if matches!(phase, Some(TaskPhase::Draining(_) | TaskPhase::Terminal(_))) {
            return;
        }
        self.tasks.get_mut(&task).expect("known task").phase = TaskPhase::Draining(outcome);

        let channel_cleanup = self.channels.cleanup_task(task);
        self.apply_channel_transition(channel_cleanup);

        if !fiber_is_terminal {
            self.emit(
                task,
                TaskEventKind::CancellationRequested,
                cause(outcome),
                false,
            );
            let cleanup = self
                .tasks
                .get_mut(&task)
                .and_then(|record| record.fiber.as_mut())
                .expect("non-terminal task owns a fiber")
                .resume(ChimiFiberCommand::Cancel);
            if !outcome.is_defect() {
                outcome = match cleanup.state {
                    value if value == ChimiFiberState::Defected as u32 => TaskOutcome::Defected {
                        origin: task,
                        code: cleanup.defect_code,
                    },
                    value if value == ChimiFiberState::StackOverflow as u32 => {
                        TaskOutcome::StackOverflow { origin: task }
                    }
                    value if value == ChimiFiberState::Parked as u32 => TaskOutcome::Defected {
                        origin: task,
                        code: SCHEDULER_DEFECT_CODE,
                    },
                    _ => outcome,
                };
                self.tasks.get_mut(&task).expect("draining task").phase =
                    TaskPhase::Draining(outcome);
            }
        }
        self.tasks.get_mut(&task).expect("draining task").fiber = None;

        let children: Vec<_> = self
            .tasks
            .get(&task)
            .expect("draining task")
            .children
            .iter()
            .copied()
            .collect();
        for child in children {
            self.begin_terminal(child, TaskOutcome::Cancelled, false);
        }
        debug_assert!(
            self.tasks
                .get(&task)
                .expect("draining task")
                .children
                .is_empty()
        );
        self.emit(task, TaskEventKind::CleanupCompleted, cause(outcome), false);
        self.publish_terminal(task, outcome);
    }

    fn publish_terminal(&mut self, task: TaskId, outcome: TaskOutcome) {
        let parent = self.tasks.get(&task).expect("known task").parent;
        self.tasks.get_mut(&task).expect("known task").phase = TaskPhase::Terminal(outcome);
        if let Some(parent) = parent {
            self.tasks
                .get_mut(&parent)
                .expect("structured parent outlives child")
                .children
                .remove(&task);
        }
        self.emit(task, TaskEventKind::Terminal, cause(outcome), false);

        let waiting_parent = parent.filter(|parent| {
            self.tasks.get(parent).is_some_and(|record| {
                record.phase == TaskPhase::Parked && record.waiting_on == Some(task)
            })
        });
        if let Some(parent) = waiting_parent {
            self.tasks
                .get_mut(&parent)
                .expect("waiting parent")
                .waiting_on = None;
            match outcome {
                TaskOutcome::Succeeded(_) => {
                    self.tasks.get_mut(&parent).expect("waiting parent").phase =
                        TaskPhase::Runnable;
                    self.runnable.push_back(parent);
                    self.emit(parent, TaskEventKind::Woken, Some(task), true);
                }
                TaskOutcome::Cancelled => {
                    self.begin_terminal(parent, TaskOutcome::Cancelled, false);
                }
                TaskOutcome::Defected { .. } | TaskOutcome::StackOverflow { .. } => {
                    self.begin_terminal(parent, outcome, false);
                }
            }
        } else if outcome.is_defect()
            && parent.is_some_and(|parent| {
                matches!(
                    self.tasks.get(&parent).map(|record| record.phase),
                    Some(TaskPhase::Runnable | TaskPhase::Parked)
                )
            })
        {
            self.begin_terminal(parent.expect("checked parent"), outcome, false);
        }
    }

    fn require_owner(&self, owner: Option<TaskId>, task: TaskId) -> Result<(), SchedulerError> {
        let expected = self
            .tasks
            .get(&task)
            .ok_or(SchedulerError::UnknownTask(task))?
            .parent;
        if expected != owner {
            return Err(SchedulerError::WrongOwner {
                task,
                expected,
                actual: owner,
            });
        }
        Ok(())
    }

    fn require_live_task(&self, task: TaskId) -> Result<(), SchedulerError> {
        let phase = self
            .tasks
            .get(&task)
            .ok_or(SchedulerError::UnknownTask(task))?
            .phase;
        if matches!(phase, TaskPhase::Draining(_) | TaskPhase::Terminal(_)) {
            return Err(SchedulerError::OwnerIsTerminal(task));
        }
        Ok(())
    }

    fn require_parked_task(&self, task: TaskId) -> Result<(), SchedulerError> {
        self.require_live_task(task)?;
        if self.tasks.get(&task).expect("live task").phase != TaskPhase::Parked {
            return Err(SchedulerError::TaskMustBeParked(task));
        }
        Ok(())
    }

    fn defect_contract_violator(&mut self, owner: Option<TaskId>) {
        let Some(owner) = owner else {
            return;
        };
        if matches!(
            self.tasks.get(&owner).map(|record| record.phase),
            Some(TaskPhase::Runnable | TaskPhase::Parked)
        ) {
            self.begin_terminal(
                owner,
                TaskOutcome::Defected {
                    origin: owner,
                    code: CAPABILITY_VIOLATION_DEFECT_CODE,
                },
                false,
            );
        }
    }

    fn emit(&mut self, task: TaskId, kind: TaskEventKind, cause: Option<TaskId>, full_only: bool) {
        if self.trace_mode == TraceMode::Off || full_only && self.trace_mode != TraceMode::Full {
            return;
        }
        self.constructed_events += 1;
        let event = TaskEvent {
            sequence: self.constructed_events,
            task,
            parent: self.tasks.get(&task).and_then(|record| record.parent),
            kind,
            cause,
        };
        self.events.push(event);
        self.publish_trace(SchedulerTraceEvent {
            family: SCHEDULER_TRACE_TASK,
            kind: kind as u32 + 1,
            sequence: event.sequence,
            task: event.task.get(),
            parent: event.parent.map_or(0, TaskId::get),
            cause: event.cause.map_or(0, TaskId::get),
            channel: 0,
            capacity: 0,
            queue_depth: 0,
            waiter_depth: 0,
            discard_count: 0,
        });
    }

    fn apply_channel_transition(&mut self, transition: Transition) {
        for observation in transition.observations {
            self.emit_channel(observation);
        }
        for task in transition.wakes {
            let _ = self.wake(task);
        }
    }

    fn emit_channel(&mut self, observation: Observation) {
        self.constructed_events += 1;
        let event = ChannelEvent {
            sequence: self.constructed_events,
            channel: observation.channel,
            task: observation.task,
            kind: observation.kind,
            cause: observation.cause,
            capacity: observation.capacity,
            queue_depth: observation.queue_depth,
            waiter_depth: observation.waiter_depth,
            discard_count: observation.discard_count,
        };
        self.channel_events.push(event);
        self.publish_trace(SchedulerTraceEvent {
            family: SCHEDULER_TRACE_CHANNEL,
            kind: event.kind as u32 + 1,
            sequence: event.sequence,
            task: event.task.map_or(0, TaskId::get),
            parent: 0,
            cause: event.cause.map_or(0, TaskId::get),
            channel: event.channel.get(),
            capacity: event.capacity as u64,
            queue_depth: event.queue_depth as u64,
            waiter_depth: event.waiter_depth as u64,
            discard_count: event.discard_count as u64,
        });
    }

    fn publish_trace(&self, event: SchedulerTraceEvent) {
        if let Some(sink) = self.trace_sink {
            // SAFETY: the C entry contract requires the caller-provided sink
            // and context to remain valid until scheduler execution returns.
            unsafe { sink(self.trace_context, &event) };
        }
    }

    fn channel_violation<T>(
        &mut self,
        task: TaskId,
        error: ChannelError,
    ) -> Result<T, SchedulerError> {
        self.begin_terminal(
            task,
            TaskOutcome::Defected {
                origin: task,
                code: CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE,
            },
            false,
        );
        Err(SchedulerError::Channel(error))
    }
}

fn scheduler_defect_outcome(code: i64) -> SchedulerOutcome {
    SchedulerOutcome {
        state: ChimiFiberState::Defected as u32,
        value: 0,
        origin: 0,
        defect_code: code,
    }
}

#[unsafe(no_mangle)]
/// Executes one finite root and every structured descendant to a terminal outcome.
///
/// # Safety
///
/// `entry` and `context` must satisfy the Chimi fiber entry contract. The
/// context must remain valid until this call returns.
pub unsafe extern "C" fn chimi_scheduler_execute(
    entry: Option<ChimiFiberEntry>,
    context: *mut c_void,
    stack_bytes: usize,
    trace_mode: u32,
    trace_sink: Option<SchedulerTraceSink>,
    trace_context: *mut c_void,
) -> SchedulerOutcome {
    let Some(entry) = entry else {
        return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    let trace_mode = match trace_mode {
        0 => TraceMode::Off,
        1 => TraceMode::Boundaries,
        2 => TraceMode::Full,
        _ => return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE),
    };
    let mut scheduler = Scheduler::new(stack_bytes, trace_mode);
    scheduler.trace_sink = trace_sink;
    scheduler.trace_context = trace_context;
    let Ok(root) = scheduler.start_root_with_context(entry, context) else {
        return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    scheduler.drive();
    let outcome = match scheduler.result(None, root) {
        Ok(TaskResult::Ready(outcome)) => outcome,
        Ok(TaskResult::Pending) | Err(_) => {
            return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
        }
    };
    match outcome {
        TaskOutcome::Succeeded(value) => SchedulerOutcome {
            state: ChimiFiberState::Succeeded as u32,
            value,
            origin: 0,
            defect_code: 0,
        },
        TaskOutcome::Cancelled => SchedulerOutcome {
            state: ChimiFiberState::Cancelled as u32,
            value: 0,
            origin: 0,
            defect_code: 0,
        },
        TaskOutcome::Defected { origin, code } => SchedulerOutcome {
            state: ChimiFiberState::Defected as u32,
            value: 0,
            origin: origin.get(),
            defect_code: code,
        },
        TaskOutcome::StackOverflow { origin } => SchedulerOutcome {
            state: ChimiFiberState::StackOverflow as u32,
            value: 0,
            origin: origin.get(),
            defect_code: 0,
        },
    }
}

fn cause(outcome: TaskOutcome) -> Option<TaskId> {
    match outcome {
        TaskOutcome::Defected { origin, .. } | TaskOutcome::StackOverflow { origin } => {
            Some(origin)
        }
        TaskOutcome::Succeeded(_) | TaskOutcome::Cancelled => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChimiFiberResult, FIXTURE_LOCK};

    unsafe extern "C" {
        fn chimi_fixture_generated_entry(context: *mut std::ffi::c_void) -> ChimiFiberResult;
        fn chimi_fixture_generated_defect(context: *mut std::ffi::c_void) -> ChimiFiberResult;
        fn chimi_fixture_generated_expected_failure_value(
            context: *mut std::ffi::c_void,
        ) -> ChimiFiberResult;
        fn chimi_fixture_generated_cancel_defect(
            context: *mut std::ffi::c_void,
        ) -> ChimiFiberResult;
        fn chimi_fixture_generated_many_parks(context: *mut std::ffi::c_void) -> ChimiFiberResult;
        fn chimi_fixture_reset_cleanup();
        fn chimi_fixture_cleanup_count() -> i32;
    }

    fn scheduler(trace_mode: TraceMode) -> Scheduler {
        Scheduler::new(64 * 1024, trace_mode)
    }

    fn start_parked(scheduler: &mut Scheduler) -> TaskId {
        let task = scheduler
            .start_root(chimi_fixture_generated_many_parks)
            .expect("task");
        scheduler.drive();
        task
    }

    #[test]
    fn ordinary_call_parking_wakes_and_publishes_only_after_cleanup() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        unsafe {
            chimi_fixture_reset_cleanup();
        }
        let mut scheduler = scheduler(TraceMode::Off);
        let root = scheduler
            .start_root(chimi_fixture_generated_entry)
            .expect("root");

        scheduler.drive();
        assert_eq!(scheduler.result(None, root), Ok(TaskResult::Pending));
        assert_eq!(scheduler.constructed_events(), 0);
        assert!(scheduler.events().is_empty());

        scheduler.wake(root).expect("wake root");
        scheduler.drive();
        assert_eq!(unsafe { chimi_fixture_cleanup_count() }, 1);
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Succeeded(42)))
        );
        assert!(scheduler.unobserved_terminal_tasks().next().is_none());
    }

    #[test]
    fn expected_terminal_values_do_not_cancel_siblings() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Boundaries);
        let root = scheduler
            .start_root(chimi_fixture_generated_entry)
            .expect("root");
        let sibling = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("sibling");
        let expected_failure = scheduler
            .start_child(root, chimi_fixture_generated_expected_failure_value)
            .expect("expected failure value");

        scheduler.drive();
        // The native Chimi outcome encoding is the fiber's success value. The
        // scheduler must not reinterpret an expected failure as a defect.
        assert_eq!(scheduler.result(None, root), Ok(TaskResult::Pending));
        assert_eq!(
            scheduler.result(Some(root), expected_failure),
            Ok(TaskResult::Ready(TaskOutcome::Succeeded(-5)))
        );
        assert_eq!(
            scheduler.result(Some(root), sibling),
            Ok(TaskResult::Pending)
        );

        scheduler.wake(sibling).expect("wake sibling");
        scheduler.drive();
        assert_eq!(
            scheduler.result(Some(root), sibling),
            Ok(TaskResult::Ready(TaskOutcome::Succeeded(42)))
        );
        scheduler.cancel(None, root).expect("cancel root");
    }

    #[test]
    fn parent_completion_drains_children_before_stable_terminal_events() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Boundaries);
        let root = scheduler
            .start_root(chimi_fixture_generated_expected_failure_value)
            .expect("root");
        let child = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("child");

        scheduler.drive();
        assert_eq!(
            scheduler.result(Some(root), child),
            Ok(TaskResult::Ready(TaskOutcome::Cancelled))
        );
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Succeeded(-5)))
        );
        let events: Vec<_> = scheduler
            .events()
            .iter()
            .map(|event| (event.task, event.kind, event.cause))
            .collect();
        assert_eq!(
            events,
            [
                (root, TaskEventKind::Started, None),
                (child, TaskEventKind::Started, None),
                (child, TaskEventKind::CancellationRequested, None),
                (child, TaskEventKind::CleanupCompleted, None),
                (child, TaskEventKind::Terminal, None),
                (root, TaskEventKind::CleanupCompleted, None),
                (root, TaskEventKind::Terminal, None),
            ]
        );
    }

    #[test]
    fn child_defect_cancels_siblings_and_defects_its_owner_with_one_cause() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        unsafe {
            chimi_fixture_reset_cleanup();
        }
        let mut scheduler = scheduler(TraceMode::Full);
        let root = scheduler
            .start_root(chimi_fixture_generated_entry)
            .expect("root");
        let sibling = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("sibling");
        let defect = scheduler
            .start_child(root, chimi_fixture_generated_defect)
            .expect("defecting child");

        scheduler.drive();
        let cause = TaskOutcome::Defected {
            origin: defect,
            code: 77,
        };
        assert_eq!(scheduler.result(None, root), Ok(TaskResult::Ready(cause)));
        assert_eq!(
            scheduler.result(Some(root), sibling),
            Ok(TaskResult::Ready(TaskOutcome::Cancelled))
        );
        // The defecting child records its entry, then its owner and sibling
        // each complete cooperative cleanup before terminal publication.
        assert_eq!(unsafe { chimi_fixture_cleanup_count() }, 3);
        assert!(scheduler.events().iter().any(|event| {
            event.task == root
                && event.kind == TaskEventKind::Terminal
                && event.cause == Some(defect)
        }));
    }

    #[test]
    fn cancellation_is_recursive_idempotent_and_publishes_descendants_first() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        unsafe {
            chimi_fixture_reset_cleanup();
        }
        let mut scheduler = scheduler(TraceMode::Full);
        let root = scheduler
            .start_root(chimi_fixture_generated_entry)
            .expect("root");
        let child = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("child");
        let grandchild = scheduler
            .start_child(child, chimi_fixture_generated_entry)
            .expect("grandchild");
        scheduler.drive();

        scheduler.cancel(None, root).expect("cancel root");
        scheduler.cancel(None, root).expect("repeat cancellation");
        assert_eq!(unsafe { chimi_fixture_cleanup_count() }, 3);
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Cancelled))
        );

        let terminals: Vec<_> = scheduler
            .events()
            .iter()
            .filter(|event| event.kind == TaskEventKind::Terminal)
            .map(|event| event.task)
            .collect();
        assert_eq!(terminals, [grandchild, child, root]);
    }

    #[test]
    fn wrong_owner_use_defects_the_offending_task_and_contains_it() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Full);
        let root = scheduler
            .start_root(chimi_fixture_generated_entry)
            .expect("root");
        let child = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("child");
        let intruder = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("intruder");
        scheduler.drive();

        assert_eq!(
            scheduler.result(Some(intruder), child),
            Err(SchedulerError::WrongOwner {
                task: child,
                expected: Some(root),
                actual: Some(intruder),
            })
        );
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Defected {
                origin: intruder,
                code: CAPABILITY_VIOLATION_DEFECT_CODE,
            }))
        );
    }

    #[test]
    fn a_defect_during_cancellation_wins_the_terminal_race() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Boundaries);
        let root = scheduler
            .start_root(chimi_fixture_generated_cancel_defect)
            .expect("root");
        scheduler.drive();
        scheduler.cancel(None, root).expect("cancel root");
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Defected {
                origin: root,
                code: 88,
            }))
        );
    }

    #[test]
    fn waiting_for_a_cancelled_child_propagates_cancellation_to_the_owner() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Full);
        let root = scheduler
            .start_root(chimi_fixture_generated_entry)
            .expect("root");
        let child = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("child");
        scheduler.drive();

        assert_eq!(scheduler.result(Some(root), child), Ok(TaskResult::Pending));
        scheduler.cancel(Some(root), child).expect("cancel child");
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Cancelled))
        );
    }

    #[test]
    fn trace_boundaries_omit_scheduler_motion_while_full_includes_it() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        for (mode, includes_motion) in [(TraceMode::Boundaries, false), (TraceMode::Full, true)] {
            let mut scheduler = scheduler(mode);
            let root = scheduler
                .start_root(chimi_fixture_generated_entry)
                .expect("root");
            scheduler.drive();
            scheduler.wake(root).expect("wake root");
            scheduler.drive();
            assert_eq!(
                scheduler.events().iter().any(|event| matches!(
                    event.kind,
                    TaskEventKind::Parked | TaskEventKind::Woken
                )),
                includes_motion
            );
        }
    }

    #[test]
    fn zero_capacity_channels_rendezvous_without_buffering() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Full);
        let receiver_task = start_parked(&mut scheduler);
        let sender_task = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(receiver_task, 0).expect("channel");

        assert_eq!(
            scheduler.channel_send(sender_task, channel.sender, 41),
            Ok(SendResult::Pending)
        );
        assert_eq!(
            scheduler.channel_metrics(channel.sender.channel()),
            Ok(ChannelMetrics {
                capacity: 0,
                queue_depth: 0,
                waiting_senders: 1,
                waiting_receiver: false,
            })
        );
        assert_eq!(
            scheduler.channel_receive(receiver_task, channel.receiver),
            Ok(ReceiveResult::Received(41))
        );
        assert_eq!(
            scheduler.take_send_result(sender_task),
            Some(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.take_receive_result(receiver_task),
            Some(ReceiveResult::Received(41))
        );
        assert!(
            scheduler
                .channel_events()
                .iter()
                .all(|event| event.queue_depth == 0)
        );
        assert!(scheduler.channel_events().iter().any(|event| {
            event.kind == ChannelEventKind::SendAccepted && event.task == Some(sender_task)
        }));
        assert!(scheduler.channel_events().iter().any(|event| {
            event.kind == ChannelEventKind::ReceiveDelivered && event.task == Some(receiver_task)
        }));
    }

    #[test]
    fn bounded_channels_preserve_fifo_and_prevent_sender_barging() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Off);
        let receiver_task = start_parked(&mut scheduler);
        let first = start_parked(&mut scheduler);
        let second = start_parked(&mut scheduler);
        let third = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(receiver_task, 1).expect("channel");

        assert_eq!(
            scheduler.channel_send(first, channel.sender, 1),
            Ok(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.take_send_result(first),
            Some(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.channel_send(second, channel.sender, 2),
            Ok(SendResult::Pending)
        );
        assert_eq!(
            scheduler.channel_send(third, channel.sender, 3),
            Ok(SendResult::Pending)
        );
        assert_eq!(
            scheduler.channel_metrics(channel.sender.channel()),
            Ok(ChannelMetrics {
                capacity: 1,
                queue_depth: 1,
                waiting_senders: 2,
                waiting_receiver: false,
            })
        );

        assert_eq!(
            scheduler.channel_receive(receiver_task, channel.receiver),
            Ok(ReceiveResult::Received(1))
        );
        assert_eq!(
            scheduler.take_receive_result(receiver_task),
            Some(ReceiveResult::Received(1))
        );
        assert_eq!(
            scheduler.take_send_result(second),
            Some(SendResult::Accepted)
        );
        scheduler.drive();

        assert_eq!(
            scheduler.channel_receive(receiver_task, channel.receiver),
            Ok(ReceiveResult::Received(2))
        );
        assert_eq!(
            scheduler.take_receive_result(receiver_task),
            Some(ReceiveResult::Received(2))
        );
        assert_eq!(
            scheduler.take_send_result(third),
            Some(SendResult::Accepted)
        );
        scheduler.drive();

        assert_eq!(
            scheduler.channel_receive(receiver_task, channel.receiver),
            Ok(ReceiveResult::Received(3))
        );
        assert_eq!(
            scheduler.take_receive_result(receiver_task),
            Some(ReceiveResult::Received(3))
        );
        assert_eq!(
            scheduler
                .channel_metrics(channel.sender.channel())
                .expect("metrics")
                .queue_depth,
            0
        );
    }

    #[test]
    fn close_rejects_waiters_and_future_sends_then_drains_accepted_values() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Boundaries);
        let owner = start_parked(&mut scheduler);
        let accepted = start_parked(&mut scheduler);
        let waiting = start_parked(&mut scheduler);
        let future = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(owner, 1).expect("channel");

        assert_eq!(
            scheduler.channel_send(accepted, channel.sender, 10),
            Ok(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.take_send_result(accepted),
            Some(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.channel_send(waiting, channel.sender, 20),
            Ok(SendResult::Pending)
        );
        scheduler
            .channel_close(owner, channel.completion)
            .expect("close");
        assert_eq!(
            scheduler.take_send_result(waiting),
            Some(SendResult::Closed(20))
        );
        scheduler.drive();

        assert_eq!(
            scheduler.channel_receive(owner, channel.receiver),
            Ok(ReceiveResult::Received(10))
        );
        assert_eq!(
            scheduler.take_receive_result(owner),
            Some(ReceiveResult::Received(10))
        );
        scheduler.drive();
        assert_eq!(
            scheduler.channel_receive(owner, channel.receiver),
            Ok(ReceiveResult::Drained)
        );
        assert_eq!(
            scheduler.take_receive_result(owner),
            Some(ReceiveResult::Drained)
        );
        assert_eq!(
            scheduler.channel_send(future, channel.sender, 30),
            Ok(SendResult::Closed(30))
        );
        assert_eq!(
            scheduler.take_send_result(future),
            Some(SendResult::Closed(30))
        );
        assert!(
            scheduler
                .channel_events()
                .iter()
                .any(|event| event.kind == ChannelEventKind::Closed)
        );
        assert!(
            scheduler
                .channel_events()
                .iter()
                .any(|event| event.kind == ChannelEventKind::Drained)
        );
    }

    #[test]
    fn abort_discards_buffered_values_and_returns_unaccepted_values() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Boundaries);
        let owner = start_parked(&mut scheduler);
        let accepted = start_parked(&mut scheduler);
        let waiting = start_parked(&mut scheduler);
        let future = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(owner, 1).expect("channel");

        assert_eq!(
            scheduler.channel_send(accepted, channel.sender, 10),
            Ok(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.take_send_result(accepted),
            Some(SendResult::Accepted)
        );
        assert_eq!(
            scheduler.channel_send(waiting, channel.sender, 20),
            Ok(SendResult::Pending)
        );
        scheduler
            .channel_abort(owner, channel.receiver)
            .expect("abort");
        assert_eq!(
            scheduler.take_send_result(waiting),
            Some(SendResult::Aborted(20))
        );
        assert_eq!(
            scheduler
                .channel_metrics(channel.sender.channel())
                .expect("metrics")
                .queue_depth,
            0
        );
        scheduler.drive();
        assert_eq!(
            scheduler.channel_receive(owner, channel.receiver),
            Ok(ReceiveResult::Aborted)
        );
        assert_eq!(
            scheduler.take_receive_result(owner),
            Some(ReceiveResult::Aborted)
        );
        assert_eq!(
            scheduler.channel_send(future, channel.sender, 30),
            Ok(SendResult::Aborted(30))
        );
        assert_eq!(
            scheduler.take_send_result(future),
            Some(SendResult::Aborted(30))
        );
        assert!(
            scheduler.channel_events().iter().any(|event| {
                event.kind == ChannelEventKind::Aborted && event.discard_count == 1
            })
        );
    }

    #[test]
    fn cancellation_linearizes_before_or_after_rendezvous_without_redelivery() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");

        let mut before = scheduler(TraceMode::Full);
        let receiver = start_parked(&mut before);
        let sender = start_parked(&mut before);
        let channel = before.create_channel(receiver, 0).expect("channel");
        assert_eq!(
            before.channel_send(sender, channel.sender, 7),
            Ok(SendResult::Pending)
        );
        before.cancel(None, sender).expect("cancel sender");
        assert_eq!(
            before.channel_receive(receiver, channel.receiver),
            Ok(ReceiveResult::Pending)
        );
        assert!(before.channel_events().iter().any(|event| {
            event.task == Some(sender) && event.kind == ChannelEventKind::SendCancelled
        }));

        let mut after = scheduler(TraceMode::Full);
        let receiver = start_parked(&mut after);
        let sender = start_parked(&mut after);
        let channel = after.create_channel(receiver, 0).expect("channel");
        assert_eq!(
            after.channel_send(sender, channel.sender, 9),
            Ok(SendResult::Pending)
        );
        assert_eq!(
            after.channel_receive(receiver, channel.receiver),
            Ok(ReceiveResult::Received(9))
        );
        after.cancel(None, sender).expect("cancel sender");
        assert_eq!(
            after.take_receive_result(receiver),
            Some(ReceiveResult::Received(9))
        );
        assert_eq!(after.take_send_result(sender), None);
    }

    #[test]
    fn receive_cancellation_has_one_winner_with_arriving_values() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");

        let mut cancellation_first = scheduler(TraceMode::Full);
        let receiver = start_parked(&mut cancellation_first);
        let sender = start_parked(&mut cancellation_first);
        let channel = cancellation_first
            .create_channel(receiver, 0)
            .expect("channel");
        assert_eq!(
            cancellation_first.channel_receive(receiver, channel.receiver),
            Ok(ReceiveResult::Pending)
        );
        cancellation_first
            .cancel(None, receiver)
            .expect("cancel receiver");
        assert_eq!(
            cancellation_first.channel_send(sender, channel.sender, 12),
            Ok(SendResult::Aborted(12))
        );
        assert_eq!(
            cancellation_first.take_send_result(sender),
            Some(SendResult::Aborted(12))
        );
        assert!(cancellation_first.channel_events().iter().any(|event| {
            event.task == Some(receiver) && event.kind == ChannelEventKind::ReceiveCancelled
        }));

        let mut delivery_first = scheduler(TraceMode::Full);
        let receiver = start_parked(&mut delivery_first);
        let sender = start_parked(&mut delivery_first);
        let later_sender = start_parked(&mut delivery_first);
        let channel = delivery_first.create_channel(receiver, 0).expect("channel");
        assert_eq!(
            delivery_first.channel_receive(receiver, channel.receiver),
            Ok(ReceiveResult::Pending)
        );
        assert_eq!(
            delivery_first.channel_send(sender, channel.sender, 13),
            Ok(SendResult::Accepted)
        );
        delivery_first
            .cancel(None, receiver)
            .expect("cancel receiver");
        assert_eq!(
            delivery_first.take_send_result(sender),
            Some(SendResult::Accepted)
        );
        assert_eq!(delivery_first.take_receive_result(receiver), None);
        assert_eq!(
            delivery_first.channel_send(later_sender, channel.sender, 14),
            Ok(SendResult::Aborted(14))
        );
    }

    #[test]
    fn abort_upgrades_close_while_close_never_downgrades_abort() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");

        let mut closed = scheduler(TraceMode::Boundaries);
        let owner = start_parked(&mut closed);
        let sender = start_parked(&mut closed);
        let channel = closed.create_channel(owner, 1).expect("channel");
        assert_eq!(
            closed.channel_send(sender, channel.sender, 1),
            Ok(SendResult::Accepted)
        );
        assert_eq!(closed.take_send_result(sender), Some(SendResult::Accepted));
        closed
            .channel_close(owner, channel.completion)
            .expect("close");
        closed.drive();
        closed
            .channel_abort(owner, channel.receiver)
            .expect("upgrade to abort");
        closed.drive();
        assert_eq!(
            closed.channel_receive(owner, channel.receiver),
            Ok(ReceiveResult::Aborted)
        );

        let mut aborted = scheduler(TraceMode::Boundaries);
        let owner = start_parked(&mut aborted);
        let channel = aborted.create_channel(owner, 0).expect("channel");
        aborted
            .channel_abort(owner, channel.receiver)
            .expect("abort");
        aborted.drive();
        aborted
            .channel_close(owner, channel.completion)
            .expect("close after abort");
        aborted.drive();
        assert_eq!(
            aborted.channel_receive(owner, channel.receiver),
            Ok(ReceiveResult::Aborted)
        );
    }

    #[test]
    fn exclusive_capabilities_transfer_and_owner_exit_aborts_the_channel() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Full);
        let original = start_parked(&mut scheduler);
        let new_owner = start_parked(&mut scheduler);
        let sender = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(original, 0).expect("channel");
        let receiver = scheduler
            .transfer_receiver(original, channel.receiver, new_owner)
            .expect("transfer receiver");
        let completion = scheduler
            .transfer_completion(original, channel.completion, new_owner)
            .expect("transfer completion");

        assert_eq!(
            scheduler.channel_receive(original, channel.receiver),
            Err(SchedulerError::Channel(ChannelError::WrongReceiverOwner {
                channel: channel.sender.channel(),
                expected: Some(new_owner),
                actual: original,
            }))
        );
        assert_eq!(
            scheduler.result(None, original),
            Ok(TaskResult::Ready(TaskOutcome::Defected {
                origin: original,
                code: CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE,
            }))
        );

        assert_eq!(
            scheduler.channel_send(sender, channel.sender, 11),
            Ok(SendResult::Pending)
        );
        scheduler.cancel(None, new_owner).expect("owner exit");
        assert_eq!(
            scheduler.take_send_result(sender),
            Some(SendResult::Aborted(11))
        );
        assert_eq!(receiver.owner(), new_owner);
        assert_eq!(completion.owner(), new_owner);
    }

    #[test]
    fn terminal_tasks_channels_and_exported_events_can_be_reclaimed() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Full);
        let owner = start_parked(&mut scheduler);
        let unrelated = start_parked(&mut scheduler);
        let owned_channels = (0..128)
            .map(|_| scheduler.create_channel(owner, 0).expect("owned channel"))
            .collect::<Vec<_>>();
        let unrelated_channel = scheduler
            .create_channel(unrelated, 0)
            .expect("unrelated channel");

        assert_eq!(
            scheduler.release_channel(owned_channels[0].sender.channel()),
            Err(SchedulerError::Channel(ChannelError::ChannelStillLive(
                owned_channels[0].sender.channel()
            )))
        );
        scheduler.cancel(None, owner).expect("cancel owner");
        assert_eq!(
            scheduler.release_task(owner),
            Err(SchedulerError::TaskStillLive(owner))
        );
        assert_eq!(
            scheduler.result(None, owner),
            Ok(TaskResult::Ready(TaskOutcome::Cancelled))
        );
        for channel in owned_channels {
            scheduler
                .release_channel(channel.sender.channel())
                .expect("release owned channel");
        }
        scheduler.release_task(owner).expect("release owner");

        assert_eq!(
            scheduler
                .channel_metrics(unrelated_channel.sender.channel())
                .expect("unrelated channel remains")
                .capacity,
            0
        );
        scheduler.cancel(None, unrelated).expect("cancel unrelated");
        assert_eq!(
            scheduler.result(None, unrelated),
            Ok(TaskResult::Ready(TaskOutcome::Cancelled))
        );
        scheduler
            .release_channel(unrelated_channel.sender.channel())
            .expect("release unrelated channel");
        scheduler
            .release_task(unrelated)
            .expect("release unrelated task");

        assert!(scheduler.metrics().task_events > 0);
        assert!(scheduler.metrics().channel_events > 0);
        let (task_events, channel_events) = scheduler.drain_events();
        assert!(!task_events.is_empty());
        assert!(!channel_events.is_empty());
        assert_eq!(
            scheduler.metrics(),
            SchedulerMetrics {
                tasks: 0,
                channels: 0,
                task_events: 0,
                channel_events: 0,
            }
        );
        assert!(scheduler.constructed_events() > 0);
    }

    #[test]
    fn channel_trace_modes_are_zero_cost_or_boundary_filtered() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        for mode in [TraceMode::Off, TraceMode::Boundaries, TraceMode::Full] {
            let mut scheduler = scheduler(mode);
            let owner = start_parked(&mut scheduler);
            let sender = start_parked(&mut scheduler);
            let channel = scheduler.create_channel(owner, 1).expect("channel");
            assert_eq!(
                scheduler.channel_send(sender, channel.sender, 1),
                Ok(SendResult::Accepted)
            );
            assert_eq!(
                scheduler.take_send_result(sender),
                Some(SendResult::Accepted)
            );
            scheduler
                .channel_close(owner, channel.completion)
                .expect("close");

            match mode {
                TraceMode::Off => {
                    assert_eq!(scheduler.constructed_events(), 0);
                    assert!(scheduler.events().is_empty());
                    assert!(scheduler.channel_events().is_empty());
                }
                TraceMode::Boundaries => {
                    assert!(
                        scheduler
                            .channel_events()
                            .iter()
                            .any(|event| event.kind == ChannelEventKind::Created)
                    );
                    assert!(
                        scheduler
                            .channel_events()
                            .iter()
                            .any(|event| event.kind == ChannelEventKind::Closed)
                    );
                    assert!(
                        !scheduler
                            .channel_events()
                            .iter()
                            .any(|event| event.kind == ChannelEventKind::SendAccepted)
                    );
                }
                TraceMode::Full => {
                    assert!(scheduler.channel_events().iter().any(|event| {
                        event.kind == ChannelEventKind::SendAccepted
                            && event.task == Some(sender)
                            && event.capacity == 1
                            && event.queue_depth == 1
                    }));
                    let sequences: Vec<_> = scheduler
                        .events()
                        .iter()
                        .map(|event| event.sequence)
                        .chain(
                            scheduler
                                .channel_events()
                                .iter()
                                .map(|event| event.sequence),
                        )
                        .collect();
                    let unique: BTreeSet<_> = sequences.iter().copied().collect();
                    assert_eq!(unique.len(), sequences.len());
                }
            }
        }
    }
}
