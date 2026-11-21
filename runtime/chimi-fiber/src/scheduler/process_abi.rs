//! C ABI for supervised processes: request records, policy parsing, and the
//! configurable scheduler entry point. Layouts match `chimi_fiber.h`.

use super::process::{
    Address, HandleKind, IncarnationId, Member, MembershipId, MonitorId, NameId, Policy,
    ProcessSpec, Receipt, Restart, Restarts, StartError, SupervisorId, SupervisorState, Waiter,
};
use super::{
    CapabilityViolationReport, MessageRelease, ProcessDefectReport, ReleaseTake, SendFailure,
    SpecContextRelease, TaskContextRelease, ValueCopy, ValueRelease,
};
use super::{
    ClockMode, DEADLOCK_DEFECT_CODE, Idle, ParkedTask, RESPONSE_CANCELLED,
    RESPONSE_CAPACITY_REACHED, RESPONSE_DEFECTED, RESPONSE_ERROR, RESPONSE_FORCED, RESPONSE_FULL,
    RESPONSE_NAME_TAKEN, RESPONSE_OK, RESPONSE_PENDING, RESPONSE_STOPPED, RESPONSE_SUCCEEDED,
    RESPONSE_UNAVAILABLE, SCHEDULER_DEFECT_CODE, Scheduler, SchedulerOptions, SchedulerOutcome,
    SchedulerTraceSink, TaskId, TaskOutcome, TaskPhase, TaskResult, TraceMode,
};
use super::carrier::{KernelAccess, Locked};
use super::ready::Carrier;
use crate::{ChimiFiberCommand, ChimiFiberEntry, ChimiFiberState};
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

pub const ADDRESS_NAMED: u32 = 1;
pub const ADDRESS_DIRECT: u32 = 2;
pub const RESTART_NEVER: u32 = 1;
pub const RESTART_ON_DEFECT: u32 = 2;
pub const MEMBER_PROCESS: u32 = 1;
pub const MEMBER_SUPERVISOR: u32 = 2;
pub const CLOCK_REAL: u32 = 0;
pub const CLOCK_VIRTUAL: u32 = 1;

/// Nesting beyond this depth is rejected as a malformed policy.
const MAX_POLICY_DEPTH: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiProcessRequest {
    pub kind: u32,
    pub status: u32,
    pub address_kind: u32,
    pub restart: u32,
    pub address: u64,
    pub supervisor: u64,
    pub membership: u64,
    pub message: u64,
    pub name: u64,
    pub owner: u64,
    pub policy: *const ChimiSupervisorPolicy,
    pub spec: *const ChimiProcessSpec,
    /// MONITOR's reply and UNMONITOR's input.
    pub monitor: u64,
    /// MAILBOX's reply: messages and notices waiting.
    pub queued: u64,
    /// MAILBOX's reply.
    pub capacity: u32,
    /// RECEIVE's reply: 0 for a message, else the `Down` code of a notice,
    /// whose handle is then in `message`.
    pub down: u32,
    /// SEND's and START_PROCESS's: null, or the exclusive capabilities the
    /// message or start argument holds.
    pub carried: *const super::ChimiMessageCapabilities,
}

impl ChimiProcessRequest {
    pub const fn new(kind: u32) -> Self {
        Self {
            kind,
            status: 0,
            address_kind: 0,
            restart: 0,
            address: 0,
            supervisor: 0,
            membership: 0,
            message: 0,
            name: 0,
            owner: 0,
            policy: std::ptr::null(),
            spec: std::ptr::null(),
            monitor: 0,
            queued: 0,
            capacity: 0,
            down: 0,
            carried: std::ptr::null(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiProcessSpec {
    pub entry: Option<ChimiFiberEntry>,
    pub context: *mut c_void,
    pub mailbox_capacity: u32,
    pub shutdown_within: i64,
    pub name: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiSupervisorMember {
    pub kind: u32,
    pub restart: u32,
    pub process: ChimiProcessSpec,
    pub supervisor: *const ChimiSupervisorPolicy,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiSupervisorPolicy {
    pub members: *const ChimiSupervisorMember,
    pub member_count: usize,
    pub capacity: u32,
    pub restart_limit: u32,
    pub restart_within: i64,
}

#[repr(C)]
pub struct ChimiSchedulerConfig {
    pub entry: Option<ChimiFiberEntry>,
    pub context: *mut c_void,
    pub stack_bytes: usize,
    pub trace_mode: u32,
    pub clock_mode: u32,
    pub trace_sink: Option<SchedulerTraceSink>,
    pub trace_context: *mut c_void,
    pub release_message: Option<MessageRelease>,
    pub release_spec_context: Option<SpecContextRelease>,
    pub release_context: *mut c_void,
    pub deadlock_report: Option<DeadlockReport>,
    pub deadlock_context: *mut c_void,
    pub release_task_value: Option<ValueRelease>,
    pub release_channel_value: Option<ValueRelease>,
    pub release_task_context: Option<TaskContextRelease>,
    pub capability_violation_report: Option<CapabilityViolationReport>,
    pub capability_violation_context: *mut c_void,
    pub process_defect_report: Option<ProcessDefectReport>,
    pub process_defect_context: *mut c_void,
    /// The runner's received-signal word: 0, or the number of the first
    /// SIGINT or SIGTERM. Read between resumes; an injected signal is
    /// written here when it fires. May be null.
    pub signal: *mut i32,
    /// Read end of the pipe the runner's handler writes to; 0 or negative
    /// for none, so a zeroed config has none.
    pub signal_fd: i32,
    /// Nonzero: deliver this signal when the clock reaches
    /// `injected_signal_at` nanoseconds.
    pub injected_signal: i32,
    pub injected_signal_at: i64,
    /// Copies a value for each broadcast or watch subscription past the
    /// first, with `release_context`. May be null only when no program
    /// publishes to more than one subscription.
    pub copy_channel_value: Option<ValueCopy>,
    /// The runner's `std/signal` word: its handler sets bit `code - 1`. May
    /// be null.
    pub user_signals: *mut u32,
    /// `std/signal` signals to latch at virtual instants; `count` entries,
    /// or null and 0.
    pub injected_user_signals: *const ChimiInjectedSignal,
    pub injected_user_signal_count: usize,
    /// Hands over the releases the carrier queued with no task running, with
    /// `release_context`; see `SchedulerOptions::take_release`. May be null.
    pub take_release: Option<ReleaseTake>,
}

/// A `std/signal` signal to latch when the clock reaches `at` nanoseconds:
/// `code` 1 SIGHUP, 2 SIGUSR1, 3 SIGUSR2.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ChimiInjectedSignal {
    pub code: u32,
    pub at: i64,
}

/// One task parked in a detected deadlock and the request kind it waits on
/// (0 for a bare park).
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChimiDeadlockWaiter {
    pub task: u64,
    pub request_kind: u32,
}

pub type DeadlockReport = unsafe extern "C" fn(*mut c_void, *const ChimiDeadlockWaiter, usize);

/// How `Scheduler::run_root` ended.
pub struct RootRun {
    pub outcome: SchedulerOutcome,
    /// Under virtual time, the parked tasks of the deadlock that defected
    /// the root, each with the request it waits on; empty otherwise.
    pub deadlock: Vec<ParkedTask>,
}

fn restart(raw: u32) -> Option<Restart> {
    match raw {
        RESTART_NEVER => Some(Restart::Never),
        RESTART_ON_DEFECT => Some(Restart::OnDefect),
        _ => None,
    }
}

fn address(kind: u32, raw: u64) -> Option<Address> {
    match kind {
        ADDRESS_NAMED => NameId::from_raw(raw).map(Address::Named),
        ADDRESS_DIRECT => Some(Address::Direct(IncarnationId::from_raw(raw))),
        _ => None,
    }
}

/// Validates a C specification. Parsing takes no ownership of the context;
/// the request handler adopts or releases it.
fn read_spec(spec: &ChimiProcessSpec) -> Option<ProcessSpec> {
    let shutdown_within = u64::try_from(spec.shutdown_within).ok()?;
    Some(ProcessSpec {
        entry: spec.entry?,
        context: spec.context,
        mailbox_capacity: NonZeroU32::new(spec.mailbox_capacity)?,
        shutdown_within: Duration::from_nanos(shutdown_within),
        name: NameId::from_raw(spec.name),
    })
}

/// # Safety
/// `policy` is null or points to a policy whose member arrays and nested
/// policies are valid for reads.
unsafe fn read_policy(policy: *const ChimiSupervisorPolicy, depth: usize) -> Option<Policy> {
    if depth > MAX_POLICY_DEPTH {
        return None;
    }
    // SAFETY: the caller guarantees a null or readable policy.
    let policy = unsafe { policy.as_ref() }?;
    let members = if policy.member_count == 0 {
        &[][..]
    } else if policy.members.is_null() {
        return None;
    } else {
        // SAFETY: a non-empty member array is readable for its count.
        unsafe { std::slice::from_raw_parts(policy.members, policy.member_count) }
    };
    let members = members
        .iter()
        .map(|member| {
            let restart = restart(member.restart)?;
            match member.kind {
                MEMBER_PROCESS => Some(Member::Process(read_spec(&member.process)?, restart)),
                MEMBER_SUPERVISOR => Some(Member::Supervisor(
                    // SAFETY: nested policies satisfy the same contract.
                    unsafe { read_policy(member.supervisor, depth + 1) }?,
                    restart,
                )),
                _ => None,
            }
        })
        .collect::<Option<Vec<_>>>()?;
    let within = Duration::from_nanos(u64::try_from(policy.restart_within).ok()?);
    let restarts = Restarts::new(policy.restart_limit, within).ok()?;
    Policy::new(members, NonZeroU32::new(policy.capacity)?, restarts).ok()
}

/// Visits the context of every process member of a policy the request
/// handed over, as far as the policy can be read: the same reads
/// `read_policy` makes, without validating any value.
///
/// # Safety
/// As for `read_policy`.
unsafe fn visit_policy_contexts(
    policy: *const ChimiSupervisorPolicy,
    depth: usize,
    visit: &mut impl FnMut(*mut c_void),
) {
    if depth > MAX_POLICY_DEPTH {
        return;
    }
    // SAFETY: the caller guarantees a null or readable policy.
    let Some(policy) = (unsafe { policy.as_ref() }) else {
        return;
    };
    if policy.member_count == 0 || policy.members.is_null() {
        return;
    }
    // SAFETY: a non-empty member array is readable for its count.
    let members = unsafe { std::slice::from_raw_parts(policy.members, policy.member_count) };
    for member in members {
        match member.kind {
            MEMBER_PROCESS => visit(member.process.context),
            // SAFETY: nested policies satisfy the same contract.
            MEMBER_SUPERVISOR => unsafe {
                visit_policy_contexts(member.supervisor, depth + 1, visit)
            },
            _ => {}
        }
    }
}

fn start_status(error: StartError) -> u32 {
    match error {
        StartError::CapacityReached => RESPONSE_CAPACITY_REACHED,
        StartError::NameTaken(_) => RESPONSE_NAME_TAKEN,
        StartError::Stopped => RESPONSE_STOPPED,
        StartError::SpawnFailed => RESPONSE_ERROR,
    }
}

impl Scheduler {
    /// Serves one process or supervisor request from a parked task. Replies
    /// are written only while the task is still parked on this request, so
    /// an operation that cancels its own caller never touches a dead stack.
    pub(super) fn handle_process_request(
        &mut self,
        owner: TaskId,
        request: *mut ChimiProcessRequest,
    ) {
        // SAFETY: `chimi_fiber_request` keeps the record alive while parked;
        // it is copied before any operation can resume the owner.
        let input = unsafe { request.read() };
        let waiter = Waiter {
            task: owner,
            park: self.tasks[&owner].park,
        };
        if self.request_awaits_own_shutdown(owner, &input) {
            // It could finish only after the caller's own incarnation ends.
            self.respond_cancel(waiter, super::RESPONSE_CANCELLED);
            return;
        }
        match input.kind {
            super::REQUEST_PROCESS_SELF => match self.self_address(owner) {
                Ok(incarnation) => self.respond(waiter, RESPONSE_OK, |r| {
                    r.address_kind = ADDRESS_DIRECT;
                    r.address = incarnation.get();
                }),
                Err(_) => self.respond(waiter, RESPONSE_ERROR, |_| {}),
            },
            super::REQUEST_PROCESS_MONITOR => {
                let (Some(watcher), Some(target)) = (
                    self.incarnation_of(owner),
                    address(input.address_kind, input.address),
                ) else {
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                // SAFETY: the host's record names capabilities that stay
                // valid while it is suspended on the request.
                let carried =
                    unsafe { super::carried_capabilities(input.carried.cast_mut().cast()) };
                if let Err(error) = self.channels.check_carried(owner, &carried) {
                    let _ = self.channel_violation::<()>(
                        owner,
                        error,
                        super::REQUEST_PROCESS_MONITOR,
                        None,
                    );
                    return;
                }
                // The notice carries its capabilities as a message does: in
                // flight until the watcher receives it, ended if it is
                // dropped unread.
                self.channels.dispatch_carried(owner, input.message as usize, &carried);
                let monitor = self.process_monitor(watcher, target, input.message);
                self.respond(waiter, RESPONSE_OK, |r| r.monitor = monitor.get());
            }
            super::REQUEST_PROCESS_UNMONITOR => {
                let (Some(watcher), Some(monitor)) = (
                    self.incarnation_of(owner),
                    MonitorId::from_raw(input.monitor),
                ) else {
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                self.process_unmonitor(watcher, monitor);
                self.respond(waiter, RESPONSE_OK, |_| {});
            }
            super::REQUEST_PROCESS_ARGUMENT => match self.take_argument(owner) {
                Some(argument) => self.respond(waiter, RESPONSE_OK, |r| r.message = argument),
                None => self.respond(waiter, RESPONSE_ERROR, |_| {}),
            },
            super::REQUEST_PROCESS_MAILBOX => {
                let Some(incarnation) = self.incarnation_of(owner) else {
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                let (queued, capacity) = self.mailbox_metrics(incarnation);
                self.respond(waiter, RESPONSE_OK, |r| {
                    r.queued = queued as u64;
                    r.capacity = capacity;
                });
            }
            super::REQUEST_PROCESS_SEND => {
                let Some(target) = address(input.address_kind, input.address) else {
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                // SAFETY: the host's record names capabilities that stay
                // valid while it is suspended on the request.
                let carried =
                    unsafe { super::carried_capabilities(input.carried.cast_mut().cast()) };
                if let Err(error) = self.channels.check_carried(owner, &carried) {
                    let _ = self.channel_violation::<()>(
                        owner,
                        error,
                        super::REQUEST_PROCESS_SEND,
                        None,
                    );
                    return;
                }
                let status = match self.process_send_carrying(
                    target,
                    input.message,
                    Some((owner, &carried)),
                ) {
                    Ok(()) => RESPONSE_OK,
                    Err(rejected) => match rejected.failure {
                        SendFailure::Full => RESPONSE_FULL,
                        SendFailure::Unavailable => RESPONSE_UNAVAILABLE,
                    },
                };
                self.respond(waiter, status, |_| {});
            }
            super::REQUEST_PROCESS_RECEIVE => {
                let Some(incarnation) = self.incarnation_of(owner) else {
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                match self.receive_for(incarnation) {
                    Receipt::Message(message) => self.respond(waiter, RESPONSE_SUCCEEDED, |r| {
                        r.message = message;
                        r.down = 0;
                    }),
                    Receipt::Notice(notice) => self.respond(waiter, RESPONSE_SUCCEEDED, |r| {
                        r.message = notice.handle;
                        r.down = notice.down.code();
                    }),
                    Receipt::Pending => self.write(waiter, RESPONSE_PENDING, |_| {}),
                    Receipt::Cancelled => self.respond_cancel(waiter, RESPONSE_CANCELLED),
                    Receipt::Forced => self.respond_cancel(waiter, RESPONSE_FORCED),
                }
            }
            super::REQUEST_SUPERVISOR_START => {
                // SAFETY: the generated runtime passes a readable policy.
                let Some(policy) = (unsafe { read_policy(input.policy, 0) }) else {
                    // SAFETY: as above; the request handed these contexts over.
                    unsafe {
                        visit_policy_contexts(input.policy, 0, &mut |context| {
                            self.release_spec_context(context)
                        })
                    };
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                match self.start_supervisor_for(Some(owner), policy) {
                    Ok(supervisor) => {
                        self.respond(waiter, RESPONSE_OK, |r| r.supervisor = supervisor.get())
                    }
                    Err(error) => self.respond(waiter, start_status(error), |r| {
                        if let StartError::NameTaken(name) = error {
                            r.name = name.get();
                        }
                    }),
                }
            }
            super::REQUEST_SUPERVISOR_START_PROCESS => {
                let supervisor = SupervisorId::from_raw(input.supervisor);
                // SAFETY: the generated runtime passes a null or readable spec.
                let raw = unsafe { input.spec.as_ref() };
                let (Some(spec), Some(restart), Some(_)) = (
                    raw.and_then(read_spec),
                    restart(input.restart),
                    self.supervisor_state(supervisor),
                ) else {
                    if let Some(raw) = raw {
                        self.release_spec_context(raw.context);
                    }
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                // SAFETY: the host's record names capabilities that stay
                // valid while it is suspended on the request.
                let carried =
                    unsafe { super::carried_capabilities(input.carried.cast_mut().cast()) };
                let started = if input.message == 0 {
                    self.start_process(supervisor, spec, restart)
                } else {
                    // A violation leaves the request unanswered, so the host
                    // still owns the spec context and the message.
                    if let Err(error) = self.channels.check_carried(owner, &carried) {
                        let _ = self.channel_violation::<()>(
                            owner,
                            error,
                            super::REQUEST_SUPERVISOR_START_PROCESS,
                            None,
                        );
                        return;
                    }
                    self.start_handoff(supervisor, spec, owner, input.message, &carried)
                };
                match started {
                    Ok(membership) => {
                        let direct = self.process_address(membership);
                        self.respond(waiter, RESPONSE_OK, |r| {
                            r.membership = membership.get();
                            if let Ok(Address::Direct(incarnation)) = direct {
                                r.address_kind = ADDRESS_DIRECT;
                                r.address = incarnation.get();
                            }
                        })
                    }
                    Err(error) => self.respond(waiter, start_status(error), |r| {
                        if let StartError::NameTaken(name) = error {
                            r.name = name.get();
                        }
                    }),
                }
            }
            super::REQUEST_SUPERVISOR_ADDRESS => {
                match self.process_address(MembershipId::from_raw(input.membership)) {
                    Ok(Address::Direct(incarnation)) => self.respond(waiter, RESPONSE_OK, |r| {
                        r.address_kind = ADDRESS_DIRECT;
                        r.address = incarnation.get();
                    }),
                    _ => self.respond(waiter, RESPONSE_ERROR, |_| {}),
                }
            }
            super::REQUEST_SUPERVISOR_START_SUPERVISOR => {
                let parent = SupervisorId::from_raw(input.supervisor);
                // SAFETY: the generated runtime passes a readable policy.
                let policy = unsafe { read_policy(input.policy, 0) };
                let (Some(policy), Some(restart), Some(_)) = (
                    policy,
                    restart(input.restart),
                    self.supervisor_state(parent),
                ) else {
                    // SAFETY: as above; the request handed these contexts over.
                    unsafe {
                        visit_policy_contexts(input.policy, 0, &mut |context| {
                            self.release_spec_context(context)
                        })
                    };
                    return self.respond(waiter, RESPONSE_ERROR, |_| {});
                };
                match self.start_nested_supervisor(parent, policy, restart) {
                    Ok(membership) => {
                        let nested = self.nested_supervisor(membership);
                        self.respond(waiter, RESPONSE_OK, |r| {
                            r.membership = membership.get();
                            r.supervisor = nested.map_or(0, SupervisorId::get);
                        })
                    }
                    Err(error) => self.respond(waiter, start_status(error), |r| {
                        if let StartError::NameTaken(name) = error {
                            r.name = name.get();
                        }
                    }),
                }
            }
            super::REQUEST_SUPERVISOR_REMOVE_PROCESS
            | super::REQUEST_SUPERVISOR_REMOVE_SUPERVISOR => {
                self.write(waiter, RESPONSE_PENDING, |_| {});
                let kind = if input.kind == super::REQUEST_SUPERVISOR_REMOVE_PROCESS {
                    HandleKind::Process
                } else {
                    HandleKind::Supervisor
                };
                let result = self.remove_for(
                    Some(waiter),
                    SupervisorId::from_raw(input.supervisor),
                    MembershipId::from_raw(input.membership),
                    kind,
                );
                if let Err(error) = result {
                    let owner = match error {
                        super::ProcessError::WrongSupervisor { owner, .. } => owner.get(),
                        _ => 0,
                    };
                    self.respond(waiter, RESPONSE_ERROR, |r| r.owner = owner);
                }
            }
            super::REQUEST_SUPERVISOR_STOP => {
                self.write(waiter, RESPONSE_PENDING, |_| {});
                if self
                    .stop_for(Some(waiter), SupervisorId::from_raw(input.supervisor))
                    .is_err()
                {
                    self.respond(waiter, RESPONSE_ERROR, |_| {});
                }
            }
            super::REQUEST_SUPERVISOR_WAIT => {
                match self.wait_for(waiter, SupervisorId::from_raw(input.supervisor)) {
                    Ok(SupervisorState::Stopped) => self.respond(waiter, RESPONSE_OK, |_| {}),
                    Ok(SupervisorState::Escalated) => {
                        self.respond(waiter, RESPONSE_DEFECTED, |_| {})
                    }
                    Ok(_) => self.write(waiter, RESPONSE_PENDING, |_| {}),
                    Err(_) => self.respond(waiter, RESPONSE_ERROR, |_| {}),
                }
            }
            _ => self.respond(waiter, RESPONSE_ERROR, |_| {}),
        }
    }

    /// Writes into the waiter's record if it is still parked on it.
    fn write(&mut self, waiter: Waiter, status: u32, fill: impl FnOnce(&mut ChimiProcessRequest)) {
        if !self.still_waiting(waiter) {
            return;
        }
        let request = self.tasks[&waiter.task]
            .request
            .cast::<ChimiProcessRequest>();
        // SAFETY: the waiter is parked on this process request record.
        let request = unsafe { &mut *request };
        fill(request);
        request.status = status;
    }

    /// Writes an immediate reply and resumes the caller next.
    fn respond(
        &mut self,
        waiter: Waiter,
        status: u32,
        fill: impl FnOnce(&mut ChimiProcessRequest),
    ) {
        if self.still_waiting(waiter) {
            self.write(waiter, status, fill);
            let _ = self.wake_front(waiter.task);
        }
    }

    /// Replies with a cancellation the body must propagate.
    fn respond_cancel(&mut self, waiter: Waiter, status: u32) {
        if self.still_waiting(waiter) {
            self.tasks
                .get_mut(&waiter.task)
                .expect("waiting task")
                .command = ChimiFiberCommand::Cancel;
            self.respond(waiter, status, |_| {});
        }
    }
}

fn outcome_of(outcome: TaskOutcome) -> SchedulerOutcome {
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

fn defect_outcome(code: i64) -> SchedulerOutcome {
    SchedulerOutcome {
        state: ChimiFiberState::Defected as u32,
        value: 0,
        origin: 0,
        defect_code: code,
    }
}

impl Scheduler {
    /// Runs a root task to its outcome on this scheduler's clock. Under
    /// virtual time a root that can never wake defects with
    /// `deadlock_code`, and the run reports each parked task. Under the
    /// real clock a host event may still wake it, so a root parked with
    /// nothing pending returns with state `Parked` and stays suspended; a
    /// later call continues it. Once the root is terminal, every supervisor
    /// still running is stopped and its cleanup is driven to completion.
    pub fn run_root(&mut self, root: TaskId, deadlock_code: i64) -> RootRun {
        run_root(self, root, deadlock_code)
    }

    /// What the root's run does after the carrier went idle with `idle`.
    pub(super) fn root_step(
        &mut self,
        root: TaskId,
        idle: Idle,
        deadlock_code: i64,
        deadlock: &mut Vec<ParkedTask>,
    ) -> RootStep {
        let root_live = |scheduler: &Self| {
            scheduler.tasks.get(&root).is_some_and(|record| record.phase.is_live())
        };
        match idle {
            Idle::AwaitingHost => return RootStep::Continue,
            Idle::Deadlock(parked) => {
                if root_live(self) {
                    *deadlock = parked;
                    self.force_terminal(
                        root,
                        TaskOutcome::Defected {
                            origin: root,
                            code: deadlock_code,
                        },
                        false,
                    );
                }
            }
            Idle::Stalled(_) if root_live(self) && self.stall_waits_for_signal() => {
                return RootStep::WaitForSignal(self.idle_wake_fds());
            }
            Idle::Stalled(_) if root_live(self) => {
                self.stop_carriers();
                return RootStep::Parked;
            }
            Idle::Stalled(_) | Idle::Quiescent => {}
        }
        let root_done = matches!(
            self.tasks.get(&root).map(|record| record.phase),
            Some(TaskPhase::Terminal(_))
        );
        if !root_done {
            return RootStep::Continue;
        }
        self.shutdown_processes();
        if !self.has_blocked_plans() && self.ready.is_empty() && self.jobs.is_empty() {
            self.stop_carriers();
            RootStep::Finished
        } else {
            RootStep::Continue
        }
    }

    /// The finished root's outcome, its value handed to the caller.
    fn root_outcome(&mut self, root: TaskId) -> SchedulerOutcome {
        match self.result(None, root) {
            Ok(TaskResult::Ready(outcome)) => {
                // The outcome's value passes to the caller; the scheduler
                // releases every other value when it is dropped.
                self.hand_out_outcome_value(root, outcome);
                outcome_of(outcome)
            }
            Ok(TaskResult::Pending) | Err(_) => defect_outcome(SCHEDULER_DEFECT_CODE),
        }
    }
}

/// What a root's run does next once its carrier is idle.
pub(super) enum RootStep {
    Continue,
    /// Wait, without the kernel, until the signal pipe or the carrier's wake
    /// pipe is readable.
    WaitForSignal(super::wake::WakeFds),
    /// The root waits on the host; the run returns with it parked.
    Parked,
    Finished,
}

/// Runs a root task to its outcome on the scheduler's clock. Under virtual
/// time a root that can never wake defects with `deadlock_code`, and the run
/// reports each parked task. Under the real clock a host event may still
/// wake it, so a root parked with nothing pending returns with state
/// `Parked` and stays suspended; a later call continues it. Once the root is
/// terminal, every supervisor still running is stopped and its cleanup is
/// driven to completion.
fn run_root(kernel: &mut impl KernelAccess, root: TaskId, deadlock_code: i64) -> RootRun {
    let mut deadlock = Vec::new();
    kernel.with(|kernel| kernel.begin_run(root));
    loop {
        let idle = super::carrier::run_until_idle(kernel);
        match kernel.with(|kernel| kernel.root_step(root, idle, deadlock_code, &mut deadlock)) {
            RootStep::Continue => {}
            RootStep::WaitForSignal(fds) => {
                if !fds.as_slice().is_empty() {
                    super::signal::wait_readable(fds.as_slice(), None);
                }
            }
            RootStep::Parked => {
                return RootRun {
                    outcome: parked_outcome(),
                    deadlock,
                };
            }
            RootStep::Finished => break,
        }
    }
    let outcome = kernel.with(|kernel| kernel.root_outcome(root));
    RootRun { outcome, deadlock }
}

fn parked_outcome() -> SchedulerOutcome {
    SchedulerOutcome {
        state: ChimiFiberState::Parked as u32,
        value: 0,
        origin: 0,
        defect_code: 0,
    }
}

/// See `chimi_scheduler_live_memberships`.
static LAST_RUN_MEMBERSHIPS: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
/// The memberships the process kernel still held when the last
/// `chimi_scheduler_run` finished its root and stopped every supervisor.
/// Removal and stop each release theirs, so a nonzero count is a leaked
/// membership (lab scenario MF03). Zero before any run.
pub extern "C" fn chimi_scheduler_live_memberships() -> usize {
    LAST_RUN_MEMBERSHIPS.load(Ordering::Relaxed)
}

/// See `chimi_scheduler_peak_task_records`.
static LAST_RUN_PEAK_TASKS: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
/// The most task records the scheduler held at once during the last
/// `chimi_scheduler_run`, the root included. Zero before any run.
pub extern "C" fn chimi_scheduler_peak_task_records() -> usize {
    LAST_RUN_PEAK_TASKS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
/// Executes one root with the configured clock, trace sink, and release
/// hooks, then stops every supervisor still running.
///
/// # Safety
///
/// `config` must be null or valid for reads. Its entry and context satisfy
/// the Chimi fiber entry contract, and every callback and context stays
/// valid until this call returns.
pub unsafe extern "C" fn chimi_scheduler_run(
    config: *const ChimiSchedulerConfig,
) -> SchedulerOutcome {
    // SAFETY: the caller guarantees a null or readable config.
    let Some(config) = (unsafe { config.as_ref() }) else {
        return defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    let Some(entry) = config.entry else {
        return defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    let trace_mode = match config.trace_mode {
        0 => TraceMode::Off,
        1 => TraceMode::Boundaries,
        2 => TraceMode::Full,
        _ => return defect_outcome(SCHEDULER_DEFECT_CODE),
    };
    let clock = match config.clock_mode {
        CLOCK_REAL => ClockMode::Real,
        CLOCK_VIRTUAL => ClockMode::Virtual,
        _ => return defect_outcome(SCHEDULER_DEFECT_CODE),
    };
    let mut scheduler = Scheduler::with_options(SchedulerOptions {
        stack_bytes: config.stack_bytes,
        trace_mode,
        clock,
        release_message: config.release_message,
        release_spec_context: config.release_spec_context,
        release_task_value: config.release_task_value,
        release_channel_value: config.release_channel_value,
        copy_channel_value: config.copy_channel_value,
        release_task_context: config.release_task_context,
        release_context: config.release_context,
        capability_violation_report: config.capability_violation_report,
        violation_context: config.capability_violation_context,
        take_release: config.take_release,
    });
    scheduler.set_trace_sink(config.trace_sink, config.trace_context);
    scheduler
        .set_process_defect_report(config.process_defect_report, config.process_defect_context);
    scheduler.set_signal_source(config.signal, config.signal_fd);
    if config.injected_signal != 0 {
        scheduler.inject_signal_at(
            super::Instant::ORIGIN.saturating_add(Duration::from_nanos(
                u64::try_from(config.injected_signal_at).unwrap_or(0),
            )),
            config.injected_signal,
        );
    }
    scheduler.set_user_signal_source(config.user_signals);
    if !config.injected_user_signals.is_null() {
        // SAFETY: the caller guarantees `count` readable entries.
        let injections = unsafe {
            std::slice::from_raw_parts(
                config.injected_user_signals,
                config.injected_user_signal_count,
            )
        };
        for injection in injections {
            scheduler.inject_user_signal_at(
                super::Instant::ORIGIN.saturating_add(Duration::from_nanos(
                    u64::try_from(injection.at).unwrap_or(0),
                )),
                injection.code,
            );
        }
    }
    let Ok(root) = scheduler.start_root_with_context(entry, config.context) else {
        return defect_outcome(SCHEDULER_DEFECT_CODE);
    };
    // The carrier loop runs against the kernel behind its lock, as each of
    // several carriers will, so the performance gates measure the lock.
    let kernel = Mutex::new(scheduler);
    let run = run_root(&mut Locked::new(&kernel, Carrier::default()), root, DEADLOCK_DEFECT_CODE);
    let scheduler = kernel.into_inner().unwrap_or_else(PoisonError::into_inner);
    LAST_RUN_MEMBERSHIPS.store(scheduler.process_metrics().memberships, Ordering::Relaxed);
    LAST_RUN_PEAK_TASKS.store(scheduler.peak_tasks(), Ordering::Relaxed);
    if let Some(report) = config.deadlock_report
        && !run.deadlock.is_empty()
    {
        let waiters: Vec<_> = run
            .deadlock
            .iter()
            .map(|parked| ChimiDeadlockWaiter {
                task: parked.task.get(),
                request_kind: parked.request_kind,
            })
            .collect();
        // SAFETY: the caller keeps the callback and its context valid until
        // this call returns; the array lives for the call.
        unsafe { report(config.deadlock_context, waiters.as_ptr(), waiters.len()) };
    }
    if run.outcome.state == ChimiFiberState::Parked as u32 {
        // The root waits on the host. Its fibers cannot be destroyed while
        // parked (destruction unwinds through foreign frames), so the
        // suspended execution is left in place rather than torn down.
        std::mem::forget(scheduler);
    }
    run.outcome
}
