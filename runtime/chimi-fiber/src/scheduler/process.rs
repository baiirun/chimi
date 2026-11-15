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
//!   its shutdown deadline timer guarantees the plan resumes. A plan that
//!   reaches a nested supervisor another plan is already stopping parks on
//!   that supervisor until it retires, so a subtree is stopped once.
//! - A task stays draining until every root supervisor it started has
//!   retired, so its terminal event follows its members' terminal events.
//! - Bookkeeping follows the slot: a membership record lives while its slot
//!   does, and a handle whose record is gone keeps only its last instance
//!   until `remove_process` or `remove_supervisor` consumes it. Retired
//!   supervisor records are dropped; an identity that was issued but has no
//!   record is a stopped (or replaced) supervisor.

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

mod monitor;

#[cfg(test)]
pub(super) use monitor::Down;
pub use monitor::{MonitorId, Notice};

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
    /// `task` is the scheduler task that runs the incarnation and parents
    /// its child tasks.
    Started {
        supervisor: SupervisorId,
        membership: MembershipId,
        incarnation: IncarnationId,
        task: TaskId,
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
        /// The body overflowed its fiber stack: a defect the host cannot see,
        /// because the body never returns from its entry.
        stack_overflow: bool,
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
    /// Emitted before the supervisor's declared members start. A root has
    /// no parent and no membership.
    SupervisorStarted {
        supervisor: SupervisorId,
        parent: Option<SupervisorId>,
        membership: Option<MembershipId>,
    },
}

impl ProcessEventKind {
    pub(super) fn trace_kind(self) -> u32 {
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
            Self::SupervisorStarted { .. } => 10,
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
    NotAProcess(TaskId),
    /// A process handle used as a supervisor handle, or the reverse.
    WrongMemberKind(MembershipId),
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
    pub monitors: usize,
    pub blocked_plans: usize,
    pub timers: usize,
    pub process_events: usize,
}

pub type MessageRelease = unsafe extern "C" fn(*mut c_void, u64);
pub type SpecContextRelease = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// How one incarnation ended. The kernel builds it for every end, whatever
/// the trace mode, so a reader that is not the trace (the runner's root
/// outcome today, monitors later) learns why a process went down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ChimiProcessTermination {
    pub membership: u64,
    pub incarnation: u64,
    /// `TerminalCause::code`, as the `process_terminated` trace event carries it.
    pub cause: u64,
    /// 1 when the body overflowed its fiber stack: a defect the host cannot
    /// see, because the body never returns from its entry.
    pub stack_overflow: u32,
}

/// Receives the termination record of every incarnation that defected, as
/// the incarnation finishes and before its `process_terminated` event. It
/// runs on the scheduler's thread with no task running, and must not call
/// back into the scheduler.
pub type ProcessDefectReport = unsafe extern "C" fn(*mut c_void, *const ChimiProcessTermination);

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

/// What runs for a membership: a process incarnation or a nested supervisor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Instance {
    Process(IncarnationId),
    Supervisor(SupervisorId),
}

struct Membership {
    supervisor: SupervisorId,
    member: MemberRecord,
    current: Option<Instance>,
    last: Option<Instance>,
    /// A `ManagedProcess` or `ManagedSupervisor` handle refers to this
    /// membership.
    handle: bool,
}

/// What a handle keeps once its membership record is gone.
#[derive(Clone, Copy, Debug)]
struct Retired {
    last: Instance,
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
    /// The body was running when shutdown began and has not parked since:
    /// its next park that would block, of any kind, is cancelled. A receive
    /// ends this and drains the mailbox instead.
    pending: bool,
    /// A child task defected during shutdown; the end is a defect.
    child_defected: bool,
    forced: bool,
}

pub(super) struct Incarnation {
    membership: MembershipId,
    task: TaskId,
    spec: Rc<SpecRecord>,
    mailbox: VecDeque<Entry>,
    /// Monitors other incarnations set on this one.
    watched_by: Vec<MonitorId>,
    /// Monitors this incarnation set.
    watching: Vec<MonitorId>,
    /// Notices in `mailbox`; the rest are messages, which the capacity bounds.
    notices: usize,
    /// False once shutdown begins or the body ends; sends then fail.
    accepting: bool,
    /// Parked in receive on an empty mailbox.
    receiving: bool,
    stopping: Option<Stopping>,
}

/// One accepted entry of a mailbox, in arrival order.
#[derive(Clone, Copy, Debug)]
enum Entry {
    Message(u64),
    /// Not counted against the capacity.
    Notice(Notice),
}

impl Entry {
    fn handle(self) -> u64 {
        match self {
            Self::Message(handle) => handle,
            Self::Notice(notice) => notice.handle,
        }
    }
}

struct Supervision {
    policy: Rc<PolicyRecord>,
    parent: Option<(SupervisorId, MembershipId)>,
    /// The task that started this root; the root stops when it ends.
    owner: Option<TaskId>,
    declared: Vec<MembershipId>,
    dynamic: Vec<MembershipId>,
    restart_times: VecDeque<Instant>,
    state: SupervisorState,
    /// A plan has taken the members to stop them. Another plan that reaches
    /// this supervisor waits for it to retire instead of stopping it again.
    members_taken: bool,
    /// A stop or removal reached this nested supervisor while an escalation
    /// was stopping it. The escalation then ends its membership as a
    /// requested end, which never restarts.
    end_requested: bool,
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
    /// Waits for a supervisor another plan is stopping to retire.
    AwaitSupervisor(SupervisorId),
    Emit(ProcessEventKind),
    /// Drops a stopped supervisor's record, answers its waiters, ends its
    /// membership in its parent, and lets a draining owner finish.
    Retire(SupervisorId),
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

/// What a parked plan waits for.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Blocker {
    Incarnation(IncarnationId),
    Supervisor(SupervisorId),
}

/// Which kind of managed handle an operation takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HandleKind {
    Process,
    Supervisor,
}

#[derive(Default)]
pub(super) struct ProcessKernel {
    next_supervisor: u64,
    next_membership: u64,
    next_incarnation: u64,
    next_monitor: u64,
    monitors: BTreeMap<MonitorId, monitor::MonitorRecord>,
    supervisors: BTreeMap<SupervisorId, Supervision>,
    memberships: BTreeMap<MembershipId, Membership>,
    retired: BTreeMap<MembershipId, Retired>,
    pub(super) incarnations: BTreeMap<IncarnationId, Incarnation>,
    names: BTreeMap<NameId, IncarnationId>,
    roots: Vec<SupervisorId>,
    blocked: BTreeMap<Blocker, Vec<Plan>>,
    program_terminated: bool,
    pub(super) events: Vec<ProcessEvent>,
    pub(super) release_message: Option<(MessageRelease, *mut c_void)>,
    pub(super) release_spec: Option<(SpecContextRelease, *mut c_void)>,
    pub(super) defect_report: Option<(ProcessDefectReport, *mut c_void)>,
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
    Notice(Notice),
    Pending,
    Cancelled,
    Forced,
}

impl Scheduler {
    /// Installs the hook that receives every defected incarnation's
    /// termination record, under any trace mode.
    pub fn set_process_defect_report(
        &mut self,
        report: Option<ProcessDefectReport>,
        context: *mut c_void,
    ) {
        self.kernel.defect_report = report.map(|report| (report, context));
    }

    // ----- Host-facing operations -----------------------------------------

    /// Starts a root supervisor and its declared members in list order.
    ///
    /// Ensures: every declared member has started and is routable, or every
    /// member already started is stopped again and the first failure is
    /// returned. No body runs before this returns.
    pub fn start_supervisor(&mut self, policy: Policy) -> Result<SupervisorId, StartError> {
        self.start_supervisor_for(None, policy)
    }

    /// Starts a root owned by `owner`: it is stopped when that task ends.
    /// A host-started root (`None`) lives until it is stopped or the
    /// scheduler shuts processes down.
    pub(super) fn start_supervisor_for(
        &mut self,
        owner: Option<TaskId>,
        policy: Policy,
    ) -> Result<SupervisorId, StartError> {
        let policy = self.adopt_policy(policy);
        let supervisor = self.build_supervisor(policy, None)?;
        self.kernel
            .supervisors
            .get_mut(&supervisor)
            .expect("just built")
            .owner = owner;
        self.kernel.roots.push(supervisor);
        Ok(supervisor)
    }

    /// Stops every running root that `task` started, newest first. Called
    /// once the task is draining; each root that retires lets the task
    /// finish once none is left (`owns_live_roots`).
    pub(super) fn stop_owned_supervisors(&mut self, task: TaskId) {
        let owned: Vec<_> = self
            .kernel
            .roots
            .iter()
            .rev()
            .copied()
            .filter(|root| {
                self.kernel
                    .supervisors
                    .get(root)
                    .is_some_and(|record| record.owner == Some(task))
            })
            .collect();
        for root in owned {
            let _ = self.stop_for(None, root);
        }
    }

    /// True while a root that `task` started is running or stopping. An
    /// escalated root terminates the program instead and does not count.
    pub(super) fn owns_live_roots(&self, task: TaskId) -> bool {
        self.kernel.roots.iter().any(|root| {
            self.kernel.supervisors.get(root).is_some_and(|record| {
                record.owner == Some(task)
                    && matches!(
                        record.state,
                        SupervisorState::Running | SupervisorState::Stopping
                    )
            })
        })
    }

    /// Admits a dynamic nested supervisor into `parent` and starts its
    /// declared members. It takes one of the parent's slots. Fail-fast like
    /// `start_process`; a member that cannot start rolls the nested
    /// supervisor back and its error is returned. The handle is the
    /// membership; `nested_supervisor` names its current supervisor.
    pub fn start_nested_supervisor(
        &mut self,
        parent: SupervisorId,
        policy: Policy,
        restart: Restart,
    ) -> Result<MembershipId, StartError> {
        let member = MemberRecord {
            kind: MemberKind::Supervisor(self.adopt_policy(policy)),
            restart,
        };
        self.admit_handle(parent, member)
    }

    /// The supervisor currently running for a nested-supervisor membership.
    /// It changes when the parent replaces the supervisor after a defect.
    pub fn nested_supervisor(
        &self,
        membership: MembershipId,
    ) -> Result<SupervisorId, ProcessError> {
        match self.last_instance(membership) {
            Some(Instance::Supervisor(supervisor)) => Ok(supervisor),
            Some(Instance::Process(_)) => Err(ProcessError::WrongMemberKind(membership)),
            None => Err(ProcessError::UnknownMembership(membership)),
        }
    }

    /// Consumes a dynamic nested-supervisor membership. Ensures, once
    /// `Done`: its whole subtree has stopped and no replacement can start.
    /// A membership that already ended is removed without stopping anything.
    pub fn remove_supervisor(
        &mut self,
        supervisor: SupervisorId,
        membership: MembershipId,
    ) -> Result<Progress, ProcessError> {
        self.remove_for(None, supervisor, membership, HandleKind::Supervisor)
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
        self.admit_handle(supervisor, member)
    }

    /// Admits a dynamic member that a managed handle will refer to.
    fn admit_handle(
        &mut self,
        supervisor: SupervisorId,
        member: MemberRecord,
    ) -> Result<MembershipId, StartError> {
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
        match self.last_instance(membership) {
            Some(Instance::Process(incarnation)) => Ok(Address::Direct(incarnation)),
            Some(Instance::Supervisor(_)) => Err(ProcessError::WrongMemberKind(membership)),
            None => Err(ProcessError::UnknownMembership(membership)),
        }
    }

    /// The membership's current or last instance, from its record or, once
    /// that is gone, from what its handle kept.
    fn last_instance(&self, membership: MembershipId) -> Option<Instance> {
        self.kernel
            .memberships
            .get(&membership)
            .and_then(|entry| entry.last)
            .or_else(|| {
                self.kernel
                    .retired
                    .get(&membership)
                    .map(|retired| retired.last)
            })
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
                let messages = incarnation.mailbox.len() - incarnation.notices;
                if messages >= incarnation.spec.mailbox_capacity.get() as usize {
                    (SendOutcome::Full, None)
                } else {
                    incarnation.mailbox.push_back(Entry::Message(message));
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
        self.remove_for(None, supervisor, membership, HandleKind::Process)
    }

    /// Stops a supervisor's subtree: routing is removed for the whole
    /// subtree first, then members stop newest dynamic first and declared in
    /// reverse. A stopped supervisor is a no-op with no event. A nested
    /// supervisor that is stopped ends its membership without a restart.
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
            monitors: kernel.monitors.len(),
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

    /// Consumes a managed handle of `kind`. The plan waits for the member's
    /// incarnation or whole subtree to stop before the slot is released.
    pub(super) fn remove_for(
        &mut self,
        requester: Option<Waiter>,
        supervisor: SupervisorId,
        membership: MembershipId,
        kind: HandleKind,
    ) -> Result<Progress, ProcessError> {
        if let Some(retired) = self.kernel.retired.get(&membership).copied() {
            // If a stop is still ending the last instance, wait for it.
            let await_last = match (kind, retired.last) {
                (HandleKind::Process, Instance::Process(incarnation)) => Step::Await(incarnation),
                (HandleKind::Supervisor, Instance::Supervisor(child)) => {
                    Step::AwaitSupervisor(child)
                }
                _ => return Err(ProcessError::WrongMemberKind(membership)),
            };
            self.kernel.retired.remove(&membership);
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
        let kind_matches = match entry.member.kind {
            MemberKind::Process(_) => kind == HandleKind::Process,
            MemberKind::Supervisor(_) => kind == HandleKind::Supervisor,
        };
        if !kind_matches {
            return Err(ProcessError::WrongMemberKind(membership));
        }
        let finish = Step::FinishRemoval {
            membership,
            requester,
        };
        let plan = match entry.current {
            Some(Instance::Process(incarnation)) => Plan::from([Step::Await(incarnation), finish]),
            Some(Instance::Supervisor(child)) if self.stop_in_flight(child) => {
                self.request_end(child);
                Plan::from([Step::AwaitSupervisor(child), finish])
            }
            Some(Instance::Supervisor(child)) => Plan::from([
                Step::StopMembers(child),
                Step::Emit(ProcessEventKind::Stopped { supervisor: child }),
                Step::Retire(child),
                finish,
            ]),
            None => Plan::from([finish]),
        };
        Ok(self.execute(plan))
    }

    /// True when a plan has already taken the supervisor's members to stop
    /// them.
    fn stop_in_flight(&self, supervisor: SupervisorId) -> bool {
        self.kernel
            .supervisors
            .get(&supervisor)
            .is_some_and(|record| record.members_taken)
    }

    /// Marks a supervisor that is already stopping so that, if an
    /// escalation is what stops it, its membership is not restarted.
    fn request_end(&mut self, supervisor: SupervisorId) {
        if let Some(record) = self.kernel.supervisors.get_mut(&supervisor) {
            record.end_requested = true;
        }
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
        match state {
            SupervisorState::Stopped | SupervisorState::Escalated => {
                if let Some(requester) = requester {
                    self.reply(requester, super::RESPONSE_OK, true);
                }
                Ok(Progress::Done)
            }
            SupervisorState::Stopping => {
                // Another stop or an escalation is in flight; finish with it.
                let record = record.expect("a stopping supervisor has a record");
                record.end_requested = true;
                record.waiters.extend(requester);
                Ok(Progress::Pending)
            }
            SupervisorState::Running => {
                let record = record.expect("a running supervisor has a record");
                record.waiters.extend(requester);
                let plan = Plan::from([
                    Step::StopMembers(supervisor),
                    Step::Emit(ProcessEventKind::Stopped { supervisor }),
                    Step::Retire(supervisor),
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
            Some(stopping) => {
                stopping.pending = false;
                match record.mailbox.pop_front() {
                    Some(entry) => monitor::receipt(&mut record.notices, entry),
                    None => {
                        stopping.told = true;
                        Receipt::Cancelled
                    }
                }
            }
            None => match record.mailbox.pop_front() {
                Some(entry) => monitor::receipt(&mut record.notices, entry),
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

    /// Delivers shutdown to a request that left its caller blocked on park
    /// number `park` (`kind` 0 is a bare park). Before the body is told, a
    /// blocking sleep, task result, or channel operation is cancelled, and
    /// so is any blocking park while the cancellation is still pending. A
    /// request that completed is answered, so a value it took is not lost.
    /// Supervisor operations from a draining handler are served.
    pub(super) fn interrupt_blocked_park(&mut self, task: TaskId, park: u64, kind: u32) {
        let blocked = self
            .tasks
            .get(&task)
            .is_some_and(|record| record.phase == TaskPhase::Parked && record.park == park);
        let Some(incarnation) = self.incarnation_of(task).filter(|_| blocked) else {
            return;
        };
        let Some(stopping) = self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .and_then(|record| record.stopping.as_mut())
            .filter(|stopping| !stopping.told)
        else {
            return;
        };
        let interrupts = stopping.pending || interrupted_while_draining(kind);
        if !interrupts {
            return;
        }
        stopping.told = true;
        stopping.pending = false;
        self.disarm_wake_sources(task);
        let record = self.tasks.get_mut(&task).expect("blocked task");
        record.command = ChimiFiberCommand::Cancel;
        set_status(record.request, super::RESPONSE_CANCELLED, true);
        let _ = self.wake_front(task);
    }

    /// Removes everything that could still wake a parked task out of band:
    /// its sleep timer, the child it awaits, its pending channel operation,
    /// and its receive. Replies to its supervisor requests are already
    /// matched to the park number.
    pub(super) fn disarm_wake_sources(&mut self, task: TaskId) {
        let record = self.tasks.get_mut(&task).expect("parked task");
        record.waiting_on = None;
        if let Some(timer) = record.timer.take() {
            self.cancel_timer(timer);
        }
        let transition = self.channels.cancel_pending(task);
        self.apply_channel_transition(transition);
        if let Some(record) = self
            .incarnation_of(task)
            .and_then(|incarnation| self.kernel.incarnations.get_mut(&incarnation))
        {
            record.receiving = false;
        }
    }

    /// True when `parent` is an incarnation under shutdown, which records a
    /// child's defect in its end instead of being torn down outside the
    /// shutdown protocol.
    pub(super) fn absorbs_child_defect(&mut self, parent: TaskId) -> bool {
        let Some(incarnation) = self.incarnation_of(parent) else {
            return false;
        };
        match self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .and_then(|record| record.stopping.as_mut())
        {
            Some(stopping) => {
                stopping.child_defected = true;
                true
            }
            None => false,
        }
    }

    /// True when the task is a forced incarnation, which must not park again.
    pub(super) fn is_forced(&self, task: TaskId) -> bool {
        self.stopping(task).is_some_and(|stopping| stopping.forced)
    }

    /// True when the task is an incarnation under shutdown. A cancellation
    /// scope never fires into it: shutdown's own cancellation wins.
    pub(super) fn is_stopping(&self, task: TaskId) -> bool {
        self.stopping(task).is_some()
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
                owner: None,
                declared: Vec::new(),
                dynamic: Vec::new(),
                restart_times: VecDeque::new(),
                state: SupervisorState::Running,
                members_taken: false,
                end_requested: false,
                waiters: Vec::new(),
            },
        );
        self.emit_process(ProcessEventKind::SupervisorStarted {
            supervisor,
            parent: parent.map(|(parent, _)| parent),
            membership: parent.map(|(_, membership)| membership),
        });
        for member in policy.members.iter().cloned() {
            if let Err(error) = self.admit(supervisor, member, false) {
                self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });
                // Every started member is still fresh, so the rollback never
                // waits for a deadline. The membership that would have held
                // a nested supervisor is released by `admit`.
                let rolled_back = self.execute(Plan::from([
                    Step::StopMembers(supervisor),
                    Step::Emit(ProcessEventKind::Stopped { supervisor }),
                ]));
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
                current: None,
                last: None,
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
                let task = self.kernel.incarnations[&incarnation].task;
                let entry = self
                    .kernel
                    .memberships
                    .get_mut(&membership)
                    .expect("membership record");
                entry.current = Some(Instance::Process(incarnation));
                entry.last = entry.current;
                self.emit_process(ProcessEventKind::Started {
                    supervisor,
                    membership,
                    incarnation,
                    task,
                    name,
                });
            }
            MemberKind::Supervisor(policy) => {
                let child = self.build_supervisor(policy, Some((supervisor, membership)))?;
                let entry = self
                    .kernel
                    .memberships
                    .get_mut(&membership)
                    .expect("membership record");
                entry.current = Some(Instance::Supervisor(child));
                entry.last = entry.current;
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
                watched_by: Vec::new(),
                watching: Vec::new(),
                notices: 0,
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
        for entry in record.mailbox {
            self.release_message(entry.handle());
        }
        let membership = record.membership;
        if let Some(entry) = self.kernel.memberships.get_mut(&membership) {
            entry.current = None;
        }
        let cause = terminal_cause(outcome, record.stopping);
        self.end_monitors(record.watched_by, record.watching, cause);
        let stack_overflow = matches!(outcome, TaskOutcome::StackOverflow { .. });
        if cause == TerminalCause::Defected
            && let Some((report, context)) = self.kernel.defect_report
        {
            let termination = ChimiProcessTermination {
                membership: membership.0,
                incarnation: incarnation.0,
                cause: cause.code(),
                stack_overflow: u32::from(stack_overflow),
            };
            // SAFETY: the hook's owner keeps it and its context valid for the
            // scheduler's lifetime; the record lives for the call.
            unsafe { report(context, &termination) };
        }
        self.emit_process(ProcessEventKind::Terminated {
            membership,
            incarnation,
            cause,
            stack_overflow,
        });
        if record.stopping.is_none() {
            self.member_ended(membership, cause);
        }
        self.resume_blocked(Blocker::Incarnation(incarnation));
    }

    fn resume_blocked(&mut self, blocker: Blocker) {
        for plan in self.kernel.blocked.remove(&blocker).unwrap_or_default() {
            self.execute(plan);
        }
    }

    /// Releases a specification context that was handed over in a request
    /// the scheduler answered without adopting it.
    pub(super) fn release_spec_context(&self, context: *mut c_void) {
        if let Some((release, hook_context)) = self.kernel.release_spec {
            // SAFETY: the hook's owner promised it stays valid for the
            // scheduler's lifetime; an unadopted context is released once.
            unsafe { release(hook_context, context) };
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
                    supervision.members_taken = true;
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
                        Some(Instance::Process(incarnation)) => {
                            plan.steps.push_front(Step::Await(incarnation));
                        }
                        Some(Instance::Supervisor(child)) if self.stop_in_flight(child) => {
                            plan.steps.push_front(Step::AwaitSupervisor(child));
                        }
                        Some(Instance::Supervisor(child)) => {
                            plan.steps.push_front(Step::Retire(child));
                            plan.steps.push_front(Step::Emit(ProcessEventKind::Stopped {
                                supervisor: child,
                            }));
                            plan.steps.push_front(Step::StopMembers(child));
                        }
                        None => {}
                    }
                }
                Step::Await(incarnation) => {
                    if !self.await_incarnation(incarnation) {
                        plan.steps.push_front(Step::Await(incarnation));
                        return self.block(Blocker::Incarnation(incarnation), plan);
                    }
                }
                Step::AwaitSupervisor(supervisor) => {
                    if self.kernel.supervisors.contains_key(&supervisor) {
                        plan.steps.push_front(Step::AwaitSupervisor(supervisor));
                        return self.block(Blocker::Supervisor(supervisor), plan);
                    }
                }
                Step::Emit(kind) => self.emit_process(kind),
                Step::Retire(supervisor) => self.retire(supervisor, front),
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

    fn block(&mut self, blocker: Blocker, mut plan: Plan) -> Progress {
        plan.resumed = true;
        self.kernel.blocked.entry(blocker).or_default().push(plan);
        Progress::Pending
    }

    /// The supervisor has stopped: drop its record and answer its waiters.
    /// A nested one ends its membership as a requested end, which never
    /// restarts; a root lets its draining owner finish.
    fn retire(&mut self, supervisor: SupervisorId, front: bool) {
        let Some(record) = self.kernel.supervisors.remove(&supervisor) else {
            return;
        };
        self.kernel.roots.retain(|root| *root != supervisor);
        for waiter in record.waiters {
            self.reply(waiter, super::RESPONSE_OK, front);
        }
        if let Some((_, membership)) = record.parent {
            if let Some(entry) = self.kernel.memberships.get_mut(&membership) {
                entry.current = None;
            }
            self.member_ended(membership, TerminalCause::Cancelled);
        }
        if let Some(owner) = record.owner {
            self.finish_draining(owner);
        }
        self.resume_blocked(Blocker::Supervisor(supervisor));
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
                let owner = record.owner;
                for waiter in waiters {
                    self.reply(waiter, super::RESPONSE_DEFECTED, false);
                }
                // An escalated root no longer holds its owner in Draining,
                // and `terminate_program` only defects tasks still running.
                if let Some(owner) = owner {
                    self.finish_draining(owner);
                }
                self.terminate_program();
            }
            Some((_, membership)) => {
                let record = self
                    .kernel
                    .supervisors
                    .remove(&supervisor)
                    .expect("escalating supervisor");
                for waiter in record.waiters {
                    self.reply(waiter, super::RESPONSE_OK, false);
                }
                if let Some(entry) = self.kernel.memberships.get_mut(&membership) {
                    entry.current = None;
                }
                let cause = if record.end_requested {
                    TerminalCause::Cancelled
                } else {
                    TerminalCause::Defected
                };
                self.member_ended(membership, cause);
                self.resume_blocked(Blocker::Supervisor(supervisor));
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
            match self
                .kernel
                .memberships
                .get(&membership)
                .and_then(|e| e.current)
            {
                Some(Instance::Process(incarnation)) => self.unroute(incarnation),
                Some(Instance::Supervisor(child)) => self.close_and_unroute(child),
                None => {}
            }
        }
    }

    /// A process body has ended: sends to it fail from now on, even while
    /// it waits for supervisors it started to stop.
    pub(super) fn unroute_task(&mut self, task: TaskId) {
        if let Some(incarnation) = self.incarnation_of(task) {
            self.unroute(incarnation);
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
        let task = self.kernel.incarnations[&incarnation].task;
        if matches!(self.tasks[&task].phase, TaskPhase::Draining(_)) {
            // The body has ended and waits for root supervisors it started;
            // their members' deadlines bound the wait, so no timer is armed.
            return false;
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

    /// Unroutes, sets the deadline, cancels and drains the body's child
    /// tasks, and arranges how the body learns of the cancellation: a fresh
    /// body never runs; a receive is reissued so it can drain; any other
    /// park is interrupted; a running body is cancelled at its next park
    /// that blocks. A zero deadline forces at once.
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
        self.kernel
            .incarnations
            .get_mut(&incarnation)
            .expect("live incarnation")
            .stopping = Some(Stopping {
            deadline,
            timer: None,
            told: false,
            pending: false,
            child_defected: false,
            forced,
        });
        let phase = self.tasks[&task].phase;
        let receiving =
            phase == TaskPhase::Parked && self.kernel.incarnations[&incarnation].receiving;
        if phase == TaskPhase::Parked {
            self.disarm_wake_sources(task);
        }
        self.cancel_children(task);
        let task_record = self.tasks.get_mut(&task).expect("incarnation task");
        let (told, pending) = match task_record.phase {
            TaskPhase::Runnable if !task_record.ran || forced => {
                task_record.command = ChimiFiberCommand::Cancel;
                set_status(task_record.request, RESPONSE_FORCED, forced);
                (true, false)
            }
            TaskPhase::Runnable => (false, true),
            TaskPhase::Parked => {
                let told = if forced {
                    task_record.command = ChimiFiberCommand::Cancel;
                    set_status(task_record.request, RESPONSE_FORCED, true);
                    false
                } else if !receiving {
                    task_record.command = ChimiFiberCommand::Cancel;
                    set_status(task_record.request, super::RESPONSE_CANCELLED, true);
                    true
                } else {
                    false
                };
                task_record.phase = TaskPhase::Runnable;
                self.runnable.push_front(task);
                (told, false)
            }
            TaskPhase::Draining(_) | TaskPhase::Terminal(_) => (false, false),
        };
        let stopping = self
            .kernel
            .incarnations
            .get_mut(&incarnation)
            .and_then(|record| record.stopping.as_mut())
            .expect("stopping incarnation");
        stopping.told = told;
        stopping.pending = pending;
    }

    /// Cancels the task's structured children; each drains before this
    /// returns.
    fn cancel_children(&mut self, task: TaskId) {
        let children: Vec<_> = self.tasks[&task].children.iter().copied().collect();
        for child in children {
            self.begin_terminal(child, TaskOutcome::Cancelled, false);
        }
    }

    /// The shutdown deadline passed: answer the pending call with a forced
    /// reply and run the body to its end.
    pub(super) fn force(&mut self, incarnation: IncarnationId) {
        let Some(record) = self.kernel.incarnations.get_mut(&incarnation) else {
            return;
        };
        let task = record.task;
        let stopping = record
            .stopping
            .as_mut()
            .expect("only stopping incarnations are forced");
        stopping.timer = None;
        if matches!(self.tasks[&task].phase, TaskPhase::Draining(_)) {
            // The body finished its cleanup in time; only supervisors it
            // started are still stopping, each under its own deadlines.
            return;
        }
        stopping.forced = true;
        let task_record = self.tasks.get_mut(&task).expect("incarnation task");
        task_record.command = ChimiFiberCommand::Cancel;
        set_status(task_record.request, RESPONSE_FORCED, true);
        if task_record.phase == TaskPhase::Parked {
            self.disarm_wake_sources(task);
            let task_record = self.tasks.get_mut(&task).expect("incarnation task");
            task_record.phase = TaskPhase::Runnable;
            self.runnable.push_front(task);
        }
        let terminated = self.await_incarnation(incarnation);
        debug_assert!(
            terminated || matches!(self.tasks[&task].phase, TaskPhase::Draining(_)),
            "a forced body always ends"
        );
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
            && let Some(last) = entry.last
        {
            self.kernel
                .retired
                .insert(membership, Retired { last, announce });
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
        let mut trace =
            SchedulerTraceEvent::blank(SCHEDULER_TRACE_PROCESS, kind.trace_kind(), event.sequence);
        trace.time = event.time.as_nanos();
        match kind {
            ProcessEventKind::Started {
                supervisor,
                membership,
                incarnation,
                task,
                name,
            } => {
                trace.task = task.get();
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
                stack_overflow,
            } => {
                trace.membership = membership.0;
                trace.incarnation = incarnation.0;
                trace.cause = cause.code();
                trace.outcome = u32::from(stack_overflow);
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
            ProcessEventKind::SupervisorStarted {
                supervisor,
                parent,
                membership,
            } => {
                trace.supervisor = supervisor.0;
                trace.parent_supervisor = parent.map_or(0, |p| p.0);
                trace.membership = membership.map_or(0, |m| m.0);
            }
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
        // Force a lingering body whose plan waits on it; a body that already
        // ended and waits only on supervisors it started cannot be forced.
        // A plan whose blocker is already gone is resumed.
        loop {
            let next = self
                .kernel
                .blocked
                .keys()
                .copied()
                .find(|blocker| match blocker {
                    Blocker::Incarnation(incarnation) => self
                        .kernel
                        .incarnations
                        .get(incarnation)
                        .is_none_or(|record| {
                            !matches!(self.tasks[&record.task].phase, TaskPhase::Draining(_))
                        }),
                    Blocker::Supervisor(supervisor) => {
                        !self.kernel.supervisors.contains_key(supervisor)
                    }
                });
            match next {
                Some(Blocker::Incarnation(incarnation))
                    if self.kernel.incarnations.contains_key(&incarnation) =>
                {
                    self.force(incarnation);
                }
                Some(blocker) => self.resume_blocked(blocker),
                None => break,
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
/// cleanup, its own or a child task's, stays a defect; any other end
/// during cleanup is `Cancelled`.
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
        Some(stopping) if natural == TerminalCause::Defected || stopping.child_defected => {
            TerminalCause::Defected
        }
        Some(_) => TerminalCause::Cancelled,
        None => natural,
    }
}

/// Parks a draining handler cannot block in: shutdown cancels them.
fn interrupted_while_draining(kind: u32) -> bool {
    matches!(
        kind,
        0 | super::REQUEST_TASK_RESULT
            | super::REQUEST_CHANNEL_SEND
            | super::REQUEST_CHANNEL_RECEIVE
            | super::REQUEST_TIMER_SLEEP_UNTIL
    )
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
            | super::REQUEST_SUPERVISOR_REMOVE_SUPERVISOR
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
