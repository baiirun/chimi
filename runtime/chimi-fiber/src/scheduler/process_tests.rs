//! Supervised-process and clock contracts against the real scheduler.
//!
//! Test names carry the conformance-lab scenario IDs
//! (docs/specification/supervised-process-conformance.md) or, for the
//! prototype's kernel decisions, the prototype test name.

use super::process_abi::{
    ADDRESS_DIRECT, ADDRESS_NAMED, CLOCK_VIRTUAL, MEMBER_PROCESS, MEMBER_SUPERVISOR, RESTART_NEVER,
    RESTART_ON_DEFECT, chimi_scheduler_run,
};
use super::*;
use crate::{ChimiFiberResult, FIXTURE_LOCK, chimi_fiber_park, chimi_fiber_request};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::ptr;
use std::time::Duration;

const STACK: usize = 64 * 1024;
const COUNTER: u64 = 1;
const PEER: u64 = 2;
const THIRD: u64 = 3;
const MS: Duration = Duration::from_millis(1);

fn name(label: u64) -> NameId {
    NameId::new(NonZeroU64::new(label).expect("nonzero label"))
}

fn named(label: u64) -> Address {
    Address::Named(name(label))
}

/// Messages are opaque handles; the test keeps what each one means.
#[derive(Clone, Copy, Debug)]
enum Msg {
    Add(i64),
    Crash,
    Return,
    Fail,
    /// Sends `Add(value)` to the address twice in one resume.
    RelayTwice(Address, i64),
    SendTo(Address, i64),
    Remove(SupervisorId, MembershipId),
    Stop(SupervisorId),
    StartSibling(SupervisorId),
    Sleep(Duration),
    SpawnSleeper,
    /// Starts a root supervisor whose member lingers in cleanup.
    StartLingeringRoot,
    /// Starts a root supervisor that escalates when `ESCALATOR` crashes.
    StartEscalatingRoot,
    Overflow,
}

#[derive(Clone, Copy, Debug, Default)]
enum OnCancel {
    #[default]
    Return,
    /// Cleanup sleeps again after being told it is cancelled.
    Stubborn,
    /// Cleanup sends to its own direct and named address.
    SendToSelf(u64),
}

#[derive(Clone, Copy, Debug, Default)]
struct Behavior {
    on_cancel: OnCancel,
    /// On its first run the body sends `Add(value)` to this name.
    greet: Option<(u64, i64)>,
}

static PLAIN: Behavior = Behavior {
    on_cancel: OnCancel::Return,
    greet: None,
};
static STUBBORN: Behavior = Behavior {
    on_cancel: OnCancel::Stubborn,
    greet: None,
};

#[derive(Default)]
struct Log {
    messages: Vec<Msg>,
    totals: BTreeMap<u64, i64>,
    /// `Add` values in receive order, per incarnation.
    adds: Vec<(u64, i64)>,
    /// Statuses of requests bodies made, in order.
    outcomes: Vec<u32>,
    released: Vec<u64>,
    released_specs: Vec<usize>,
    /// Times at which sleeping tasks woke.
    woke: Vec<(u64, i64)>,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::new(Log::default());
}

fn log<T>(f: impl FnOnce(&mut Log) -> T) -> T {
    LOG.with(|log| f(&mut log.borrow_mut()))
}

const HANDLE_BASE: u64 = 1_000;

fn message(msg: Msg) -> u64 {
    log(|log| {
        log.messages.push(msg);
        HANDLE_BASE + log.messages.len() as u64 - 1
    })
}

fn msg(handle: u64) -> Msg {
    log(|log| log.messages[(handle - HANDLE_BASE) as usize])
}

fn total(incarnation: IncarnationId) -> i64 {
    log(|log| log.totals.get(&incarnation.get()).copied().unwrap_or(0))
}

fn adds_of(incarnation: IncarnationId) -> Vec<i64> {
    log(|log| {
        log.adds
            .iter()
            .filter(|(i, _)| *i == incarnation.get())
            .map(|(_, v)| *v)
            .collect()
    })
}

unsafe extern "C" fn record_release(_context: *mut c_void, message: u64) {
    log(|log| log.released.push(message));
}

unsafe extern "C" fn record_spec_release(_context: *mut c_void, spec: *mut c_void) {
    log(|log| log.released_specs.push(spec as usize));
}

// ----- What a body does, through the request ABI -------------------------

fn process_call(request: &mut ChimiProcessRequest) -> ChimiFiberCommand {
    unsafe { chimi_fiber_request((request as *mut ChimiProcessRequest).cast(), ptr::null()) }
}

fn scheduler_call(request: &mut SchedulerRequest) -> ChimiFiberCommand {
    unsafe { chimi_fiber_request((request as *mut SchedulerRequest).cast(), ptr::null()) }
}

fn blank_request(kind: u32) -> SchedulerRequest {
    SchedulerRequest {
        kind,
        status: 0,
        task: 0,
        channel: 0,
        owner: 0,
        token: 0,
        value: 0,
        entry: None,
        context: ptr::null_mut(),
    }
}

enum Received {
    Message(u64),
    Cancelled,
    Forced,
}

fn receive() -> Received {
    loop {
        let mut request = ChimiProcessRequest::new(REQUEST_PROCESS_RECEIVE);
        let command = process_call(&mut request);
        match request.status {
            RESPONSE_FORCED => return Received::Forced,
            RESPONSE_PENDING if command == ChimiFiberCommand::Resume => continue,
            RESPONSE_SUCCEEDED if command == ChimiFiberCommand::Resume => {
                return Received::Message(request.message);
            }
            _ => return Received::Cancelled,
        }
    }
}

fn now() -> i64 {
    let mut request = blank_request(REQUEST_TIMER_NOW);
    scheduler_call(&mut request);
    request.value
}

/// Returns the final status of a sleep.
fn sleep_until(deadline: i64) -> u32 {
    loop {
        let mut request = blank_request(REQUEST_TIMER_SLEEP_UNTIL);
        request.value = deadline;
        let command = scheduler_call(&mut request);
        if command == ChimiFiberCommand::Cancel || request.status != RESPONSE_PENDING {
            return request.status;
        }
    }
}

fn send(address: Address, handle: u64) -> u32 {
    let mut request = ChimiProcessRequest::new(REQUEST_PROCESS_SEND);
    match address {
        Address::Named(name) => {
            request.address_kind = ADDRESS_NAMED;
            request.address = name.get();
        }
        Address::Direct(incarnation) => {
            request.address_kind = ADDRESS_DIRECT;
            request.address = incarnation.get();
        }
    }
    request.message = handle;
    process_call(&mut request);
    request.status
}

fn self_incarnation() -> u64 {
    let mut request = ChimiProcessRequest::new(REQUEST_PROCESS_SELF);
    process_call(&mut request);
    assert_eq!(request.status, RESPONSE_OK);
    request.address
}

/// Issues a supervisor request; returns `None` when the caller is cancelled.
fn supervisor_call(mut request: ChimiProcessRequest) -> Option<ChimiProcessRequest> {
    let command = process_call(&mut request);
    (command == ChimiFiberCommand::Resume).then_some(request)
}

fn ended(value: i64) -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Succeeded as u32,
        value,
        defect_code: 0,
    }
}

fn defected() -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Defected as u32,
        value: 0,
        defect_code: 7,
    }
}

#[inline(never)]
#[allow(unconditional_recursion)]
fn overflow(depth: u64) -> u64 {
    let frame = std::hint::black_box([depth; 64]);
    overflow(frame[0] + 1) + frame[1]
}

unsafe extern "C" fn sleeper_task(_context: *mut c_void) -> ChimiFiberResult {
    sleep_until(i64::MAX);
    ChimiFiberResult {
        state: ChimiFiberState::Cancelled as u32,
        value: 0,
        defect_code: 0,
    }
}

/// A process body that interprets `Msg` and records what it saw.
unsafe extern "C" fn worker(context: *mut c_void) -> ChimiFiberResult {
    let behavior = unsafe { *context.cast::<Behavior>() };
    let me = self_incarnation();
    if let Some((target, value)) = behavior.greet {
        let status = send(named(target), message(Msg::Add(value)));
        log(|log| log.outcomes.push(status));
    }
    loop {
        let handle = match receive() {
            Received::Message(handle) => handle,
            Received::Forced => return ended(PROCESS_END_RETURNED),
            Received::Cancelled => return cleanup(behavior, me),
        };
        match msg(handle) {
            Msg::Add(amount) => log(|log| {
                *log.totals.entry(me).or_default() += amount;
                log.adds.push((me, amount));
            }),
            Msg::Crash => return defected(),
            Msg::Return => return ended(PROCESS_END_RETURNED),
            Msg::Fail => return ended(PROCESS_END_FAILED),
            Msg::RelayTwice(to, value) => {
                for _ in 0..2 {
                    let status = send(to, message(Msg::Add(value)));
                    log(|log| log.outcomes.push(status));
                }
            }
            Msg::SendTo(to, value) => {
                let status = send(to, message(Msg::Add(value)));
                log(|log| log.outcomes.push(status));
            }
            Msg::Remove(supervisor, membership) => {
                let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_REMOVE_PROCESS);
                request.supervisor = supervisor.get();
                request.membership = membership.get();
                match supervisor_call(request) {
                    Some(reply) => log(|log| log.outcomes.push(reply.status)),
                    None => return ended(PROCESS_END_RETURNED),
                }
            }
            Msg::Stop(supervisor) => {
                let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_STOP);
                request.supervisor = supervisor.get();
                match supervisor_call(request) {
                    Some(reply) => log(|log| log.outcomes.push(reply.status)),
                    None => return ended(PROCESS_END_RETURNED),
                }
            }
            Msg::StartSibling(supervisor) => {
                let spec = ChimiProcessSpec {
                    entry: Some(worker),
                    context: (&raw const PLAIN).cast_mut().cast(),
                    mailbox_capacity: 8,
                    shutdown_within: 5_000_000_000,
                    name: THIRD,
                };
                let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START_PROCESS);
                request.supervisor = supervisor.get();
                request.restart = RESTART_ON_DEFECT;
                request.spec = &spec;
                let reply = supervisor_call(request).expect("not cancelled");
                log(|log| log.outcomes.push(reply.status));
            }
            Msg::Sleep(duration) => {
                let status = sleep_until(now() + duration.as_nanos() as i64);
                if status != RESPONSE_OK {
                    return ended(PROCESS_END_RETURNED);
                }
                log(|log| log.woke.push((me, now())));
            }
            Msg::SpawnSleeper => {
                let mut request = blank_request(REQUEST_TASK_START);
                request.entry = Some(sleeper_task);
                scheduler_call(&mut request);
                assert_eq!(request.status, RESPONSE_OK);
            }
            Msg::StartLingeringRoot => {
                let status = start_lingering_root();
                log(|log| log.outcomes.push(status));
            }
            Msg::StartEscalatingRoot => {
                let status = start_escalating_root();
                log(|log| log.outcomes.push(status));
            }
            Msg::Overflow => {
                std::hint::black_box(overflow(0));
            }
        }
    }
}

fn cleanup(behavior: Behavior, me: u64) -> ChimiFiberResult {
    match behavior.on_cancel {
        OnCancel::Return => {}
        OnCancel::Stubborn => loop {
            if sleep_until(i64::MAX) == RESPONSE_FORCED {
                break;
            }
        },
        OnCancel::SendToSelf(label) => {
            let direct = send(
                Address::Direct(IncarnationId::from_raw(me)),
                message(Msg::Add(1)),
            );
            let by_name = send(named(label), message(Msg::Add(1)));
            log(|log| log.outcomes.extend([direct, by_name]));
        }
    }
    ended(PROCESS_END_RETURNED)
}

/// Sleeps the deadline carried in its context, then succeeds.
unsafe extern "C" fn host_sleeper(context: *mut c_void) -> ChimiFiberResult {
    let status = sleep_until(context as usize as i64);
    ended(i64::from(status))
}

/// Sleeps until its context deadline and records when it woke.
unsafe extern "C" fn recording_sleeper(context: *mut c_void) -> ChimiFiberResult {
    if sleep_until(context as usize as i64) != RESPONSE_OK {
        return ChimiFiberResult {
            state: ChimiFiberState::Cancelled as u32,
            value: 0,
            defect_code: 0,
        };
    }
    let woke = now();
    let task = log(|log| log.woke.len() as u64);
    log(|log| log.woke.push((task, woke)));
    ended(0)
}

fn behavior_context(behavior: &'static Behavior) -> *mut c_void {
    (behavior as *const Behavior).cast_mut().cast()
}

fn spec_with(behavior: &'static Behavior, label: Option<u64>, capacity: u32) -> ProcessSpec {
    ProcessSpec {
        entry: worker,
        context: behavior_context(behavior),
        mailbox_capacity: NonZeroU32::new(capacity).expect("positive capacity"),
        shutdown_within: Duration::from_secs(5),
        name: label.map(name),
    }
}

fn spec(label: Option<u64>, capacity: u32) -> ProcessSpec {
    spec_with(&PLAIN, label, capacity)
}

fn policy_with(members: Vec<Member>, capacity: u32, limit: u32, within: Duration) -> Policy {
    Policy::new(
        members,
        NonZeroU32::new(capacity).expect("positive capacity"),
        Restarts::new(limit, within).expect("positive window"),
    )
    .expect("valid policy")
}

fn policy(members: Vec<Member>, limit: u32) -> Policy {
    policy_with(members, 16, limit, Duration::from_secs(30))
}

fn counter_member(label: u64) -> Member {
    Member::Process(spec(Some(label), 64), Restart::OnDefect)
}

/// A scheduler on virtual time with release hooks that record into `LOG`.
struct Lab {
    scheduler: Scheduler,
}

impl Lab {
    fn new() -> Self {
        Self::traced(TraceMode::Full)
    }

    fn traced(trace_mode: TraceMode) -> Self {
        LOG.with(|log| *log.borrow_mut() = Log::default());
        let mut options = SchedulerOptions::new(STACK, trace_mode, ClockMode::Virtual);
        options.release_message = Some(record_release);
        options.release_spec_context = Some(record_spec_release);
        Self {
            scheduler: Scheduler::with_options(options),
        }
    }

    fn send(&mut self, address: Address, msg: Msg) -> Result<(), SendRejected> {
        self.scheduler.process_send(address, message(msg))
    }

    fn deliver(&mut self, address: Address, msg: Msg) {
        self.send(address, msg).expect("accepted");
    }

    /// Lets every runnable process run until it parks.
    fn settle(&mut self) {
        assert_eq!(self.scheduler.run_until_idle(), Idle::Quiescent);
    }

    /// Advances virtual time by `duration` the way a sleeping main would.
    fn sleep(&mut self, duration: Duration) {
        let deadline = self.scheduler.now().saturating_add(duration);
        self.scheduler
            .start_root_with_context(host_sleeper, deadline.as_nanos() as usize as *mut c_void)
            .expect("sleeper");
        self.settle();
        assert_eq!(self.scheduler.now(), deadline);
    }

    fn kinds(&self) -> Vec<ProcessEventKind> {
        self.scheduler
            .process_events()
            .iter()
            .map(|event| event.kind)
            .collect()
    }

    fn lifecycle(&self) -> Vec<ProcessEventKind> {
        self.kinds()
            .into_iter()
            .filter(|kind| !matches!(kind, ProcessEventKind::Sent { .. }))
            .collect()
    }

    fn started(&self) -> Vec<(MembershipId, IncarnationId)> {
        self.kinds()
            .into_iter()
            .filter_map(|kind| match kind {
                ProcessEventKind::Started {
                    membership,
                    incarnation,
                    ..
                } => Some((membership, incarnation)),
                _ => None,
            })
            .collect()
    }

    fn terminations(&self) -> Vec<(MembershipId, TerminalCause)> {
        self.kinds()
            .into_iter()
            .filter_map(|kind| match kind {
                ProcessEventKind::Terminated {
                    membership, cause, ..
                } => Some((membership, cause)),
                _ => None,
            })
            .collect()
    }

    fn restarts(&self) -> Vec<(MembershipId, u32)> {
        self.kinds()
            .into_iter()
            .filter_map(|kind| match kind {
                ProcessEventKind::RestartAttempted {
                    membership,
                    attempts_in_window,
                    ..
                } => Some((membership, attempts_in_window)),
                _ => None,
            })
            .collect()
    }

    fn transcript(&self) -> Vec<String> {
        self.scheduler
            .process_events()
            .iter()
            .map(|event| render(event.time.as_nanos(), event.kind))
            .collect()
    }
}

// ----- Transcript rendering in the lab's format ---------------------------

fn label(raw: u64) -> String {
    match raw {
        COUNTER => "app/main.counter".to_owned(),
        PEER => "app/main.peer".to_owned(),
        11 => "app/main.p".to_owned(),
        12 => "app/main.q".to_owned(),
        other => format!("name{other}"),
    }
}

fn duration_text(nanos: i64) -> String {
    let nanos = nanos as u64;
    if nanos == 0 {
        return "0s".to_owned();
    }
    if nanos < 1_000_000_000 && nanos.is_multiple_of(1_000_000) {
        return format!("{}ms", nanos / 1_000_000);
    }
    let seconds = nanos / 1_000_000_000;
    let fraction = nanos % 1_000_000_000;
    if fraction == 0 {
        return format!("{seconds}s");
    }
    let fraction = format!("{fraction:09}");
    format!("{seconds}.{}s", fraction.trim_end_matches('0'))
}

fn cause_text(cause: TerminalCause) -> &'static str {
    match cause {
        TerminalCause::Returned => "returned",
        TerminalCause::Failed => "failed",
        TerminalCause::Defected => "defected",
        TerminalCause::Cancelled => "cancelled",
        TerminalCause::Forced => "forced",
    }
}

fn render(time: i64, kind: ProcessEventKind) -> String {
    let at = duration_text(time);
    let body = match kind {
        ProcessEventKind::Started {
            supervisor,
            membership,
            incarnation,
            name,
            ..
        } => {
            let name = name.map_or(String::new(), |n| format!(" name={}", label(n.get())));
            format!(
                "process_started supervisor=S{} membership=M{} incarnation=I{}{name}",
                supervisor.get(),
                membership.get(),
                incarnation.get()
            )
        }
        ProcessEventKind::AdmissionRejected { supervisor, error } => {
            let reason = match error {
                StartError::CapacityReached => "capacity_reached".to_owned(),
                StartError::NameTaken(name) => format!("name_taken name={}", label(name.get())),
                StartError::Stopped => "stopped".to_owned(),
                StartError::SpawnFailed => "start_failed".to_owned(),
            };
            format!(
                "admission_rejected supervisor=S{} reason={reason}",
                supervisor.get()
            )
        }
        ProcessEventKind::Terminated {
            membership,
            incarnation,
            cause,
            ..
        } => format!(
            "process_terminated membership=M{} incarnation=I{} cause={}",
            membership.get(),
            incarnation.get(),
            cause_text(cause)
        ),
        ProcessEventKind::RestartAttempted {
            supervisor,
            membership,
            attempts_in_window,
        } => format!(
            "restart_attempted supervisor=S{} membership=M{} attempts_in_window={attempts_in_window}",
            supervisor.get(),
            membership.get()
        ),
        ProcessEventKind::RestartLimitExceeded { supervisor } => {
            format!("restart_limit_exceeded supervisor=S{}", supervisor.get())
        }
        ProcessEventKind::Escalated { supervisor, parent } => format!(
            "supervisor_escalated supervisor=S{} parent={}",
            supervisor.get(),
            parent.map_or("root".to_owned(), |p| format!("S{}", p.get()))
        ),
        ProcessEventKind::Removed { membership } => {
            format!("membership_removed membership=M{}", membership.get())
        }
        ProcessEventKind::Stopped { supervisor } => {
            format!("supervisor_stopped supervisor=S{}", supervisor.get())
        }
        ProcessEventKind::SupervisorStarted {
            supervisor,
            parent,
            membership,
        } => match (parent, membership) {
            (Some(parent), Some(membership)) => format!(
                "supervisor_started supervisor=S{} parent=S{} membership=M{}",
                supervisor.get(),
                parent.get(),
                membership.get()
            ),
            _ => format!(
                "supervisor_started supervisor=S{} parent=root",
                supervisor.get()
            ),
        },
        ProcessEventKind::Sent {
            address,
            incarnation,
            outcome,
        } => {
            let kind = match address {
                Address::Named(name) => format!("named name={}", label(name.get())),
                Address::Direct(_) => "direct".to_owned(),
            };
            let outcome = match outcome {
                SendOutcome::Accepted => "accepted",
                SendOutcome::Full => "full",
                SendOutcome::Unavailable => "unavailable",
            };
            format!(
                "process_send kind={kind} incarnation={} outcome={outcome}",
                incarnation.map_or("-".to_owned(), |i| format!("I{}", i.get()))
            )
        }
    };
    format!("@{at} {body}")
}

/// Renders C-ABI trace events the same way, from the process family only.
fn render_trace(events: &[SchedulerTraceEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.family == SCHEDULER_TRACE_PROCESS)
        .map(|event| {
            let supervisor = SupervisorId::from_raw(event.supervisor);
            let membership = MembershipId::from_raw(event.membership);
            let incarnation = IncarnationId::from_raw(event.incarnation);
            let cause = match event.cause {
                1 => TerminalCause::Returned,
                2 => TerminalCause::Failed,
                3 => TerminalCause::Defected,
                4 => TerminalCause::Cancelled,
                _ => TerminalCause::Forced,
            };
            let kind = match event.kind {
                1 => ProcessEventKind::Started {
                    supervisor,
                    membership,
                    incarnation,
                    task: TaskId::from_raw(event.task),
                    name: NameId::from_raw(event.name),
                },
                3 => ProcessEventKind::Terminated {
                    membership,
                    incarnation,
                    cause,
                    stack_overflow: event.outcome == 1,
                },
                4 => ProcessEventKind::RestartAttempted {
                    supervisor,
                    membership,
                    attempts_in_window: event.attempts_in_window as u32,
                },
                5 => ProcessEventKind::RestartLimitExceeded { supervisor },
                6 => ProcessEventKind::Escalated {
                    supervisor,
                    parent: (event.parent_supervisor != 0)
                        .then(|| SupervisorId::from_raw(event.parent_supervisor)),
                },
                7 => ProcessEventKind::Removed { membership },
                8 => ProcessEventKind::Stopped { supervisor },
                10 => ProcessEventKind::SupervisorStarted {
                    supervisor,
                    parent: (event.parent_supervisor != 0)
                        .then(|| SupervisorId::from_raw(event.parent_supervisor)),
                    membership: (event.membership != 0).then_some(membership),
                },
                9 => ProcessEventKind::Sent {
                    address: if event.address_kind == ADDRESS_NAMED {
                        Address::Named(name(event.name))
                    } else {
                        Address::Direct(incarnation)
                    },
                    incarnation: (event.incarnation != 0).then_some(incarnation),
                    outcome: match event.outcome {
                        1 => SendOutcome::Accepted,
                        2 => SendOutcome::Full,
                        _ => SendOutcome::Unavailable,
                    },
                },
                other => panic!("unexpected process event kind {other}"),
            };
            render(event.time, kind)
        })
        .collect()
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

// ----- Mailbox and delivery ----------------------------------------------

#[test]
fn mb01_delivery_follows_acceptance_order_across_senders() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER)],
            0,
        ))
        .expect("started");
    lab.deliver(named(COUNTER), Msg::Add(1));
    lab.deliver(named(PEER), Msg::SendTo(named(COUNTER), 2));
    lab.deliver(named(COUNTER), Msg::Add(3));
    lab.settle();
    let counter = lab.started()[0].1;
    assert_eq!(adds_of(counter), [1, 3, 2]);
}

#[test]
fn mb02_a_full_mailbox_returns_the_message_and_records_no_incarnation() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    lab.scheduler
        .start_process(root, spec(Some(COUNTER), 2), Restart::Never)
        .expect("admitted");
    lab.deliver(named(COUNTER), Msg::Add(1));
    lab.deliver(named(COUNTER), Msg::Add(2));
    let third = message(Msg::Add(3));
    assert_eq!(
        lab.scheduler.process_send(named(COUNTER), third),
        Err(SendRejected {
            failure: SendFailure::Full,
            message: third
        })
    );
    assert_eq!(
        lab.transcript().last().map(String::as_str),
        Some("@0s process_send kind=named name=app/main.counter incarnation=- outcome=full")
    );
    lab.settle();
    assert_eq!(adds_of(lab.started()[0].1), [1, 2]);
    assert!(
        !log(|log| log.released.contains(&third)),
        "the sender owns a rejected message"
    );
}

#[test]
fn mb03_a_send_from_a_body_never_parks_and_reports_a_full_mailbox() {
    let mut lab = Lab::new();
    let relaying = Member::Process(spec(Some(COUNTER), 8), Restart::Never);
    let receiving = Member::Process(spec(Some(PEER), 1), Restart::Never);
    lab.scheduler
        .start_supervisor(policy(vec![relaying, receiving], 0))
        .expect("started");
    lab.settle();
    // The two sends run in one resume, so the receiver cannot drain its one
    // slot between them. A send that yielded would let it, and both succeed.
    lab.deliver(named(COUNTER), Msg::RelayTwice(named(PEER), 6));
    lab.settle();
    assert_eq!(
        log(|log| log.outcomes.clone()),
        [RESPONSE_OK, RESPONSE_FULL]
    );
    assert_eq!(total(lab.started()[1].1), 6);
}

#[test]
fn mb04_a_declared_member_is_routable_before_its_first_run() {
    static GREETER: Behavior = Behavior {
        on_cancel: OnCancel::Return,
        greet: Some((PEER, 5)),
    };
    let mut lab = Lab::new();
    let greeter = Member::Process(spec_with(&GREETER, Some(COUNTER), 8), Restart::Never);
    lab.scheduler
        .start_supervisor(policy(vec![greeter, counter_member(PEER)], 0))
        .expect("started");
    lab.settle();
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_OK]);
    assert_eq!(total(lab.started()[1].1), 5);
}

#[test]
fn mb05_messages_queued_before_a_defect_are_discarded_not_carried_over() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 1))
        .expect("started");
    lab.deliver(named(COUNTER), Msg::Crash);
    let add = message(Msg::Add(5));
    lab.scheduler
        .process_send(named(COUNTER), add)
        .expect("accepted");
    lab.settle();
    let [_, (_, replacement)] = lab.started()[..] else {
        panic!("{:?}", lab.kinds())
    };
    assert_eq!(total(replacement), 0);
    assert_eq!(log(|log| log.released.clone()), [add]);
}

#[test]
fn mb06_mb07_mb11_returns_and_failures_end_routing_and_only_defects_restart() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 4)).unwrap();
    let ends = [Msg::Return, Msg::Fail, Msg::Crash];
    for label in [COUNTER, PEER, THIRD] {
        lab.scheduler
            .start_process(root, spec(Some(label), 8), Restart::OnDefect)
            .expect("admitted");
    }
    for (label, end) in [COUNTER, PEER, THIRD].into_iter().zip(ends) {
        lab.deliver(named(label), end);
    }
    lab.settle();
    let memberships: Vec<_> = lab.started().iter().map(|(m, _)| *m).collect();
    assert_eq!(
        lab.terminations(),
        [
            (memberships[0], TerminalCause::Returned),
            (memberships[1], TerminalCause::Failed),
            (memberships[2], TerminalCause::Defected),
        ]
    );
    assert_eq!(lab.restarts(), [(memberships[2], 1)]);
    for label in [COUNTER, PEER] {
        let handle = message(Msg::Add(1));
        assert_eq!(
            lab.scheduler.process_send(named(label), handle),
            Err(SendRejected {
                failure: SendFailure::Unavailable,
                message: handle
            })
        );
    }
    lab.deliver(named(THIRD), Msg::Add(1));
}

#[test]
fn mb08_removal_drains_messages_accepted_before_it_then_cancels() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .unwrap();
    lab.settle();
    let direct = lab.scheduler.process_address(managed).unwrap();
    for value in [1, 2, 3] {
        lab.deliver(direct, Msg::Add(value));
    }
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Done)
    );
    let (membership, incarnation) = lab.started()[0];
    assert_eq!(adds_of(incarnation), [1, 2, 3]);
    assert_eq!(lab.terminations(), [(membership, TerminalCause::Cancelled)]);
    assert!(log(|log| log.released.is_empty()));
}

#[test]
fn mb09_sends_to_itself_during_cleanup_are_unavailable() {
    static SELF_SENDER: Behavior = Behavior {
        on_cancel: OnCancel::SendToSelf(COUNTER),
        greet: None,
    };
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(
            root,
            spec_with(&SELF_SENDER, Some(COUNTER), 8),
            Restart::Never,
        )
        .unwrap();
    lab.settle();
    lab.scheduler.remove_process(root, managed).unwrap();
    assert_eq!(
        log(|log| log.outcomes.clone()),
        [RESPONSE_UNAVAILABLE, RESPONSE_UNAVAILABLE]
    );
    assert_eq!(lab.terminations()[0].1, TerminalCause::Cancelled);
}

// ----- Routing -----------------------------------------------------------

#[test]
fn rt01_names_follow_the_replacement_and_direct_addresses_stay_unavailable() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 2)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec(Some(COUNTER), 8), Restart::OnDefect)
        .unwrap();
    let direct = lab.scheduler.process_address(managed).unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(matches!(
        lab.send(direct, Msg::Add(1)),
        Err(SendRejected {
            failure: SendFailure::Unavailable,
            ..
        })
    ));
    lab.deliver(named(COUNTER), Msg::Add(4));
    let fresh = lab.scheduler.process_address(managed).unwrap();
    assert_ne!(fresh, direct);
    lab.deliver(fresh, Msg::Add(1));
    lab.settle();
    let [_, (_, replacement)] = lab.started()[..] else {
        panic!("{:?}", lab.kinds())
    };
    assert_eq!(total(replacement), 5);
}

#[test]
fn rt02_a_never_member_that_defects_releases_its_name() {
    let mut lab = Lab::new();
    let member = Member::Process(spec(Some(COUNTER), 8), Restart::Never);
    lab.scheduler
        .start_supervisor(policy(vec![member], 5))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(lab.send(named(COUNTER), Msg::Add(1)).is_err());
    assert!(lab.restarts().is_empty());
    assert_eq!(lab.scheduler.process_metrics().registered_names, 0);
}

#[test]
fn rt03_a_held_name_rejects_admission_with_its_label() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    let taken = lab
        .scheduler
        .start_process(root, spec(Some(COUNTER), 8), Restart::Never);
    assert_eq!(taken, Err(StartError::NameTaken(name(COUNTER))));
    assert_eq!(
        lab.transcript().last().map(String::as_str),
        Some("@0s admission_rejected supervisor=S1 reason=name_taken name=app/main.counter")
    );
    assert_eq!(
        lab.scheduler.process_metrics().memberships,
        1,
        "no membership allocated"
    );
}

#[test]
fn rt06_a_root_whose_member_takes_a_held_name_stops_what_it_started() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    let clash = policy(vec![counter_member(PEER), counter_member(COUNTER)], 0);
    assert_eq!(
        lab.scheduler.start_supervisor(clash),
        Err(StartError::NameTaken(name(COUNTER)))
    );
    let peer = lab.started()[1].0;
    assert_eq!(lab.terminations(), [(peer, TerminalCause::Cancelled)]);
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(PEER)], 0))
        .expect("the partial root released its names");
}

// ----- Supervisor lifecycle and admission --------------------------------

#[test]
fn sv01_declared_members_start_in_list_order() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(
            vec![
                counter_member(COUNTER),
                counter_member(PEER),
                counter_member(THIRD),
            ],
            0,
        ))
        .unwrap();
    assert_eq!(
        lab.transcript(),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
             @0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.peer
             @0s process_started supervisor=S1 membership=M3 incarnation=I3 name=name3"
        )
    );
}

#[test]
fn sv02_sv06_admission_fails_fast_and_a_stopped_supervisor_ignores_repeats() {
    let mut lab = Lab::new();
    let small = lab
        .scheduler
        .start_supervisor(policy_with(vec![], 1, 0, Duration::from_secs(1)))
        .unwrap();
    let only = lab
        .scheduler
        .start_process(small, spec(Some(PEER), 8), Restart::Never)
        .unwrap();
    assert_eq!(
        lab.scheduler
            .start_process(small, spec(None, 8), Restart::Never),
        Err(StartError::CapacityReached)
    );
    assert!(matches!(
        lab.lifecycle().last(),
        Some(ProcessEventKind::AdmissionRejected {
            error: StartError::CapacityReached,
            ..
        })
    ));
    lab.scheduler.remove_process(small, only).unwrap();
    let reused = lab
        .scheduler
        .start_process(small, spec(Some(PEER), 8), Restart::Never)
        .expect("removal freed the slot and the name");
    assert_eq!(lab.scheduler.stop_supervisor(small), Ok(Progress::Done));
    assert_eq!(
        lab.scheduler
            .start_process(small, spec(None, 8), Restart::Never),
        Err(StartError::Stopped)
    );
    let events = lab.kinds().len();
    assert_eq!(lab.scheduler.stop_supervisor(small), Ok(Progress::Done));
    assert_eq!(
        lab.scheduler.remove_process(small, reused),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.kinds().len(),
        events,
        "repeat stop and removal record nothing"
    );
    assert_eq!(
        lab.scheduler.supervisor_state(small),
        Some(SupervisorState::Stopped)
    );
}

#[test]
fn sv03_sv04_ends_that_do_not_restart_free_their_slots() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy_with(vec![], 2, 5, Duration::from_secs(30)))
        .unwrap();
    lab.scheduler
        .start_process(root, spec(Some(COUNTER), 8), Restart::OnDefect)
        .unwrap();
    lab.scheduler
        .start_process(root, spec(Some(PEER), 8), Restart::Never)
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Fail);
    lab.deliver(named(PEER), Msg::Crash);
    lab.settle();
    assert!(lab.restarts().is_empty());
    let a = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .expect("a failure freed its slot");
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .expect("a defect without OnDefect freed its slot");
    lab.scheduler.remove_process(root, a).unwrap();
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .expect("a cancelled member freed its slot");
    assert!(lab.restarts().is_empty());
}

/// Starts a root of one counter named COUNTER through the request ABI, asks
/// the counter to stop its own root, then waits on the root.
unsafe extern "C" fn sv05_main(_context: *mut c_void) -> ChimiFiberResult {
    let member = ChimiSupervisorMember {
        kind: MEMBER_PROCESS,
        restart: RESTART_ON_DEFECT,
        process: ChimiProcessSpec {
            entry: Some(worker),
            context: behavior_context(&PLAIN),
            mailbox_capacity: 8,
            shutdown_within: 5_000_000_000,
            name: COUNTER,
        },
        supervisor: ptr::null(),
    };
    let policy = ChimiSupervisorPolicy {
        members: &member,
        member_count: 1,
        capacity: 4,
        restart_limit: 0,
        restart_within: 1_000_000_000,
    };
    let mut start = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START);
    start.policy = &policy;
    let start = supervisor_call(start).expect("not cancelled");
    assert_eq!(start.status, RESPONSE_OK);
    let root = SupervisorId::from_raw(start.supervisor);
    assert_eq!(send(named(COUNTER), message(Msg::Stop(root))), RESPONSE_OK);
    let mut wait = ChimiProcessRequest::new(REQUEST_SUPERVISOR_WAIT);
    wait.supervisor = root.get();
    let wait = supervisor_call(wait).expect("not cancelled");
    ended(i64::from(wait.status))
}

#[test]
fn sv05_sd08_a_member_that_stops_its_own_root_is_cancelled_and_wait_returns() {
    let mut lab = Lab::new();
    let main = lab.scheduler.start_root(sv05_main).unwrap();
    assert_eq!(
        lab.scheduler
            .run_root(main, DEADLOCK_DEFECT_CODE)
            .outcome
            .state,
        ChimiFiberState::Succeeded as u32
    );
    assert_eq!(
        lab.scheduler.result(None, main),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(i64::from(
            RESPONSE_OK
        ))))
    );
    let member = lab.started()[0].0;
    assert_eq!(lab.terminations(), [(member, TerminalCause::Cancelled)]);
    assert!(matches!(
        lab.lifecycle().last(),
        Some(ProcessEventKind::Stopped { .. })
    ));
    assert!(
        log(|log| log.outcomes.is_empty()),
        "the stop never returned to its caller"
    );
}

#[test]
fn sv07_a_member_starts_a_sibling_through_the_shared_running() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::StartSibling(root));
    lab.settle();
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_OK]);
    assert!(matches!(
        lab.lifecycle().last(),
        Some(ProcessEventKind::Started { supervisor, name: Some(n), .. })
            if *supervisor == root && *n == name(THIRD)
    ));
    lab.deliver(named(THIRD), Msg::Add(1));
}

#[test]
fn sv09_a_nested_supervisor_starts_in_member_order_and_stops_with_its_subtree() {
    let mut lab = Lab::new();
    let nested = policy(vec![counter_member(PEER), counter_member(THIRD)], 0);
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![
                counter_member(COUNTER),
                Member::Supervisor(nested, Restart::Never),
                counter_member(4),
            ],
            0,
        ))
        .unwrap();
    lab.settle();
    lab.scheduler.stop_supervisor(root).unwrap();
    assert_eq!(
        lab.transcript(),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
             @0s supervisor_started supervisor=S2 parent=S1 membership=M2
             @0s process_started supervisor=S2 membership=M3 incarnation=I2 name=app/main.peer
             @0s process_started supervisor=S2 membership=M4 incarnation=I3 name=name3
             @0s process_started supervisor=S1 membership=M5 incarnation=I4 name=name4
             @0s process_terminated membership=M5 incarnation=I4 cause=cancelled
             @0s process_terminated membership=M4 incarnation=I3 cause=cancelled
             @0s process_terminated membership=M3 incarnation=I2 cause=cancelled
             @0s supervisor_stopped supervisor=S2
             @0s process_terminated membership=M1 incarnation=I1 cause=cancelled
             @0s supervisor_stopped supervisor=S1"
        )
    );
}

/// Starts policies through the request ABI and reports each status.
unsafe extern "C" fn malformed_policies_main(_context: *mut c_void) -> ChimiFiberResult {
    let member = |capacity: u32, shutdown_within: i64| ChimiSupervisorMember {
        kind: MEMBER_PROCESS,
        restart: RESTART_ON_DEFECT,
        process: ChimiProcessSpec {
            entry: Some(worker),
            context: behavior_context(&PLAIN),
            mailbox_capacity: capacity,
            shutdown_within,
            name: 0,
        },
        supervisor: ptr::null(),
    };
    let members = [member(1, 0), member(1, 0)];
    let zero_capacity = [member(0, 0)];
    let negative_deadline = [member(1, -1)];
    let cases: [(&[ChimiSupervisorMember], u32, i64); 4] = [
        (&members, 1, 1),
        (&zero_capacity, 4, 1),
        (&negative_deadline, 4, 1),
        (&members[..1], 4, 0),
    ];
    for (members, capacity, within) in cases {
        let policy = ChimiSupervisorPolicy {
            members: members.as_ptr(),
            member_count: members.len(),
            capacity,
            restart_limit: 1,
            restart_within: within,
        };
        let mut start = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START);
        start.policy = &policy;
        let reply = supervisor_call(start).expect("not cancelled");
        log(|log| log.outcomes.push(reply.status));
    }
    ended(0)
}

#[test]
fn sv11_malformed_policies_are_errors_that_release_every_context_once() {
    let mut lab = Lab::new();
    let main = lab.scheduler.start_root(malformed_policies_main).unwrap();
    lab.scheduler.run_root(main, DEADLOCK_DEFECT_CODE);
    // Members over capacity, mailbox capacity 0, a negative deadline, and an
    // empty restart window.
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_ERROR; 4]);
    assert!(lab.scheduler.process_events().is_empty());
    // The scheduler owns each member context from the request that hands
    // it over: five members across the four policies, each released once.
    let plain = behavior_context(&PLAIN) as usize;
    assert_eq!(log(|log| log.released_specs.clone()), [plain; 5]);
}

#[test]
fn sv12_removal_through_another_supervisor_names_both() {
    let mut lab = Lab::new();
    let first = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let second = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(first, spec(Some(COUNTER), 8), Restart::Never)
        .unwrap();
    assert_eq!(
        lab.scheduler.remove_process(second, managed),
        Err(ProcessError::WrongSupervisor {
            membership: managed,
            owner: first,
            actual: second
        })
    );
    lab.deliver(named(COUNTER), Msg::Add(1));
}

// ----- Restart accounting -------------------------------------------------

#[test]
fn ra01_a_defect_restarts_only_that_member_with_fresh_state() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER)],
            2,
        ))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Add(5));
    lab.deliver(named(PEER), Msg::Add(7));
    lab.settle();
    let before = lab.transcript().len();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Add(3));
    lab.deliver(named(PEER), Msg::Add(1));
    lab.settle();
    let [(counter, first), (_, peer), (restarted, second)] = lab.started()[..] else {
        panic!("{:?}", lab.kinds())
    };
    assert_eq!(restarted, counter);
    assert_eq!((total(first), total(second), total(peer)), (5, 3, 8));
    let cycle: Vec<_> = lab.transcript()[before..]
        .iter()
        .filter(|line| !line.contains("process_send"))
        .cloned()
        .collect();
    assert_eq!(
        cycle,
        lines(
            "@0s process_terminated membership=M1 incarnation=I1 cause=defected
             @0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1
             @0s process_started supervisor=S1 membership=M1 incarnation=I3 name=app/main.counter"
        )
    );
}

#[test]
fn ra02_the_restart_limit_is_shared_by_every_member() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER)],
            2,
        ))
        .unwrap();
    for target in [COUNTER, PEER] {
        lab.deliver(named(target), Msg::Crash);
        lab.settle();
    }
    assert!(!lab.scheduler.program_terminated());
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(lab.scheduler.program_terminated());
}

#[test]
fn ra03_defects_further_apart_than_the_window_do_not_accumulate() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 1))
        .unwrap();
    for _ in 0..3 {
        lab.deliver(named(COUNTER), Msg::Crash);
        lab.settle();
        lab.sleep(Duration::from_secs(31));
    }
    assert!(!lab.scheduler.program_terminated());
    assert_eq!(lab.started().len(), 4);
    assert!(lab.restarts().iter().all(|(_, attempts)| *attempts == 1));
}

#[test]
fn ra04_a_restart_exactly_within_after_the_last_is_outside_the_window() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 1))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    lab.sleep(Duration::from_secs(30));
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(!lab.scheduler.program_terminated());
    assert_eq!(lab.restarts().len(), 2);
}

#[test]
fn ra05_a_zero_limit_escalates_the_first_defect() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(lab.scheduler.program_terminated());
    assert!(lab.restarts().is_empty());
}

#[test]
fn ra06_a_replacement_that_fails_to_start_is_still_charged() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 2))
        .unwrap();
    lab.scheduler.kernel.failing_spawns = 1;
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    let after_first: Vec<_> = lab.transcript().into_iter().skip(3).collect();
    assert_eq!(
        after_first,
        lines(
            "@0s process_terminated membership=M1 incarnation=I1 cause=defected
             @0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1
             @0s admission_rejected supervisor=S1 reason=start_failed
             @0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=2
             @0s process_started supervisor=S1 membership=M1 incarnation=I2 name=app/main.counter"
        )
    );
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(
        lab.scheduler.program_terminated(),
        "the failed start used budget"
    );
}

#[test]
fn ra07_child_tasks_drain_before_the_replacement_starts() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 1))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::SpawnSleeper);
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.process_metrics().timers, 1);
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.now(), Instant::ORIGIN);
    let child_terminal = lab
        .scheduler
        .events()
        .iter()
        .find(|event| event.kind == TaskEventKind::Terminal)
        .expect("the child task terminated")
        .sequence;
    let replacement = lab
        .scheduler
        .process_events()
        .iter()
        .filter(|event| matches!(event.kind, ProcessEventKind::Started { .. }))
        .nth(1)
        .expect("a replacement")
        .sequence;
    assert!(child_terminal < replacement);
    assert_eq!(
        lab.scheduler.process_metrics().timers,
        0,
        "the child's sleep was removed"
    );
}

fn restart_storm() -> Vec<(i64, String)> {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER)],
            2,
        ))
        .unwrap();
    for cycle in 0..1_000 {
        lab.deliver(named(COUNTER), Msg::Crash);
        lab.deliver(named(PEER), Msg::Add(1));
        lab.settle();
        if cycle % 2 == 1 {
            lab.sleep(Duration::from_secs(31));
        }
    }
    assert!(!lab.scheduler.program_terminated());
    assert_eq!(lab.started().len(), 1_002);
    lab.scheduler
        .process_events()
        .iter()
        .map(|event| {
            (
                event.sequence as i64,
                render(event.time.as_nanos(), event.kind),
            )
        })
        .collect()
}

#[test]
fn ra08_a_restart_storm_is_deterministic() {
    let first = restart_storm();
    assert_eq!(first, restart_storm());
}

// ----- Escalation --------------------------------------------------------

#[test]
fn es02_a_nested_on_defect_supervisor_is_restarted_by_its_parent() {
    let mut lab = Lab::new();
    let nested = policy(vec![counter_member(COUNTER)], 0);
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![
                Member::Supervisor(nested, Restart::OnDefect),
                counter_member(PEER),
            ],
            1,
        ))
        .unwrap();
    lab.deliver(named(PEER), Msg::Add(4));
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Add(2));
    lab.settle();
    let lifecycle = lab.lifecycle();
    assert!(lifecycle.contains(&ProcessEventKind::Escalated {
        supervisor: SupervisorId::from_raw(2),
        parent: Some(root)
    }));
    assert_eq!(lab.restarts().len(), 1, "one parent-level restart");
    assert!(!lab.scheduler.program_terminated());
    let (_, rebuilt) = *lab.started().last().unwrap();
    assert_eq!(total(rebuilt), 2);
    assert_eq!(total(lab.started()[1].1), 4, "the peer kept its state");
}

#[test]
fn es03_a_nested_never_supervisor_stays_stopped_and_its_parent_continues() {
    let mut lab = Lab::new();
    let nested = policy(vec![counter_member(COUNTER)], 0);
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![
                Member::Supervisor(nested, Restart::Never),
                counter_member(PEER),
            ],
            1,
        ))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(lab.send(named(COUNTER), Msg::Add(1)).is_err());
    assert!(lab.restarts().is_empty());
    assert_eq!(
        lab.scheduler.supervisor_state(root),
        Some(SupervisorState::Running)
    );
    assert!(!lab.scheduler.program_terminated());
    lab.deliver(named(PEER), Msg::Add(1));
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .expect("the escalated subtree freed its slot");
}

#[test]
fn es04_an_escalation_beyond_the_parents_limit_terminates_the_program() {
    let mut lab = Lab::new();
    let nested = policy(vec![counter_member(COUNTER)], 0);
    lab.scheduler
        .start_supervisor(policy(
            vec![
                Member::Supervisor(nested, Restart::OnDefect),
                counter_member(PEER),
            ],
            0,
        ))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(lab.scheduler.program_terminated());
    let escalations: Vec<_> = lab
        .lifecycle()
        .into_iter()
        .filter_map(|kind| match kind {
            ProcessEventKind::Escalated { supervisor, parent } => Some((supervisor, parent)),
            _ => None,
        })
        .collect();
    let (root, nested) = (SupervisorId::from_raw(1), SupervisorId::from_raw(2));
    assert_eq!(escalations, [(nested, Some(root)), (root, None)]);
    assert_eq!(lab.scheduler.process_metrics().incarnations, 0);
}

#[test]
fn es05_escalation_stops_newest_dynamic_first_then_declared_in_reverse() {
    let mut lab = Lab::new();
    let third = Member::Process(spec(None, 8), Restart::OnDefect);
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER), third],
            1,
        ))
        .unwrap();
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .unwrap();
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    let tail: Vec<_> = lab
        .transcript()
        .into_iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    assert_eq!(
        tail,
        lines(
            "@0s process_terminated membership=M1 incarnation=I5 cause=defected
             @0s restart_limit_exceeded supervisor=S1
             @0s process_terminated membership=M4 incarnation=I4 cause=cancelled
             @0s process_terminated membership=M3 incarnation=I3 cause=cancelled
             @0s process_terminated membership=M2 incarnation=I2 cause=cancelled
             @0s supervisor_escalated supervisor=S1 parent=root"
        )
    );
    assert!(lab.scheduler.program_terminated());
}

// ----- Removal races -----------------------------------------------------

#[test]
fn rr01_rt05_removing_a_parked_process_cancels_it_and_frees_its_name() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 2)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec(Some(COUNTER), 8), Restart::OnDefect)
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Add(5));
    lab.settle();
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.transcript()[lab.transcript().len() - 2..],
        lines(
            "@0s process_terminated membership=M1 incarnation=I1 cause=cancelled
             @0s membership_removed membership=M1"
        )
    );
    assert!(lab.send(named(COUNTER), Msg::Add(1)).is_err());
    lab.scheduler
        .start_process(root, spec(Some(COUNTER), 8), Restart::Never)
        .expect("name free");
    lab.deliver(named(COUNTER), Msg::Add(1));
}

#[test]
fn rr02_removal_before_the_replacement_runs_still_consumes_restart_budget() {
    let (p, q) = (11, 12);
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy_with(vec![], 16, 2, Duration::from_secs(30)))
        .unwrap();
    let managed_p = lab
        .scheduler
        .start_process(root, spec(Some(p), 8), Restart::OnDefect)
        .unwrap();
    lab.scheduler
        .start_process(root, spec(Some(q), 8), Restart::Never)
        .unwrap();
    lab.deliver(named(p), Msg::Crash);
    lab.deliver(named(q), Msg::Remove(root, managed_p));
    lab.sleep(MS);
    let managed_r = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .unwrap();
    let r = lab.scheduler.process_address(managed_r).unwrap();
    lab.deliver(r, Msg::Crash);
    lab.sleep(MS);
    lab.scheduler.stop_supervisor(root).unwrap();
    assert_eq!(
        lab.transcript(),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.p
             @0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.q
             @0s process_send kind=named name=app/main.p incarnation=I1 outcome=accepted
             @0s process_send kind=named name=app/main.q incarnation=I2 outcome=accepted
             @0s process_terminated membership=M1 incarnation=I1 cause=defected
             @0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1
             @0s process_started supervisor=S1 membership=M1 incarnation=I3 name=app/main.p
             @0s process_terminated membership=M1 incarnation=I3 cause=cancelled
             @0s membership_removed membership=M1
             @1ms process_started supervisor=S1 membership=M3 incarnation=I4
             @1ms process_send kind=direct incarnation=I4 outcome=accepted
             @1ms process_terminated membership=M3 incarnation=I4 cause=defected
             @1ms restart_attempted supervisor=S1 membership=M3 attempts_in_window=2
             @1ms process_started supervisor=S1 membership=M3 incarnation=I5
             @2ms process_terminated membership=M3 incarnation=I5 cause=cancelled
             @2ms process_terminated membership=M2 incarnation=I2 cause=cancelled
             @2ms supervisor_stopped supervisor=S1"
        )
    );
}

#[test]
fn rr03_a_defect_while_draining_for_removal_is_not_restarted() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 5)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec(Some(COUNTER), 8), Restart::OnDefect)
        .unwrap();
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Add(1));
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.scheduler.remove_process(root, managed).unwrap();
    assert_eq!(
        lab.lifecycle()[2..],
        [
            ProcessEventKind::Terminated {
                membership: managed,
                incarnation: IncarnationId::from_raw(1),
                cause: TerminalCause::Defected,
                stack_overflow: false,
            },
            ProcessEventKind::Removed {
                membership: managed
            },
        ]
    );
    assert_eq!(lab.scheduler.process_metrics().incarnations, 0);
}

#[test]
fn rr04_removing_a_membership_that_returned_records_only_the_removal() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .unwrap();
    let direct = lab.scheduler.process_address(managed).unwrap();
    lab.deliver(direct, Msg::Return);
    lab.settle();
    let before = lab.lifecycle().len();
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.lifecycle()[before..],
        [ProcessEventKind::Removed {
            membership: managed
        }]
    );
    assert_eq!(lab.scheduler.process_metrics().retired_handles, 0);
}

#[test]
fn rr05_removal_interrupts_a_sleeping_handler_without_advancing_time() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .unwrap();
    let direct = lab.scheduler.process_address(managed).unwrap();
    lab.deliver(direct, Msg::Sleep(Duration::from_secs(60)));
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.process_metrics().timers, 1);
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Done)
    );
    lab.settle();
    assert_eq!(lab.terminations()[0].1, TerminalCause::Cancelled);
    assert_eq!(lab.scheduler.now(), Instant::ORIGIN);
    assert_eq!(lab.scheduler.process_metrics().timers, 0);
    assert!(log(|log| log.woke.is_empty()));
}

#[test]
fn rr06_a_removal_during_stop_terminates_the_sibling_once_and_stop_skips_it() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let older = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .unwrap();
    let newest = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::OnDefect)
        .unwrap();
    lab.settle();
    let newest_address = lab.scheduler.process_address(newest).unwrap();
    lab.deliver(newest_address, Msg::Remove(root, older));
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Done));
    assert_eq!(
        lab.transcript()[4..],
        lines(
            "@0s process_terminated membership=M1 incarnation=I1 cause=cancelled
             @0s membership_removed membership=M1
             @0s process_terminated membership=M2 incarnation=I2 cause=cancelled
             @0s supervisor_stopped supervisor=S1"
        )
    );
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_OK]);
}

// ----- Shutdown and forced termination -----------------------------------

#[test]
fn sd01_stop_runs_newest_dynamic_first_then_declared_in_reverse_and_frees_unread_messages() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER)],
            2,
        ))
        .unwrap();
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .unwrap();
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .unwrap();
    // Queued before any body runs, so nothing receives it.
    let unread = message(Msg::Add(9));
    lab.scheduler.process_send(named(COUNTER), unread).unwrap();
    let memberships: Vec<_> = lab.started().into_iter().map(|(m, _)| m).collect();
    lab.scheduler.stop_supervisor(root).unwrap();
    let order: Vec<_> = lab.terminations().into_iter().map(|(m, _)| m).collect();
    assert_eq!(order, memberships.iter().rev().copied().collect::<Vec<_>>());
    assert!(
        lab.terminations()
            .iter()
            .all(|(_, cause)| *cause == TerminalCause::Cancelled)
    );
    assert_eq!(log(|log| log.released.clone()), [unread]);
    assert!(matches!(
        lab.lifecycle().last(),
        Some(ProcessEventKind::Stopped { .. })
    ));
}

fn removal_with_queued_messages(shutdown_within: Duration) -> (Lab, IncarnationId, Vec<u64>) {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let mut spec = spec(None, 8);
    spec.shutdown_within = shutdown_within;
    let managed = lab
        .scheduler
        .start_process(root, spec, Restart::OnDefect)
        .unwrap();
    lab.settle();
    let direct = lab.scheduler.process_address(managed).unwrap();
    let handles: Vec<_> = (1..=3).map(|value| message(Msg::Add(value))).collect();
    for handle in &handles {
        lab.scheduler.process_send(direct, *handle).unwrap();
    }
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Done)
    );
    let Address::Direct(incarnation) = direct else {
        unreachable!()
    };
    (lab, incarnation, handles)
}

#[test]
fn sd02_a_zero_deadline_forces_at_once_and_discards_unread_messages() {
    let (lab, incarnation, handles) = removal_with_queued_messages(Duration::ZERO);
    assert_eq!(lab.terminations()[0].1, TerminalCause::Forced);
    assert!(adds_of(incarnation).is_empty());
    assert_eq!(log(|log| log.released.clone()), handles);
    assert!(
        lab.restarts().is_empty(),
        "SD07: a forced end never restarts"
    );
}

#[test]
fn sd02_a_zero_deadline_forces_a_sleeping_handler_without_running_its_cleanup() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let mut spec = spec(None, 8);
    spec.shutdown_within = Duration::ZERO;
    let managed = lab
        .scheduler
        .start_process(root, spec, Restart::OnDefect)
        .unwrap();
    let direct = lab.scheduler.process_address(managed).unwrap();
    lab.deliver(direct, Msg::Sleep(Duration::from_secs(60)));
    lab.scheduler.drive();
    // The handler returns however its sleep ends; only a forced reply makes
    // that end `forced` rather than `cancelled`.
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(timed_terminations(&lab), [(0, TerminalCause::Forced)]);
    assert!(lab.restarts().is_empty());
}

#[test]
fn sd03_cleanup_within_its_deadline_drains_everything_and_time_stays_put() {
    let (lab, incarnation, _) = removal_with_queued_messages(Duration::from_secs(5));
    assert_eq!(lab.terminations()[0].1, TerminalCause::Cancelled);
    assert_eq!(adds_of(incarnation), [1, 2, 3]);
    assert_eq!(lab.scheduler.now(), Instant::ORIGIN);
    assert!(
        lab.restarts().is_empty(),
        "SD07: a cancelled end never restarts"
    );
}

#[test]
fn sd04_cleanup_that_parks_again_is_forced_at_its_deadline() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 3)).unwrap();
    let managed = lab
        .scheduler
        .start_process(
            root,
            spec_with(&STUBBORN, Some(COUNTER), 8),
            Restart::OnDefect,
        )
        .unwrap();
    lab.settle();
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Ok(Progress::Pending)
    );
    assert!(
        lab.send(named(COUNTER), Msg::Add(1)).is_err(),
        "unrouted while forcing"
    );
    lab.settle();
    assert_eq!(lab.scheduler.now(), Instant::from_nanos(5_000_000_000));
    assert_eq!(
        lab.transcript()[lab.transcript().len() - 2..],
        lines(
            "@5s process_terminated membership=M1 incarnation=I1 cause=forced
             @5s membership_removed membership=M1"
        )
    );
    assert!(lab.restarts().is_empty());
}

#[test]
fn sd05_each_members_children_drain_before_the_next_member_terminates() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    for _ in 0..2 {
        let managed = lab
            .scheduler
            .start_process(root, spec(None, 8), Restart::Never)
            .unwrap();
        let address = lab.scheduler.process_address(managed).unwrap();
        lab.deliver(address, Msg::SpawnSleeper);
    }
    // Drive only: settling would advance time to the sleepers' deadline.
    lab.scheduler.drive();
    lab.scheduler.stop_supervisor(root).unwrap();
    let mut order: Vec<(u64, &str)> = lab
        .scheduler
        .events()
        .iter()
        .filter(|event| event.kind == TaskEventKind::Terminal)
        .map(|event| (event.sequence, "child"))
        .chain(
            lab.scheduler
                .process_events()
                .iter()
                .filter(|event| matches!(event.kind, ProcessEventKind::Terminated { .. }))
                .map(|event| (event.sequence, "member")),
        )
        .collect();
    order.sort();
    let order: Vec<_> = order.into_iter().map(|(_, what)| what).collect();
    assert_eq!(order, ["child", "member", "child", "member"]);
}

#[test]
fn sd06_routing_is_removed_for_the_whole_subtree_before_any_cleanup() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    let newest = lab
        .scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .unwrap();
    lab.settle();
    let newest = lab.scheduler.process_address(newest).unwrap();
    // The newest member stops first; the counter it sends to is still
    // running, but already unrouted.
    lab.deliver(newest, Msg::SendTo(named(COUNTER), 1));
    lab.scheduler.stop_supervisor(root).unwrap();
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_UNAVAILABLE]);
}

#[test]
fn sd07_shutdown_of_on_defect_members_never_restarts_them() {
    let mut lab = Lab::new();
    let mut zero = spec(None, 8);
    zero.shutdown_within = Duration::ZERO;
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![
                counter_member(COUNTER),
                Member::Process(zero, Restart::OnDefect),
            ],
            5,
        ))
        .unwrap();
    lab.settle();
    lab.scheduler.stop_supervisor(root).unwrap();
    let causes: Vec<_> = lab.terminations().into_iter().map(|(_, c)| c).collect();
    assert_eq!(causes, [TerminalCause::Forced, TerminalCause::Cancelled]);
    assert!(lab.restarts().is_empty());
    assert_eq!(lab.started().len(), 2);
}

// ----- Prototype kernel decisions ----------------------------------------

#[test]
fn a_handle_outlives_a_stop_and_an_escalation_of_its_supervisor() {
    for escalate in [false, true] {
        let mut lab = Lab::new();
        let root = lab
            .scheduler
            .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
            .unwrap();
        let managed = lab
            .scheduler
            .start_process(root, spec(None, 8), Restart::Never)
            .unwrap();
        let direct = lab.scheduler.process_address(managed).unwrap();
        if escalate {
            lab.deliver(named(COUNTER), Msg::Crash);
            lab.settle();
            assert!(lab.scheduler.program_terminated());
        } else {
            lab.scheduler.stop_supervisor(root).unwrap();
        }
        assert_eq!(lab.scheduler.process_address(managed), Ok(direct));
        assert!(lab.send(direct, Msg::Add(1)).is_err());
        let events = lab.lifecycle().len();
        assert_eq!(
            lab.scheduler.remove_process(root, managed),
            Ok(Progress::Done)
        );
        assert_eq!(
            lab.lifecycle().len(),
            events,
            "removing an ended membership records nothing"
        );
    }
}

#[test]
fn membership_bookkeeping_does_not_grow_with_ended_removed_or_rebuilt_members() {
    let mut lab = Lab::traced(TraceMode::Off);
    let ends = Member::Process(spec(Some(PEER), 8), Restart::Never);
    let nested = policy(vec![counter_member(COUNTER), ends], 0);
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![Member::Supervisor(nested, Restart::OnDefect)],
            1_000_000,
        ))
        .unwrap();
    lab.settle();
    let baseline = (
        lab.scheduler.process_metrics(),
        lab.scheduler.metrics().tasks,
    );
    for _ in 0..300 {
        lab.deliver(named(PEER), Msg::Crash);
        lab.settle();
        lab.deliver(named(COUNTER), Msg::Crash);
        lab.settle();
        let removed = lab
            .scheduler
            .start_process(root, spec(None, 8), Restart::Never)
            .unwrap();
        let ended = lab
            .scheduler
            .start_process(root, spec(Some(THIRD), 8), Restart::Never)
            .unwrap();
        lab.deliver(named(THIRD), Msg::Crash);
        lab.settle();
        lab.scheduler.remove_process(root, removed).unwrap();
        lab.scheduler.remove_process(root, ended).unwrap();
    }
    assert!(!lab.scheduler.program_terminated());
    assert_eq!(
        (
            lab.scheduler.process_metrics(),
            lab.scheduler.metrics().tasks
        ),
        baseline
    );
}

#[test]
fn a_process_handles_a_million_messages_in_bounded_bookkeeping() {
    let mut lab = Lab::traced(TraceMode::Off);
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    let handle = message(Msg::Add(1));
    let mut after_warmup = None;
    for batch in 0..15_625 {
        for _ in 0..64 {
            lab.scheduler.process_send(named(COUNTER), handle).unwrap();
        }
        lab.settle();
        if batch == 100 {
            after_warmup = Some((lab.scheduler.process_metrics(), lab.scheduler.metrics()));
        }
    }
    assert_eq!(
        Some((lab.scheduler.process_metrics(), lab.scheduler.metrics())),
        after_warmup
    );
    assert_eq!(total(lab.started_quiet()), 1_000_000);
    assert_eq!(lab.scheduler.constructed_events(), 0);
}

impl Lab {
    /// The only incarnation when tracing is off.
    fn started_quiet(&self) -> IncarnationId {
        IncarnationId::from_raw(1)
    }
}

#[test]
fn a_stack_overflowing_body_is_a_contained_defect() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(
            vec![counter_member(COUNTER), counter_member(PEER)],
            1,
        ))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Overflow);
    lab.deliver(named(PEER), Msg::Add(4));
    lab.settle();
    assert_eq!(lab.terminations()[0].1, TerminalCause::Defected);
    assert_eq!(
        lab.started().len(),
        3,
        "the overflowing member restarted once"
    );
    assert_eq!(total(lab.started()[1].1), 4);
}

#[test]
fn spec_contexts_are_released_once_when_no_record_needs_them() {
    static A: Behavior = Behavior {
        on_cancel: OnCancel::Return,
        greet: None,
    };
    static B: Behavior = Behavior {
        on_cancel: OnCancel::Return,
        greet: None,
    };
    let a = behavior_context(&A) as usize;
    let b = behavior_context(&B) as usize;
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![Member::Process(
                spec_with(&A, Some(COUNTER), 8),
                Restart::OnDefect,
            )],
            1,
        ))
        .unwrap();
    let taken = lab
        .scheduler
        .start_process(root, spec_with(&B, Some(COUNTER), 8), Restart::Never);
    assert!(taken.is_err());
    assert_eq!(
        log(|log| log.released_specs.clone()),
        [b],
        "a rejected spec is released"
    );
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert_eq!(
        log(|log| log.released_specs.len()),
        1,
        "a restart keeps the declared spec"
    );
    lab.scheduler.stop_supervisor(root).unwrap();
    assert_eq!(log(|log| log.released_specs.clone()), [b, a]);
}

// ----- Clock and timers --------------------------------------------------

/// Reads the clock repeatedly; every read yields to the scheduler.
unsafe extern "C" fn busy_reader(_context: *mut c_void) -> ChimiFiberResult {
    let mut latest = 0;
    for _ in 0..1_000 {
        latest = latest.max(now());
    }
    ended(latest)
}

#[test]
fn virtual_time_advances_only_when_nothing_is_runnable() {
    let mut lab = Lab::new();
    let sleeper = lab
        .scheduler
        .start_root_with_context(recording_sleeper, 10_000_000 as *mut c_void)
        .unwrap();
    // The sleeper's timer is pending while the reader is runnable.
    lab.scheduler.drive();
    let reader = lab.scheduler.start_root(busy_reader).unwrap();
    assert_eq!(lab.scheduler.run_until_idle(), Idle::Quiescent);
    assert_eq!(
        lab.scheduler.result(None, reader),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(0)))
    );
    assert!(matches!(
        lab.scheduler.result(None, sleeper),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(0)))
    ));
    assert_eq!(log(|log| log.woke.clone()), [(0, 10_000_000)]);
    assert!(lab.scheduler.clock_events().iter().any(|event| {
        event.kind == ClockEventKind::Advanced && event.time == Instant::from_nanos(10_000_000)
    }));
}

#[test]
fn timers_fire_earliest_first_and_in_registration_order_at_one_deadline() {
    let mut lab = Lab::new();
    for deadline in [20_000_000_usize, 20_000_000, 20_000_000, 5_000_000] {
        lab.scheduler
            .start_root_with_context(recording_sleeper, deadline as *mut c_void)
            .unwrap();
    }
    lab.settle();
    let woke: Vec<_> = log(|log| log.woke.clone());
    assert_eq!(
        woke.iter().map(|(_, at)| *at).collect::<Vec<_>>(),
        [5_000_000, 20_000_000, 20_000_000, 20_000_000]
    );
    let finished: Vec<_> = lab
        .scheduler
        .events()
        .iter()
        .filter(|event| event.kind == TaskEventKind::Terminal)
        .map(|event| event.task.get())
        .collect();
    assert_eq!(finished, [4, 1, 2, 3]);
}

unsafe extern "C" fn parks_forever(_context: *mut c_void) -> ChimiFiberResult {
    unsafe { chimi_fiber_park(ptr::null()) };
    ended(0)
}

#[test]
fn a_virtual_deadlock_names_each_parked_task_and_defects_the_root() {
    let mut lab = Lab::new();
    let stuck = lab.scheduler.start_root(parks_forever).unwrap();
    let sleeping = lab
        .scheduler
        .start_root_with_context(recording_sleeper, 1_000_000 as *mut c_void)
        .unwrap();
    assert_eq!(
        lab.scheduler.run_until_idle(),
        Idle::Deadlock(vec![ParkedTask {
            task: stuck,
            request_kind: 0
        }])
    );
    assert!(matches!(
        lab.scheduler.result(None, sleeping),
        Ok(TaskResult::Ready(_))
    ));
    let run = lab.scheduler.run_root(stuck, DEADLOCK_DEFECT_CODE);
    assert_eq!(
        (run.outcome.state, run.outcome.defect_code),
        (ChimiFiberState::Defected as u32, DEADLOCK_DEFECT_CODE)
    );
    assert_eq!(
        run.deadlock,
        [ParkedTask {
            task: stuck,
            request_kind: 0
        }],
        "the root's defect names each parked task and what it waits on"
    );
}

#[test]
fn a_host_operation_in_flight_holds_virtual_time() {
    let mut lab = Lab::new();
    lab.scheduler
        .start_root_with_context(recording_sleeper, 10_000_000 as *mut c_void)
        .unwrap();
    lab.scheduler.begin_host_operation();
    assert_eq!(lab.scheduler.run_until_idle(), Idle::AwaitingHost);
    assert_eq!(lab.scheduler.now(), Instant::ORIGIN);
    lab.scheduler.end_host_operation();
    lab.settle();
    assert_eq!(lab.scheduler.now(), Instant::from_nanos(10_000_000));
}

#[test]
fn real_time_sleeps_until_the_timer() {
    LOG.with(|log| *log.borrow_mut() = Log::default());
    let mut scheduler = Scheduler::with_options(SchedulerOptions::new(
        STACK,
        TraceMode::Off,
        ClockMode::Real,
    ));
    let deadline = scheduler.now().saturating_add(Duration::from_millis(20));
    scheduler
        .start_root_with_context(
            recording_sleeper,
            deadline.as_nanos() as usize as *mut c_void,
        )
        .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(scheduler.run_until_idle(), Idle::Quiescent);
    assert!(started.elapsed() >= Duration::from_millis(15));
    assert!(log(|log| log.woke[0].1) >= deadline.as_nanos());
}

#[test]
fn cancelling_a_sleeper_removes_its_timer_and_time_never_reaches_it() {
    let mut lab = Lab::new();
    let sleeper = lab
        .scheduler
        .start_root_with_context(recording_sleeper, 60_000_000_000 as *mut c_void)
        .unwrap();
    lab.scheduler.drive();
    lab.scheduler.cancel(None, sleeper).unwrap();
    lab.settle();
    assert_eq!(lab.scheduler.now(), Instant::ORIGIN);
    assert!(log(|log| log.woke.is_empty()));
    assert_eq!(lab.scheduler.process_metrics().timers, 0);
}

// ----- C entry fixtures --------------------------------------------------

unsafe extern "C" {
    fn chimi_fixture_es01_main(context: *mut c_void) -> ChimiFiberResult;
    fn chimi_fixture_sd04_main(context: *mut c_void) -> ChimiFiberResult;
    fn chimi_fixture_deadlocked_main(context: *mut c_void) -> ChimiFiberResult;
    fn chimi_fixture_sd04_reset();
    fn chimi_fixture_sd04_ran_after_forced() -> i32;
    fn chimi_fixture_sd04_forced_replies() -> i32;
    fn chimi_fixture_process_layouts(out: *mut usize) -> usize;
}

thread_local! {
    static DEADLOCK: RefCell<Vec<ChimiDeadlockWaiter>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn collect_deadlock(
    _context: *mut c_void,
    waiters: *const ChimiDeadlockWaiter,
    count: usize,
) {
    let waiters = unsafe { std::slice::from_raw_parts(waiters, count) };
    DEADLOCK.with(|deadlock| deadlock.borrow_mut().extend_from_slice(waiters));
}

unsafe extern "C" fn collect_trace(context: *mut c_void, event: *const SchedulerTraceEvent) {
    let events = unsafe { &mut *context.cast::<Vec<SchedulerTraceEvent>>() };
    events.push(unsafe { std::ptr::read(event) });
}

fn run_c(entry: ChimiFiberEntry, trace_mode: u32) -> (SchedulerOutcome, Vec<SchedulerTraceEvent>) {
    let mut events: Vec<SchedulerTraceEvent> = Vec::new();
    let config = ChimiSchedulerConfig {
        entry: Some(entry),
        context: ptr::null_mut(),
        stack_bytes: STACK,
        trace_mode,
        clock_mode: CLOCK_VIRTUAL,
        trace_sink: Some(collect_trace),
        trace_context: (&mut events as *mut Vec<SchedulerTraceEvent>).cast(),
        release_message: None,
        release_spec_context: None,
        release_context: ptr::null_mut(),
        deadlock_report: Some(collect_deadlock),
        deadlock_context: ptr::null_mut(),
        release_task_value: None,
        release_channel_value: None,
        release_task_context: None,
        capability_violation_report: None,
        capability_violation_context: ptr::null_mut(),
    };
    let outcome = unsafe { chimi_scheduler_run(&config) };
    (outcome, events)
}

const ES01: &str = "
@0s supervisor_started supervisor=S1 parent=root
@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
@0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.peer
@0s process_send kind=named name=app/main.counter incarnation=I1 outcome=accepted
@0s process_terminated membership=M1 incarnation=I1 cause=defected
@0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1
@0s process_started supervisor=S1 membership=M1 incarnation=I3 name=app/main.counter
@1ms process_send kind=named name=app/main.counter incarnation=I3 outcome=accepted
@1ms process_terminated membership=M1 incarnation=I3 cause=defected
@1ms restart_limit_exceeded supervisor=S1
@1ms process_terminated membership=M2 incarnation=I2 cause=cancelled
@1ms supervisor_escalated supervisor=S1 parent=root
";

#[test]
fn es01_ob01_ob02_ob05_the_c_entry_reproduces_the_root_escalation_transcript() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let (outcome, full) = run_c(chimi_fixture_es01_main, 2);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, RESTART_LIMIT_DEFECT_CODE)
    );
    assert_eq!(render_trace(&full), lines(ES01));
    let sequences: Vec<_> = full.iter().map(|event| event.sequence).collect();
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "one strictly increasing sequence across families"
    );
    assert!(
        full.iter()
            .any(|event| event.family == SCHEDULER_TRACE_TASK)
    );
    assert!(full.iter().any(|event| {
        event.family == SCHEDULER_TRACE_CLOCK && event.kind == ClockEventKind::Advanced as u32 + 1
    }));

    let (_, boundaries) = run_c(chimi_fixture_es01_main, 1);
    let without_sends: Vec<_> = lines(ES01)
        .into_iter()
        .filter(|line| !line.contains("process_send"))
        .collect();
    assert_eq!(render_trace(&boundaries), without_sends);

    let (off_outcome, off) = run_c(chimi_fixture_es01_main, 0);
    assert!(off.is_empty());
    assert_eq!(off_outcome.defect_code, RESTART_LIMIT_DEFECT_CODE);
}

#[test]
fn sd04_the_c_entry_forces_cleanup_that_parks_again_and_never_resumes_it() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    unsafe { chimi_fixture_sd04_reset() };
    let (outcome, events) = run_c(chimi_fixture_sd04_main, 1);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 5_001_000_000),
        "the stop returned at the deadline"
    );
    assert_eq!(
        render_trace(&events),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1
             @5.001s process_terminated membership=M1 incarnation=I1 cause=forced
             @5.001s supervisor_stopped supervisor=S1"
        )
    );
    assert_eq!(unsafe { chimi_fixture_sd04_forced_replies() }, 1);
    assert_eq!(unsafe { chimi_fixture_sd04_ran_after_forced() }, 0);
}

#[test]
fn a_c_root_with_nothing_to_wake_it_defects_with_the_deadlock_code() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    DEADLOCK.with(|deadlock| deadlock.borrow_mut().clear());
    let (outcome, events) = run_c(chimi_fixture_deadlocked_main, 1);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, DEADLOCK_DEFECT_CODE)
    );
    assert_eq!(
        DEADLOCK.with(|deadlock| deadlock.borrow().clone()),
        [ChimiDeadlockWaiter {
            task: 1,
            request_kind: 0
        }],
        "the deadlock report reaches the C caller without a trace sink"
    );
    assert!(events.iter().any(|event| {
        event.family == SCHEDULER_TRACE_CLOCK
            && event.kind == ClockEventKind::DeadlockWaiter as u32 + 1
    }));
}

#[test]
fn ob07_the_process_abi_layouts_match_the_c_header() {
    use std::mem::{offset_of, size_of};
    let mut c = [0_usize; 96];
    let count = unsafe { chimi_fixture_process_layouts(c.as_mut_ptr()) };
    let rust = [
        size_of::<ChimiProcessRequest>(),
        offset_of!(ChimiProcessRequest, status),
        offset_of!(ChimiProcessRequest, address_kind),
        offset_of!(ChimiProcessRequest, restart),
        offset_of!(ChimiProcessRequest, address),
        offset_of!(ChimiProcessRequest, supervisor),
        offset_of!(ChimiProcessRequest, membership),
        offset_of!(ChimiProcessRequest, message),
        offset_of!(ChimiProcessRequest, name),
        offset_of!(ChimiProcessRequest, owner),
        offset_of!(ChimiProcessRequest, policy),
        offset_of!(ChimiProcessRequest, spec),
        size_of::<ChimiProcessSpec>(),
        offset_of!(ChimiProcessSpec, context),
        offset_of!(ChimiProcessSpec, mailbox_capacity),
        offset_of!(ChimiProcessSpec, shutdown_within),
        offset_of!(ChimiProcessSpec, name),
        size_of::<ChimiSupervisorMember>(),
        offset_of!(ChimiSupervisorMember, restart),
        offset_of!(ChimiSupervisorMember, process),
        offset_of!(ChimiSupervisorMember, supervisor),
        size_of::<ChimiSupervisorPolicy>(),
        offset_of!(ChimiSupervisorPolicy, member_count),
        offset_of!(ChimiSupervisorPolicy, capacity),
        offset_of!(ChimiSupervisorPolicy, restart_limit),
        offset_of!(ChimiSupervisorPolicy, restart_within),
        size_of::<SchedulerTraceEvent>(),
        offset_of!(SchedulerTraceEvent, discard_count),
        offset_of!(SchedulerTraceEvent, supervisor),
        offset_of!(SchedulerTraceEvent, membership),
        offset_of!(SchedulerTraceEvent, incarnation),
        offset_of!(SchedulerTraceEvent, name),
        offset_of!(SchedulerTraceEvent, parent_supervisor),
        offset_of!(SchedulerTraceEvent, attempts_in_window),
        offset_of!(SchedulerTraceEvent, time),
        offset_of!(SchedulerTraceEvent, address_kind),
        offset_of!(SchedulerTraceEvent, outcome),
        size_of::<ChimiSchedulerConfig>(),
        offset_of!(ChimiSchedulerConfig, stack_bytes),
        offset_of!(ChimiSchedulerConfig, trace_mode),
        offset_of!(ChimiSchedulerConfig, clock_mode),
        offset_of!(ChimiSchedulerConfig, trace_sink),
        offset_of!(ChimiSchedulerConfig, trace_context),
        offset_of!(ChimiSchedulerConfig, release_message),
        offset_of!(ChimiSchedulerConfig, release_spec_context),
        offset_of!(ChimiSchedulerConfig, release_context),
        offset_of!(ChimiSchedulerConfig, deadlock_report),
        offset_of!(ChimiSchedulerConfig, deadlock_context),
        offset_of!(ChimiSchedulerConfig, release_task_value),
        offset_of!(ChimiSchedulerConfig, release_channel_value),
        offset_of!(ChimiSchedulerConfig, release_task_context),
        offset_of!(ChimiSchedulerConfig, capability_violation_report),
        offset_of!(ChimiSchedulerConfig, capability_violation_context),
        size_of::<ChimiDeadlockWaiter>(),
        offset_of!(ChimiDeadlockWaiter, request_kind),
        size_of::<ChimiCapabilityViolation>(),
        offset_of!(ChimiCapabilityViolation, channel),
        offset_of!(ChimiCapabilityViolation, owner),
        offset_of!(ChimiCapabilityViolation, destination),
        offset_of!(ChimiCapabilityViolation, request_kind),
        offset_of!(ChimiCapabilityViolation, capability),
        offset_of!(ChimiCapabilityViolation, reason),
        VIOLATION_WRONG_OWNER as usize,
        VIOLATION_CHANNEL_STILL_LIVE as usize,
        CAPABILITY_RECEIVER as usize,
        CAPABILITY_COMPLETION as usize,
        REQUEST_SUPERVISOR_START_SUPERVISOR as usize,
        REQUEST_SUPERVISOR_REMOVE_SUPERVISOR as usize,
        ProcessEventKind::SupervisorStarted {
            supervisor: SupervisorId::from_raw(1),
            parent: None,
            membership: None,
        }
        .trace_kind() as usize,
    ];
    assert_eq!(&c[..count], &rust[..]);
    assert_eq!(crate::chimi_fiber_abi_version(), 6);
}

// ----- Review regressions (ts-c1f62f, ts-3d5b38) --------------------------

fn process_spec(entry: ChimiFiberEntry, context: *mut c_void) -> ProcessSpec {
    ProcessSpec {
        entry,
        context,
        mailbox_capacity: NonZeroU32::new(4).expect("positive capacity"),
        shutdown_within: Duration::from_secs(5),
        name: None,
    }
}

/// Terminations with the time they happened at.
fn timed_terminations(lab: &Lab) -> Vec<(i64, TerminalCause)> {
    lab.scheduler
        .process_events()
        .iter()
        .filter_map(|event| match event.kind {
            ProcessEventKind::Terminated { cause, .. } => Some((event.time.as_nanos(), cause)),
            _ => None,
        })
        .collect()
}

fn incarnation_task(lab: &Lab) -> TaskId {
    let mut tasks = lab
        .scheduler
        .tasks
        .iter()
        .filter(|(_, record)| record.process.is_some())
        .map(|(task, _)| *task);
    let task = tasks.next().expect("an incarnation task");
    assert!(tasks.next().is_none(), "exactly one incarnation");
    task
}

/// Cleanup that ignores cancellation: it sleeps again until forced and
/// records the status of every sleep.
fn stubborn_cleanup() -> ChimiFiberResult {
    loop {
        let status = sleep_until(i64::MAX);
        log(|log| log.outcomes.push(status));
        if status == RESPONSE_FORCED {
            return ended(PROCESS_END_RETURNED);
        }
    }
}

unsafe extern "C" fn awaits_a_task_it_does_not_own(_context: *mut c_void) -> ChimiFiberResult {
    let mut request = blank_request(REQUEST_TASK_RESULT);
    request.task = 999;
    scheduler_call(&mut request);
    ended(0)
}

unsafe extern "C" fn sends_on_an_unknown_channel(_context: *mut c_void) -> ChimiFiberResult {
    let mut request = blank_request(REQUEST_CHANNEL_SEND);
    request.channel = 999;
    scheduler_call(&mut request);
    ended(0)
}

#[test]
fn a_contract_violation_defects_its_caller_without_writing_into_the_destroyed_stack() {
    let cases: [(ChimiFiberEntry, i64); 2] = [
        (
            awaits_a_task_it_does_not_own,
            CAPABILITY_VIOLATION_DEFECT_CODE,
        ),
        (
            sends_on_an_unknown_channel,
            CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE,
        ),
    ];
    for (entry, code) in cases {
        let mut scheduler = Scheduler::new(STACK, TraceMode::Off);
        let root = scheduler.start_root(entry).expect("root");
        scheduler.drive();
        assert_eq!(
            scheduler.result(None, root),
            Ok(TaskResult::Ready(TaskOutcome::Defected {
                origin: root,
                code
            }))
        );
    }
}

/// Starts a child that sleeps 1ms, waits for its result, and on
/// cancellation runs stubborn cleanup.
unsafe extern "C" fn awaits_a_sleeping_child(_context: *mut c_void) -> ChimiFiberResult {
    let mut start = blank_request(REQUEST_TASK_START);
    start.entry = Some(host_sleeper);
    start.context = 1_000_000usize as *mut c_void;
    scheduler_call(&mut start);
    assert_eq!(start.status, RESPONSE_OK);
    loop {
        let mut request = blank_request(REQUEST_TASK_RESULT);
        request.task = start.task;
        if scheduler_call(&mut request) == ChimiFiberCommand::Cancel {
            return stubborn_cleanup();
        }
        if request.status != RESPONSE_PENDING {
            return ended(PROCESS_END_RETURNED);
        }
    }
}

#[test]
fn shutdown_drains_child_tasks_so_a_finishing_child_cannot_wake_cleanup() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("root");
    lab.scheduler
        .start_process(
            root,
            process_spec(awaits_a_sleeping_child, ptr::null_mut()),
            Restart::Never,
        )
        .expect("admitted");
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Pending));
    let child_ended = lab
        .scheduler
        .events()
        .iter()
        .any(|event| event.kind == TaskEventKind::Terminal);
    assert!(child_ended, "the child is cancelled when shutdown starts");
    let _ = lab.scheduler.run_until_idle();
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_FORCED]);
    assert_eq!(
        timed_terminations(&lab),
        [(5_000_000_000, TerminalCause::Forced)]
    );
}

/// A child that defects however its sleep ends.
unsafe extern "C" fn defects_after_sleeping(_context: *mut c_void) -> ChimiFiberResult {
    sleep_until(1_000_000);
    defected()
}

/// Waits for a child that defects; context 1 makes its cleanup stubborn.
unsafe extern "C" fn awaits_a_defecting_child(context: *mut c_void) -> ChimiFiberResult {
    let mut start = blank_request(REQUEST_TASK_START);
    start.entry = Some(defects_after_sleeping);
    scheduler_call(&mut start);
    assert_eq!(start.status, RESPONSE_OK);
    let mut request = blank_request(REQUEST_TASK_RESULT);
    request.task = start.task;
    if scheduler_call(&mut request) != ChimiFiberCommand::Cancel {
        return ended(PROCESS_END_RETURNED);
    }
    if context as usize == 1 {
        return stubborn_cleanup();
    }
    ended(PROCESS_END_RETURNED)
}

#[test]
fn a_child_defect_during_shutdown_stays_inside_the_shutdown_protocol() {
    for (stubborn, expected) in [
        (0_usize, (0, TerminalCause::Defected)),
        (1, (5_000_000_000, TerminalCause::Forced)),
    ] {
        let mut lab = Lab::new();
        let root = lab
            .scheduler
            .start_supervisor(policy(vec![], 3))
            .expect("root");
        let spec = process_spec(awaits_a_defecting_child, stubborn as *mut c_void);
        lab.scheduler
            .start_process(root, spec, Restart::OnDefect)
            .expect("admitted");
        lab.scheduler.drive();
        let _ = lab.scheduler.stop_supervisor(root);
        let _ = lab.scheduler.run_until_idle();
        assert_eq!(timed_terminations(&lab), [expected]);
        assert!(lab.restarts().is_empty(), "shutdown never restarts");
        assert_eq!(
            lab.scheduler.supervisor_state(root),
            Some(SupervisorState::Stopped)
        );
    }
}

thread_local! {
    static CHANNEL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Receives on a channel it creates; on cancellation runs stubborn cleanup.
unsafe extern "C" fn receives_on_its_channel(_context: *mut c_void) -> ChimiFiberResult {
    let mut bounded = blank_request(REQUEST_CHANNEL_BOUNDED);
    bounded.value = 1;
    scheduler_call(&mut bounded);
    assert_eq!(bounded.status, RESPONSE_OK);
    CHANNEL.with(|channel| channel.set(bounded.channel));
    loop {
        let mut request = blank_request(REQUEST_CHANNEL_RECEIVE);
        request.channel = bounded.channel;
        request.owner = bounded.owner;
        request.token = bounded.token;
        if scheduler_call(&mut request) == ChimiFiberCommand::Cancel {
            return stubborn_cleanup();
        }
        if request.status != RESPONSE_PENDING {
            return ended(PROCESS_END_RETURNED);
        }
    }
}

/// Sleeps 1ms, then sends 5 on the recorded channel.
unsafe extern "C" fn sends_later(_context: *mut c_void) -> ChimiFiberResult {
    sleep_until(1_000_000);
    let mut request = blank_request(REQUEST_CHANNEL_SEND);
    request.channel = CHANNEL.with(std::cell::Cell::get);
    request.value = 5;
    while scheduler_call(&mut request) == ChimiFiberCommand::Resume
        && request.status == RESPONSE_PENDING
    {}
    ended(i64::from(request.status))
}

#[test]
fn shutdown_cancels_a_pending_channel_receive_so_a_later_send_does_not_reach_cleanup() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("root");
    lab.scheduler
        .start_process(
            root,
            process_spec(receives_on_its_channel, ptr::null_mut()),
            Restart::Never,
        )
        .expect("admitted");
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Pending));
    let sender = lab.scheduler.start_root(sends_later).expect("sender");
    let _ = lab.scheduler.run_until_idle();
    assert_eq!(
        lab.scheduler.result(None, sender),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(i64::from(
            RESPONSE_OK
        )))),
        "the value is queued, not handed to the interrupted receive"
    );
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_FORCED]);
    assert_eq!(
        timed_terminations(&lab),
        [(5_000_000_000, TerminalCause::Forced)]
    );
}

/// Parks bare until the host wakes it, then waits on the supervisor in its
/// context and records the reply.
unsafe extern "C" fn parks_then_waits(context: *mut c_void) -> ChimiFiberResult {
    unsafe { chimi_fiber_park(ptr::null()) };
    let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_WAIT);
    request.supervisor = context as u64;
    process_call(&mut request);
    log(|log| log.outcomes.push(request.status));
    ended(PROCESS_END_RETURNED)
}

#[test]
fn an_incarnation_runnable_when_shutdown_begins_is_cancelled_at_its_next_park() {
    let mut lab = Lab::new();
    let other = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("other");
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("root");
    let spec = process_spec(parks_then_waits, other.get() as usize as *mut c_void);
    lab.scheduler
        .start_process(root, spec, Restart::Never)
        .expect("admitted");
    lab.scheduler.drive();
    let task = incarnation_task(&lab);
    lab.scheduler.wake(task).expect("host wake");
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Done));
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_CANCELLED]);
    assert_eq!(timed_terminations(&lab), [(0, TerminalCause::Cancelled)]);
}

thread_local! {
    static REMOVE_TARGET: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// Removes the recorded membership; if cancelled while waiting, cleanup
/// sleeps until forced and records the status.
unsafe extern "C" fn remover(_context: *mut c_void) -> ChimiFiberResult {
    let (supervisor, membership) = REMOVE_TARGET.with(std::cell::Cell::get);
    let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_REMOVE_PROCESS);
    request.supervisor = supervisor;
    request.membership = membership;
    if supervisor_call(request).is_some() {
        return ended(PROCESS_END_RETURNED);
    }
    let status = sleep_until(i64::MAX);
    log(|log| log.outcomes.push(status));
    ended(PROCESS_END_RETURNED)
}

#[test]
fn a_removal_reply_for_an_interrupted_park_never_reaches_a_later_park() {
    let mut lab = Lab::new();
    let a = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("a");
    let b = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("b");
    let mut stubborn = spec_with(&STUBBORN, None, 4);
    stubborn.shutdown_within = Duration::from_secs(1);
    let target = lab
        .scheduler
        .start_process(a, stubborn, Restart::Never)
        .expect("target");
    REMOVE_TARGET.with(|cell| cell.set((a.get(), target.get())));
    lab.scheduler
        .start_process(b, process_spec(remover, ptr::null_mut()), Restart::Never)
        .expect("remover");
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.stop_supervisor(b), Ok(Progress::Pending));
    let _ = lab.scheduler.run_until_idle();
    // The removal finishes at 1s, while the remover's cleanup is parked in
    // a later sleep; that sleep ends only at the remover's own deadline.
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_FORCED]);
    assert_eq!(
        timed_terminations(&lab),
        [
            (1_000_000_000, TerminalCause::Forced),
            (5_000_000_000, TerminalCause::Forced)
        ]
    );
}

/// Issues malformed start requests whose contexts are the values 0x11..
/// and records each status. The context is a running supervisor.
unsafe extern "C" fn hands_over_contexts_in_bad_requests(context: *mut c_void) -> ChimiFiberResult {
    let spec = |tag: usize, mailbox_capacity: u32, shutdown_within: i64| ChimiProcessSpec {
        entry: Some(worker),
        context: tag as *mut c_void,
        mailbox_capacity,
        shutdown_within,
        name: 0,
    };
    let start_process = |supervisor: u64, restart: u32, spec: ChimiProcessSpec| {
        let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START_PROCESS);
        request.supervisor = supervisor;
        request.restart = restart;
        request.spec = &spec;
        supervisor_call(request).expect("not cancelled").status
    };
    let supervisor = context as u64;
    let mut statuses = vec![
        start_process(supervisor, 0, spec(0x11, 4, 0)),
        start_process(999, RESTART_ON_DEFECT, spec(0x12, 4, 0)),
        start_process(supervisor, RESTART_ON_DEFECT, spec(0x13, 0, 0)),
    ];
    let process = |spec: ChimiProcessSpec| ChimiSupervisorMember {
        kind: MEMBER_PROCESS,
        restart: RESTART_ON_DEFECT,
        process: spec,
        supervisor: ptr::null(),
    };
    // Parsing fails at the second member, after the first was read.
    let flat = [process(spec(0x14, 4, 0)), process(spec(0x15, 4, -1))];
    let inner_members = [process(spec(0x16, 4, 0))];
    let inner = ChimiSupervisorPolicy {
        members: inner_members.as_ptr(),
        member_count: 1,
        capacity: 1,
        restart_limit: 1,
        restart_within: 1,
    };
    let nested = [ChimiSupervisorMember {
        kind: MEMBER_SUPERVISOR,
        restart: RESTART_ON_DEFECT,
        process: spec(0, 0, 0),
        supervisor: &inner,
    }];
    // The nested policy parses; the outer restart window does not.
    let policies = [
        ChimiSupervisorPolicy {
            members: flat.as_ptr(),
            member_count: 2,
            capacity: 4,
            restart_limit: 1,
            restart_within: 1,
        },
        ChimiSupervisorPolicy {
            members: nested.as_ptr(),
            member_count: 1,
            capacity: 4,
            restart_limit: 1,
            restart_within: 0,
        },
    ];
    for policy in &policies {
        let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START);
        request.policy = policy;
        statuses.push(supervisor_call(request).expect("not cancelled").status);
    }
    log(|log| log.outcomes.extend(statuses));
    ended(0)
}

#[test]
fn every_error_reply_releases_each_context_it_was_handed_once() {
    let mut lab = Lab::new();
    let supervisor = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("root");
    let main = lab
        .scheduler
        .start_root_with_context(
            hands_over_contexts_in_bad_requests,
            supervisor.get() as usize as *mut c_void,
        )
        .expect("main");
    lab.scheduler.drive();
    assert!(matches!(
        lab.scheduler.result(None, main),
        Ok(TaskResult::Ready(_))
    ));
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_ERROR; 5]);
    let mut released = log(|log| log.released_specs.clone());
    released.sort_unstable();
    assert_eq!(released, [0x11, 0x12, 0x13, 0x14, 0x15, 0x16]);
    assert!(lab.started().is_empty());
}

#[test]
fn a_real_clock_root_with_nothing_pending_waits_for_the_host_instead_of_defecting() {
    let mut scheduler = Scheduler::new(STACK, TraceMode::Off);
    let root = scheduler.start_root(parks_forever).expect("root");
    let first = scheduler.run_root(root, DEADLOCK_DEFECT_CODE).outcome;
    assert_eq!(first.state, ChimiFiberState::Parked as u32);
    scheduler.wake(root).expect("a host event");
    let second = scheduler.run_root(root, DEADLOCK_DEFECT_CODE).outcome;
    assert_eq!(
        (second.state, second.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
}

#[test]
fn process_started_trace_events_carry_the_task_that_parents_its_child_tasks() {
    let mut lab = Lab::new();
    let mut events: Vec<SchedulerTraceEvent> = Vec::new();
    lab.scheduler.set_trace_sink(
        Some(collect_trace),
        (&mut events as *mut Vec<SchedulerTraceEvent>).cast(),
    );
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![], 3))
        .expect("root");
    let managed = lab
        .scheduler
        .start_process(root, spec(None, 4), Restart::Never)
        .expect("admitted");
    let address = lab.scheduler.process_address(managed).expect("address");
    lab.deliver(address, Msg::SpawnSleeper);
    lab.scheduler.drive();
    lab.scheduler.set_trace_sink(None, ptr::null_mut());
    let started = events
        .iter()
        .find(|event| event.family == SCHEDULER_TRACE_PROCESS && event.kind == 1)
        .expect("process_started");
    let child = events
        .iter()
        .find(|event| {
            event.family == SCHEDULER_TRACE_TASK
                && event.kind == TaskEventKind::Started as u32 + 1
                && event.parent != 0
        })
        .expect("the child task started");
    assert_ne!(started.task, 0);
    assert_eq!(started.task, child.parent);
}

/// Starts a root supervisor with one member, then returns its identity.
unsafe extern "C" fn starts_a_supervisor_and_returns(_context: *mut c_void) -> ChimiFiberResult {
    let member = ChimiSupervisorMember {
        kind: MEMBER_PROCESS,
        restart: RESTART_ON_DEFECT,
        process: ChimiProcessSpec {
            entry: Some(worker),
            context: behavior_context(&PLAIN),
            mailbox_capacity: 4,
            shutdown_within: 5_000_000_000,
            name: 0,
        },
        supervisor: ptr::null(),
    };
    let policy = ChimiSupervisorPolicy {
        members: &member,
        member_count: 1,
        capacity: 4,
        restart_limit: 1,
        restart_within: 1_000_000_000,
    };
    let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START);
    request.policy = &policy;
    let reply = supervisor_call(request).expect("not cancelled");
    assert_eq!(reply.status, RESPONSE_OK);
    ended(reply.supervisor as i64)
}

#[test]
fn a_root_supervisor_stops_when_the_task_that_started_it_ends() {
    let mut lab = Lab::new();
    let main = lab
        .scheduler
        .start_root(starts_a_supervisor_and_returns)
        .expect("main");
    lab.scheduler.drive();
    let Ok(TaskResult::Ready(TaskOutcome::Succeeded(raw))) = lab.scheduler.result(None, main)
    else {
        panic!("main returned its supervisor");
    };
    let supervisor = SupervisorId::from_raw(raw as u64);
    assert_eq!(
        lab.scheduler.supervisor_state(supervisor),
        Some(SupervisorState::Stopped)
    );
    assert_eq!(timed_terminations(&lab), [(0, TerminalCause::Cancelled)]);
    let stopped = lab
        .scheduler
        .process_events()
        .iter()
        .find(|event| matches!(event.kind, ProcessEventKind::Stopped { .. }))
        .expect("supervisor_stopped")
        .sequence;
    let main_terminal = lab
        .scheduler
        .events()
        .iter()
        .find(|event| event.task == main && event.kind == TaskEventKind::Terminal)
        .expect("main terminal")
        .sequence;
    assert!(
        stopped < main_terminal,
        "the supervisor stops before its owner is terminal"
    );
}

// ----- Dynamic nested supervisors (SV10) ---------------------------------

fn nested_of(lab: &Lab, managed: MembershipId) -> SupervisorId {
    lab.scheduler
        .nested_supervisor(managed)
        .expect("a nested supervisor")
}

#[test]
fn sv10_a_dynamic_nested_supervisor_takes_a_parent_slot_and_removal_stops_its_subtree() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy_with(
            vec![counter_member(COUNTER)],
            2,
            0,
            Duration::from_secs(30),
        ))
        .unwrap();
    let nested = policy(vec![counter_member(PEER), counter_member(THIRD)], 0);
    let managed = lab
        .scheduler
        .start_nested_supervisor(root, nested, Restart::Never)
        .unwrap();
    assert_eq!(
        lab.scheduler
            .start_process(root, spec(None, 8), Restart::Never),
        Err(StartError::CapacityReached),
        "the nested supervisor holds one of the parent's two slots"
    );
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Err(ProcessError::WrongMemberKind(managed))
    );
    lab.settle();
    lab.deliver(named(PEER), Msg::Add(1));
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.transcript(),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
             @0s supervisor_started supervisor=S2 parent=S1 membership=M2
             @0s process_started supervisor=S2 membership=M3 incarnation=I2 name=app/main.peer
             @0s process_started supervisor=S2 membership=M4 incarnation=I3 name=name3
             @0s admission_rejected supervisor=S1 reason=capacity_reached
             @0s process_send kind=named name=app/main.peer incarnation=I2 outcome=accepted
             @0s process_terminated membership=M4 incarnation=I3 cause=cancelled
             @0s process_terminated membership=M3 incarnation=I2 cause=cancelled
             @0s supervisor_stopped supervisor=S2
             @0s membership_removed membership=M2"
        )
    );
    assert!(
        lab.send(named(PEER), Msg::Add(1)).is_err(),
        "the subtree's names are free"
    );
    assert_eq!(
        lab.scheduler.supervisor_state(SupervisorId::from_raw(2)),
        Some(SupervisorState::Stopped)
    );
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .expect("removal released the slot");
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Err(ProcessError::UnknownMembership(managed)),
        "removal consumed the handle"
    );
}

/// Starts a root and a dynamic nested supervisor whose one member lingers in
/// cleanup, then removes it through the request ABI. Returns the nested
/// supervisor's identity; logs each status and the instant removal returned.
unsafe extern "C" fn sv10_abi_main(_context: *mut c_void) -> ChimiFiberResult {
    let lingering = ChimiSupervisorMember {
        kind: MEMBER_PROCESS,
        restart: RESTART_ON_DEFECT,
        process: ChimiProcessSpec {
            entry: Some(worker),
            context: behavior_context(&STUBBORN),
            mailbox_capacity: 4,
            shutdown_within: 5_000_000_000,
            name: 0,
        },
        supervisor: ptr::null(),
    };
    let nested = ChimiSupervisorPolicy {
        members: &lingering,
        member_count: 1,
        capacity: 2,
        restart_limit: 0,
        restart_within: 1_000_000_000,
    };
    let root_policy = ChimiSupervisorPolicy {
        members: ptr::null(),
        member_count: 0,
        capacity: 1,
        restart_limit: 0,
        restart_within: 1_000_000_000,
    };
    let mut start = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START);
    start.policy = &root_policy;
    let root = supervisor_call(start).expect("not cancelled").supervisor;
    let start_nested = || {
        let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START_SUPERVISOR);
        request.supervisor = root;
        request.restart = RESTART_NEVER;
        request.policy = &nested;
        supervisor_call(request).expect("not cancelled")
    };
    let started = start_nested();
    let rejected = start_nested();
    // Let the nested member run and park in receive.
    sleep_until(now() + 1_000_000);
    let remove = |kind: u32| {
        let mut request = ChimiProcessRequest::new(kind);
        request.supervisor = root;
        request.membership = started.membership;
        supervisor_call(request).expect("not cancelled").status
    };
    let wrong_kind = remove(REQUEST_SUPERVISOR_REMOVE_PROCESS);
    let removed = remove(REQUEST_SUPERVISOR_REMOVE_SUPERVISOR);
    let returned_at = now();
    log(|log| {
        log.outcomes
            .extend([started.status, rejected.status, wrong_kind, removed]);
        log.woke.push((started.membership, returned_at));
    });
    ended(started.supervisor as i64)
}

#[test]
fn sv10_the_request_abi_starts_a_nested_supervisor_and_removal_parks_until_its_subtree_stops() {
    let mut lab = Lab::new();
    let main = lab.scheduler.start_root(sv10_abi_main).unwrap();
    let run = lab.scheduler.run_root(main, DEADLOCK_DEFECT_CODE);
    assert_eq!(
        (run.outcome.state, run.outcome.value),
        (ChimiFiberState::Succeeded as u32, 2)
    );
    assert_eq!(
        log(|log| log.outcomes.clone()),
        [
            RESPONSE_OK,
            RESPONSE_CAPACITY_REACHED,
            RESPONSE_ERROR,
            RESPONSE_OK
        ]
    );
    assert_eq!(
        log(|log| log.woke.clone()),
        [(1, 5_001_000_000)],
        "removal waited for the deadline"
    );
    assert_eq!(
        lab.transcript(),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s supervisor_started supervisor=S2 parent=S1 membership=M1
             @0s process_started supervisor=S2 membership=M2 incarnation=I1
             @0s admission_rejected supervisor=S1 reason=capacity_reached
             @5.001s process_terminated membership=M2 incarnation=I1 cause=forced
             @5.001s supervisor_stopped supervisor=S2
             @5.001s membership_removed membership=M1
             @5.001s supervisor_stopped supervisor=S1"
        )
    );
}

#[test]
fn sv10_stopping_a_nested_supervisor_ends_its_membership_without_a_restart() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy_with(vec![], 1, 2, Duration::from_secs(30)))
        .unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(
            root,
            policy(vec![counter_member(PEER)], 0),
            Restart::OnDefect,
        )
        .unwrap();
    let nested = nested_of(&lab, managed);
    lab.settle();
    assert_eq!(lab.scheduler.stop_supervisor(nested), Ok(Progress::Done));
    assert!(
        lab.restarts().is_empty(),
        "a requested stop is not a defect"
    );
    assert_eq!(
        lab.scheduler.supervisor_state(nested),
        Some(SupervisorState::Stopped)
    );
    assert_eq!(
        lab.scheduler.supervisor_state(root),
        Some(SupervisorState::Running)
    );
    assert_eq!(
        lab.lifecycle()[lab.lifecycle().len() - 2..],
        [
            ProcessEventKind::Terminated {
                membership: MembershipId::from_raw(2),
                incarnation: IncarnationId::from_raw(1),
                cause: TerminalCause::Cancelled,
                stack_overflow: false,
            },
            ProcessEventKind::Stopped { supervisor: nested },
        ]
    );
    lab.scheduler
        .start_process(root, spec(None, 8), Restart::Never)
        .expect("the stopped nested supervisor freed its slot");
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.lifecycle().last(),
        Some(&ProcessEventKind::Removed {
            membership: managed
        }),
        "a handle whose supervisor stopped on its own is still removed visibly"
    );
}

#[test]
fn sv10_a_parent_stop_waits_for_a_nested_stop_already_in_flight_and_stops_it_once() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER)], 0))
        .unwrap();
    let lingering = Member::Process(spec_with(&STUBBORN, None, 8), Restart::Never);
    let managed = lab
        .scheduler
        .start_nested_supervisor(root, policy(vec![lingering], 0), Restart::Never)
        .unwrap();
    let nested = nested_of(&lab, managed);
    lab.settle();
    assert_eq!(lab.scheduler.stop_supervisor(nested), Ok(Progress::Pending));
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Pending));
    lab.settle();
    assert_eq!(
        lab.transcript()[4..],
        lines(
            "@5s process_terminated membership=M3 incarnation=I2 cause=forced
             @5s supervisor_stopped supervisor=S2
             @5s process_terminated membership=M1 incarnation=I1 cause=cancelled
             @5s supervisor_stopped supervisor=S1"
        )
    );
}

#[test]
fn sv10_a_dynamic_nested_supervisor_escalates_to_its_parent_which_replaces_it() {
    let mut lab = Lab::new();
    let root = lab
        .scheduler
        .start_supervisor(policy(vec![counter_member(PEER)], 1))
        .unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(
            root,
            policy(vec![counter_member(COUNTER)], 0),
            Restart::OnDefect,
        )
        .unwrap();
    let first = nested_of(&lab, managed);
    lab.deliver(named(PEER), Msg::Add(4));
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Add(2));
    lab.settle();
    let replacement = nested_of(&lab, managed);
    assert_ne!(replacement, first, "the handle follows the replacement");
    let lifecycle = lab.lifecycle();
    assert!(lifecycle.contains(&ProcessEventKind::Escalated {
        supervisor: first,
        parent: Some(root)
    }));
    assert!(lifecycle.contains(&ProcessEventKind::SupervisorStarted {
        supervisor: replacement,
        parent: Some(root),
        membership: Some(managed),
    }));
    assert_eq!(lab.restarts(), [(managed, 1)], "one parent-level restart");
    assert_eq!(
        lab.scheduler.supervisor_state(first),
        Some(SupervisorState::Stopped)
    );
    let (_, rebuilt) = *lab.started().last().unwrap();
    assert_eq!(total(rebuilt), 2);
    assert_eq!(
        total(lab.started()[0].1),
        4,
        "the parent's other member kept its state"
    );
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.lifecycle()[lab.lifecycle().len() - 3..],
        [
            ProcessEventKind::Terminated {
                membership: lab.started().last().unwrap().0,
                incarnation: rebuilt,
                cause: TerminalCause::Cancelled,
                stack_overflow: false,
            },
            ProcessEventKind::Stopped {
                supervisor: replacement
            },
            ProcessEventKind::Removed {
                membership: managed
            },
        ]
    );

    // With no parent-level budget the escalation continues to the root.
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(
            root,
            policy(vec![counter_member(COUNTER)], 0),
            Restart::OnDefect,
        )
        .unwrap();
    let nested = nested_of(&lab, managed);
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.settle();
    assert!(lab.scheduler.program_terminated());
    let escalations: Vec<_> = lab
        .lifecycle()
        .into_iter()
        .filter_map(|kind| match kind {
            ProcessEventKind::Escalated { supervisor, parent } => Some((supervisor, parent)),
            _ => None,
        })
        .collect();
    assert_eq!(escalations, [(nested, Some(root)), (root, None)]);
}

/// Waits on the supervisor named by its context and returns the status.
unsafe extern "C" fn waits_on_supervisor(context: *mut c_void) -> ChimiFiberResult {
    let mut wait = ChimiProcessRequest::new(REQUEST_SUPERVISOR_WAIT);
    wait.supervisor = context as usize as u64;
    match supervisor_call(wait) {
        Some(reply) => ended(i64::from(reply.status)),
        None => ended(-1),
    }
}

#[test]
fn sv10_a_wait_on_a_nested_supervisor_returns_when_its_parent_stops_it() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(root, policy(vec![counter_member(PEER)], 0), Restart::Never)
        .unwrap();
    let nested = nested_of(&lab, managed);
    let waiter = lab
        .scheduler
        .start_root_with_context(waits_on_supervisor, nested.get() as usize as *mut c_void)
        .unwrap();
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.result(None, waiter), Ok(TaskResult::Pending));
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Done));
    lab.scheduler.drive();
    assert_eq!(
        lab.scheduler.result(None, waiter),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(i64::from(
            RESPONSE_OK
        ))))
    );
}

// ----- Structured stop: an owner drains until its supervisors stop -------

/// A restartable root member whose cleanup has a 5 s deadline; `name` 0 is
/// unnamed.
fn root_member(behavior: &'static Behavior, name: u64) -> ChimiSupervisorMember {
    ChimiSupervisorMember {
        kind: MEMBER_PROCESS,
        restart: RESTART_ON_DEFECT,
        process: ChimiProcessSpec {
            entry: Some(worker),
            context: behavior_context(behavior),
            mailbox_capacity: 4,
            shutdown_within: 5_000_000_000,
            name,
        },
        supervisor: ptr::null(),
    }
}

/// Starts a root supervisor (restart limit 0) with `members`; returns the
/// request status.
fn start_root_of(members: &[ChimiSupervisorMember]) -> u32 {
    let policy = ChimiSupervisorPolicy {
        members: members.as_ptr(),
        member_count: members.len(),
        capacity: members.len() as u32,
        restart_limit: 0,
        restart_within: 1_000_000_000,
    };
    let mut request = ChimiProcessRequest::new(REQUEST_SUPERVISOR_START);
    request.policy = &policy;
    supervisor_call(request).expect("not cancelled").status
}

/// Starts a root supervisor whose one member lingers in cleanup until its
/// 5 s deadline; returns the request status.
fn start_lingering_root() -> u32 {
    start_root_of(&[root_member(&STUBBORN, 0)])
}

/// The name of the member of `start_escalating_root` that a crash makes
/// its root escalate.
const ESCALATOR: u64 = 77;

/// Like `start_lingering_root`, plus a member named `ESCALATOR`: a crash
/// there exceeds the limit, and the escalation then waits on the lingering
/// member's cleanup.
fn start_escalating_root() -> u32 {
    start_root_of(&[root_member(&STUBBORN, 0), root_member(&PLAIN, ESCALATOR)])
}

/// Starts a lingering root, lets its member park, then returns.
unsafe extern "C" fn starts_a_lingering_root_and_returns(
    _context: *mut c_void,
) -> ChimiFiberResult {
    assert_eq!(start_lingering_root(), RESPONSE_OK);
    sleep_until(now() + 1_000_000);
    ended(0)
}

/// Starts a child that owns a lingering root, outlives the child's body by
/// a millisecond without awaiting it, then returns.
unsafe extern "C" fn outlives_a_child_that_owns_a_lingering_root(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let mut request = blank_request(REQUEST_TASK_START);
    request.entry = Some(starts_a_lingering_root_and_returns);
    scheduler_call(&mut request);
    assert_eq!(request.status, RESPONSE_OK);
    sleep_until(now() + 2_000_000);
    ended(0)
}

/// Process lifecycle events and the named tasks' terminal events in one
/// sequence order.
fn timeline(lab: &Lab, tasks: &[(TaskId, &str)]) -> Vec<String> {
    let mut lines: Vec<(u64, String)> = lab
        .scheduler
        .process_events()
        .iter()
        .filter(|event| !matches!(event.kind, ProcessEventKind::Sent { .. }))
        .map(|event| (event.sequence, render(event.time.as_nanos(), event.kind)))
        .collect();
    lines.extend(lab.scheduler.events().iter().filter_map(|event| {
        let (_, label) = tasks.iter().find(|(task, _)| *task == event.task)?;
        (event.kind == TaskEventKind::Terminal)
            .then(|| (event.sequence, format!("{label} terminal")))
    }));
    lines.sort();
    lines.into_iter().map(|(_, line)| line).collect()
}

fn structured_stop_run() -> (Vec<String>, Instant) {
    let mut lab = Lab::new();
    let main = lab
        .scheduler
        .start_root(starts_a_lingering_root_and_returns)
        .unwrap();
    let run = lab.scheduler.run_root(main, DEADLOCK_DEFECT_CODE);
    assert_eq!(
        (run.outcome.state, run.outcome.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
    (timeline(&lab, &[(main, "main")]), lab.scheduler.now())
}

#[test]
fn an_owner_task_stays_draining_until_the_supervisor_it_started_has_stopped() {
    let (timeline, ended_at) = structured_stop_run();
    assert_eq!(
        timeline,
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1
             @5.001s process_terminated membership=M1 incarnation=I1 cause=forced
             @5.001s supervisor_stopped supervisor=S1
             main terminal"
        )
    );
    assert_eq!(ended_at, Instant::from_nanos(5_001_000_000));
    assert_eq!(
        structured_stop_run(),
        (timeline, ended_at),
        "deterministic under virtual time"
    );
}

#[test]
fn a_parent_task_stays_draining_until_a_child_waiting_on_its_supervisor_is_terminal() {
    let mut lab = Lab::new();
    let main = lab
        .scheduler
        .start_root(outlives_a_child_that_owns_a_lingering_root)
        .unwrap();
    let run = lab.scheduler.run_root(main, DEADLOCK_DEFECT_CODE);
    assert_eq!(run.outcome.state, ChimiFiberState::Succeeded as u32);
    let child = lab
        .scheduler
        .events()
        .iter()
        .find(|event| event.kind == TaskEventKind::Started && event.parent == Some(main))
        .expect("the child started")
        .task;
    assert_eq!(
        timeline(&lab, &[(main, "main"), (child, "child")]),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1
             @5.001s process_terminated membership=M1 incarnation=I1 cause=forced
             @5.001s supervisor_stopped supervisor=S1
             child terminal
             main terminal"
        )
    );
}

#[test]
fn removing_a_process_that_started_a_root_waits_for_that_root_to_stop() {
    let mut lab = Lab::new();
    let outer_root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(outer_root, spec(Some(COUNTER), 8), Restart::Never)
        .unwrap();
    lab.deliver(named(COUNTER), Msg::StartLingeringRoot);
    lab.scheduler.drive();
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_OK]);
    assert_eq!(
        lab.scheduler.remove_process(outer_root, managed),
        Ok(Progress::Pending)
    );
    assert!(
        lab.send(named(COUNTER), Msg::Add(1)).is_err(),
        "unrouted while the root it started stops"
    );
    lab.settle();
    assert_eq!(
        lab.transcript()
            .into_iter()
            .filter(|line| !line.contains("process_send"))
            .collect::<Vec<_>>(),
        lines(
            "@0s supervisor_started supervisor=S1 parent=root
             @0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
             @0s supervisor_started supervisor=S2 parent=root
             @0s process_started supervisor=S2 membership=M2 incarnation=I2
             @5s process_terminated membership=M2 incarnation=I2 cause=forced
             @5s supervisor_stopped supervisor=S2
             @5s process_terminated membership=M1 incarnation=I1 cause=cancelled
             @5s membership_removed membership=M1"
        )
    );
}

#[test]
fn es02_a_parent_stop_during_a_nested_escalation_waits_for_it_instead_of_stopping_it_again() {
    let mut lab = Lab::new();
    let lingering = Member::Process(spec_with(&STUBBORN, None, 8), Restart::Never);
    let nested = policy(vec![lingering, counter_member(COUNTER)], 0);
    let root = lab
        .scheduler
        .start_supervisor(policy(
            vec![Member::Supervisor(nested, Restart::OnDefect)],
            1,
        ))
        .unwrap();
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.stop_supervisor(root), Ok(Progress::Pending));
    lab.settle();
    assert_eq!(
        lab.transcript()
            .into_iter()
            .skip(4)
            .filter(|line| !line.contains("process_send"))
            .collect::<Vec<_>>(),
        lines(
            "@0s process_terminated membership=M3 incarnation=I2 cause=defected
             @0s restart_limit_exceeded supervisor=S2
             @5s process_terminated membership=M2 incarnation=I1 cause=forced
             @5s supervisor_escalated supervisor=S2 parent=S1
             @5s supervisor_stopped supervisor=S1"
        )
    );
    assert!(
        lab.restarts().is_empty(),
        "a stopping parent does not replace it"
    );
}

// ----- A stop or removal that meets a nested escalation -----------------

/// A root (limit 1) holding a dynamic `OnDefect` nested supervisor (limit
/// 0) with a lingering member and `COUNTER`. After the counter crashes, the
/// nested escalation waits 5 s on the lingering member's cleanup.
fn nested_escalation_in_flight(lab: &mut Lab) -> (SupervisorId, MembershipId, SupervisorId) {
    let lingering = Member::Process(spec_with(&STUBBORN, None, 8), Restart::Never);
    let root = lab.scheduler.start_supervisor(policy(vec![], 1)).unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(
            root,
            policy(vec![lingering, counter_member(COUNTER)], 0),
            Restart::OnDefect,
        )
        .unwrap();
    let nested = nested_of(lab, managed);
    lab.settle();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.scheduler.drive();
    assert_eq!(
        lab.scheduler.supervisor_state(nested),
        Some(SupervisorState::Stopping)
    );
    (root, managed, nested)
}

fn supervisors_started(lab: &Lab) -> Vec<SupervisorId> {
    lab.lifecycle()
        .into_iter()
        .filter_map(|kind| match kind {
            ProcessEventKind::SupervisorStarted { supervisor, .. } => Some(supervisor),
            _ => None,
        })
        .collect()
}

#[test]
fn sv10_removing_a_nested_supervisor_during_its_escalation_starts_no_replacement() {
    let mut lab = Lab::new();
    let (root, managed, nested) = nested_escalation_in_flight(&mut lab);
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Ok(Progress::Pending)
    );
    lab.settle();
    assert_eq!(
        supervisors_started(&lab),
        [root, nested],
        "no replacement started"
    );
    assert!(lab.restarts().is_empty());
    assert_eq!(
        lab.lifecycle().last(),
        Some(&ProcessEventKind::Removed {
            membership: managed
        })
    );
    assert_eq!(
        lab.scheduler.nested_supervisor(managed),
        Err(ProcessError::UnknownMembership(managed)),
        "the handle is consumed"
    );
    assert_eq!(
        lab.scheduler
            .start_process(root, spec(None, 8), Restart::Never)
            .map(|_| ()),
        Ok(()),
        "the removed membership's slot is free"
    );
    assert!(!lab.scheduler.has_blocked_plans());
}

#[test]
fn sv10_stopping_a_nested_supervisor_during_its_escalation_does_not_restart_it() {
    let mut lab = Lab::new();
    let (root, managed, nested) = nested_escalation_in_flight(&mut lab);
    assert_eq!(lab.scheduler.stop_supervisor(nested), Ok(Progress::Pending));
    lab.settle();
    assert_eq!(
        supervisors_started(&lab),
        [root, nested],
        "no replacement started"
    );
    assert!(lab.restarts().is_empty());
    assert_eq!(
        lab.scheduler.supervisor_state(nested),
        Some(SupervisorState::Stopped)
    );
    assert_eq!(
        lab.scheduler.supervisor_state(root),
        Some(SupervisorState::Running)
    );
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.lifecycle().last(),
        Some(&ProcessEventKind::Removed {
            membership: managed
        }),
        "a stopped nested supervisor's handle is still removed visibly"
    );
}

#[test]
fn sv10_a_wait_on_a_nested_supervisor_returns_when_it_escalates() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 1)).unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(
            root,
            policy(vec![counter_member(COUNTER)], 0),
            Restart::OnDefect,
        )
        .unwrap();
    let nested = nested_of(&lab, managed);
    let waiter = lab
        .scheduler
        .start_root_with_context(waits_on_supervisor, nested.get() as usize as *mut c_void)
        .unwrap();
    lab.scheduler.drive();
    assert_eq!(lab.scheduler.result(None, waiter), Ok(TaskResult::Pending));
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.scheduler.drive();
    assert_ne!(nested_of(&lab, managed), nested, "the parent replaced it");
    assert_eq!(
        lab.scheduler.result(None, waiter),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(i64::from(
            RESPONSE_OK
        ))))
    );
}

#[test]
fn sv10_a_retired_handle_is_still_kind_checked() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 1)).unwrap();
    let managed = lab
        .scheduler
        .start_nested_supervisor(
            root,
            policy(vec![counter_member(COUNTER)], 0),
            Restart::Never,
        )
        .unwrap();
    let process = lab
        .scheduler
        .start_process(root, spec(Some(PEER), 8), Restart::Never)
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Crash);
    lab.deliver(named(PEER), Msg::Return);
    lab.settle();
    assert_eq!(
        lab.scheduler.supervisor_state(nested_of(&lab, managed)),
        Some(SupervisorState::Stopped),
        "it escalated and was not replaced"
    );
    assert_eq!(
        lab.scheduler.remove_process(root, managed),
        Err(ProcessError::WrongMemberKind(managed))
    );
    assert_eq!(
        lab.scheduler.remove_supervisor(root, process),
        Err(ProcessError::WrongMemberKind(process))
    );
    assert_eq!(
        lab.scheduler.remove_supervisor(root, managed),
        Ok(Progress::Done)
    );
    assert_eq!(
        lab.scheduler.remove_process(root, process),
        Ok(Progress::Done)
    );
    let removed: Vec<_> = lab
        .lifecycle()
        .into_iter()
        .filter(|kind| matches!(kind, ProcessEventKind::Removed { .. }))
        .collect();
    assert_eq!(
        removed,
        [
            ProcessEventKind::Removed {
                membership: managed
            },
            ProcessEventKind::Removed {
                membership: process
            },
        ],
        "a wrong-kind attempt does not consume the handle"
    );
}

// ----- An owner whose root escalates while the owner drains -------------

/// An outer root holding process `COUNTER`, which starts an escalating root
/// S2; `ESCALATOR` then crashes, so S2's escalation waits on its lingering
/// member. Returns the outer root and the process membership.
fn owner_of_an_escalating_root(lab: &mut Lab) -> (SupervisorId, MembershipId) {
    let outer_root = lab.scheduler.start_supervisor(policy(vec![], 0)).unwrap();
    let managed = lab
        .scheduler
        .start_process(outer_root, spec(Some(COUNTER), 8), Restart::Never)
        .unwrap();
    lab.deliver(named(COUNTER), Msg::StartEscalatingRoot);
    lab.scheduler.drive();
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_OK]);
    lab.deliver(named(ESCALATOR), Msg::Crash);
    lab.scheduler.drive();
    (outer_root, managed)
}

#[test]
fn removing_an_owner_whose_root_escalates_while_it_drains_finishes() {
    let mut lab = Lab::new();
    let (outer_root, managed) = owner_of_an_escalating_root(&mut lab);
    assert_eq!(
        lab.scheduler.remove_process(outer_root, managed),
        Ok(Progress::Pending)
    );
    lab.settle();
    assert!(lab.scheduler.program_terminated());
    assert!(!lab.scheduler.has_blocked_plans(), "the removal finished");
    let tail: Vec<_> = lab
        .transcript()
        .into_iter()
        .filter(|line| !line.contains("process_send"))
        .skip_while(|line| !line.contains("restart_limit_exceeded"))
        .collect();
    assert_eq!(
        tail,
        lines(
            "@0s restart_limit_exceeded supervisor=S2
             @5s process_terminated membership=M2 incarnation=I2 cause=forced
             @5s supervisor_escalated supervisor=S2 parent=root
             @5s process_terminated membership=M1 incarnation=I1 cause=cancelled
             @5s membership_removed membership=M1
             @5s supervisor_stopped supervisor=S1"
        )
    );
}

#[test]
fn tearing_down_with_an_owner_draining_under_an_escalating_root_finishes_the_owner() {
    let mut lab = Lab::new();
    let (outer_root, managed) = owner_of_an_escalating_root(&mut lab);
    assert_eq!(
        lab.scheduler.remove_process(outer_root, managed),
        Ok(Progress::Pending)
    );
    lab.scheduler.tear_down_processes();
    assert!(
        lab.scheduler.kernel.incarnations.is_empty(),
        "no incarnation outlives teardown"
    );
    assert!(!lab.scheduler.has_blocked_plans());
    assert!(lab.lifecycle().contains(&ProcessEventKind::Removed {
        membership: managed
    }));
}
