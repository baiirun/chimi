//! Supervised processes as scheduler-owned state.
//!
//! Global invariants:
//! - Each incarnation runs on its own scheduler task, which has no parent and
//!   emits no task-family events; the process family describes it instead.
//! - A message handle belongs to exactly one place: the sender until a send
//!   is accepted, then the mailbox until a receive hands it to the body, or
//!   until termination releases it through the scheduler's release hook.
//!   Rejected sends hand the handle back.
//! - Incarnations of one membership never overlap, and a name maps to at most
//!   one live incarnation. Shutdown unroutes before cleanup runs.
//! - Supervisor decisions run between fiber resumes. A natural end, its
//!   termination event, the restart charge, and the replacement's creation
//!   happen in one step.
//! - Stops, removals, and escalations run as plans. A plan step that must
//!   wait for an incarnation's cleanup parks the plan on that incarnation;
//!   its shutdown deadline timer guarantees the plan resumes.
//! - Bookkeeping follows the slot: a membership record lives while its slot
//!   does, and a handle whose record is gone keeps only its last incarnation
//!   until `remove_process` consumes it. Stopped root records are dropped;
//!   an identity that was issued but has no record is a stopped supervisor.

use super::clock::{Instant, TimerTarget};
use super::{
    RequestHeader, Scheduler, SchedulerTraceEvent, TaskId, TaskOutcome, TaskPhase, TraceMode,
};
use crate::{ChimiFiberCommand, ChimiFiberEntry};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::c_void;
use std::num::{NonZeroU32, NonZeroU64};
use std::rc::Rc;
use std::time::Duration;

pub(super) const SCHEDULER_TRACE_PROCESS: u32 = 3;
pub(super) const RESPONSE_FORCED: u32 = 16;
pub(super) const RESTART_LIMIT_DEFECT_CODE: i64 = -6;
pub(super) const PROCESS_END_RETURNED: i64 = 0;
pub(super) const PROCESS_END_FAILED: i64 = 1;

macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u64);

        impl $name {
            pub fn get(self) -> u64 {
                self.0
            }

            pub fn from_raw(raw: u64) -> Self {
                Self(raw)
            }
        }
    };
}

identity!(SupervisorId);
identity!(MembershipId);
identity!(IncarnationId);

/// The declaration identity of a `process name`; its value is the trace
/// label's string-table index chosen by the runtime.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NameId(NonZeroU64);

impl NameId {
    pub fn new(label: NonZeroU64) -> Self {
        Self(label)
    }

    pub fn from_raw(raw: u64) -> Option<Self> {
        NonZeroU64::new(raw).map(Self)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Restart {
    Never,
    OnDefect,
}

/// A finite rolling restart limit with a positive window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Restarts {
    limit: u32,
    within: Duration,
}

impl Restarts {
    pub fn new(limit: u32, within: Duration) -> Result<Self, PolicyError> {
        if within.is_zero() {
            return Err(PolicyError::EmptyRestartWindow);
        }
        Ok(Self { limit, within })
    }
}

/// How to run one process incarnation. `context` is passed to every
/// incarnation's entry; the scheduler owns it from admission (see
/// `SchedulerOptions::release_spec_context`).
#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub entry: ChimiFiberEntry,
    pub context: *mut c_void,
    pub mailbox_capacity: NonZeroU32,
    pub shutdown_within: Duration,
    pub name: Option<NameId>,
}

#[derive(Clone, Debug)]
pub enum Member {
    Process(ProcessSpec, Restart),
    Supervisor(Policy, Restart),
}

/// Static topology and limits; never more declared members than capacity.
#[derive(Clone, Debug)]
pub struct Policy {
    members: Vec<Member>,
    capacity: NonZeroU32,
    restarts: Restarts,
}

impl Policy {
    pub fn new(
        members: Vec<Member>,
        capacity: NonZeroU32,
        restarts: Restarts,
    ) -> Result<Self, PolicyError> {
        if members.len() > capacity.get() as usize {
            return Err(PolicyError::MembersExceedCapacity {
                members: members.len(),
                capacity: capacity.get(),
            });
        }
        Ok(Self {
            members,
            capacity,
            restarts,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    EmptyRestartWindow,
    MembersExceedCapacity { members: usize, capacity: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Address {
    Named(NameId),
    Direct(IncarnationId),
}

/// Expected start failures; each is reported with an admission event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartError {
    CapacityReached,
    NameTaken(NameId),
    Stopped,
    /// The incarnation's fiber could not be created.
    SpawnFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendFailure {
    Full,
    Unavailable,
}

/// A rejected send; the sender keeps ownership of `message`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SendRejected {
    pub failure: SendFailure,
    pub message: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalCause {
    Returned,
    Failed,
    Defected,
    /// Requested by stop or removal; cleanup finished before its deadline.
    Cancelled,
    /// Requested by stop or removal; cleanup missed its deadline.
    Forced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendOutcome {
    Accepted,
    Full,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessEventKind {
    Started {
        supervisor: SupervisorId,
        membership: MembershipId,
        incarnation: IncarnationId,
        name: Option<NameId>,
    },
    AdmissionRejected {
        supervisor: SupervisorId,
        error: StartError,
    },
    Terminated {
        membership: MembershipId,
        incarnation: IncarnationId,
        cause: TerminalCause,
    },
    RestartAttempted {
        supervisor: SupervisorId,
        membership: MembershipId,
        attempts_in_window: u32,
    },
    RestartLimitExceeded {
        supervisor: SupervisorId,
    },
    /// `parent: None` means the root escalated and the program terminates.
    Escalated {
        supervisor: SupervisorId,
        parent: Option<SupervisorId>,
    },
    Removed {
        membership: MembershipId,
    },
    Stopped {
        supervisor: SupervisorId,
    },
    /// A full-trace event; `incarnation` is the one that accepted, if any.
    Sent {
        address: Address,
        incarnation: Option<IncarnationId>,
        outcome: SendOutcome,
    },
}

impl ProcessEventKind {
    fn trace_kind(self) -> u32 {
        match self {
            Self::Started { .. } => 1,
            Self::AdmissionRejected { .. } => 2,
            Self::Terminated { .. } => 3,
            Self::RestartAttempted { .. } => 4,
            Self::RestartLimitExceeded { .. } => 5,
            Self::Escalated { .. } => 6,
            Self::Removed { .. } => 7,
            Self::Stopped { .. } => 8,
            Self::Sent { .. } => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessEvent {
    pub sequence: u64,
    pub time: Instant,
    pub kind: ProcessEventKind,
}

/// Whether an operation that waits for shutdown has finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    Done,
    /// Cleanup is still running toward its deadline; drive the scheduler.
    Pending,
}

/// Contract violations; the runtime turns these into defects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessError {
    UnknownSupervisor(SupervisorId),
    UnknownMembership(MembershipId),
    WrongSupervisor {
        membership: MembershipId,
        owner: SupervisorId,
        actual: SupervisorId,
    },
    NotARoot(SupervisorId),
    NotAProcess(TaskId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorState {
    Running,
    /// Admission is closed and members are being stopped.
    Stopping,
    Stopped,
    /// The root exceeded its restart limit; the program is terminated.
    Escalated,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessMetrics {
    pub supervisors: usize,
    pub memberships: usize,
    pub retired_handles: usize,
    pub incarnations: usize,
    pub registered_names: usize,
    pub queued_messages: usize,
    pub blocked_plans: usize,
    pub timers: usize,
    pub process_events: usize,
}

pub type MessageRelease = unsafe extern "C" fn(*mut c_void, u64);
pub type SpecContextRelease = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// An admitted specification. Dropping the last reference releases its
/// context through the scheduler's hook.
pub(super) struct SpecRecord {
    pub(super) entry: ChimiFiberEntry,
    pub(super) context: *mut c_void,
    mailbox_capacity: NonZeroU32,
    shutdown_within: Duration,
    name: Option<NameId>,
    release: Option<(SpecContextRelease, *mut c_void)>,
}

impl Drop for SpecRecord {
    fn drop(&mut self) {
        if let Some((release, context)) = self.release {
            // SAFETY: the hook's owner promised it stays valid for the
            // scheduler's lifetime, and each context is released once.
            unsafe { release(context, self.context) };
        }
    }
}

struct PolicyRecord {
    members: Vec<MemberRecord>,
    capacity: NonZeroU32,
    restarts: Restarts,
}

#[derive(Clone)]
enum MemberKind {
    Process(Rc<SpecRecord>),
    Supervisor(Rc<PolicyRecord>),
}

#[derive(Clone)]
struct MemberRecord {
    kind: MemberKind,
    restart: Restart,
}

#[derive(Clone, Copy, Debug)]
enum Current {
    None,
    Process(IncarnationId),
    Supervisor(SupervisorId),
}

struct Membership {
    supervisor: SupervisorId,
    member: MemberRecord,
    current: Current,
    last_incarnation: Option<IncarnationId>,
    /// A `ManagedProcess` handle refers to this membership.
    handle: bool,
}

/// What a handle keeps once its membership record is gone.
#[derive(Clone, Copy, Debug)]
struct Retired {
    incarnation: IncarnationId,
    /// The member ended on its own, so removing it is still announced. A
    /// member ended by its supervisor's stop or escalation is removed
    /// silently.
    announce: bool,
}

#[derive(Clone, Copy, Debug)]
struct Stopping {
    deadline: Instant,
    timer: Option<super::clock::TimerId>,
    /// The body has been told it is cancelled; its next park (other than a
    /// receive) waits for the deadline.
    told: bool,
    forced: bool,
}

pub(super) struct Incarnation {
    membership: MembershipId,
    task: TaskId,
    spec: Rc<SpecRecord>,
    mailbox: VecDeque<u64>,
    /// False once shutdown begins or the body ends; sends then fail.
    accepting: bool,
    /// Parked in receive on an empty mailbox.
    receiving: bool,
    stopping: Option<Stopping>,
}

struct Supervision {
    policy: Rc<PolicyRecord>,
    parent: Option<(SupervisorId, MembershipId)>,
    declared: Vec<MembershipId>,
    dynamic: Vec<MembershipId>,
    restart_times: VecDeque<Instant>,
    state: SupervisorState,
    /// Tasks parked in `Supervisor.wait` or a `stop` that has not finished.
    waiters: Vec<Waiter>,
}

/// A task parked on a request, identified by its park count so a reply
/// never reaches a later request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Waiter {
    pub(super) task: TaskId,
    pub(super) park: u64,
}

#[derive(Clone, Copy, Debug)]
enum Step {
    /// Closes the subtree and expands into `StopMember` in shutdown order.
    StopMembers(SupervisorId),
    StopMember(MembershipId),
    /// Shuts the incarnation down and waits for it to terminate.
    Await(IncarnationId),
    Emit(ProcessEventKind),
    DropSupervisor(SupervisorId),
    FinishStop(SupervisorId),
    FinishEscalation(SupervisorId),
    FinishRemoval {
        membership: MembershipId,
        requester: Option<Waiter>,
    },
    Reply(Option<Waiter>),
}

struct Plan {
    steps: VecDeque<Step>,
    /// Set once the plan has waited for a deadline. A plan that completes
    /// without waiting resumes its caller next, as a non-parking call does.
    resumed: bool,
}

impl<const N: usize> From<[Step; N]> for Plan {
    fn from(steps: [Step; N]) -> Self {
        Self {
            steps: VecDeque::from(steps),
            resumed: false,
        }
    }
}

#[derive(Default)]
pub(super) struct ProcessKernel {
    next_supervisor: u64,
    next_membership: u64,
    next_incarnation: u64,
    supervisors: BTreeMap<SupervisorId, Supervision>,
    memberships: BTreeMap<MembershipId, Membership>,
    retired: BTreeMap<MembershipId, Retired>,
    pub(super) incarnations: BTreeMap<IncarnationId, Incarnation>,
    names: BTreeMap<NameId, IncarnationId>,
    roots: Vec<SupervisorId>,
    blocked: BTreeMap<IncarnationId, Vec<Plan>>,
    program_terminated: bool,
    pub(super) events: Vec<ProcessEvent>,
    pub(super) release_message: Option<(MessageRelease, *mut c_void)>,
    pub(super) release_spec: Option<(SpecContextRelease, *mut c_void)>,
    #[cfg(test)]
    pub(super) failing_spawns: u32,
}

impl ProcessKernel {
    pub(super) fn new(
        release_message: Option<(MessageRelease, *mut c_void)>,
        release_spec: Option<(SpecContextRelease, *mut c_void)>,
    ) -> Self {
        Self {
            release_message,
            release_spec,
            ..Self::default()
        }
    }
}

/// How a receive request is answered.
pub(super) enum Receipt {
    Message(u64),
    Pending,
    Cancelled,
    Forced,
}

impl Scheduler {
    // ----- Host-facing operations -----------------------------------------

    /// Starts a root supervisor and its declared members in list order.
    ///
    /// Ensures: every declared member has started and is routable, or every
    /// member already started is stopped again and the first failure is
    /// returned. No body runs before this returns.
    pub fn start_supervisor(&mut self, policy: Policy) -> Result<SupervisorId, StartError> {
        let policy = self.adopt_policy(policy);
        let supervisor = self.build_supervisor(policy, None)?;
        self.kernel.roots.push(supervisor);
        Ok(supervisor)
    }

    /// Admits a dynamic process. Fail-fast: a closed supervisor, a full one,
    /// or a held name is rejected with an admission event. The body first
    /// runs from the ready queue.
    pub fn start_process(
        &mut self,
        supervisor: SupervisorId,
        spec: ProcessSpec,
        restart: Restart,
    ) -> Result<MembershipId, StartError> {
        let member = MemberRecord {
            kind: MemberKind::Process(self.adopt_spec(spec)),
            restart,
        };
        match self.admit(supervisor, member, true) {
            Ok(membership) => {
                self.kernel
                    .memberships
                    .get_mut(&membership)
                    .expect("just admitted")
                    .handle = true;
                Ok(membership)
            }
            Err(error) => {
                self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });
                Err(error)
            }
        }
    }

    /// A direct address pinned to the membership's current incarnation, or
    /// to its last one once it has ended (which stays unavailable).
    pub fn process_address(&self, membership: MembershipId) -> Result<Address, ProcessError> {
        self.kernel
            .memberships
            .get(&membership)
            .and_then(|entry| entry.last_incarnation)
            .or_else(|| {
                self.kernel
                    .retired
                    .get(&membership)
                    .map(|retired| retired.incarnation)
            })
            .map(Address::Direct)
            .ok_or(ProcessError::UnknownMembership(membership))
    }

    /// Never parks. Accepted messages belong to the mailbox; a rejected
    /// message is handed back.
    pub fn process_send(&mut self, address: Address, message: u64) -> Result<(), SendRejected> {
        let target = self
            .resolve(address)
            .filter(|id| self.kernel.incarnations[id].accepting);
        let (outcome, accepted) = match target {
            None => (SendOutcome::Unavailable, None),
            Some(id) => {
                let incarnation = self.kernel.incarnations.get_mut(&id).expect("resolved");
                if incarnation.mailbox.len() >= incarnation.spec.mailbox_capacity.get() as usize {
                    (SendOutcome::Full, None)
                } else {
                    incarnation.mailbox.push_back(message);
                    let wake = std::mem::take(&mut incarnation.receiving);
                    let task = incarnation.task;
                    if wake {
                        let _ = self.wake(task);
                    }
                    (SendOutcome::Accepted, Some(id))
                }
            }
        };
        self.emit_process(ProcessEventKind::Sent {
            address,
            incarnation: accepted,
            outcome,
        });
        match outcome {
            SendOutcome::Accepted => Ok(()),
            SendOutcome::Full => Err(SendRejected {
                failure: SendFailure::Full,
                message,
            }),
            SendOutcome::Unavailable => Err(SendRejected {
                failure: SendFailure::Unavailable,
                message,
            }),
        }
    }

    /// Consumes a dynamic membership. Ensures, once `Done`: no incarnation
    /// of it runs and no pending restart can create one. A membership that
    /// already ended is removed without an event.
    pub fn remove_process(
        &mut self,
        supervisor: SupervisorId,
        membership: MembershipId,
    ) -> Result<Progress, ProcessError> {
        self.remove_process_for(None, supervisor, membership)
    }

    /// Stops a root supervisor's subtree: routing is removed for the whole
    /// subtree first, then members stop newest dynamic first and declared in
    /// reverse. A stopped supervisor is a no-op with no event.
    pub fn stop_supervisor(&mut self, supervisor: SupervisorId) -> Result<Progress, ProcessError> {
        self.stop_for(None, supervisor)
    }

    pub fn supervisor_state(&self, supervisor: SupervisorId) -> Option<SupervisorState> {
        match self.kernel.supervisors.get(&supervisor) {
            Some(record) => Some(record.state),
            None if (1..=self.kernel.next_supervisor).contains(&supervisor.0) => {
                Some(SupervisorState::Stopped)
            }
            None => None,
        }
    }

    /// True once a restart-limit escalation has reached a root.
    pub fn program_terminated(&self) -> bool {
        self.kernel.program_terminated
    }

    pub fn process_events(&self) -> &[ProcessEvent] {
        &self.kernel.events
    }

    pub fn drain_process_events(&mut self) -> Vec<ProcessEvent> {
        std::mem::take(&mut self.kernel.events)
    }

    pub fn process_metrics(&self) -> ProcessMetrics {
        let kernel = &self.kernel;
        ProcessMetrics {
            supervisors: kernel.supervisors.len(),
            memberships: kernel.memberships.len(),
            retired_handles: kernel.retired.len(),
            incarnations: kernel.incarnations.len(),
            registered_names: kernel.names.len(),
            queued_messages: kernel.incarnations.values().map(|i| i.mailbox.len()).sum(),
            blocked_plans: kernel.blocked.values().map(Vec::len).sum(),
            timers: self.timers.len(),
            process_events: kernel.events.len(),
        }
    }

    /// Stops every running root, newest first. Cleanup that misses its
    /// deadline finishes as the scheduler is driven.
    pub fn shutdown_processes(&mut self) {
        for root in self.kernel.roots.clone().into_iter().rev() {
            let _ = self.stop_for(None, root);
        }
    }

    pub(super) fn has_blocked_plans(&self) -> bool {
        !self.kernel.blocked.is_empty()
    }

    pub(super) fn incarnation_of(&self, task: TaskId) -> Option<IncarnationId> {
        self.tasks.get(&task).and_then(|record| record.process)
    }

    // ----- Request-facing operations --------------------------------------

    pub(super) fn remove_process_for(
        &mut self,
        requester: Option<Waiter>,
        supervisor: SupervisorId,
        membership: MembershipId,
    ) -> Result<Progress, ProcessError> {
        if let Some(retired) = self.kernel.retired.remove(&membership) {
            // If a stop is still forcing the last incarnation, wait for it.
            let await_last = Step::Await(retired.incarnation);
            let plan = if retired.announce {
                Plan::from([
                    await_last,
                    Step::Emit(ProcessEventKind::Removed { membership }),
                    Step::Reply(requester),
                ])
            } else {
                Plan::from([await_last, Step::Reply(requester)])
            };
            return Ok(self.execute(plan));
        }
        let entry = self
            .kernel
            .memberships
            .get(&membership)
            .ok_or(ProcessError::UnknownMembership(membership))?;
        if entry.supervisor != supervisor {
            return Err(ProcessError::WrongSupervisor {
                membership,
                owner: entry.supervisor,
                actual: supervisor,
            });
        }
        let finish = Step::FinishRemoval {
            membership,
            requester,
        };
        let plan = match entry.current {
            Current::Process(incarnation) => Plan::from([Step::Await(incarnation), finish]),
            Current::None | Current::Supervisor(_) => Plan::from([finish]),
        };
        Ok(self.execute(plan))
    }

    pub(super) fn stop_for(
        &mut self,
        requester: Option<Waiter>,
        supervisor: SupervisorId,
    ) -> Result<Progress, ProcessError> {
        let Some(state) = self.supervisor_state(supervisor) else {
            return Err(ProcessError::UnknownSupervisor(supervisor));
        };
        let record = self.kernel.supervisors.get_mut(&supervisor);
        if record.as_ref().is_some_and(|record| record.parent.is_some()) {
            return Err(ProcessError::NotARoot(supervisor));
        }
        match state {
            SupervisorState::Stopped | SupervisorState::Escalated => {
                if let Some(requester) = requester {
                    self.reply(requester, super::RESPONSE_OK, true);
                }
                Ok(Progress::Done)
            }
            SupervisorState::Stopping => {
                // Another stop is in flight; finish with it.
                let record = record.expect("a stopping supervisor has a record");
                record.waiters.extend(requester);
                Ok(Progress::Pending)
            }
            SupervisorState::Running => {
                let record = record.expect("a running supervisor has a record");
                record.waiters.extend(requester);
                let plan = Plan::from([
                    Step::StopMembers(supervisor),
                    Step::Emit(ProcessEventKind::Stopped { supervisor }),
                    Step::FinishStop(supervisor),
                ]);
                Ok(self.execute(plan))
            }
        }
    }

    /// Ok(true) when the supervisor has already stopped, Ok(false) when the
    /// waiter was registered. An escalated root reports `Escalated`.
    pub(super) fn wait_for(
        &mut self,
        waiter: Waiter,
        supervisor: SupervisorId,
    ) -> Result<SupervisorState, ProcessError> {
        let state = self
            .supervisor_state(supervisor)
            .ok_or(ProcessError::UnknownSupervisor(supervisor))?;
        if matches!(state, SupervisorState::Running | SupervisorState::Stopping) {
            self.kernel
                .supervisors
                .get_mut(&supervisor)
                .expect("live supervisor")
                .waiters
                .push(waiter);
        }
        Ok(state)
    }

    pub(super) fn self_address(&self, task: TaskId) -> Result<IncarnationId, ProcessError> {
        self.incarnation_of(task)
            .ok_or(ProcessError::NotAProcess(task))
    }

    /// Answers a receive from `incarnation`. Ensures: parks only while the
    /// mailbox is empty and no shutdown is pending; FIFO across senders.
    /// During cleanup it returns accepted messages until the mailbox is
    /// empty, then `Cancelled` on every call; past the deadline, `Forced`.
    pub(super) fn receive_for(&mut self, incarnation: IncarnationId) -> Receipt {
        let now = self.clock.now();
        let record = self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .expect("live incarnation");
        match record.stopping.as_mut() {
            Some(stopping) if stopping.forced || now >= stopping.deadline => {
                stopping.forced = true;
                Receipt::Forced
            }
            Some(stopping) => match record.mailbox.pop_front() {
                Some(message) => Receipt::Message(message),
                None => {
                    stopping.told = true;
                    Receipt::Cancelled
                }
            },
            None => match record.mailbox.pop_front() {
                Some(message) => Receipt::Message(message),
                None => {
                    record.receiving = true;
                    Receipt::Pending
                }
            },
        }
    }

    /// True when a request from this task must not be answered: its
    /// incarnation was already told it is cancelled and the request is one
    /// that can park, so it waits for the shutdown deadline. Requests that
    /// never park (send, self, now, starts) are still answered, and a
    /// receive answers `Cancelled` again.
    pub(super) fn park_lingers(&self, task: TaskId, kind: u32) -> bool {
        can_park(kind) && self.stopping(task).is_some_and(|stopping| stopping.told)
    }

    /// Interrupts a sleep in cleanup: the body is told it is cancelled.
    pub(super) fn interrupts_sleep(&mut self, task: TaskId) -> bool {
        let Some(incarnation) = self.incarnation_of(task) else {
            return false;
        };
        match self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .and_then(|record| record.stopping.as_mut())
        {
            Some(stopping) => {
                stopping.told = true;
                true
            }
            None => false,
        }
    }

    /// True when the task is a forced incarnation, which must not park again.
    pub(super) fn is_forced(&self, task: TaskId) -> bool {
        self.stopping(task).is_some_and(|stopping| stopping.forced)
    }

    fn stopping(&self, task: TaskId) -> Option<Stopping> {
        let incarnation = self.incarnation_of(task)?;
        self.kernel.incarnations.get(&incarnation)?.stopping
    }

    // ----- Admission and restart ------------------------------------------

    fn adopt_spec(&self, spec: ProcessSpec) -> Rc<SpecRecord> {
        Rc::new(SpecRecord {
            entry: spec.entry,
            context: spec.context,
            mailbox_capacity: spec.mailbox_capacity,
            shutdown_within: spec.shutdown_within,
            name: spec.name,
            release: self.kernel.release_spec,
        })
    }

    fn adopt_policy(&self, policy: Policy) -> Rc<PolicyRecord> {
        let members = policy
            .members
            .into_iter()
            .map(|member| match member {
                Member::Process(spec, restart) => MemberRecord {
                    kind: MemberKind::Process(self.adopt_spec(spec)),
                    restart,
                },
                Member::Supervisor(policy, restart) => MemberRecord {
                    kind: MemberKind::Supervisor(self.adopt_policy(policy)),
                    restart,
                },
            })
            .collect();
        Rc::new(PolicyRecord {
            members,
            capacity: policy.capacity,
            restarts: policy.restarts,
        })
    }

    fn build_supervisor(
        &mut self,
        policy: Rc<PolicyRecord>,
        parent: Option<(SupervisorId, MembershipId)>,
    ) -> Result<SupervisorId, StartError> {
        self.kernel.next_supervisor += 1;
        let supervisor = SupervisorId(self.kernel.next_supervisor);
        self.kernel.supervisors.insert(
            supervisor,
            Supervision {
                policy: Rc::clone(&policy),
                parent,
                declared: Vec::new(),
                dynamic: Vec::new(),
                restart_times: VecDeque::new(),
                state: SupervisorState::Running,
                waiters: Vec::new(),
            },
        );
        for member in policy.members.iter().cloned() {
            if let Err(error) = self.admit(supervisor, member, false) {
                self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });
                // Every started member is still fresh, so the rollback never
                // waits for a deadline.
                let rolled_back = self.execute(Plan::from([Step::StopMembers(supervisor)]));
                debug_assert_eq!(rolled_back, Progress::Done);
                self.kernel.supervisors.remove(&supervisor);
                return Err(error);
            }
        }
        Ok(supervisor)
    }

    /// Reserves a slot and starts the member's first incarnation. Allocates
    /// no membership when admission fails.
    fn admit(
        &mut self,
        supervisor: SupervisorId,
        member: MemberRecord,
        dynamic: bool,
    ) -> Result<MembershipId, StartError> {
        let supervision = self
            .kernel
            .supervisors
            .get(&supervisor)
            .ok_or(StartError::Stopped)?;
        if supervision.state != SupervisorState::Running {
            return Err(StartError::Stopped);
        }
        let members = supervision.declared.len() + supervision.dynamic.len();
        if members >= supervision.policy.capacity.get() as usize {
            return Err(StartError::CapacityReached);
        }
        if let MemberKind::Process(spec) = &member.kind {
            self.check_name(spec)?;
        }
        self.kernel.next_membership += 1;
        let membership = MembershipId(self.kernel.next_membership);
        self.kernel.memberships.insert(
            membership,
            Membership {
                supervisor,
                member,
                current: Current::None,
                last_incarnation: None,
                handle: false,
            },
        );
        let supervision = self
            .kernel
            .supervisors
            .get_mut(&supervisor)
            .expect("checked above");
        if dynamic {
            supervision.dynamic.push(membership);
        } else {
            supervision.declared.push(membership);
        }
        if let Err(error) = self.start_incarnation(membership) {
            self.release_slot(membership);
            self.kernel.memberships.remove(&membership);
            return Err(error);
        }
        Ok(membership)
    }

    fn check_name(&self, spec: &SpecRecord) -> Result<(), StartError> {
        match spec.name {
            Some(name) if self.kernel.names.contains_key(&name) => Err(StartError::NameTaken(name)),
            _ => Ok(()),
        }
    }

    /// Starts a new incarnation of a process, or rebuilds a nested
    /// supervisor from its declared members.
    fn start_incarnation(&mut self, membership: MembershipId) -> Result<(), StartError> {
        let entry = &self.kernel.memberships[&membership];
        let supervisor = entry.supervisor;
        match entry.member.kind.clone() {
            MemberKind::Process(spec) => {
                self.check_name(&spec)?;
                let name = spec.name;
                let incarnation = self.spawn(membership, spec)?;
                let entry = self
                    .kernel
                    .memberships
                    .get_mut(&membership)
                    .expect("membership record");
                entry.current = Current::Process(incarnation);
                entry.last_incarnation = Some(incarnation);
                self.emit_process(ProcessEventKind::Started {
                    supervisor,
                    membership,
                    incarnation,
                    name,
                });
            }
            MemberKind::Supervisor(policy) => {
                let child = self.build_supervisor(policy, Some((supervisor, membership)))?;
                self.kernel
                    .memberships
                    .get_mut(&membership)
                    .expect("membership record")
                    .current = Current::Supervisor(child);
            }
        }
        Ok(())
    }

    /// Creates the fiber and mailbox and registers the name before the body
    /// runs. The body's first run joins the tail of the ready queue.
    fn spawn(
        &mut self,
        membership: MembershipId,
        spec: Rc<SpecRecord>,
    ) -> Result<IncarnationId, StartError> {
        #[cfg(test)]
        if self.kernel.failing_spawns > 0 {
            self.kernel.failing_spawns -= 1;
            return Err(StartError::SpawnFailed);
        }
        let incarnation = IncarnationId(self.kernel.next_incarnation + 1);
        let task = self
            .create_task(None, spec.entry, spec.context, Some(incarnation))
            .map_err(|_| StartError::SpawnFailed)?;
        self.kernel.next_incarnation += 1;
        if let Some(name) = spec.name {
            self.kernel.names.insert(name, incarnation);
        }
        self.kernel.incarnations.insert(
            incarnation,
            Incarnation {
                membership,
                task,
                spec,
                mailbox: VecDeque::new(),
                accepting: true,
                receiving: false,
                stopping: None,
            },
        );
        Ok(incarnation)
    }

    fn resolve(&self, address: Address) -> Option<IncarnationId> {
        match address {
            Address::Named(name) => self.kernel.names.get(&name).copied(),
            Address::Direct(incarnation) => self
                .kernel
                .incarnations
                .contains_key(&incarnation)
                .then_some(incarnation),
        }
    }

    /// Called once the incarnation's task is terminal and its structured
    /// children have drained. Releases routing, discards unread messages,
    /// records the end, and lets the supervisor react to a natural end.
    pub(super) fn incarnation_finished(&mut self, task: TaskId, outcome: TaskOutcome) {
        let incarnation = self
            .tasks
            .remove(&task)
            .and_then(|record| record.process)
            .expect("an incarnation task");
        let record = self
            .kernel
            .incarnations
            .remove(&incarnation)
            .expect("live incarnation");
        if let Some(name) = record.spec.name
            && self.kernel.names.get(&name) == Some(&incarnation)
        {
            self.kernel.names.remove(&name);
        }
        if let Some(timer) = record.stopping.and_then(|stopping| stopping.timer) {
            self.cancel_timer(timer);
        }
        for message in record.mailbox {
            self.release_message(message);
        }
        let membership = record.membership;
        if let Some(entry) = self.kernel.memberships.get_mut(&membership) {
            entry.current = Current::None;
        }
        let cause = terminal_cause(outcome, record.stopping);
        self.emit_process(ProcessEventKind::Terminated {
            membership,
            incarnation,
            cause,
        });
        if record.stopping.is_none() {
            self.member_ended(membership, cause);
        }
        for plan in self.kernel.blocked.remove(&incarnation).unwrap_or_default() {
            self.execute(plan);
        }
    }

    pub(super) fn release_message(&mut self, message: u64) {
        if let Some((release, context)) = self.kernel.release_message {
            // SAFETY: the hook's owner promised it stays valid for the
            // scheduler's lifetime; each accepted handle is released once.
            unsafe { release(context, message) };
        }
    }

    /// Reacts to a member ending on its own: restart a defect under
    /// `OnDefect`, or free the slot and the membership record.
    fn member_ended(&mut self, membership: MembershipId, cause: TerminalCause) {
        let Some(entry) = self.kernel.memberships.get(&membership) else {
            return;
        };
        let supervisor = entry.supervisor;
        let open = self
            .kernel
            .supervisors
            .get(&supervisor)
            .is_some_and(|record| record.state == SupervisorState::Running);
        if cause == TerminalCause::Defected && entry.member.restart == Restart::OnDefect && open {
            self.restart(supervisor, membership);
        } else {
            self.release_slot(membership);
            self.forget(membership, true);
        }
    }

    /// Charges the rolling limit before the replacement starts. A restart
    /// exactly `within` after a counted one is outside the window.
    fn restart(&mut self, supervisor: SupervisorId, membership: MembershipId) {
        let now = self.clock.now();
        let supervision = self
            .kernel
            .supervisors
            .get_mut(&supervisor)
            .expect("open supervisor");
        let restarts = supervision.policy.restarts;
        while supervision
            .restart_times
            .front()
            .is_some_and(|at| at.saturating_add(restarts.within) <= now)
        {
            supervision.restart_times.pop_front();
        }
        let attempts = supervision.restart_times.len() as u32 + 1;
        if attempts > restarts.limit {
            self.escalate(supervisor);
            return;
        }
        supervision.restart_times.push_back(now);
        self.emit_process(ProcessEventKind::RestartAttempted {
            supervisor,
            membership,
            attempts_in_window: attempts,
        });
        if let Err(error) = self.start_incarnation(membership) {
            self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });
            self.member_ended(membership, TerminalCause::Defected);
        }
    }

    /// Stops the supervisor's members in shutdown order, then defects to its
    /// parent. A root escalation terminates the program.
    fn escalate(&mut self, supervisor: SupervisorId) {
        self.emit_process(ProcessEventKind::RestartLimitExceeded { supervisor });
        let parent = self.kernel.supervisors[&supervisor].parent.map(|(p, _)| p);
        self.execute(Plan::from([
            Step::StopMembers(supervisor),
            Step::Emit(ProcessEventKind::Escalated { supervisor, parent }),
            Step::FinishEscalation(supervisor),
        ]));
    }

    /// Ends the program after a root escalation: waiters see the escalation,
    /// every root task defects, and every other root stops.
    fn terminate_program(&mut self) {
        self.kernel.program_terminated = true;
        let roots: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, record)| {
                record.parent.is_none()
                    && record.process.is_none()
                    && matches!(record.phase, TaskPhase::Runnable | TaskPhase::Parked)
            })
            .map(|(task, _)| *task)
            .collect();
        for task in roots {
            self.begin_terminal(
                task,
                TaskOutcome::Defected {
                    origin: task,
                    code: RESTART_LIMIT_DEFECT_CODE,
                },
                false,
            );
        }
        self.shutdown_processes();
    }

    // ----- Plans --------------------------------------------------------

    /// Runs plan steps in order. Returns `Pending` when a step parked the
    /// plan on an incarnation whose cleanup is still running.
    fn execute(&mut self, mut plan: Plan) -> Progress {
        let front = !plan.resumed;
        while let Some(step) = plan.steps.pop_front() {
            match step {
                Step::StopMembers(supervisor) => {
                    self.close_and_unroute(supervisor);
                    let Some(supervision) = self.kernel.supervisors.get_mut(&supervisor) else {
                        continue;
                    };
                    let dynamic = std::mem::take(&mut supervision.dynamic);
                    let declared = std::mem::take(&mut supervision.declared);
                    // Shutdown order is newest dynamic first, then declared
                    // in reverse; push_front reverses, so push oldest first.
                    for membership in declared.into_iter().chain(dynamic) {
                        plan.steps.push_front(Step::StopMember(membership));
                    }
                }
                Step::StopMember(membership) => {
                    let Some(entry) = self.forget(membership, false) else {
                        continue;
                    };
                    match entry.current {
                        Current::Process(incarnation) => plan.steps.push_front(Step::Await(incarnation)),
                        Current::Supervisor(child) => {
                            plan.steps.push_front(Step::DropSupervisor(child));
                            plan.steps.push_front(Step::Emit(ProcessEventKind::Stopped {
                                supervisor: child,
                            }));
                            plan.steps.push_front(Step::StopMembers(child));
                        }
                        Current::None => {}
                    }
                }
                Step::Await(incarnation) => {
                    if !self.await_incarnation(incarnation) {
                        plan.steps.push_front(Step::Await(incarnation));
                        plan.resumed = true;
                        self.kernel
                            .blocked
                            .entry(incarnation)
                            .or_default()
                            .push(plan);
                        return Progress::Pending;
                    }
                }
                Step::Emit(kind) => self.emit_process(kind),
                Step::DropSupervisor(supervisor) => {
                    self.kernel.supervisors.remove(&supervisor);
                }
                Step::FinishStop(supervisor) => {
                    let record = self
                        .kernel
                        .supervisors
                        .remove(&supervisor)
                        .expect("stopping root");
                    self.kernel.roots.retain(|root| *root != supervisor);
                    for waiter in record.waiters {
                        self.reply(waiter, super::RESPONSE_OK, front);
                    }
                }
                Step::FinishEscalation(supervisor) => self.finish_escalation(supervisor),
                Step::FinishRemoval {
                    membership,
                    requester,
                } => {
                    if self.kernel.memberships.contains_key(&membership) {
                        self.release_slot(membership);
                        self.kernel.memberships.remove(&membership);
                    }
                    self.kernel.retired.remove(&membership);
                    self.emit_process(ProcessEventKind::Removed { membership });
                    if let Some(requester) = requester {
                        self.reply(requester, super::RESPONSE_OK, front);
                    }
                }
                Step::Reply(requester) => {
                    if let Some(requester) = requester {
                        self.reply(requester, super::RESPONSE_OK, front);
                    }
                }
            }
        }
        Progress::Done
    }

    fn finish_escalation(&mut self, supervisor: SupervisorId) {
        let record = self
            .kernel
            .supervisors
            .get_mut(&supervisor)
            .expect("escalating supervisor");
        match record.parent {
            None => {
                record.state = SupervisorState::Escalated;
                let waiters = std::mem::take(&mut record.waiters);
                for waiter in waiters {
                    self.reply(waiter, super::RESPONSE_DEFECTED, false);
                }
                self.terminate_program();
            }
            Some((_, membership)) => {
                self.kernel.supervisors.remove(&supervisor);
                if let Some(entry) = self.kernel.memberships.get_mut(&membership) {
                    entry.current = Current::None;
                }
                self.member_ended(membership, TerminalCause::Defected);
            }
        }
    }

    /// Closes admission and restarts, and unroutes every incarnation in the
    /// subtree before any of them is cancelled.
    fn close_and_unroute(&mut self, supervisor: SupervisorId) {
        let Some(supervision) = self.kernel.supervisors.get_mut(&supervisor) else {
            return;
        };
        if supervision.state == SupervisorState::Running {
            supervision.state = SupervisorState::Stopping;
        }
        let members: Vec<_> = supervision
            .dynamic
            .iter()
            .chain(&supervision.declared)
            .copied()
            .collect();
        for membership in members {
            match self.kernel.memberships.get(&membership).map(|e| e.current) {
                Some(Current::Process(incarnation)) => self.unroute(incarnation),
                Some(Current::Supervisor(child)) => self.close_and_unroute(child),
                _ => {}
            }
        }
    }

    fn unroute(&mut self, incarnation: IncarnationId) {
        let Some(record) = self.kernel.incarnations.get_mut(&incarnation) else {
            return;
        };
        record.accepting = false;
        if let Some(name) = record.spec.name
            && self.kernel.names.get(&name) == Some(&incarnation)
        {
            self.kernel.names.remove(&name);
        }
    }

    /// Shuts the incarnation down (once) and runs its cleanup inline until
    /// it terminates or parks on something that has not completed. Returns
    /// true once it has terminated.
    fn await_incarnation(&mut self, incarnation: IncarnationId) -> bool {
        self.begin_shutdown(incarnation);
        loop {
            let Some(record) = self.kernel.incarnations.get(&incarnation) else {
                return true;
            };
            let task = record.task;
            if self.tasks[&task].phase != TaskPhase::Runnable {
                break;
            }
            if self.runnable.front() == Some(&task) {
                self.runnable.pop_front();
            } else {
                self.runnable.retain(|queued| *queued != task);
            }
            self.run(task);
        }
        let now = self.clock.now();
        let record = self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .expect("lingering incarnation");
        let stopping = record.stopping.as_mut().expect("stopping incarnation");
        if stopping.timer.is_none() {
            let deadline = stopping.deadline.max(now);
            let timer = self
                .timers
                .insert(deadline, TimerTarget::ShutdownDeadline(incarnation));
            self.kernel
                .incarnations
                .get_mut(&incarnation)
                .and_then(|record| record.stopping.as_mut())
                .expect("stopping incarnation")
                .timer = Some(timer);
            self.emit_timer(super::ClockEventKind::TimerStarted, None, deadline);
        }
        false
    }

    /// Unroutes, sets the deadline, and arranges how the body learns of the
    /// cancellation: a fresh body never runs; a receive is reissued so it
    /// can drain; any other park is interrupted. A zero deadline forces at
    /// once.
    fn begin_shutdown(&mut self, incarnation: IncarnationId) {
        let now = self.clock.now();
        let Some(record) = self.kernel.incarnations.get(&incarnation) else {
            return;
        };
        if record.stopping.is_some() {
            return;
        }
        let deadline = now.saturating_add(record.spec.shutdown_within);
        let forced = deadline <= now;
        let task = record.task;
        self.unroute(incarnation);
        let record = self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .expect("live incarnation");
        let receiving = std::mem::take(&mut record.receiving);
        let mut stopping = Stopping {
            deadline,
            timer: None,
            told: false,
            forced,
        };
        let timer = self.tasks.get_mut(&task).and_then(|t| t.timer.take());
        if let Some(timer) = timer {
            self.cancel_timer(timer);
        }
        let task_record = self.tasks.get_mut(&task).expect("incarnation task");
        match task_record.phase {
            TaskPhase::Runnable if !task_record.ran || forced => {
                task_record.command = ChimiFiberCommand::Cancel;
                set_status(task_record.request, RESPONSE_FORCED, forced);
                stopping.told = true;
            }
            TaskPhase::Runnable => {}
            TaskPhase::Parked => {
                if forced {
                    task_record.command = ChimiFiberCommand::Cancel;
                    set_status(task_record.request, RESPONSE_FORCED, true);
                } else if !receiving {
                    task_record.command = ChimiFiberCommand::Cancel;
                    set_status(task_record.request, super::RESPONSE_CANCELLED, true);
                    stopping.told = true;
                }
                task_record.phase = TaskPhase::Runnable;
                self.runnable.push_front(task);
            }
            TaskPhase::Draining(_) | TaskPhase::Terminal(_) => {}
        }
        self.kernel
            .incarnations
            .get_mut(&incarnation)
            .expect("live incarnation")
            .stopping = Some(stopping);
    }

    /// The shutdown deadline passed: answer the pending call with a forced
    /// reply and run the body to its end.
    pub(super) fn force(&mut self, incarnation: IncarnationId) {
        let Some(record) = self.kernel.incarnations.get_mut(&incarnation) else {
            return;
        };
        let task = record.task;
        let stopping = record.stopping.as_mut().expect("only stopping incarnations are forced");
        stopping.forced = true;
        stopping.timer = None;
        let task_record = self.tasks.get_mut(&task).expect("incarnation task");
        task_record.command = ChimiFiberCommand::Cancel;
        set_status(task_record.request, RESPONSE_FORCED, true);
        if task_record.phase == TaskPhase::Parked {
            task_record.phase = TaskPhase::Runnable;
            self.runnable.push_front(task);
        }
        let terminated = self.await_incarnation(incarnation);
        debug_assert!(terminated, "a forced incarnation always terminates");
    }

    /// A forced body parked again. Its fiber cannot be destroyed while
    /// parked (destruction would unwind through C and abort), so it is
    /// abandoned and the incarnation terminates as forced.
    pub(super) fn abandon_forced(&mut self, task: TaskId) {
        let record = self.tasks.get_mut(&task).expect("forced incarnation");
        std::mem::forget(record.fiber.take());
        record.request = std::ptr::null_mut();
        self.begin_terminal(task, TaskOutcome::Cancelled, true);
    }

    /// Drops a membership record, keeping only what a live handle needs.
    fn forget(&mut self, membership: MembershipId, announce: bool) -> Option<Membership> {
        let entry = self.kernel.memberships.remove(&membership)?;
        if entry.handle
            && let Some(incarnation) = entry.last_incarnation
        {
            self.kernel.retired.insert(
                membership,
                Retired {
                    incarnation,
                    announce,
                },
            );
        }
        Some(entry)
    }

    fn release_slot(&mut self, membership: MembershipId) {
        let supervisor = self.kernel.memberships[&membership].supervisor;
        if let Some(supervision) = self.kernel.supervisors.get_mut(&supervisor) {
            supervision.declared.retain(|m| *m != membership);
            supervision.dynamic.retain(|m| *m != membership);
        }
    }

    /// Writes `status` into the waiter's request and wakes it, unless it
    /// has since been resumed, cancelled, or terminated.
    pub(super) fn reply(&mut self, waiter: Waiter, status: u32, front: bool) -> bool {
        if !self.still_waiting(waiter) {
            return false;
        }
        let request = self.tasks[&waiter.task].request;
        set_status(request, status, true);
        let _ = if front {
            self.wake_front(waiter.task)
        } else {
            self.wake(waiter.task)
        };
        true
    }

    pub(super) fn still_waiting(&self, waiter: Waiter) -> bool {
        self.tasks.get(&waiter.task).is_some_and(|record| {
            record.phase == TaskPhase::Parked
                && record.park == waiter.park
                && !record.request.is_null()
        })
    }

    pub(super) fn emit_process(&mut self, kind: ProcessEventKind) {
        if self.trace_mode == TraceMode::Off
            || matches!(kind, ProcessEventKind::Sent { .. }) && self.trace_mode != TraceMode::Full
        {
            return;
        }
        self.constructed_events += 1;
        let event = ProcessEvent {
            sequence: self.constructed_events,
            time: self.clock.now(),
            kind,
        };
        self.kernel.events.push(event);
        let mut trace = SchedulerTraceEvent::blank(
            SCHEDULER_TRACE_PROCESS,
            kind.trace_kind(),
            event.sequence,
        );
        trace.time = event.time.as_nanos();
        match kind {
            ProcessEventKind::Started {
                supervisor,
                membership,
                incarnation,
                name,
            } => {
                trace.supervisor = supervisor.0;
                trace.membership = membership.0;
                trace.incarnation = incarnation.0;
                trace.name = name.map_or(0, NameId::get);
            }
            ProcessEventKind::AdmissionRejected { supervisor, error } => {
                trace.supervisor = supervisor.0;
                trace.cause = match error {
                    StartError::CapacityReached => 1,
                    StartError::NameTaken(_) => 2,
                    StartError::Stopped => 3,
                    StartError::SpawnFailed => 4,
                };
                if let StartError::NameTaken(name) = error {
                    trace.name = name.get();
                }
            }
            ProcessEventKind::Terminated {
                membership,
                incarnation,
                cause,
            } => {
                trace.membership = membership.0;
                trace.incarnation = incarnation.0;
                trace.cause = cause.code();
            }
            ProcessEventKind::RestartAttempted {
                supervisor,
                membership,
                attempts_in_window,
            } => {
                trace.supervisor = supervisor.0;
                trace.membership = membership.0;
                trace.attempts_in_window = u64::from(attempts_in_window);
            }
            ProcessEventKind::RestartLimitExceeded { supervisor }
            | ProcessEventKind::Stopped { supervisor } => trace.supervisor = supervisor.0,
            ProcessEventKind::Escalated { supervisor, parent } => {
                trace.supervisor = supervisor.0;
                trace.parent_supervisor = parent.map_or(0, |p| p.0);
            }
            ProcessEventKind::Removed { membership } => trace.membership = membership.0,
            ProcessEventKind::Sent {
                address,
                incarnation,
                outcome,
            } => {
                match address {
                    Address::Named(name) => {
                        trace.address_kind = 1;
                        trace.name = name.get();
                    }
                    Address::Direct(_) => trace.address_kind = 2,
                }
                trace.incarnation = incarnation.map_or(0, |i| i.0);
                trace.outcome = match outcome {
                    SendOutcome::Accepted => 1,
                    SendOutcome::Full => 2,
                    SendOutcome::Unavailable => 3,
                };
            }
        }
        self.publish_trace(trace);
    }

    /// Tears down every process synchronously, forcing cleanup that has not
    /// finished. Used when the scheduler is dropped.
    pub(super) fn tear_down_processes(&mut self) {
        self.shutdown_processes();
        while let Some(incarnation) = self.kernel.blocked.keys().next().copied() {
            if self.kernel.incarnations.contains_key(&incarnation) {
                self.force(incarnation);
            } else {
                let plans = self.kernel.blocked.remove(&incarnation).unwrap_or_default();
                for plan in plans {
                    self.execute(plan);
                }
            }
        }
        let leftover: Vec<_> = self.kernel.incarnations.keys().copied().collect();
        for incarnation in leftover {
            if self.kernel.incarnations.contains_key(&incarnation) {
                self.begin_shutdown(incarnation);
                if !self.await_incarnation(incarnation) {
                    self.force(incarnation);
                }
            }
        }
    }
}

impl TerminalCause {
    pub(super) fn code(self) -> u64 {
        match self {
            Self::Returned => 1,
            Self::Failed => 2,
            Self::Defected => 3,
            Self::Cancelled => 4,
            Self::Forced => 5,
        }
    }
}

/// Classifies an incarnation's end. A forced end wins; a defect during
/// cleanup stays a defect; any other end during cleanup is `Cancelled`.
fn terminal_cause(outcome: TaskOutcome, stopping: Option<Stopping>) -> TerminalCause {
    let natural = match outcome {
        TaskOutcome::Succeeded(PROCESS_END_RETURNED) => TerminalCause::Returned,
        TaskOutcome::Succeeded(PROCESS_END_FAILED) => TerminalCause::Failed,
        TaskOutcome::Cancelled => TerminalCause::Cancelled,
        TaskOutcome::Succeeded(_)
        | TaskOutcome::Defected { .. }
        | TaskOutcome::StackOverflow { .. } => TerminalCause::Defected,
    };
    match stopping {
        Some(stopping) if stopping.forced => TerminalCause::Forced,
        Some(_) if natural == TerminalCause::Defected => TerminalCause::Defected,
        Some(_) => TerminalCause::Cancelled,
        None => natural,
    }
}

/// Request kinds that may leave the caller parked.
fn can_park(kind: u32) -> bool {
    matches!(
        kind,
        super::REQUEST_TASK_RESULT
            | super::REQUEST_CHANNEL_SEND
            | super::REQUEST_CHANNEL_RECEIVE
            | super::REQUEST_TIMER_SLEEP_UNTIL
            | super::REQUEST_SUPERVISOR_REMOVE_PROCESS
            | super::REQUEST_SUPERVISOR_STOP
            | super::REQUEST_SUPERVISOR_WAIT
    )
}

/// Writes a status into a parked request's shared header.
fn set_status(request: *mut c_void, status: u32, when: bool) {
    if when && !request.is_null() {
        // SAFETY: a non-null request pointer is recorded only while its
        // fiber is parked, and every request starts with `RequestHeader`.
        unsafe { (*request.cast::<RequestHeader>()).status = status };
    }
}
