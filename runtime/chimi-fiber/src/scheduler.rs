//! Deterministic structured lifecycle for finite Chimi tasks, supervised
//! processes, and the execution clock.

mod child;
#[cfg(test)]
mod child_tests;
mod carrier;
mod clock;
mod jobs;
mod process;
mod process_abi;
#[cfg(test)]
mod process_tests;
mod reactor;
mod ready;
#[cfg(test)]
mod reactor_tests;
#[cfg(test)]
mod release_tests;
mod scope;
mod select;
#[cfg(test)]
mod select_tests;
mod signal;

use self::clock::{Clock, TimerQueue, TimerTarget};
pub use self::clock::{ClockMode, Instant, TimerId};
pub use self::reactor::{IO_ERROR, IO_HANGUP, IO_READABLE, IO_WRITABLE};
use self::process::ProcessKernel;
pub use self::process::{
    Address, ChimiProcessTermination, IncarnationId, Member, MembershipId, MessageRelease, NameId,
    Policy, PolicyError, ProcessDefectReport, ProcessError, ProcessEvent, ProcessEventKind,
    ProcessMetrics, ProcessSpec, Progress, Restart, Restarts, SendFailure, SendOutcome,
    SendRejected, SpecContextRelease, StartError, SupervisorId, SupervisorState, TerminalCause,
};
pub use self::process_abi::{
    ChimiDeadlockWaiter, ChimiProcessRequest, ChimiProcessSpec, ChimiSchedulerConfig,
    ChimiSupervisorMember, ChimiSupervisorPolicy, DeadlockReport, RootRun,
};
pub use self::scope::{
    ScopeExit, ScopeId, chimi_fiber_cancel_is_terminal, chimi_fiber_scope_cancel_pending,
};
pub use self::select::{
    ChimiSelectSource, SELECT_INBOX, SELECT_RECEIVE, SELECT_TASK, SELECT_TIMER,
};
pub use crate::channel::{
    CapabilityKind, Carried, ChannelCapabilities, ChannelError, ChannelEventKind, ChannelId,
    ChannelMetrics, Completion, HubKind, ReceiveResult, Receiver, SendResult, Sender,
};
use crate::channel::{ChannelRegistry, Observation, Transition};
use crate::{
    ChimiFiber, ChimiFiberCommand, ChimiFiberConfig, ChimiFiberEntry, ChimiFiberEvent,
    ChimiFiberState, chimi_fiber_create, chimi_fiber_destroy, chimi_fiber_resume,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::ptr::NonNull;

const SCHEDULER_DEFECT_CODE: i64 = -2;
const CAPABILITY_VIOLATION_DEFECT_CODE: i64 = -3;
const CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE: i64 = -4;
pub const DEADLOCK_DEFECT_CODE: i64 = -5;
pub const RESTART_LIMIT_DEFECT_CODE: i64 = process::RESTART_LIMIT_DEFECT_CODE;

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
pub const REQUEST_TIMER_NOW: u32 = 11;
pub const REQUEST_TIMER_SLEEP_UNTIL: u32 = 12;
pub const REQUEST_PROCESS_SELF: u32 = 13;
pub const REQUEST_PROCESS_SEND: u32 = 14;
pub const REQUEST_PROCESS_RECEIVE: u32 = 15;
pub const REQUEST_SUPERVISOR_START: u32 = 16;
pub const REQUEST_SUPERVISOR_START_PROCESS: u32 = 17;
pub const REQUEST_SUPERVISOR_ADDRESS: u32 = 18;
pub const REQUEST_SUPERVISOR_REMOVE_PROCESS: u32 = 19;
pub const REQUEST_SUPERVISOR_STOP: u32 = 20;
pub const REQUEST_SUPERVISOR_WAIT: u32 = 21;
pub const REQUEST_SUPERVISOR_START_SUPERVISOR: u32 = 22;
pub const REQUEST_SUPERVISOR_REMOVE_SUPERVISOR: u32 = 23;
pub const REQUEST_CHANNEL_RELEASE: u32 = 24;
/// Enters a cancellation scope: `value` is its deadline; the reply's `token`
/// is the scope id. A cancellation point.
pub const REQUEST_SCOPE_ENTER: u32 = 25;
/// Leaves the scope whose id is `token`; the reply's `value` is 1 when that
/// scope's cancellation is the one unwinding the task. PENDING while children
/// it cancelled drain. Never a cancellation point.
pub const REQUEST_SCOPE_EXIT: u32 = 26;
/// A loop pass's cancellation point: answered OK, or with a pending scope
/// cancellation.
pub const REQUEST_SCOPE_CHECK: u32 = 27;
pub const REQUEST_PROCESS_MONITOR: u32 = 28;
pub const REQUEST_PROCESS_UNMONITOR: u32 = 29;
pub const REQUEST_PROCESS_MAILBOX: u32 = 30;
/// From a handoff incarnation's body: takes the argument its start handed
/// over, which the incarnation owns from then on.
pub const REQUEST_PROCESS_ARGUMENT: u32 = 31;
/// `context` points to `value` `ChimiSelectSource`s. OK with the index of the
/// first ready source in `value`, or PENDING until one may be ready; reissue
/// the request after every wake. Consumes nothing (`scheduler/select.rs`).
pub const REQUEST_SELECT: u32 = 32;
/// `Resource.scoped` entries (`scheduler/scope.rs`); never cancellation
/// points. ENTER pushes a masked entry and replies with it in `token`; USE
/// unmasks it once acquire succeeded; RELEASE masks it again; EXIT leaves it.
/// USE and EXIT reply `value` 1 when a cancellation the entry held off is now
/// delivered.
pub const REQUEST_RESOURCE_ENTER: u32 = 33;
pub const REQUEST_RESOURCE_RELEASE: u32 = 34;
pub const REQUEST_RESOURCE_EXIT: u32 = 35;
pub const REQUEST_RESOURCE_USE: u32 = 49;
/// `channel`, `owner`, `token`: one of the caller's receivers. OK with
/// another receiver of the same queue in `channel`, `owner`, `token`.
pub const REQUEST_CHANNEL_ADD_RECEIVER: u32 = 36;
/// `value`: a positive capacity per subscription. OK with the broadcast's
/// id in `channel` (its publisher and completion) and the caller as owner.
pub const REQUEST_CHANNEL_BROADCAST: u32 = 37;
/// `value`: the initial value, which the scheduler takes on OK. OK with the
/// watch's id in `channel` and the caller as owner.
pub const REQUEST_CHANNEL_WATCH: u32 = 38;
/// `channel`: a broadcast or watch. OK with a new subscription receiver in
/// `channel`, `owner`, `token`.
pub const REQUEST_CHANNEL_SUBSCRIBE: u32 = 39;
/// Cancels the live children of the scope whose id is `token` (and of the
/// scopes nested in it) without cancelling the caller; leaving the scope then
/// waits for them to drain. ERROR when the task is not inside it. Never a
/// cancellation point.
pub const REQUEST_SCOPE_CANCEL: u32 = 41;
/// Waits for the first of several children to terminate: `context` points to
/// `value` task ids, all owned by the caller. Replies like a ready
/// `REQUEST_TASK_RESULT`, with `task` set to the listed child that terminated
/// first in termination order; PENDING while none has. The child's outcome is
/// only reported: a cancelled child does not cancel the caller. A
/// cancellation point, and a firing scope withdraws it.
pub const REQUEST_TASK_ANY: u32 = 42;
/// `REQUEST_TASK_ANY` for draining children a combinator already cancelled:
/// neither a cancellation point nor withdrawn by a scope, so a scope that
/// fires meanwhile cannot replace the outcome the combinator already chose.
pub const REQUEST_TASK_DRAIN: u32 = 43;
/// Waits until the descriptor in `value` is ready for the `token` interest
/// (`IO_READABLE`, `IO_WRITABLE`): OK with the ready conditions in `value`,
/// or PENDING until it may be ready; reissue after every wake. A
/// cancellation point (`scheduler/reactor.rs`). Kinds 36 to 39 are the
/// channel-variant requests, 41 cancels a scope, 42 and 43 are the task
/// combinators', and 44 withdraws IO waits.
pub const REQUEST_IO_WAIT: u32 = 48;
/// Wakes every task whose IO wait names the resource key in `channel`
/// (nonzero), before the host closes that resource's descriptors: OK with
/// the number woken in `value`. Never parks, and not a cancellation point,
/// so a cancelled task can still close what it owns (`scheduler/reactor.rs`).
pub const REQUEST_IO_WITHDRAW: u32 = 44;
/// Waits for a `std/signal` signal: `value` holds up to eight signal codes
/// (1 SIGHUP, 2 SIGUSR1, 3 SIGUSR2), one per byte from the lowest, ended by
/// a zero byte. OK with the first listed code that is pending in `value`,
/// consuming it; PENDING while none is, so reissue after every wake. ERROR
/// for an empty list or another code. A cancellation point, withdrawn by a
/// cancellation (`scheduler/signal.rs`).
pub const REQUEST_SIGNAL_WAIT: u32 = 45;
/// Waits until the child process whose pid is in `value` has exited: OK
/// once it has, leaving it unreaped for the host's `waitpid`; PENDING while
/// it runs, so reissue after every wake; ERROR for a pid that is not an
/// unreaped child. Holds virtual time like an IO wait. A cancellation point,
/// withdrawn by a cancellation (`scheduler/child.rs`).
pub const REQUEST_CHILD_WAIT: u32 = 47;
/// No value names `task` any more (`Scheduler::release_task`): OK, or ERROR
/// for an unknown task, a process body, a live root, or a second release.
/// Any task may issue it. Never parks, and not a cancellation point.
pub const REQUEST_TASK_RELEASE: u32 = 46;
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
pub const RESPONSE_FULL: u32 = 11;
pub const RESPONSE_UNAVAILABLE: u32 = 12;
pub const RESPONSE_CAPACITY_REACHED: u32 = 13;
pub const RESPONSE_NAME_TAKEN: u32 = 14;
pub const RESPONSE_STOPPED: u32 = 15;
pub const RESPONSE_FORCED: u32 = process::RESPONSE_FORCED;
pub const PROCESS_END_RETURNED: i64 = process::PROCESS_END_RETURNED;
pub const PROCESS_END_FAILED: i64 = process::PROCESS_END_FAILED;

/// The fields every request record starts with.
#[repr(C)]
pub(crate) struct RequestHeader {
    kind: u32,
    status: u32,
}

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
pub const SCHEDULER_TRACE_PROCESS: u32 = process::SCHEDULER_TRACE_PROCESS;
pub const SCHEDULER_TRACE_CLOCK: u32 = 4;

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
    pub supervisor: u64,
    pub membership: u64,
    pub incarnation: u64,
    pub name: u64,
    pub parent_supervisor: u64,
    pub attempts_in_window: u64,
    pub time: i64,
    pub address_kind: u32,
    pub outcome: u32,
}

impl SchedulerTraceEvent {
    pub(crate) const fn blank(family: u32, kind: u32, sequence: u64) -> Self {
        Self {
            family,
            kind,
            sequence,
            task: 0,
            parent: 0,
            cause: 0,
            channel: 0,
            capacity: 0,
            queue_depth: 0,
            waiter_depth: 0,
            discard_count: 0,
            supervisor: 0,
            membership: 0,
            incarnation: 0,
            name: 0,
            parent_supervisor: 0,
            attempts_in_window: 0,
            time: 0,
            address_kind: 0,
            outcome: 0,
        }
    }
}

pub type SchedulerTraceSink = unsafe extern "C" fn(*mut c_void, *const SchedulerTraceEvent);

/// Releases one host word the scheduler took over and will never hand back:
/// a value a task's fiber ended with, or a value a sender handed to a
/// channel. See `ChimiValueRelease` in `include/chimi_fiber.h`.
pub type ValueRelease = unsafe extern "C" fn(*mut c_void, i64);
/// Duplicates a channel value for another broadcast or watch subscription,
/// returning the copy, which the scheduler then holds like any accepted
/// value. Receives `release_context`.
pub type ValueCopy = unsafe extern "C" fn(*mut c_void, i64) -> i64;

/// Releases the entry context of a started task whose entry never ran. See
/// `ChimiTaskContextRelease` in `include/chimi_fiber.h`.
pub type TaskContextRelease = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// Takes one release the carrier running now queued while no task was
/// running, such as a count that reached zero inside a release hook. Writes
/// `REQUEST_CHANNEL_RELEASE` or `REQUEST_TASK_RELEASE` and its id and returns
/// true, or returns false once the queue is empty. See `ChimiReleaseTake` in
/// `include/chimi_fiber.h`.
pub type ReleaseTake = unsafe extern "C" fn(*mut c_void, *mut u32, *mut u64) -> bool;

/// Why a channel request broke the capability contract. See
/// `ChimiCapabilityViolation` in `include/chimi_fiber.h`.
pub const VIOLATION_WRONG_OWNER: u32 = 1;
pub const VIOLATION_OVERLAPPING_OPERATION: u32 = 2;
pub const VIOLATION_PENDING_OPERATION: u32 = 3;
pub const VIOLATION_UNKNOWN_CHANNEL: u32 = 4;
pub const VIOLATION_CHANNEL_STILL_LIVE: u32 = 5;
pub const VIOLATION_IN_FLIGHT: u32 = 6;
pub const VIOLATION_PUBLISHED_CAPABILITY: u32 = 7;
pub const VIOLATION_WRONG_KIND: u32 = 8;
pub const VIOLATION_OWN_RECEIVER: u32 = 9;
pub const CAPABILITY_NONE: u32 = 0;
pub const CAPABILITY_RECEIVER: u32 = 1;
pub const CAPABILITY_COMPLETION: u32 = 2;

/// One channel capability violation, reported the moment the scheduler
/// defects the task that made it. Identities are 0 when absent.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChimiCapabilityViolation {
    /// The task whose request broke the contract; it defects.
    pub task: u64,
    pub channel: u64,
    /// The task that owns the capability now, or 0 when its last owner ended.
    pub owner: u64,
    /// For a transfer, the task it was to be handed to.
    pub destination: u64,
    /// The request kind (`REQUEST_CHANNEL_RECEIVE`, ...).
    pub request_kind: u32,
    pub capability: u32,
    pub reason: u32,
}

impl ChimiCapabilityViolation {
    fn new(
        task: TaskId,
        error: ChannelError,
        request_kind: u32,
        destination: Option<TaskId>,
    ) -> Self {
        let (reason, capability, channel, owner) = match error {
            ChannelError::WrongReceiverOwner {
                channel, expected, ..
            } => (
                VIOLATION_WRONG_OWNER,
                CAPABILITY_RECEIVER,
                Some(channel),
                expected,
            ),
            ChannelError::WrongCompletionOwner {
                channel, expected, ..
            } => (
                VIOLATION_WRONG_OWNER,
                CAPABILITY_COMPLETION,
                Some(channel),
                expected,
            ),
            ChannelError::OverlappingOperation(_) => {
                (VIOLATION_OVERLAPPING_OPERATION, CAPABILITY_NONE, None, None)
            }
            ChannelError::CapabilityHasPendingOperation(_) => {
                (VIOLATION_PENDING_OPERATION, CAPABILITY_NONE, None, None)
            }
            ChannelError::UnknownChannel(channel) => (
                VIOLATION_UNKNOWN_CHANNEL,
                CAPABILITY_NONE,
                Some(channel),
                None,
            ),
            ChannelError::ChannelStillLive(channel) => (
                VIOLATION_CHANNEL_STILL_LIVE,
                CAPABILITY_NONE,
                Some(channel),
                None,
            ),
            ChannelError::PublishedCapability(channel) => (
                VIOLATION_PUBLISHED_CAPABILITY,
                CAPABILITY_NONE,
                Some(channel),
                None,
            ),
            ChannelError::OwnReceiver(channel) => (
                VIOLATION_OWN_RECEIVER,
                CAPABILITY_RECEIVER,
                Some(channel),
                None,
            ),
            ChannelError::WrongKind(channel) => (
                VIOLATION_WRONG_KIND,
                CAPABILITY_NONE,
                Some(channel),
                None,
            ),
            ChannelError::CapabilityInFlight { channel, kind } => (
                VIOLATION_IN_FLIGHT,
                match kind {
                    CapabilityKind::Receiver => CAPABILITY_RECEIVER,
                    CapabilityKind::Completion => CAPABILITY_COMPLETION,
                },
                Some(channel),
                None,
            ),
        };
        Self {
            task: task.get(),
            channel: channel.map_or(0, ChannelId::get),
            owner: owner.map_or(0, TaskId::get),
            destination: destination.map_or(0, TaskId::get),
            request_kind,
            capability,
            reason,
        }
    }
}

/// Receives each channel capability violation as it defects its task. See
/// `ChimiCapabilityViolationReport` in `include/chimi_fiber.h`.
pub type CapabilityViolationReport =
    unsafe extern "C" fn(*mut c_void, *const ChimiCapabilityViolation);

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
    /// Its fiber is being resumed. With one carrier no other scheduler code
    /// runs meanwhile, so nothing observes it; with several, a cancel,
    /// force or defect decided elsewhere is recorded in `command` and the
    /// task's flags and takes effect at its next park, as for `Runnable`.
    Running,
    Parked,
    Draining(TaskOutcome),
    Terminal(TaskOutcome),
}

impl TaskPhase {
    /// True while the task's fiber can still run: it is ready, running or
    /// parked, not draining or terminal.
    fn is_live(self) -> bool {
        matches!(self, Self::Runnable | Self::Running | Self::Parked)
    }
}

/// The carrier thread a fiber belongs to: the one that first resumed it. A
/// started fiber's stack holds that thread's host frames and C runtime
/// state, so it must be resumed and destroyed there and nowhere else
/// (parallel-scheduler.md, "Step 3 plan"). An unstarted fiber has no home
/// yet and may be handed to another carrier.
#[derive(Debug, Default)]
struct Home(Option<std::thread::ThreadId>);

impl Home {
    /// Claims the current thread on the first call. Ensures: false when a
    /// later call comes from another thread.
    fn enter(&mut self) -> bool {
        let here = std::thread::current().id();
        *self.0.get_or_insert(here) == here
    }

    /// True when the fiber never ran or runs on the current thread.
    fn is_here(&self) -> bool {
        self.0.is_none_or(|home| home == std::thread::current().id())
    }
}

struct PinnedFiber {
    home: Home,
    fiber: NonNull<ChimiFiber>,
}

impl PinnedFiber {
    fn new(
        entry: ChimiFiberEntry,
        context: *mut c_void,
        config: ChimiFiberConfig,
    ) -> Result<Self, SchedulerError> {
        let fiber = NonNull::new(unsafe { chimi_fiber_create(Some(entry), context, config) })
            .ok_or(SchedulerError::FiberCreationFailed)?;
        Ok(Self {
            home: Home::default(),
            fiber,
        })
    }

    /// Checks that this thread is the fiber's home and sets the context its
    /// resume reads; the returned fiber is then resumed with `resume_fiber`.
    /// The task record keeps the fiber, which is never dropped or removed
    /// while its task is `Running`, so the pointer stays valid without the
    /// record (or the kernel lock) being held across the resume.
    fn enter(&mut self, context: scope::ResumeContext) -> NonNull<ChimiFiber> {
        assert!(self.home.enter(), "a fiber resumed off its home carrier");
        scope::set_resume_context(context);
        self.fiber
    }
}

/// Resumes a fiber `PinnedFiber::enter` returned on this thread.
fn resume_fiber(fiber: NonNull<ChimiFiber>, command: ChimiFiberCommand) -> ChimiFiberEvent {
    // SAFETY: `enter` checked that this is the fiber's home thread, and the
    // running task's record keeps the fiber alive until the resume returns.
    unsafe { chimi_fiber_resume(fiber.as_ptr(), command as u32) }
}

impl Drop for PinnedFiber {
    fn drop(&mut self) {
        assert!(self.home.is_here(), "a fiber destroyed off its home carrier");
        unsafe {
            chimi_fiber_destroy(self.fiber.as_ptr());
        }
    }
}

struct Task {
    parent: Option<TaskId>,
    children: BTreeSet<TaskId>,
    waiting_on: Option<TaskId>,
    /// The children a `REQUEST_TASK_ANY` or `REQUEST_TASK_DRAIN` waits for.
    waiting_any: Vec<TaskId>,
    /// Stamped when the task becomes terminal; orders `REQUEST_TASK_ANY`.
    terminal_sequence: u64,
    fiber: Option<PinnedFiber>,
    phase: TaskPhase,
    observed: bool,
    /// No value names this task any more (`Scheduler::release_task`): once
    /// terminal, its record and kept result are removed.
    handle_released: bool,
    /// The incarnation this task runs, if it is a process body.
    process: Option<IncarnationId>,
    /// The command for the next resume.
    command: ChimiFiberCommand,
    /// The request the fiber is parked on; null while running or after a
    /// bare park.
    request: *mut c_void,
    /// Counts parks, so a delayed reply never reaches a later request.
    park: u64,
    ran: bool,
    /// The context a `REQUEST_TASK_START` handed over, until the entry runs
    /// and takes it; null otherwise.
    entry_context: *mut c_void,
    /// The sleep timer this task is parked on.
    timer: Option<TimerId>,
    /// The cancellation scopes the task is inside, innermost last.
    scopes: Vec<scope::Scope>,
    /// The parent's innermost scope when this task was started.
    started_in_scope: Option<ScopeId>,
    /// What the task's parked `Select.first` waits on.
    select: Option<select::SelectWait>,
    /// The descriptor wait the task is parked on.
    io_wait: Option<reactor::IoWaitId>,
    /// The channel registry's arrival stamp when this task started; what it
    /// hands its parent dates from then (`ChannelRegistry::backdate`).
    started_at: u64,
    /// The outcome of a cancellation that waits for the task's releases
    /// (`Scheduler::cancel_task`).
    terminating: Option<TaskOutcome>,
    /// The carrier whose ready queue holds this task, and whose thread its
    /// fiber runs on: its parent's, or the current one for a root.
    carrier: ready::Carrier,
    /// An end decided on another carrier while this task's fiber ran there
    /// (`TaskPhase::Running`); its own carrier applies it when the resume
    /// returns (`complete_run`).
    pending_end: Option<jobs::PendingEnd>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockEventKind {
    /// Virtual time jumped to `time`. A boundary event.
    Advanced,
    TimerStarted,
    TimerFired,
    TimerCancelled,
    /// One parked task in a detected deadlock. A boundary event.
    DeadlockWaiter,
    /// A cancellation scope's deadline passed. A boundary event.
    ScopeFired,
    /// A `Select.first` found nothing ready and parked. A boundary event.
    SelectParked,
    /// A `Select.first` committed to the event whose index is
    /// `request_kind`. A boundary event.
    SelectCommitted,
    /// SIGINT or SIGTERM (its number in `request_kind`) cancelled the
    /// root. A boundary event.
    SignalReceived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockEvent {
    pub sequence: u64,
    pub kind: ClockEventKind,
    pub task: Option<TaskId>,
    pub time: Instant,
    /// For a deadlock waiter: the request kind it is parked on, or 0. For a
    /// select commit: the committed event's index.
    pub request_kind: u32,
}

/// A task parked with nothing able to wake it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParkedTask {
    pub task: TaskId,
    pub request_kind: u32,
}

/// Why `run_until_idle` returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Idle {
    /// Nothing is runnable, no timer is pending, and no non-process task is
    /// parked. Processes may still wait in receive.
    Quiescent,
    /// A host operation is in flight; time does not advance until it ends.
    AwaitingHost,
    /// Virtual time: tasks are parked and nothing can ever wake them.
    Deadlock(Vec<ParkedTask>),
    /// Real time: tasks are parked with no timer pending.
    Stalled(Vec<ParkedTask>),
}

/// Scheduler construction options beyond stack size and trace mode.
#[derive(Clone, Copy, Debug)]
pub struct SchedulerOptions {
    pub stack_bytes: usize,
    pub trace_mode: TraceMode,
    pub clock: ClockMode,
    /// Called once for every accepted message the scheduler discards unread.
    pub release_message: Option<MessageRelease>,
    /// Called once for every admitted specification context when no record
    /// needs it any more.
    pub release_spec_context: Option<SpecContextRelease>,
    /// Called once for every value a non-process task's fiber ended with: a
    /// result once the task's handle is released (`Scheduler::release_task`),
    /// otherwise when the scheduler is dropped. The root's value is released too,
    /// unless `chimi_scheduler_execute_with_config` hands it to its caller.
    pub release_task_value: Option<ValueRelease>,
    /// Called once for every value the scheduler accepted from a sender and
    /// discards instead of delivering it or handing it back.
    pub release_channel_value: Option<ValueRelease>,
    /// Copies a value for each broadcast or watch subscription past the
    /// first. Without it, every subscription holds the same value.
    pub copy_channel_value: Option<ValueCopy>,
    /// Called once for the context of every task a `REQUEST_TASK_START`
    /// started whose entry never runs.
    pub release_task_context: Option<TaskContextRelease>,
    pub release_context: *mut c_void,
    /// Called once for every channel capability violation, with
    /// `violation_context`, when the scheduler defects the violating task.
    pub capability_violation_report: Option<CapabilityViolationReport>,
    pub violation_context: *mut c_void,
    /// Called with `release_context` whenever this carrier's ready queue is
    /// empty, until it returns false: the releases it hands over are applied
    /// with no task attributed.
    pub take_release: Option<ReleaseTake>,
}

impl SchedulerOptions {
    pub fn new(stack_bytes: usize, trace_mode: TraceMode, clock: ClockMode) -> Self {
        Self {
            stack_bytes,
            trace_mode,
            clock,
            release_message: None,
            release_spec_context: None,
            release_task_value: None,
            release_channel_value: None,
            copy_channel_value: None,
            release_task_context: None,
            release_context: std::ptr::null_mut(),
            capability_violation_report: None,
            violation_context: std::ptr::null_mut(),
            take_release: None,
        }
    }
}

/// A word a task's fiber ended with. A defect code and a result are kept
/// apart so that handing out one never matches the other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskValue {
    Result(i64),
    Defect(i64),
}

impl TaskValue {
    fn word(self) -> i64 {
        match self {
            Self::Result(word) | Self::Defect(word) => word,
        }
    }
}

pub struct Scheduler {
    tasks: BTreeMap<TaskId, Task>,
    ready: ready::ReadyQueues,
    /// Work a carrier must do on its own thread for another carrier's
    /// decision, run before its ready tasks.
    jobs: jobs::Jobs,
    /// The carrier running this scheduler's loop now.
    current: ready::Carrier,
    /// Set while the scheduler is torn down on one thread: every fiber is
    /// then resumed where the teardown runs, whatever its home carrier.
    any_carrier: bool,
    next_task: u64,
    stack_bytes: usize,
    trace_mode: TraceMode,
    events: Vec<TaskEvent>,
    channels: ChannelRegistry,
    channel_events: Vec<ChannelEvent>,
    constructed_events: u64,
    trace_sink: Option<SchedulerTraceSink>,
    trace_context: *mut c_void,
    clock: Clock,
    timers: TimerQueue,
    clock_events: Vec<ClockEvent>,
    host_operations: usize,
    kernel: ProcessKernel,
    release_task_value: Option<(ValueRelease, *mut c_void)>,
    release_channel_value: Option<(ValueRelease, *mut c_void)>,
    release_task_context: Option<(TaskContextRelease, *mut c_void)>,
    take_release: Option<(ReleaseTake, *mut c_void)>,
    capability_violation_report: Option<(CapabilityViolationReport, *mut c_void)>,
    /// The value each task's fiber ended with, kept only while
    /// `release_task_value` is set. `Task.result` copies a value without
    /// consuming it, so a result stays until its task's handle is released
    /// and a defect code until the scheduler is dropped.
    task_values: BTreeMap<TaskId, TaskValue>,
    /// Terminal tasks whose handle was released, removed by `drive` between
    /// resumes, where no scheduler operation is still using their record.
    released_terminal: Vec<TaskId>,
    /// The most task records held at once (`chimi_scheduler_peak_task_records`).
    peak_tasks: usize,
    next_scope: u64,
    signals: signal::Signals,
    reactor: reactor::Reactor,
    children: child::Children,
    /// Nonzero while a forced end runs: cancellation then never waits for
    /// releases (`force_terminal`).
    forcing: u32,
    next_terminal_sequence: u64,
}

/// Writes a terminal child's outcome into a `REQUEST_TASK_RESULT`,
/// `REQUEST_TASK_ANY`, or `REQUEST_TASK_DRAIN` reply.
fn reply_with_outcome(request: &mut SchedulerRequest, outcome: TaskOutcome) {
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
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        // Teardown runs on one thread. With several carriers each must run
        // its own jobs and process teardown before it exits (PR 5c).
        self.any_carrier = true;
        self.run_queued_jobs();
        if !self.kernel.incarnations.is_empty() || self.has_blocked_plans() {
            self.tear_down_processes();
        }
        self.release_surviving_incarnations();
        self.release_held_values();
    }
}

impl Scheduler {
    /// A scheduler on the real clock with no release hooks.
    pub fn new(stack_bytes: usize, trace_mode: TraceMode) -> Self {
        Self::with_options(SchedulerOptions::new(
            stack_bytes,
            trace_mode,
            ClockMode::Real,
        ))
    }

    pub fn with_options(options: SchedulerOptions) -> Self {
        let SchedulerOptions {
            stack_bytes,
            trace_mode,
            clock,
            release_message,
            release_spec_context,
            release_task_value,
            release_channel_value,
            copy_channel_value,
            release_task_context,
            release_context,
            capability_violation_report,
            violation_context,
            take_release,
        } = options;
        let mut channels =
            ChannelRegistry::new(trace_mode != TraceMode::Off, trace_mode == TraceMode::Full);
        if let Some(copy) = copy_channel_value {
            channels.set_copier(Box::new(move |value| {
                // SAFETY: the configuring caller keeps the hook and its
                // context valid until execution returns.
                unsafe { copy(release_context, value as i64) as usize }
            }));
        }
        let kernel = ProcessKernel::new(
            release_message.map(|release| (release, release_context)),
            release_spec_context.map(|release| (release, release_context)),
        );
        Self {
            tasks: BTreeMap::new(),
            ready: ready::ReadyQueues::new(1),
            jobs: jobs::Jobs::new(1),
            current: ready::Carrier::default(),
            any_carrier: false,
            next_task: 1,
            stack_bytes,
            trace_mode,
            events: Vec::new(),
            channels,
            channel_events: Vec::new(),
            constructed_events: 0,
            trace_sink: None,
            trace_context: std::ptr::null_mut(),
            clock: Clock::new(clock),
            timers: TimerQueue::default(),
            clock_events: Vec::new(),
            host_operations: 0,
            kernel,
            release_task_value: release_task_value.map(|release| (release, release_context)),
            release_channel_value: release_channel_value.map(|release| (release, release_context)),
            release_task_context: release_task_context.map(|release| (release, release_context)),
            take_release: take_release.map(|take| (take, release_context)),
            capability_violation_report: capability_violation_report
                .map(|report| (report, violation_context)),
            task_values: BTreeMap::new(),
            released_terminal: Vec::new(),
            peak_tasks: 0,
            next_scope: 0,
            signals: signal::Signals::default(),
            reactor: reactor::Reactor::default(),
            children: child::Children::default(),
            forcing: 0,
            next_terminal_sequence: 0,
        }
    }

    pub fn set_trace_sink(&mut self, sink: Option<SchedulerTraceSink>, context: *mut c_void) {
        self.trace_sink = sink;
        self.trace_context = context;
    }

    pub fn clock_mode(&self) -> ClockMode {
        self.clock.mode()
    }

    /// The current instant of the execution time source.
    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// Marks a host operation in flight. Virtual time does not advance until
    /// every begun operation has ended.
    pub fn begin_host_operation(&mut self) {
        self.host_operations += 1;
    }

    pub fn end_host_operation(&mut self) {
        self.host_operations = self
            .host_operations
            .checked_sub(1)
            .expect("end_host_operation without a matching begin");
    }

    /// Registers a timer that wakes `task` from its park at `deadline`.
    /// Timers with equal deadlines fire in registration order. The timer is
    /// removed if the task is cancelled first.
    pub fn start_wake_timer(
        &mut self,
        task: TaskId,
        deadline: Instant,
    ) -> Result<TimerId, SchedulerError> {
        self.require_live_task(task)?;
        let id = self.timers.insert(deadline, TimerTarget::Wake(task));
        if let Some(previous) = self
            .tasks
            .get_mut(&task)
            .expect("live task")
            .timer
            .replace(id)
        {
            self.cancel_timer(previous);
        }
        self.emit_timer(ClockEventKind::TimerStarted, Some(task), deadline);
        Ok(id)
    }

    /// Removes a pending timer. Returns false if it already fired or was
    /// cancelled.
    pub fn cancel_timer(&mut self, timer: TimerId) -> bool {
        let Some(target) = self.timers.cancel(timer) else {
            return false;
        };
        let task = match target {
            TimerTarget::Wake(task) | TimerTarget::ScopeDeadline(task, _) => Some(task),
            TimerTarget::ShutdownDeadline(_)
            | TimerTarget::Signal(_)
            | TimerTarget::UserSignal(_) => None,
        };
        let now = self.clock.now();
        self.emit_timer(ClockEventKind::TimerCancelled, task, now);
        true
    }

    /// Drives runnable work, and whenever nothing is runnable and no host
    /// operation is in flight, waits for the earliest timer: virtual time
    /// jumps to it, real time sleeps until it. Returns when nothing more can
    /// happen without the host.
    pub fn run_until_idle(&mut self) -> Idle {
        carrier::run_until_idle(self)
    }

    /// What an idle carrier does next, decided under the kernel lock once
    /// `drive` found nothing runnable.
    fn idle_step(&mut self) -> carrier::IdleStep {
        // The host must be able to end the operations it began, so they
        // return before the scheduler blocks in a descriptor wait.
        if self.host_operations > 0 {
            return carrier::IdleStep::Done(Idle::AwaitingHost);
        }
        // A descriptor or child wait is a host operation in flight: it
        // holds virtual time, and no parked task is stuck while one
        // exists.
        if !self.reactor.is_empty() || !self.children.is_empty() {
            return carrier::IdleStep::Io(self.prepare_io_wait());
        }
        let Some(deadline) = self.timers.earliest() else {
            return carrier::IdleStep::Done(self.idle_outcome());
        };
        if self.clock.mode() == ClockMode::Real {
            return carrier::IdleStep::Sleep(
                clock::real_wait(self.clock.now(), deadline),
                self.signals.wake_fd(),
            );
        }
        if self.clock.wait_until(deadline, self.signals.wake_fd()) {
            self.emit_clock(ClockEvent {
                sequence: 0,
                kind: ClockEventKind::Advanced,
                task: None,
                time: deadline,
                request_kind: 0,
            });
        }
        self.fire_due_timers();
        carrier::IdleStep::Continue
    }

    /// Why a carrier with nothing runnable, no host operation, no IO wait
    /// and no timer is idle: quiescent, or stuck with parked tasks.
    fn idle_outcome(&mut self) -> Idle {
        let parked: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, record)| record.phase == TaskPhase::Parked)
            .map(|(task, record)| ParkedTask {
                task: *task,
                request_kind: if record.request.is_null() {
                    0
                } else {
                    // SAFETY: a recorded request stays valid while parked.
                    unsafe { (*record.request.cast::<RequestHeader>()).kind }
                },
            })
            .collect();
        let stuck = parked
            .iter()
            .any(|parked| self.tasks[&parked.task].process.is_none());
        if !stuck {
            return Idle::Quiescent;
        }
        match self.clock.mode() {
            ClockMode::Virtual => {
                for waiter in &parked {
                    let now = self.clock.now();
                    self.emit_clock(ClockEvent {
                        sequence: 0,
                        kind: ClockEventKind::DeadlockWaiter,
                        task: Some(waiter.task),
                        time: now,
                        request_kind: waiter.request_kind,
                    });
                }
                Idle::Deadlock(parked)
            }
            ClockMode::Real => Idle::Stalled(parked),
        }
    }

    pub fn clock_events(&self) -> &[ClockEvent] {
        &self.clock_events
    }

    pub fn drain_clock_events(&mut self) -> Vec<ClockEvent> {
        std::mem::take(&mut self.clock_events)
    }

    fn fire_due_timers(&mut self) {
        let now = self.clock.now();
        while let Some((id, deadline, target)) = self.timers.pop_due(now) {
            match target {
                TimerTarget::Wake(task) => {
                    self.emit_timer(ClockEventKind::TimerFired, Some(task), deadline);
                    let record = self.tasks.get_mut(&task);
                    if let Some(record) = record.filter(|record| record.timer == Some(id)) {
                        record.timer = None;
                        let _ = self.wake(task);
                    }
                }
                TimerTarget::ShutdownDeadline(incarnation) => {
                    self.emit_timer(ClockEventKind::TimerFired, None, deadline);
                    self.force(incarnation);
                }
                TimerTarget::ScopeDeadline(task, scope) => {
                    self.emit_timer(ClockEventKind::TimerFired, Some(task), deadline);
                    self.fire_scope(task, scope, true);
                }
                TimerTarget::Signal(signal) => {
                    self.emit_timer(ClockEventKind::TimerFired, None, deadline);
                    self.fire_signal(signal);
                }
                TimerTarget::UserSignal(code) => {
                    self.emit_timer(ClockEventKind::TimerFired, None, deadline);
                    self.fire_user_signal(code);
                }
            }
        }
    }

    fn emit_timer(&mut self, kind: ClockEventKind, task: Option<TaskId>, time: Instant) {
        if self.trace_mode != TraceMode::Full {
            return;
        }
        self.emit_clock(ClockEvent {
            sequence: 0,
            kind,
            task,
            time,
            request_kind: 0,
        });
    }

    fn emit_clock(&mut self, mut event: ClockEvent) {
        if self.trace_mode == TraceMode::Off {
            return;
        }
        self.constructed_events += 1;
        event.sequence = self.constructed_events;
        self.clock_events.push(event);
        self.publish_trace(SchedulerTraceEvent {
            task: event.task.map_or(0, TaskId::get),
            cause: u64::from(event.request_kind),
            time: event.time.as_nanos(),
            ..SchedulerTraceEvent::blank(
                SCHEDULER_TRACE_CLOCK,
                event.kind as u32 + 1,
                event.sequence,
            )
        })
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
        carrier::drive(self);
    }

    /// Makes `carrier` the one running the loop until the next entry: the
    /// queue it pops and the home of the tasks it starts without a parent.
    fn enter_carrier(&mut self, carrier: ready::Carrier) {
        debug_assert!(carrier.0 < self.ready.carriers(), "unknown carrier {carrier:?}");
        self.current = carrier;
    }

    /// A kernel with `carriers` ready queues, driven from one thread by
    /// `enter_carrier`, for tests of routing between carriers.
    #[cfg(test)]
    fn with_carriers(stack_bytes: usize, trace_mode: TraceMode, carriers: u32) -> Self {
        let mut scheduler = Self::new(stack_bytes, trace_mode);
        scheduler.set_carriers(carriers);
        scheduler
    }

    /// Gives a kernel with no ready task or job `carriers` carriers.
    #[cfg(test)]
    fn set_carriers(&mut self, carriers: u32) {
        assert!(self.ready.is_empty() && self.jobs.is_empty(), "set before any work");
        self.ready = ready::ReadyQueues::new(carriers);
        self.jobs = jobs::Jobs::new(carriers);
    }

    /// Housekeeping `drive` does before its first resume and after each one.
    fn between_resumes(&mut self) {
        self.remove_released_terminal();
        self.collect_user_signals();
        self.deliver_signal();
    }

    /// After one resume (`resumes` counts them): polls IO and rechecks
    /// children every `IO_POLL_INTERVAL` resumes.
    fn after_resume(&mut self, resumes: &mut u32) {
        self.between_resumes();
        *resumes = resumes.wrapping_add(1);
        if resumes.is_multiple_of(reactor::IO_POLL_INTERVAL) {
            if !self.reactor.is_empty() {
                self.poll_io_now();
            }
            self.recheck_children();
        }
    }

    /// The next task this carrier resumes, entered and marked running; None
    /// once its ready queue is empty and no drained release refilled it.
    /// Releases a fiber's request did not carry (one queued in a release
    /// hook, or by a fiber that ended without another request) are drained
    /// before the carrier idles; their wakes refill the ready queue.
    fn next_runnable(&mut self, resumes: &mut u32) -> Option<carrier::Runnable> {
        loop {
            while let Some(job) = self.jobs.pop_front(self.current) {
                if let Some(runnable) = self.prepare_job(job) {
                    return Some(runnable);
                }
            }
            while let Some(task) = self.ready.pop_front(self.current) {
                if let Some(runnable) = self.prepare_run(task) {
                    return Some(runnable);
                }
                self.after_resume(resumes);
            }
            if !self.drain_host_releases() {
                return None;
            }
        }
    }

    /// Applies every release `take_release` hands over, with no task
    /// attributed; true if there was one. Runs only between resumes, never
    /// inside a channel transition, so the registry is never mid-update.
    /// Keys the host queues after the last drive (during shutdown or the
    /// scheduler's teardown) are not drained: teardown releases every value
    /// the scheduler holds, so only their trace events are missing, and the
    /// host clears its queues before the next run.
    fn drain_host_releases(&mut self) -> bool {
        let Some((take, context)) = self.take_release else {
            return false;
        };
        let mut drained = false;
        loop {
            let (mut kind, mut id) = (0_u32, 0_u64);
            // SAFETY: the configuring caller keeps the hook and its context
            // valid until execution returns; both outputs are writable.
            if !unsafe { take(context, &mut kind, &mut id) } {
                return drained;
            }
            drained = true;
            let applied = match kind {
                REQUEST_CHANNEL_RELEASE => {
                    match self
                        .channels
                        .release_unreferenced(None, ChannelId::from_raw(id))
                    {
                        Ok(transition) => {
                            self.apply_channel_transition(transition);
                            true
                        }
                        Err(_) => false,
                    }
                }
                REQUEST_TASK_RELEASE => self.release_task(TaskId(id)).is_ok(),
                _ => false,
            };
            // The host's count table names only ids it was handed, each once.
            debug_assert!(applied, "the host drained an unknown release {kind} {id}");
            self.remove_released_terminal();
        }
    }

    /// Queues `task` on its home carrier: the one way work becomes ready,
    /// so a carrier other than the current one can be told about it.
    fn enqueue(&mut self, task: TaskId, front: bool) {
        let carrier = self.tasks[&task].carrier;
        if front {
            self.ready.push_front(carrier, task);
        } else {
            self.ready.push_back(carrier, task);
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
            self.enqueue(task, front);
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
        self.cancel_task(task, TaskOutcome::Cancelled);
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

    /// A broadcast (`initial` is None) or a watch owned by `owner`. Returns
    /// its completion, whose channel id is also the publisher's.
    pub fn create_hub(
        &mut self,
        owner: TaskId,
        kind: HubKind,
        capacity: usize,
        initial: Option<usize>,
    ) -> Result<Completion, SchedulerError> {
        self.require_live_task(owner)?;
        let (_, completion, transition) = self.channels.create_hub(owner, kind, capacity, initial);
        self.apply_channel_transition(transition);
        Ok(completion)
    }

    /// A new subscription of the broadcast or watch `publisher` names.
    pub fn channel_subscribe(
        &mut self,
        task: TaskId,
        publisher: Sender,
    ) -> Result<Receiver, SchedulerError> {
        self.require_live_task(task)?;
        match self.channels.subscribe(task, publisher) {
            Ok((receiver, transition)) => {
                self.apply_channel_transition(transition);
                Ok(receiver)
            }
            Err(error) => {
                self.channel_violation(task, error, REQUEST_CHANNEL_SUBSCRIBE, None)
            }
        }
    }

    /// Another receiver of the queue `receiver` receives from.
    pub fn channel_add_receiver(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<Receiver, SchedulerError> {
        self.require_live_task(task)?;
        match self.channels.add_receiver(task, receiver) {
            Ok((added, transition)) => {
                self.apply_channel_transition(transition);
                Ok(added)
            }
            Err(error) => {
                self.channel_violation(task, error, REQUEST_CHANNEL_ADD_RECEIVER, None)
            }
        }
    }

    pub fn channel_send(
        &mut self,
        task: TaskId,
        sender: Sender,
        value: usize,
    ) -> Result<SendResult, SchedulerError> {
        self.channel_send_carrying(task, sender, value, &[])
    }

    /// `channel_send` of a value that holds `carried`, `task`'s exclusive
    /// capabilities (`ChannelRegistry::send_carrying`).
    pub fn channel_send_carrying(
        &mut self,
        task: TaskId,
        sender: Sender,
        value: usize,
        carried: &[Carried],
    ) -> Result<SendResult, SchedulerError> {
        self.require_parked_task(task)?;
        match self.channels.send_carrying(task, sender, value, carried) {
            Ok((result, transition)) => {
                self.apply_channel_transition(transition);
                Ok(result)
            }
            Err(error) => self.channel_violation(task, error, REQUEST_CHANNEL_SEND, None),
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
            Err(error) => self.channel_violation(task, error, REQUEST_CHANNEL_RECEIVE, None),
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
            Err(error) => self.channel_violation(task, error, REQUEST_CHANNEL_CLOSE, None),
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
            Err(error) => self.channel_violation(task, error, REQUEST_CHANNEL_ABORT, None),
        }
    }

    /// Ends a channel the host holds no endpoint of any more, on behalf of
    /// the parked `task`. Queued values are released, parked peers are
    /// answered aborted, and the id becomes unknown. Naming an unknown id
    /// defects `task`.
    pub fn channel_release(
        &mut self,
        task: TaskId,
        channel: ChannelId,
    ) -> Result<(), SchedulerError> {
        self.require_parked_task(task)?;
        match self.channels.release_unreferenced(Some(task), channel) {
            Ok(transition) => {
                self.apply_channel_transition(transition);
                Ok(())
            }
            Err(error) => self.channel_violation(task, error, REQUEST_CHANNEL_RELEASE, None),
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
            Ok((transferred, transition)) => {
                self.apply_channel_transition(transition);
                self.backdate_hand_back(task, new_owner, receiver.channel());
                Ok(transferred)
            }
            Err(error) => self.channel_violation(
                task,
                error,
                REQUEST_CHANNEL_TRANSFER_RECEIVER,
                Some(new_owner),
            ),
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
            Ok((transferred, transition)) => {
                self.apply_channel_transition(transition);
                self.backdate_hand_back(task, new_owner, completion.channel());
                Ok(transferred)
            }
            Err(error) => self.channel_violation(
                task,
                error,
                REQUEST_CHANNEL_TRANSFER_COMPLETION,
                Some(new_owner),
            ),
        }
    }

    /// A child hands the capabilities in its result to its parent as it ends;
    /// the parent can read them only through `Task.result`, so they count as
    /// gained when the child started, and a timeout scope the child started
    /// outside of leaves them alone.
    fn backdate_hand_back(&mut self, task: TaskId, new_owner: TaskId, channel: ChannelId) {
        let record = &self.tasks[&task];
        if record.parent == Some(new_owner) {
            let since = record.started_at;
            self.channels.backdate(channel, new_owner, since);
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

    /// Records that no value names `task` any more. Task handles never leave
    /// the task that started the child, so nothing can observe it again:
    /// once it is terminal, now or later, its record is removed and its kept
    /// result value released. A defect code is kept, because the defect may
    /// still be carrying it to the root. Process bodies have no handle; their
    /// kernel ends them. A root is released only once it has ended, because
    /// its runner reads its outcome from the record.
    pub fn release_task(&mut self, task: TaskId) -> Result<(), SchedulerError> {
        let record = self
            .tasks
            .get_mut(&task)
            .ok_or(SchedulerError::UnknownTask(task))?;
        let live_root =
            record.parent.is_none() && !matches!(record.phase, TaskPhase::Terminal(_));
        if record.process.is_some() || record.handle_released || live_root {
            return Err(SchedulerError::TaskStillLive(task));
        }
        record.handle_released = true;
        if matches!(record.phase, TaskPhase::Terminal(_)) {
            self.released_terminal.push(task);
            self.remove_released_terminal();
        }
        Ok(())
    }

    fn remove_released_terminal(&mut self) {
        for task in std::mem::take(&mut self.released_terminal) {
            let record = self.tasks.remove(&task).expect("a released terminal task");
            debug_assert!(
                record.children.is_empty(),
                "a terminal task has no live children"
            );
            if let Some(TaskValue::Result(word)) = self.task_values.get(&task).copied() {
                self.task_values.remove(&task);
                if let Some((release, context)) = self.release_task_value {
                    // SAFETY: as for `release_channel_value`; the value is
                    // removed from the map before it is released.
                    unsafe { release(context, word) };
                }
            }
        }
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

    /// Hands `root`'s outcome value to the caller: it leaves the scheduler's
    /// keeping and is not released when the scheduler is dropped. The value
    /// belongs to the root for a result and to the origin for a defect code;
    /// a scheduler-made defect code was never kept.
    fn hand_out_outcome_value(&mut self, root: TaskId, outcome: TaskOutcome) {
        let (owner, value) = match outcome {
            TaskOutcome::Succeeded(word) => (root, TaskValue::Result(word)),
            TaskOutcome::Defected { origin, code } => (origin, TaskValue::Defect(code)),
            TaskOutcome::Cancelled | TaskOutcome::StackOverflow { .. } => return,
        };
        if self.task_values.get(&owner) == Some(&value) {
            self.task_values.remove(&owner);
        }
    }

    /// Keeps the word a non-process task's fiber ended with, so it can be
    /// released once the execution ends. A process body ends with an end
    /// code the kernel reads, not a host value.
    fn keep_task_value(&mut self, task: TaskId, event: &ChimiFiberEvent) {
        if self.release_task_value.is_none() || self.tasks[&task].process.is_some() {
            return;
        }
        let value = match event.state {
            state if state == ChimiFiberState::Succeeded as u32 => TaskValue::Result(event.value),
            state if state == ChimiFiberState::Defected as u32 => {
                TaskValue::Defect(event.defect_code)
            }
            _ => return,
        };
        let previous = self.task_values.insert(task, value);
        debug_assert!(previous.is_none(), "a fiber ends once");
    }

    fn release_channel_value(&self, value: usize) {
        if let Some((release, context)) = self.release_channel_value {
            // SAFETY: the configuring caller keeps the hook and its context
            // valid until execution returns; the registry gives each
            // accepted value up at most once.
            unsafe { release(context, value as i64) };
        }
    }

    fn release_task_context(&self, task_context: *mut c_void) {
        if task_context.is_null() {
            return;
        }
        if let Some((release, context)) = self.release_task_context {
            // SAFETY: as for `release_channel_value`; a task's context is
            // cleared when it is released or its entry runs, so it is
            // released at most once and never after the entry took it.
            unsafe { release(context, task_context) };
        }
    }

    /// Releases every task value still kept, every value still held by a
    /// channel, and the context of every task whose entry never ran. Runs
    /// once, when the scheduler is dropped.
    fn release_held_values(&mut self) {
        let unrun_contexts: Vec<_> = self
            .tasks
            .values_mut()
            .map(|record| std::mem::replace(&mut record.entry_context, std::ptr::null_mut()))
            .collect();
        for task_context in unrun_contexts {
            self.release_task_context(task_context);
        }
        for value in self.channels.drain_held_values() {
            self.release_channel_value(value);
        }
        let task_values = std::mem::take(&mut self.task_values);
        if let Some((release, context)) = self.release_task_value {
            for value in task_values.into_values() {
                // SAFETY: as for `release_channel_value`; each kept value is
                // removed from the map before it is released.
                unsafe { release(context, value.word()) };
            }
        }
    }

    fn start(
        &mut self,
        parent: Option<TaskId>,
        entry: ChimiFiberEntry,
        context: *mut c_void,
    ) -> Result<TaskId, SchedulerError> {
        self.create_task(parent, entry, context, None)
    }

    /// Creates a runnable task at the tail of the ready queue. An
    /// incarnation task has no parent and emits no task events.
    fn create_task(
        &mut self,
        parent: Option<TaskId>,
        entry: ChimiFiberEntry,
        context: *mut c_void,
        process: Option<IncarnationId>,
    ) -> Result<TaskId, SchedulerError> {
        let id = TaskId(self.next_task);
        let started_at = self.channels.tick();
        let carrier = parent.map_or(self.current, |parent| self.tasks[&parent].carrier);
        let fiber = PinnedFiber::new(
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
        self.next_task = self
            .next_task
            .checked_add(1)
            .expect("task identity space exhausted");
        self.tasks.insert(
            id,
            Task {
                parent,
                children: BTreeSet::new(),
                waiting_on: None,
                waiting_any: Vec::new(),
                terminal_sequence: 0,
                fiber: Some(fiber),
                phase: TaskPhase::Runnable,
                observed: false,
                handle_released: false,
                process,
                command: ChimiFiberCommand::Resume,
                request: std::ptr::null_mut(),
                park: 0,
                ran: false,
                entry_context: std::ptr::null_mut(),
                timer: None,
                scopes: Vec::new(),
                started_in_scope: parent
                    .and_then(|parent| self.tasks[&parent].scopes.last().map(|entry| entry.id)),
                select: None,
                io_wait: None,
                started_at,
                terminating: None,
                carrier,
                pending_end: None,
            },
        );
        if let Some(parent) = parent {
            self.tasks
                .get_mut(&parent)
                .expect("validated parent remains live")
                .children
                .insert(id);
        }
        self.peak_tasks = self.peak_tasks.max(self.tasks.len());
        self.enqueue(id, false);
        self.emit(id, TaskEventKind::Started, None, false);
        Ok(id)
    }

    /// The most task records held at once so far.
    pub fn peak_tasks(&self) -> usize {
        self.peak_tasks
    }

    /// Resumes `task` inline, holding the kernel across the resume. Only the
    /// two inline resumes use it (`begin_terminal`, `await_incarnation`),
    /// until they become jobs; the carrier loop resumes without the kernel.
    fn run(&mut self, task: TaskId) {
        if let Some(runnable) = self.prepare_run(task) {
            let event = resume_fiber(runnable.fiber, runnable.command);
            self.complete_run(task, event);
        }
    }

    /// Marks a runnable `task` running and enters its fiber; None if it is no
    /// longer runnable.
    fn prepare_run(&mut self, task: TaskId) -> Option<carrier::Runnable> {
        let record = self
            .tasks
            .get_mut(&task)
            .filter(|record| record.phase == TaskPhase::Runnable)?;
        debug_assert!(
            self.any_carrier || record.carrier == self.current,
            "a task resumed off its home carrier"
        );
        let command = std::mem::replace(&mut record.command, ChimiFiberCommand::Resume);
        record.phase = TaskPhase::Running;
        record.request = std::ptr::null_mut();
        record.ran = true;
        record.entry_context = std::ptr::null_mut();
        let context = scope::ResumeContext {
            // A forced incarnation is abandoned at its next park, so its
            // unwind must issue no request either, scope exits included.
            terminal: command == ChimiFiberCommand::Cancel
                && self.tasks[&task].process.is_some()
                && self.is_forced(task),
            scope_pending: self.scope_cancel_pending(task),
            polls: self.polls_scope_deadlines(task),
        };
        let fiber = self
            .tasks
            .get_mut(&task)
            .expect("running task")
            .fiber
            .as_mut()
            .expect("runnable task owns a fiber")
            .enter(context);
        Some(carrier::Runnable {
            task,
            fiber,
            command,
            kind: carrier::RunKind::Step,
        })
    }

    /// True when `task`'s fiber, resumed with `command`, may run on the
    /// current carrier: it is the fiber's home, the fiber never ran and the
    /// command is Cancel (it then returns without running, so nothing is
    /// pinned), or the scheduler is being torn down.
    fn may_resume_here(&self, task: TaskId, command: ChimiFiberCommand) -> bool {
        let record = &self.tasks[&task];
        self.any_carrier
            || record.carrier == self.current
            || (!record.ran && command == ChimiFiberCommand::Cancel)
    }

    /// Runs every carrier's queued jobs here, for teardown.
    fn run_queued_jobs(&mut self) {
        for carrier in 0..self.ready.carriers() {
            while let Some(job) = self.jobs.pop_front(ready::Carrier(carrier)) {
                if let Some(runnable) = self.prepare_job(job) {
                    let event = resume_fiber(runnable.fiber, runnable.command);
                    self.complete(runnable, event);
                }
            }
        }
    }

    /// Enters the fiber a job resumes; None if the job's task is gone.
    fn prepare_job(&mut self, job: jobs::Job) -> Option<carrier::Runnable> {
        match job {
            jobs::Job::Unwind { task, forced } => {
                let record = self.tasks.get_mut(&task)?;
                debug_assert!(
                    self.any_carrier || record.carrier == self.current,
                    "a job run off its home carrier"
                );
                debug_assert!(matches!(record.phase, TaskPhase::Draining(_)), "an unwind job's task drains");
                let fiber = record.fiber.as_mut()?.enter(scope::ResumeContext::TERMINAL);
                Some(carrier::Runnable {
                    task,
                    fiber,
                    command: ChimiFiberCommand::Cancel,
                    kind: carrier::RunKind::Unwind { forced },
                })
            }
        }
    }

    /// Applies what a resume `drive` ran returned, as its kind says.
    fn complete(&mut self, runnable: carrier::Runnable, event: ChimiFiberEvent) {
        match runnable.kind {
            carrier::RunKind::Step => self.complete_run(runnable.task, event),
            carrier::RunKind::Unwind { forced } => {
                let outcome = match self.tasks[&runnable.task].phase {
                    TaskPhase::Draining(outcome) => outcome,
                    phase => unreachable!("an unwind job's task is {phase:?}"),
                };
                self.forcing += u32::from(forced);
                self.end_after_unwind(runnable.task, outcome, &event);
                self.forcing -= u32::from(forced);
            }
        }
    }

    /// Applies what `task`'s resume returned: its park and request, or its end.
    fn complete_run(&mut self, task: TaskId, event: ChimiFiberEvent) {
        let end = self.tasks.get_mut(&task).and_then(|record| record.pending_end.take());
        self.apply_step(task, event);
        // An end decided while the fiber ran on this carrier applies as if
        // decided just after this step: the step's park is served first, as
        // one carrier always serves it, and an end the step already reached
        // is not overridden.
        if let Some(end) = end
            && self.tasks.get(&task).is_some_and(|record| record.phase.is_live())
        {
            if end.forced {
                self.force_terminal(task, end.outcome, false);
            } else {
                self.cancel_task(task, end.outcome);
            }
        }
    }

    /// Applies an ordinary step's return: a park and its request, or the end.
    fn apply_step(&mut self, task: TaskId, event: ChimiFiberEvent) {
        if event.state == ChimiFiberState::Parked as u32 {
            let record = self.tasks.get_mut(&task).expect("running task");
            record.phase = TaskPhase::Parked;
            record.request = event.request;
            record.park += 1;
            let process = record.process.is_some();
            self.emit(task, TaskEventKind::Parked, None, true);
            if process && self.is_forced(task) {
                self.abandon_forced(task);
                return;
            }
            if !event.request.is_null() {
                self.handle_request(task, event.request.cast());
            }
            return;
        }
        self.keep_task_value(task, &event);
        let outcome = self.outcome(task, event);
        if self.tasks[&task].process.is_some() && self.is_forced(task) {
            // A forced body's children end with it, releases or not.
            self.force_terminal(task, outcome, true);
        } else {
            self.begin_terminal(task, outcome, true);
        }
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
        // SAFETY: every request record starts with `RequestHeader`, and
        // `chimi_fiber_request` keeps it alive until the fiber resumes.
        let kind = unsafe { (*request.cast::<RequestHeader>()).kind };
        let park = self.tasks[&owner].park;
        // A running task is no longer parked in a select, whatever it asks
        // now (a reissued select registers afresh), so a scope refusal cannot
        // leave its watchers and wake timer behind.
        self.clear_select(owner);
        self.clear_io_wait(owner);
        self.clear_child_wait(owner);
        // A reissued signal wait keeps the task's place among the waiters
        // and any bit it was woken for; anything else leaves the wait.
        if kind != REQUEST_SIGNAL_WAIT {
            self.withdraw_signal_wait(owner);
        }
        self.poll_scope_deadlines(owner);
        if self.refuse_for_scope(owner, kind) {
            self.withdraw_signal_wait(owner);
            return;
        }
        if (REQUEST_PROCESS_SELF..=REQUEST_SUPERVISOR_REMOVE_SUPERVISOR).contains(&kind)
            || (REQUEST_PROCESS_MONITOR..=REQUEST_PROCESS_ARGUMENT).contains(&kind)
        {
            self.handle_process_request(owner, request.cast());
        } else {
            self.handle_task_request(owner, park, request);
        }
    }

    /// True while `task` still owns the fiber stack that holds the request
    /// of its park number `park`. A contract violation defects the caller
    /// and destroys that stack before the reply is written.
    fn request_is_live(&self, task: TaskId, park: u64) -> bool {
        self.tasks.get(&task).is_some_and(|record| {
            record.park == park && record.fiber.is_some() && !record.request.is_null()
        })
    }

    /// Serves a task, channel, or timer request. The reply is built in a
    /// copy and written back only if the caller's stack still exists.
    fn handle_task_request(&mut self, owner: TaskId, park: u64, record: *mut SchedulerRequest) {
        // SAFETY: the remaining kinds use `SchedulerRequest`; the record is
        // read before any operation can destroy the caller's fiber.
        let mut reply = unsafe { record.read() };
        let request = &mut reply;
        request.status = RESPONSE_UNSET;
        match request.kind {
            REQUEST_TIMER_NOW => {
                request.value = self.clock.now().as_nanos();
                request.status = RESPONSE_OK;
                let _ = self.wake_front(owner);
            }
            REQUEST_TIMER_SLEEP_UNTIL => {
                let deadline = Instant::from_nanos(request.value);
                if deadline <= self.clock.now() {
                    request.status = RESPONSE_OK;
                    let _ = self.wake_front(owner);
                } else {
                    request.status = RESPONSE_PENDING;
                    let _ = self.start_wake_timer(owner, deadline);
                }
            }
            REQUEST_SCOPE_ENTER => {
                let scope = self.enter_scope(owner, Instant::from_nanos(request.value));
                request.token = scope.get();
                request.status = RESPONSE_OK;
                let _ = self.wake_front(owner);
            }
            REQUEST_RESOURCE_ENTER => {
                request.token = self.enter_resource(owner).get();
                request.status = RESPONSE_OK;
                let _ = self.wake_front(owner);
            }
            REQUEST_RESOURCE_RELEASE => {
                request.status = if self.mask_resource(owner, ScopeId::from_raw(request.token)) {
                    RESPONSE_OK
                } else {
                    RESPONSE_ERROR
                };
                let _ = self.wake_front(owner);
            }
            REQUEST_RESOURCE_USE => {
                match self.use_resource(owner, ScopeId::from_raw(request.token)) {
                    Some(delivered) => {
                        request.value = i64::from(delivered);
                        request.status = RESPONSE_OK;
                    }
                    None => request.status = RESPONSE_ERROR,
                }
                let _ = self.wake_front(owner);
            }
            REQUEST_RESOURCE_EXIT => {
                match self.exit_resource(owner, ScopeId::from_raw(request.token)) {
                    Some(delivered) => {
                        request.value = i64::from(delivered);
                        request.status = RESPONSE_OK;
                    }
                    None => request.status = RESPONSE_ERROR,
                }
                let _ = self.wake_front(owner);
            }
            REQUEST_SCOPE_EXIT => {
                match self.exit_scope(owner, ScopeId::from_raw(request.token), request.value != 0) {
                    ScopeExit::Pending => request.status = RESPONSE_PENDING,
                    ScopeExit::Exited { delivered } => {
                        request.value = i64::from(delivered);
                        request.status = RESPONSE_OK;
                        let _ = self.wake_front(owner);
                    }
                    ScopeExit::Unknown => {
                        request.status = RESPONSE_ERROR;
                        let _ = self.wake_front(owner);
                    }
                }
            }
            REQUEST_SCOPE_CHECK => {
                request.status = RESPONSE_OK;
                let _ = self.wake_front(owner);
            }
            REQUEST_SCOPE_CANCEL => {
                request.status =
                    if self.cancel_scope_children(owner, ScopeId::from_raw(request.token)) {
                        RESPONSE_OK
                    } else {
                        RESPONSE_ERROR
                    };
                let _ = self.wake_front(owner);
            }
            REQUEST_SELECT => self.handle_select(owner, request),
            REQUEST_IO_WAIT => self.handle_io_wait(owner, request),
            REQUEST_IO_WITHDRAW => self.handle_io_withdraw(owner, request),
            REQUEST_SIGNAL_WAIT => self.handle_signal_wait(owner, request),
            REQUEST_CHILD_WAIT => self.handle_child_wait(owner, request),
            REQUEST_TASK_RELEASE => {
                request.status = if self.release_task(TaskId(request.task)).is_ok() {
                    RESPONSE_OK
                } else {
                    RESPONSE_ERROR
                };
                let _ = self.wake(owner);
            }
            REQUEST_TASK_START => {
                let Some(entry) = request.entry else {
                    request.status = RESPONSE_ERROR;
                    let _ = self.wake(owner);
                    return;
                };
                match self.start(Some(owner), entry, request.context) {
                    Ok(task) => {
                        self.tasks
                            .get_mut(&task)
                            .expect("started task")
                            .entry_context = request.context;
                        request.task = task.get();
                        // The child hands the capabilities in its result to
                        // this task (`include/chimi_fiber.h`).
                        request.owner = owner.get();
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
                    reply_with_outcome(request, outcome);
                    let _ = self.wake(owner);
                }
                Err(_) => request.status = RESPONSE_ERROR,
            },
            REQUEST_TASK_ANY | REQUEST_TASK_DRAIN => {
                let count = usize::try_from(request.value).unwrap_or(0);
                let tasks: Vec<TaskId> = if count == 0 || request.context.is_null() {
                    Vec::new()
                } else {
                    // SAFETY: the caller's record names `count` task ids that
                    // stay valid while it is suspended on the request.
                    unsafe { std::slice::from_raw_parts(request.context.cast::<u64>(), count) }
                        .iter()
                        .map(|raw| TaskId(*raw))
                        .collect()
                };
                if tasks.is_empty() {
                    request.status = RESPONSE_ERROR;
                    let _ = self.wake_front(owner);
                } else {
                    match self.first_terminal(owner, &tasks) {
                        Ok(Some(task)) => {
                            request.task = task.get();
                            let TaskPhase::Terminal(outcome) = self.tasks[&task].phase else {
                                unreachable!("first_terminal returns a terminal child");
                            };
                            self.tasks.get_mut(&task).expect("terminal child").observed = true;
                            reply_with_outcome(request, outcome);
                            let _ = self.wake_front(owner);
                        }
                        Ok(None) => request.status = RESPONSE_PENDING,
                        Err(_) => request.status = RESPONSE_ERROR,
                    }
                }
            }
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
                    // SAFETY: the host's record names `context` capabilities
                    // that stay valid while it is suspended on the request.
                    let carried = unsafe { carried_capabilities(request.context) };
                    self.channel_send_carrying(
                        owner,
                        Sender::from_raw(request.channel),
                        request.value as usize,
                        &carried,
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
                    Some(SendResult::Published(reached)) => {
                        let _ = self.take_send_result(owner);
                        request.value = reached as i64;
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
            REQUEST_CHANNEL_RELEASE => {
                request.status = if self
                    .channel_release(owner, ChannelId::from_raw(request.channel))
                    .is_ok()
                {
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
            REQUEST_CHANNEL_ADD_RECEIVER => {
                let receiver = Receiver::from_raw(request.channel, request.owner, request.token);
                match self.channel_add_receiver(owner, receiver) {
                    Ok(added) => {
                        request.channel = added.channel().get();
                        request.owner = added.owner().get();
                        request.token = added.token();
                        request.status = RESPONSE_OK;
                    }
                    Err(_) => request.status = RESPONSE_ERROR,
                }
                let _ = self.wake_front(owner);
            }
            REQUEST_CHANNEL_BROADCAST | REQUEST_CHANNEL_WATCH => {
                let (kind, capacity, initial) = if request.kind == REQUEST_CHANNEL_BROADCAST {
                    match usize::try_from(request.value) {
                        Ok(capacity) if capacity > 0 => (HubKind::Broadcast, capacity, None),
                        _ => (HubKind::Broadcast, 0, None),
                    }
                } else {
                    (HubKind::Watch, 1, Some(request.value as usize))
                };
                if capacity == 0 {
                    request.status = RESPONSE_ERROR;
                } else {
                    match self.create_hub(owner, kind, capacity, initial) {
                        Ok(completion) => {
                            request.channel = completion.channel().get();
                            request.owner = owner.get();
                            request.token = completion.token();
                            request.status = RESPONSE_OK;
                        }
                        Err(_) => request.status = RESPONSE_ERROR,
                    }
                }
                let _ = self.wake_front(owner);
            }
            REQUEST_CHANNEL_SUBSCRIBE => {
                match self.channel_subscribe(owner, Sender::from_raw(request.channel)) {
                    Ok(subscription) => {
                        request.channel = subscription.channel().get();
                        request.owner = subscription.owner().get();
                        request.token = subscription.token();
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
        if self.request_is_live(owner, park) {
            // SAFETY: the caller is still suspended on this record.
            unsafe { record.write(reply) };
        }
    }

    fn begin_terminal(&mut self, task: TaskId, mut outcome: TaskOutcome, fiber_is_terminal: bool) {
        let phase = self.tasks.get(&task).map(|record| record.phase);
        if matches!(phase, Some(TaskPhase::Draining(_) | TaskPhase::Terminal(_))) {
            return;
        }
        if !fiber_is_terminal && phase == Some(TaskPhase::Running) {
            // Its fiber runs on another carrier now: its own carrier applies
            // the end when the resume returns.
            self.record_pending_end(task, outcome);
            return;
        }
        let record = self.tasks.get_mut(&task).expect("known task");
        // A cancellation that waited for releases decides the outcome, unless
        // the fiber or this end is a defect.
        if let Some(recorded) = record.terminating.take()
            && (recorded.is_defect() || !outcome.is_defect())
        {
            outcome = recorded;
        }
        record.phase = TaskPhase::Draining(outcome);
        record.request = std::ptr::null_mut();
        if let Some(timer) = record.timer.take() {
            self.cancel_timer(timer);
        }
        self.clear_select(task);
        self.clear_io_wait(task);
        self.clear_child_wait(task);
        self.withdraw_signal_wait(task);
        self.clear_scopes(task);
        self.unroute_task(task);

        let channel_cleanup = self.channels.cleanup_task(task);
        self.apply_channel_transition(channel_cleanup);

        if fiber_is_terminal {
            self.end_drained(task, false);
            return;
        }
        self.emit(
            task,
            TaskEventKind::CancellationRequested,
            cause(outcome),
            false,
        );
        if !self.may_resume_here(task, ChimiFiberCommand::Cancel) {
            // A started fiber unwinds only on its home carrier's thread: a
            // job there resumes it, then ends the task.
            let forced = self.forcing > 0;
            let carrier = self.tasks[&task].carrier;
            self.jobs.push_back(carrier, jobs::Job::Unwind { task, forced });
            return;
        }
        let cleanup = self
            .tasks
            .get_mut(&task)
            .and_then(|record| record.fiber.as_mut())
            .expect("non-terminal task owns a fiber")
            .enter(scope::ResumeContext::TERMINAL);
        let cleanup = resume_fiber(cleanup, ChimiFiberCommand::Cancel);
        self.end_after_unwind(task, outcome, &cleanup);
    }

    /// Records an end decided while `task`'s fiber runs; the strongest wins,
    /// as for `terminating`: a defect over any other outcome.
    fn record_pending_end(&mut self, task: TaskId, outcome: TaskOutcome) {
        let forced = self.forcing > 0;
        let record = self.tasks.get_mut(&task).expect("running task");
        debug_assert_ne!(record.carrier, self.current, "a running task ends on its own carrier");
        record.pending_end = Some(match record.pending_end {
            Some(recorded) if recorded.outcome.is_defect() || !outcome.is_defect() => jobs::PendingEnd {
                outcome: recorded.outcome,
                forced: recorded.forced || forced,
            },
            Some(recorded) => jobs::PendingEnd {
                outcome,
                forced: recorded.forced || forced,
            },
            None => jobs::PendingEnd { outcome, forced },
        });
    }

    /// Finishes a draining task whose fiber's cancellation unwind returned
    /// `cleanup`: a defect, an overflow or a park in the unwind decides the
    /// outcome unless it already is a defect.
    fn end_after_unwind(&mut self, task: TaskId, mut outcome: TaskOutcome, cleanup: &ChimiFiberEvent) {
        self.keep_task_value(task, cleanup);
        let cleanup_parked = cleanup.state == ChimiFiberState::Parked as u32;
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
            self.tasks.get_mut(&task).expect("draining task").phase = TaskPhase::Draining(outcome);
        }
        self.end_drained(task, cleanup_parked);
    }

    /// The rest of a task's end once its fiber is done: drops the fiber
    /// (abandoning it when its unwind parked again), releases an unrun
    /// context, cancels its children and root supervisors, and publishes its
    /// terminal once they are done.
    fn end_drained(&mut self, task: TaskId, cleanup_parked: bool) {
        let record = self.tasks.get_mut(&task).expect("draining task");
        if cleanup_parked {
            // Cleanup parked again. A parked fiber cannot be destroyed
            // (destruction unwinds through C and aborts), so it is abandoned.
            std::mem::forget(record.fiber.take());
        }
        record.fiber = None;
        let unrun_context = std::mem::replace(&mut record.entry_context, std::ptr::null_mut());
        self.release_task_context(unrun_context);

        let children: Vec<_> = self
            .tasks
            .get(&task)
            .expect("draining task")
            .children
            .iter()
            .copied()
            .collect();
        for child in children {
            self.cancel_task(child, TaskOutcome::Cancelled);
        }
        // Root supervisors the task started are structured children too.
        self.stop_owned_supervisors(task);
        self.finish_draining(task);
    }

    /// Publishes a draining task's terminal once its children are terminal
    /// and every root supervisor it started has retired; a child or root
    /// that is still stopping calls this again when it finishes. A no-op for
    /// a task that is not draining.
    fn finish_draining(&mut self, task: TaskId) {
        let Some(record) = self.tasks.get(&task) else {
            return;
        };
        let TaskPhase::Draining(outcome) = record.phase else {
            return;
        };
        // A draining task still holding its fiber waits for its unwind (a
        // job on its home carrier) before anything else ends it.
        if record.fiber.is_some() || !record.children.is_empty() || self.owns_live_roots(task) {
            return;
        }
        self.emit(task, TaskEventKind::CleanupCompleted, cause(outcome), false);
        self.publish_terminal(task, outcome);
        if self.incarnation_of(task).is_some() {
            self.incarnation_finished(task, outcome);
        }
    }

    fn publish_terminal(&mut self, task: TaskId, outcome: TaskOutcome) {
        let parent = self.tasks.get(&task).expect("known task").parent;
        self.next_terminal_sequence += 1;
        let record = self.tasks.get_mut(&task).expect("known task");
        record.phase = TaskPhase::Terminal(outcome);
        record.terminal_sequence = self.next_terminal_sequence;
        if record.handle_released {
            self.released_terminal.push(task);
        }
        if let Some(parent) = parent {
            self.tasks
                .get_mut(&parent)
                .expect("structured parent outlives child")
                .children
                .remove(&task);
        }
        self.emit(task, TaskEventKind::Terminal, cause(outcome), false);
        if let Some(parent) = parent {
            self.finish_draining(parent);
            self.wake_scope_exit(parent);
            // A defect instead reaches the parent through containment below.
            if !outcome.is_defect() {
                self.wake_selecting_parent(parent, task);
            }
        }

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
                    let record = self.tasks.get_mut(&parent).expect("waiting parent");
                    record.phase = TaskPhase::Runnable;
                    self.enqueue(parent, false);
                    self.emit(parent, TaskEventKind::Woken, Some(task), true);
                }
                TaskOutcome::Cancelled => {
                    self.cancel_waiting_parent(parent, TaskOutcome::Cancelled);
                }
                TaskOutcome::Defected { .. } | TaskOutcome::StackOverflow { .. } => {
                    self.cancel_waiting_parent(parent, outcome);
                }
            }
        } else if outcome.is_defect()
            && let Some(parent) = parent.filter(|parent| {
                self.tasks.get(parent).is_some_and(|record| record.phase.is_live())
            })
            && !self.absorbs_child_defect(parent)
        {
            self.cancel_task(parent, outcome);
        } else if let Some(parent) = parent.filter(|parent| {
            self.tasks.get(parent).is_some_and(|record| {
                record.phase == TaskPhase::Parked && record.waiting_any.contains(&task)
            })
        }) {
            self.tasks
                .get_mut(&parent)
                .expect("waiting parent")
                .waiting_any
                .clear();
            let _ = self.wake(parent);
        }
        // A parent that keeps running, to finish a release or because shutdown
        // absorbed the defect, may be selecting on this child.
        if outcome.is_defect()
            && let Some(parent) = parent
        {
            self.wake_selecting_parent(parent, task);
        }
    }

    /// Ends a parent parked in `Task.result` of a child that ended cancelled
    /// or defected. A parent that keeps running to finish its releases
    /// (`cancel_task`) and is still parked, because a release is the one
    /// waiting, is woken to read the child's outcome itself.
    fn cancel_waiting_parent(&mut self, parent: TaskId, outcome: TaskOutcome) {
        self.cancel_task(parent, outcome);
        if self
            .tasks
            .get(&parent)
            .is_some_and(|record| record.phase == TaskPhase::Parked && record.fiber.is_some())
        {
            let _ = self.wake(parent);
        }
    }

    /// Ends `task` now even while it waits for its releases: a forced end
    /// (deadlock, contract violation, process force, program termination)
    /// reaches every descendant the same way.
    pub(super) fn force_terminal(
        &mut self,
        task: TaskId,
        outcome: TaskOutcome,
        fiber_is_terminal: bool,
    ) {
        self.forcing += 1;
        self.begin_terminal(task, outcome, fiber_is_terminal);
        self.force_live_descendants(task);
        self.forcing -= 1;
    }

    /// Ends every running descendant of `task` now, including those below a
    /// child that is already draining while it waits for them.
    pub(super) fn force_live_descendants(&mut self, task: TaskId) {
        let children: Vec<TaskId> = self
            .tasks
            .get(&task)
            .map(|record| record.children.iter().copied().collect())
            .unwrap_or_default();
        for child in children {
            match self.tasks.get(&child).map(|record| record.phase) {
                Some(TaskPhase::Runnable | TaskPhase::Running | TaskPhase::Parked) => {
                    self.force_terminal(child, TaskOutcome::Cancelled, false);
                }
                Some(TaskPhase::Draining(_)) => self.force_live_descendants(child),
                _ => {}
            }
        }
    }

    /// True when some descendant of `task` is still running.
    pub(super) fn has_live_descendants(&self, task: TaskId) -> bool {
        self.tasks.get(&task).is_some_and(|record| {
            record.children.iter().any(|child| {
                self.tasks.get(child).is_some_and(|record| {
                    record.phase.is_live()
                }) || self.has_live_descendants(*child)
            })
        })
    }

    /// Serves `REQUEST_TASK_ANY` and `REQUEST_TASK_DRAIN`: the listed child
    /// that terminated first, or `None` after arming the wait.
    fn first_terminal(
        &mut self,
        owner: TaskId,
        tasks: &[TaskId],
    ) -> Result<Option<TaskId>, SchedulerError> {
        for task in tasks {
            if let Err(error) = self.require_owner(Some(owner), *task) {
                self.defect_contract_violator(Some(owner));
                return Err(error);
            }
        }
        let first = tasks
            .iter()
            .filter_map(|task| {
                let record = &self.tasks[task];
                matches!(record.phase, TaskPhase::Terminal(_))
                    .then_some((record.terminal_sequence, *task))
            })
            .min()
            .map(|(_, task)| task);
        if first.is_none() {
            self.tasks
                .get_mut(&owner)
                .expect("validated owner")
                .waiting_any = tasks.to_vec();
        }
        Ok(first)
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
        if self.tasks.get(&owner).is_some_and(|record| record.phase.is_live()) {
            self.force_terminal(
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
        let record = self.tasks.get(&task);
        if record.is_some_and(|record| record.process.is_some()) {
            return;
        }
        let parent = record.and_then(|record| record.parent);
        self.constructed_events += 1;
        let event = TaskEvent {
            sequence: self.constructed_events,
            task,
            parent,
            kind,
            cause,
        };
        self.events.push(event);
        self.publish_trace(SchedulerTraceEvent {
            task: event.task.get(),
            parent: event.parent.map_or(0, TaskId::get),
            cause: event.cause.map_or(0, TaskId::get),
            ..SchedulerTraceEvent::blank(SCHEDULER_TRACE_TASK, kind as u32 + 1, event.sequence)
        });
    }

    fn apply_channel_transition(&mut self, transition: Transition) {
        for value in transition.discarded {
            self.release_channel_value(value);
        }
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
            task: event.task.map_or(0, TaskId::get),
            cause: event.cause.map_or(0, TaskId::get),
            channel: event.channel.get(),
            capacity: event.capacity as u64,
            queue_depth: event.queue_depth as u64,
            waiter_depth: event.waiter_depth as u64,
            discard_count: event.discard_count as u64,
            ..SchedulerTraceEvent::blank(
                SCHEDULER_TRACE_CHANNEL,
                event.kind as u32 + 1,
                event.sequence,
            )
        });
    }

    fn publish_trace(&self, event: SchedulerTraceEvent) {
        if let Some(sink) = self.trace_sink {
            // SAFETY: the C entry contract requires the caller-provided sink
            // and context to remain valid until scheduler execution returns.
            unsafe { sink(self.trace_context, &event) };
        }
    }

    /// Defects `task` for a channel request that broke the capability
    /// contract, and tells the host which capability, owner, and request.
    fn channel_violation<T>(
        &mut self,
        task: TaskId,
        error: ChannelError,
        request_kind: u32,
        destination: Option<TaskId>,
    ) -> Result<T, SchedulerError> {
        if let Some((report, context)) = self.capability_violation_report {
            let violation = ChimiCapabilityViolation::new(task, error, request_kind, destination);
            // SAFETY: the host keeps the hook and its context valid while the
            // scheduler runs; the record lives for the call.
            unsafe { report(context, &violation) };
        }
        self.force_terminal(
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

/// The capabilities a CHIMI_REQUEST_CHANNEL_SEND's message carries: `context`
/// is null or a `ChimiMessageCapabilities` record.
///
/// # Safety
///
/// A non-null `context` points to a readable record whose `items` holds
/// `count` readable entries.
unsafe fn carried_capabilities(context: *mut c_void) -> Vec<Carried> {
    // SAFETY: the caller guarantees a null or readable record.
    let Some(record) = (unsafe { context.cast::<ChimiMessageCapabilities>().as_ref() }) else {
        return Vec::new();
    };
    if record.count == 0 || record.items.is_null() {
        return Vec::new();
    }
    // SAFETY: as above, `items` holds `count` entries.
    let items = unsafe { std::slice::from_raw_parts(record.items, record.count) };
    items
        .iter()
        .filter_map(|item| {
            let kind = match item.capability {
                CAPABILITY_RECEIVER => CapabilityKind::Receiver,
                CAPABILITY_COMPLETION => CapabilityKind::Completion,
                _ => return None,
            };
            Some(Carried {
                channel: ChannelId::from_raw(item.channel),
                kind,
                token: item.token,
            })
        })
        .collect()
}

/// One exclusive capability a sent message holds: the sender's copy.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiCarriedCapability {
    pub channel: u64,
    pub token: u64,
    pub capability: u32,
}

/// The `context` of a CHIMI_REQUEST_CHANNEL_SEND whose message holds
/// exclusive capabilities.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiMessageCapabilities {
    pub items: *const ChimiCarriedCapability,
    pub count: usize,
}

fn scheduler_defect_outcome(code: i64) -> SchedulerOutcome {
    SchedulerOutcome {
        state: ChimiFiberState::Defected as u32,
        value: 0,
        origin: 0,
        defect_code: code,
    }
}

/// The arguments of `chimi_scheduler_execute_with_config`.
#[repr(C)]
pub struct ChimiSchedulerExecuteConfig {
    pub entry: Option<ChimiFiberEntry>,
    pub context: *mut c_void,
    pub stack_bytes: usize,
    pub trace_mode: u32,
    pub trace_sink: Option<SchedulerTraceSink>,
    pub trace_context: *mut c_void,
    pub release_task_value: Option<ValueRelease>,
    pub release_channel_value: Option<ValueRelease>,
    pub release_context: *mut c_void,
    pub release_task_context: Option<TaskContextRelease>,
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
    let config = ChimiSchedulerExecuteConfig {
        entry,
        context,
        stack_bytes,
        trace_mode,
        trace_sink,
        trace_context,
        release_task_value: None,
        release_channel_value: None,
        release_context: std::ptr::null_mut(),
        release_task_context: None,
    };
    // SAFETY: the config lives for the call, and the caller's guarantees
    // about the entry, context, and sink carry over unchanged.
    unsafe { chimi_scheduler_execute_with_config(&config) }
}

#[unsafe(no_mangle)]
/// `chimi_scheduler_execute` with release hooks for the host values the
/// scheduler discards.
///
/// # Safety
///
/// `config` must be null or valid for reads. Its entry and context satisfy
/// the Chimi fiber entry contract, and every callback and context stays
/// valid until this call returns.
pub unsafe extern "C" fn chimi_scheduler_execute_with_config(
    config: *const ChimiSchedulerExecuteConfig,
) -> SchedulerOutcome {
    // SAFETY: the caller guarantees a null or readable config.
    let Some(config) = (unsafe { config.as_ref() }) else {
        return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    let Some(entry) = config.entry else {
        return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    let trace_mode = match config.trace_mode {
        0 => TraceMode::Off,
        1 => TraceMode::Boundaries,
        2 => TraceMode::Full,
        _ => return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE),
    };
    let mut scheduler = Scheduler::with_options(SchedulerOptions {
        release_task_value: config.release_task_value,
        release_channel_value: config.release_channel_value,
        release_task_context: config.release_task_context,
        release_context: config.release_context,
        ..SchedulerOptions::new(config.stack_bytes, trace_mode, ClockMode::Real)
    });
    scheduler.set_trace_sink(config.trace_sink, config.trace_context);
    let Ok(root) = scheduler.start_root_with_context(entry, config.context) else {
        return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    scheduler.drive();
    let outcome = match scheduler.result(None, root) {
        Ok(TaskResult::Ready(outcome)) => outcome,
        Ok(TaskResult::Pending) | Err(_) => {
            return scheduler_defect_outcome(SCHEDULER_DEFECT_CODE);
        }
    };
    // The outcome's value passes to the caller; the scheduler releases
    // every other value when it is dropped below.
    scheduler.hand_out_outcome_value(root, outcome);
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

    /// A fiber's home is the thread of its first resume: later resumes and
    /// its destruction must come from that thread, and a fiber that never ran
    /// can go anywhere.
    #[test]
    fn a_fiber_home_is_the_thread_of_its_first_resume() {
        let mut home = Home::default();
        assert!(home.is_here(), "an unstarted fiber may be destroyed anywhere");
        let mut elsewhere = std::thread::spawn(move || {
            assert!(home.enter());
            assert!(home.enter());
            home
        })
        .join()
        .expect("first carrier");
        assert!(!elsewhere.is_here());
        assert!(!elsewhere.enter());
        let mut fresh = Home::default();
        std::thread::spawn(move || assert!(fresh.enter()))
            .join()
            .expect("an unstarted fiber may move");
    }

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

    thread_local! {
        static RELEASED_VALUES: std::cell::RefCell<Vec<i64>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn record_released_value(_context: *mut c_void, value: i64) {
        RELEASED_VALUES.with(|values| values.borrow_mut().push(value));
    }

    fn released_values() -> Vec<i64> {
        RELEASED_VALUES.with(|values| values.borrow().clone())
    }

    fn releasing_scheduler() -> Scheduler {
        RELEASED_VALUES.with(|values| values.borrow_mut().clear());
        let mut options = SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Real);
        options.release_task_value = Some(record_released_value);
        Scheduler::with_options(options)
    }

    /// A select that answers with a ready receiver takes the value in the same
    /// step, so a rival receiver of the same queue served before the selecting
    /// task's follow-up receive cannot take it (parallel-scheduler.md,
    /// "Ordering rules under several carriers"). A value kept for a task that
    /// ends before its receive is released, not lost or redelivered.
    #[test]
    fn a_committed_select_keeps_its_value_from_a_rival_receiver() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        for selector_ends_first in [false, true] {
            RELEASED_VALUES.with(|values| values.borrow_mut().clear());
            let mut options = SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Real);
            options.release_channel_value = Some(record_released_value);
            let mut scheduler = Scheduler::with_options(options);
            let selector = start_parked(&mut scheduler);
            let rival = start_parked(&mut scheduler);
            let sender = start_parked(&mut scheduler);
            let channel = scheduler.create_channel(selector, 1).expect("channel");
            let added = scheduler
                .channel_add_receiver(selector, channel.receiver)
                .expect("second receiver");
            let rivals = scheduler
                .transfer_receiver(selector, added, rival)
                .expect("handed to the rival");
            assert_eq!(
                scheduler.channel_send(sender, channel.sender, 21),
                Ok(SendResult::Accepted)
            );
            let mut sources = [ChimiSelectSource {
                kind: SELECT_RECEIVE,
                channel: channel.receiver.channel().get(),
                owner: channel.receiver.owner().get(),
                token: channel.receiver.token(),
                task: 0,
                deadline: 0,
            }];
            let mut request = SchedulerRequest {
                kind: REQUEST_SELECT,
                status: 0,
                task: 0,
                channel: 0,
                owner: 0,
                token: 0,
                value: 1,
                entry: None,
                context: sources.as_mut_ptr().cast(),
            };
            scheduler.handle_select(selector, &mut request);
            assert_eq!((request.status, request.value), (RESPONSE_OK, 0));
            // The rival runs before the selector's receive.
            assert_eq!(
                scheduler.channel_receive(rival, rivals),
                Ok(ReceiveResult::Pending),
                "the committed value is the selector's"
            );
            if selector_ends_first {
                scheduler.cancel(None, selector).expect("cancel selector");
                assert_eq!(released_values(), [21], "released once");
            } else {
                assert_eq!(
                    scheduler.take_receive_result(selector),
                    Some(ReceiveResult::Received(21))
                );
                assert!(released_values().is_empty());
            }
        }
    }

    /// A select committed on a closed and drained channel keeps `Drained`
    /// for its receive, as the receive itself would have answered.
    #[test]
    fn a_select_committed_on_a_drained_channel_keeps_drained() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = scheduler(TraceMode::Off);
        let selector = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(selector, 1).expect("channel");
        scheduler
            .channel_close(selector, channel.completion)
            .expect("close");
        let mut sources = [ChimiSelectSource {
            kind: SELECT_RECEIVE,
            channel: channel.receiver.channel().get(),
            owner: channel.receiver.owner().get(),
            token: channel.receiver.token(),
            task: 0,
            deadline: 0,
        }];
        let mut request = SchedulerRequest {
            kind: REQUEST_SELECT,
            status: 0,
            task: 0,
            channel: 0,
            owner: 0,
            token: 0,
            value: 1,
            entry: None,
            context: sources.as_mut_ptr().cast(),
        };
        scheduler.handle_select(selector, &mut request);
        assert_eq!((request.status, request.value), (RESPONSE_OK, 0));
        assert_eq!(
            scheduler.take_receive_result(selector),
            Some(ReceiveResult::Drained)
        );
    }

    /// A task is woken onto its home carrier's queue, not onto the queue of
    /// the carrier that woke it, and a task started without a parent is
    /// homed on the carrier that started it.
    #[test]
    fn a_woken_task_waits_on_its_home_carrier() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (zero, one) = (ready::Carrier(0), ready::Carrier(1));
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(one);
        let task = start_parked(&mut scheduler);
        assert_eq!(scheduler.tasks[&task].carrier, one);
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Parked);

        scheduler.enter_carrier(zero);
        scheduler.wake(task).expect("wake");
        scheduler.drive();
        assert_eq!(
            scheduler.tasks[&task].phase,
            TaskPhase::Runnable,
            "carrier 0 does not run carrier 1's task"
        );
        scheduler.enter_carrier(one);
        scheduler.drive();
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Parked, "its home ran it");
    }

    /// Starts a parked root on `home` with one parked child, cancels the
    /// root from carrier 0, and drives every carrier until both are
    /// terminal. Returns the kernel, the two tasks, and whether a job was
    /// waiting (with the root still draining and the child live) before
    /// `home` ran.
    fn cancel_from_carrier_zero(carriers: u32, home: ready::Carrier) -> (Scheduler, TaskId, TaskId, bool) {
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Full, carriers);
        scheduler.enter_carrier(home);
        let root = start_parked(&mut scheduler);
        let child = scheduler
            .start_child(root, chimi_fixture_generated_many_parks)
            .expect("child");
        scheduler.drive();
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, root).expect("cancel");
        scheduler.drive();
        let waiting = matches!(scheduler.tasks[&root].phase, TaskPhase::Draining(_))
            && scheduler.tasks[&root].fiber.is_some()
            && scheduler.tasks[&child].phase == TaskPhase::Parked;
        scheduler.enter_carrier(home);
        scheduler.drive();
        (scheduler, root, child, waiting)
    }

    fn kinds_of(scheduler: &Scheduler, task: TaskId) -> Vec<TaskEventKind> {
        scheduler
            .events()
            .iter()
            .filter(|event| event.task == task)
            .map(|event| event.kind)
            .collect()
    }

    /// A cancel decided on carrier 0 of a parked task homed on carrier 1
    /// waits, draining, for a job on carrier 1, which unwinds the fiber on
    /// its home and then ends the task and its child. Each task records the
    /// same events as when one carrier does it all.
    #[test]
    fn a_cancel_from_another_carrier_unwinds_on_the_home_carrier() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (two, root, child, waiting) = cancel_from_carrier_zero(2, ready::Carrier(1));
        assert!(waiting, "the root drains with its fiber until its job runs");
        assert!(two.jobs.is_empty());
        assert_eq!(two.tasks[&root].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
        assert_eq!(two.tasks[&child].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));

        let (one, one_root, one_child, inline) = cancel_from_carrier_zero(1, ready::Carrier(0));
        assert!(!inline, "one carrier unwinds inline");
        assert_eq!(kinds_of(&two, root), kinds_of(&one, one_root));
        assert_eq!(kinds_of(&two, child), kinds_of(&one, one_child));
    }

    /// A task that never ran is ended at once from any carrier: its fiber
    /// returns Cancelled without running, so nothing pins it to a thread.
    #[test]
    fn a_cancel_from_another_carrier_ends_an_unstarted_task_at_once() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let task = scheduler
            .start_root(chimi_fixture_generated_many_parks)
            .expect("task");
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, task).expect("cancel");
        assert!(scheduler.jobs.is_empty());
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
    }

    thread_local! {
        static SERVED_STATUS: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
    }

    /// Asks the time once, records the answer's status, then parks until
    /// cancelled.
    unsafe extern "C" fn ask_the_time_then_park(_context: *mut c_void) -> crate::ChimiFiberResult {
        let mut request = SchedulerRequest {
            kind: REQUEST_TIMER_NOW,
            status: 0,
            task: 0,
            channel: 0,
            owner: 0,
            token: 0,
            value: 0,
            entry: None,
            context: std::ptr::null_mut(),
        };
        let mut command = unsafe {
            crate::chimi_fiber_request((&raw mut request).cast(), std::ptr::null())
        };
        SERVED_STATUS.with(|served| served.set(request.status));
        while command != ChimiFiberCommand::Cancel {
            command = unsafe { crate::chimi_fiber_park(std::ptr::null()) };
        }
        crate::ChimiFiberResult {
            state: ChimiFiberState::Cancelled as u32,
            value: 0,
            defect_code: 0,
        }
    }

    /// A cancel decided while the fiber runs elsewhere applies after the
    /// step it interrupts: the request that step parks on is served first,
    /// as one carrier serves it, and only then is the fiber unwound.
    #[test]
    fn a_pending_end_serves_the_request_its_step_parked_on() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        SERVED_STATUS.with(|served| served.set(u32::MAX));
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let task = scheduler.start_root(ask_the_time_then_park).expect("task");
        let runnable = scheduler
            .ready
            .pop_front(ready::Carrier(1))
            .and_then(|task| scheduler.prepare_run(task))
            .expect("runnable");
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, task).expect("cancel");
        scheduler.enter_carrier(ready::Carrier(1));
        let event = resume_fiber(runnable.fiber, runnable.command);
        scheduler.complete(runnable, event);
        assert_eq!(SERVED_STATUS.with(std::cell::Cell::get), RESPONSE_OK);
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
    }

    thread_local! {
        static UNWIND_TERMINAL: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
    }

    /// Parks until cancelled, then records whether its unwind is terminal.
    unsafe extern "C" fn record_terminal_unwind(_context: *mut c_void) -> crate::ChimiFiberResult {
        while unsafe { crate::chimi_fiber_park(std::ptr::null()) } != ChimiFiberCommand::Cancel {}
        UNWIND_TERMINAL.with(|terminal| terminal.set(scope::chimi_fiber_cancel_is_terminal()));
        crate::ChimiFiberResult {
            state: ChimiFiberState::Cancelled as u32,
            value: 0,
            defect_code: 0,
        }
    }

    /// A job unwinds as the inline path does: in a terminal unwind, and with
    /// the unwind's own result (here a defect) deciding the outcome.
    #[test]
    fn a_jobs_unwind_is_terminal_and_its_result_decides_the_outcome() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        UNWIND_TERMINAL.with(|terminal| terminal.set(u32::MAX));
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let recorder = scheduler.start_root(record_terminal_unwind).expect("recorder");
        let defecting = scheduler
            .start_root(chimi_fixture_generated_cancel_defect)
            .expect("defecting");
        scheduler.drive();
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, recorder).expect("cancel recorder");
        scheduler.cancel(None, defecting).expect("cancel defecting");
        scheduler.enter_carrier(ready::Carrier(1));
        scheduler.drive();
        assert_eq!(UNWIND_TERMINAL.with(std::cell::Cell::get), 1);
        assert_eq!(
            scheduler.tasks[&defecting].phase,
            TaskPhase::Terminal(TaskOutcome::Defected { origin: defecting, code: 88 })
        );
    }

    /// A carrier runs its jobs before its ready tasks.
    #[test]
    fn a_carrier_runs_its_jobs_before_its_ready_tasks() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Full, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let ready_task = start_parked(&mut scheduler);
        let cancelled = start_parked(&mut scheduler);
        scheduler.wake(ready_task).expect("wake");
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, cancelled).expect("cancel");
        scheduler.enter_carrier(ready::Carrier(1));
        scheduler.drive();
        let sequence = |task, kind| {
            scheduler
                .events()
                .iter()
                .rfind(|event| event.task == task && event.kind == kind)
                .map(|event| event.sequence)
                .expect("event")
        };
        assert!(
            sequence(cancelled, TaskEventKind::CleanupCompleted)
                < sequence(ready_task, TaskEventKind::Parked),
            "the job ran first"
        );
    }

    /// Ends decided while a fiber runs elsewhere merge as `terminating`
    /// does: a defect wins over a cancel, and any forced end makes it forced.
    #[test]
    fn pending_ends_merge_with_a_defect_and_a_force_winning() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let task = start_parked(&mut scheduler);
        scheduler.wake(task).expect("wake");
        let _runnable = scheduler
            .ready
            .pop_front(ready::Carrier(1))
            .and_then(|task| scheduler.prepare_run(task))
            .expect("runnable");
        scheduler.enter_carrier(ready::Carrier(0));
        let defect = TaskOutcome::Defected { origin: task, code: 5 };
        scheduler.cancel_task(task, TaskOutcome::Cancelled);
        scheduler.cancel_task(task, defect);
        assert_eq!(
            scheduler.tasks[&task].pending_end,
            Some(jobs::PendingEnd { outcome: defect, forced: false })
        );
        scheduler.force_terminal(task, TaskOutcome::Cancelled, false);
        assert_eq!(
            scheduler.tasks[&task].pending_end,
            Some(jobs::PendingEnd { outcome: defect, forced: true })
        );
        // The fiber is left parked; its record is forgotten at drop.
        scheduler.tasks.get_mut(&task).expect("task").pending_end = None;
        scheduler.tasks.get_mut(&task).expect("task").phase = TaskPhase::Parked;
    }

    /// A forced end decided while the fiber runs elsewhere stays forced when
    /// it applies: a task waiting for its releases (`terminating`) ends at
    /// once instead of waiting on.
    #[test]
    fn a_pending_forced_end_does_not_wait_for_releases() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let task = start_parked(&mut scheduler);
        scheduler.wake(task).expect("wake");
        let runnable = scheduler
            .ready
            .pop_front(ready::Carrier(1))
            .and_then(|task| scheduler.prepare_run(task))
            .expect("runnable");
        // As if a cancellation already waits for its releases.
        scheduler.tasks.get_mut(&task).expect("task").terminating = Some(TaskOutcome::Cancelled);
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.force_terminal(task, TaskOutcome::Cancelled, false);
        scheduler.enter_carrier(ready::Carrier(1));
        let event = resume_fiber(runnable.fiber, runnable.command);
        scheduler.complete(runnable, event);
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
    }

    /// A forced job's unwind ends the task's children at once, even one
    /// waiting for its releases.
    #[test]
    fn a_forced_job_forces_the_tasks_children() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let root = start_parked(&mut scheduler);
        let child = scheduler
            .start_child(root, chimi_fixture_generated_many_parks)
            .expect("child");
        scheduler.drive();
        // As if the child's cancellation already waits for its releases.
        scheduler.tasks.get_mut(&child).expect("child").terminating = Some(TaskOutcome::Cancelled);
        scheduler.enter_carrier(ready::Carrier(0));
        // A forced end of the root alone, as `force_terminal` begins one.
        scheduler.forcing += 1;
        scheduler.begin_terminal(root, TaskOutcome::Cancelled, false);
        scheduler.forcing -= 1;
        assert_eq!(scheduler.tasks[&child].phase, TaskPhase::Parked);
        scheduler.enter_carrier(ready::Carrier(1));
        scheduler.drive();
        assert_eq!(scheduler.tasks[&child].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
        assert_eq!(scheduler.tasks[&root].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
    }

    /// A task waiting for its unwind job is not published terminal when its
    /// last child ends meanwhile; the job ends it.
    #[test]
    fn a_task_waiting_for_its_unwind_outlives_its_last_child() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let root = start_parked(&mut scheduler);
        let child = scheduler
            .start_child(root, chimi_fixture_generated_many_parks)
            .expect("unstarted child");
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, root).expect("cancel root");
        scheduler.cancel(Some(root), child).expect("cancel child");
        assert_eq!(scheduler.tasks[&child].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
        assert!(
            matches!(scheduler.tasks[&root].phase, TaskPhase::Draining(_)),
            "the root waits for its unwind"
        );
        scheduler.enter_carrier(ready::Carrier(1));
        scheduler.drive();
        assert_eq!(scheduler.tasks[&root].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
    }

    /// A forced end decided on another carrier is marked forced on its job.
    #[test]
    fn a_forced_end_from_another_carrier_queues_a_forced_job() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let task = start_parked(&mut scheduler);
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.force_terminal(task, TaskOutcome::Cancelled, false);
        assert_eq!(
            scheduler.jobs.pop_front(ready::Carrier(1)),
            Some(jobs::Job::Unwind { task, forced: true })
        );
    }

    /// An end decided while the task's fiber runs on another carrier waits
    /// for the resume to return, then unwinds the fiber where it parked.
    #[test]
    fn a_cancel_while_the_fiber_runs_elsewhere_applies_when_it_parks() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(ready::Carrier(1));
        let task = start_parked(&mut scheduler);
        scheduler.wake(task).expect("wake");
        let runnable = scheduler
            .ready
            .pop_front(ready::Carrier(1))
            .and_then(|task| scheduler.prepare_run(task))
            .expect("runnable");
        scheduler.enter_carrier(ready::Carrier(0));
        scheduler.cancel(None, task).expect("cancel");
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Running, "the fiber still runs");
        assert!(scheduler.jobs.is_empty());
        scheduler.enter_carrier(ready::Carrier(1));
        let event = resume_fiber(runnable.fiber, runnable.command);
        assert_eq!(event.state, ChimiFiberState::Parked as u32, "the step ran as it was");
        scheduler.complete(runnable, event);
        assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
    }

    /// A child is homed on its parent's carrier, not on the carrier that
    /// started it, so only the parent's carrier runs it.
    #[test]
    fn a_child_is_homed_on_its_parents_carrier() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let (zero, one) = (ready::Carrier(0), ready::Carrier(1));
        let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
        scheduler.enter_carrier(one);
        let parent = start_parked(&mut scheduler);
        scheduler.enter_carrier(zero);
        let child = scheduler
            .start_child(parent, chimi_fixture_generated_many_parks)
            .expect("child");
        assert_eq!(scheduler.tasks[&child].carrier, one);
        scheduler.drive();
        assert_eq!(scheduler.tasks[&child].phase, TaskPhase::Runnable, "carrier 0 does not run it");
        scheduler.enter_carrier(one);
        scheduler.drive();
        assert_eq!(scheduler.tasks[&child].phase, TaskPhase::Parked, "its home ran it");
    }

    /// Each kernel lock tells the kernel which carrier holds it: a task
    /// started through carrier 1's lock is homed there and only carrier 1's
    /// loop runs it.
    #[test]
    fn the_kernel_lock_enters_its_carrier() {
        use carrier::{KernelAccess, Locked};
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let kernel = std::sync::Mutex::new(Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2));
        let (mut zero, mut one) = (
            Locked::new(&kernel, ready::Carrier(0)),
            Locked::new(&kernel, ready::Carrier(1)),
        );
        let task = one
            .with(|kernel| kernel.start_root(chimi_fixture_generated_many_parks))
            .expect("task");
        carrier::drive(&mut zero);
        let phase = |kernel: &std::sync::Mutex<Scheduler>| kernel.lock().expect("kernel").tasks[&task].phase;
        assert_eq!(phase(&kernel), TaskPhase::Runnable, "carrier 0 does not run it");
        carrier::drive(&mut one);
        assert_eq!(phase(&kernel), TaskPhase::Parked);
        assert_eq!(kernel.lock().expect("kernel").tasks[&task].carrier, ready::Carrier(1));
    }

    #[test]
    fn a_released_task_is_removed_with_its_result_once_it_ends() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = releasing_scheduler();
        let root = start_parked(&mut scheduler);
        let child = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("child");
        scheduler.drive();
        assert_eq!(scheduler.release_task(child), Ok(()));
        assert_eq!(scheduler.metrics().tasks, 2, "a parked child stays");
        assert_eq!(
            scheduler.release_task(child),
            Err(SchedulerError::TaskStillLive(child)),
            "a handle is released once"
        );
        scheduler.wake(child).expect("wake child");
        scheduler.drive();
        assert_eq!(scheduler.metrics().tasks, 1);
        assert_eq!(released_values(), [42]);
        assert_eq!(
            scheduler.release_task(child),
            Err(SchedulerError::UnknownTask(child))
        );
        drop(scheduler);
        assert_eq!(released_values(), [42], "released once");
    }

    #[test]
    fn releasing_an_observed_terminal_task_removes_it_at_once() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = releasing_scheduler();
        let root = start_parked(&mut scheduler);
        let child = scheduler
            .start_child(root, chimi_fixture_generated_entry)
            .expect("child");
        scheduler.drive();
        scheduler.wake(child).expect("wake child");
        scheduler.drive();
        assert_eq!(
            scheduler.result(Some(root), child),
            Ok(TaskResult::Ready(TaskOutcome::Succeeded(42)))
        );
        assert_eq!(scheduler.release_task(child), Ok(()));
        assert_eq!(scheduler.metrics().tasks, 1);
        assert_eq!(released_values(), [42]);
        assert_eq!(
            scheduler.release_task(child),
            Err(SchedulerError::UnknownTask(child)),
            "a handle is released once"
        );
        assert_eq!(
            scheduler.release_task(root),
            Err(SchedulerError::TaskStillLive(root)),
            "a live root is released only once it has ended"
        );
    }

    #[test]
    fn a_released_defected_task_keeps_its_defect_code_until_the_end() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut scheduler = releasing_scheduler();
        let root = start_parked(&mut scheduler);
        let child = scheduler
            .start_child(root, chimi_fixture_generated_defect)
            .expect("child");
        assert_eq!(scheduler.release_task(child), Ok(()));
        scheduler.drive();
        assert!(
            !scheduler.tasks.contains_key(&child),
            "the defected child's record goes"
        );
        assert!(released_values().is_empty(), "its defect code stays");
        drop(scheduler);
        assert_eq!(
            released_values().len(),
            1,
            "the defect code goes at the end"
        );
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

    unsafe extern "C" fn collect_violation(
        context: *mut c_void,
        violation: *const ChimiCapabilityViolation,
    ) {
        let violations = unsafe { &mut *context.cast::<Vec<ChimiCapabilityViolation>>() };
        violations.push(unsafe { *violation });
    }

    // The host's capability-violation message names the capability, the task
    // that broke the contract, and the task that owns the capability, so the
    // hook must carry all three. A stale copy handed to a third task reports the
    // transfer's destination; a stale completion reports the completion; a
    // receiver whose owner ended reports no owner. Rejects a report that is
    // missing, sent for the wrong task, or that names the old owner.
    #[test]
    fn a_channel_capability_violation_reports_the_capability_and_its_owner() {
        let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
        let mut violations: Vec<ChimiCapabilityViolation> = Vec::new();
        let mut scheduler = Scheduler::with_options(SchedulerOptions {
            capability_violation_report: Some(collect_violation),
            violation_context: (&mut violations as *mut Vec<ChimiCapabilityViolation>).cast(),
            ..SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Real)
        });
        let parent = start_parked(&mut scheduler);
        let first = start_parked(&mut scheduler);
        let second = start_parked(&mut scheduler);
        let channel = scheduler.create_channel(parent, 1).expect("channel");
        scheduler
            .transfer_receiver(parent, channel.receiver, first)
            .expect("first transfer");
        assert!(
            scheduler
                .transfer_receiver(parent, channel.receiver, second)
                .is_err()
        );
        let other = start_parked(&mut scheduler);
        let other_channel = scheduler.create_channel(other, 1).expect("channel");
        scheduler
            .transfer_completion(other, other_channel.completion, first)
            .expect("completion transfer");
        assert!(
            scheduler
                .channel_close(other, other_channel.completion)
                .is_err()
        );
        let reader = start_parked(&mut scheduler);
        let ended = start_parked(&mut scheduler);
        let third_channel = scheduler.create_channel(reader, 1).expect("channel");
        scheduler
            .transfer_receiver(reader, third_channel.receiver, ended)
            .expect("transfer to the task that ends");
        scheduler.cancel(None, ended).expect("owner ends");
        scheduler.drive();
        assert!(
            scheduler
                .channel_receive(reader, third_channel.receiver)
                .is_err()
        );

        let channel_id = |capabilities: &ChannelCapabilities| capabilities.sender.channel().get();
        assert_eq!(
            violations,
            [
                ChimiCapabilityViolation {
                    task: parent.get(),
                    channel: channel_id(&channel),
                    owner: first.get(),
                    destination: second.get(),
                    request_kind: REQUEST_CHANNEL_TRANSFER_RECEIVER,
                    capability: CAPABILITY_RECEIVER,
                    reason: VIOLATION_WRONG_OWNER,
                },
                ChimiCapabilityViolation {
                    task: other.get(),
                    channel: channel_id(&other_channel),
                    owner: first.get(),
                    destination: 0,
                    request_kind: REQUEST_CHANNEL_CLOSE,
                    capability: CAPABILITY_COMPLETION,
                    reason: VIOLATION_WRONG_OWNER,
                },
                ChimiCapabilityViolation {
                    task: reader.get(),
                    channel: channel_id(&third_channel),
                    owner: 0,
                    destination: 0,
                    request_kind: REQUEST_CHANNEL_RECEIVE,
                    capability: CAPABILITY_RECEIVER,
                    reason: VIOLATION_WRONG_OWNER,
                },
            ]
        );
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
