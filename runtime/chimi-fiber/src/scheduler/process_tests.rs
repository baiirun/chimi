//! Supervised-process and clock contracts against the real scheduler.
//!
//! Test names carry the conformance-lab scenario IDs
//! (docs/specification/supervised-process-conformance.md) or, for the
//! prototype's kernel decisions, the prototype test name.

use super::process_abi::{
    ADDRESS_DIRECT, ADDRESS_NAMED, CLOCK_VIRTUAL, MEMBER_PROCESS, RESTART_ON_DEFECT,
    chimi_scheduler_run,
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
            let direct = send(Address::Direct(IncarnationId::from_raw(me)), message(Msg::Add(1)));
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
                    name: NameId::from_raw(event.name),
                },
                3 => ProcessEventKind::Terminated {
                    membership,
                    incarnation,
                    cause,
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
        .start_supervisor(policy(vec![counter_member(COUNTER), counter_member(PEER)], 0))
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
    assert!(!log(|log| log.released.contains(&third)), "the sender owns a rejected message");
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
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_OK, RESPONSE_FULL]);
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
    lab.scheduler.process_send(named(COUNTER), add).expect("accepted");
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
    assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Done));
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
        .start_process(root, spec_with(&SELF_SENDER, Some(COUNTER), 8), Restart::Never)
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
    lab.scheduler.start_supervisor(policy(vec![member], 5)).unwrap();
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
    assert_eq!(lab.scheduler.process_metrics().memberships, 1, "no membership allocated");
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
            vec![counter_member(COUNTER), counter_member(PEER), counter_member(THIRD)],
            0,
        ))
        .unwrap();
    assert_eq!(
        lab.transcript(),
        lines(
            "@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
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
        lab.scheduler.start_process(small, spec(None, 8), Restart::Never),
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
        lab.scheduler.start_process(small, spec(None, 8), Restart::Never),
        Err(StartError::Stopped)
    );
    let events = lab.kinds().len();
    assert_eq!(lab.scheduler.stop_supervisor(small), Ok(Progress::Done));
    assert_eq!(lab.scheduler.remove_process(small, reused), Ok(Progress::Done));
    assert_eq!(lab.kinds().len(), events, "repeat stop and removal record nothing");
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
        lab.scheduler.run_root(main, DEADLOCK_DEFECT_CODE).state,
        ChimiFiberState::Succeeded as u32
    );
    assert_eq!(
        lab.scheduler.result(None, main),
        Ok(TaskResult::Ready(TaskOutcome::Succeeded(i64::from(RESPONSE_OK))))
    );
    let member = lab.started()[0].0;
    assert_eq!(lab.terminations(), [(member, TerminalCause::Cancelled)]);
    assert!(matches!(
        lab.lifecycle().last(),
        Some(ProcessEventKind::Stopped { .. })
    ));
    assert!(log(|log| log.outcomes.is_empty()), "the stop never returned to its caller");
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
            "@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
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
fn sv11_malformed_policies_are_errors_that_take_no_ownership() {
    let mut lab = Lab::new();
    let main = lab.scheduler.start_root(malformed_policies_main).unwrap();
    lab.scheduler.run_root(main, DEADLOCK_DEFECT_CODE);
    // Members over capacity, mailbox capacity 0, a negative deadline, and an
    // empty restart window.
    assert_eq!(log(|log| log.outcomes.clone()), [RESPONSE_ERROR; 4]);
    assert!(lab.scheduler.process_events().is_empty());
    assert!(log(|log| log.released_specs.is_empty()));
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
        .start_supervisor(policy(vec![counter_member(COUNTER), counter_member(PEER)], 2))
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
        .start_supervisor(policy(vec![counter_member(COUNTER), counter_member(PEER)], 2))
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
    let after_first: Vec<_> = lab.transcript().into_iter().skip(2).collect();
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
    assert!(lab.scheduler.program_terminated(), "the failed start used budget");
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
    assert_eq!(lab.scheduler.process_metrics().timers, 0, "the child's sleep was removed");
}

fn restart_storm() -> Vec<(i64, String)> {
    let mut lab = Lab::new();
    lab.scheduler
        .start_supervisor(policy(vec![counter_member(COUNTER), counter_member(PEER)], 2))
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
        .map(|event| (event.sequence as i64, render(event.time.as_nanos(), event.kind)))
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
            vec![Member::Supervisor(nested, Restart::Never), counter_member(PEER)],
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
    assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Done));
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
            "@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.p
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
        lab.lifecycle()[1..],
        [
            ProcessEventKind::Terminated {
                membership: managed,
                incarnation: IncarnationId::from_raw(1),
                cause: TerminalCause::Defected
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
    assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Done));
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
    assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Done));
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
        lab.transcript()[3..],
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
        .start_supervisor(policy(vec![counter_member(COUNTER), counter_member(PEER)], 2))
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
    assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Done));
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
    assert!(lab.restarts().is_empty(), "SD07: a forced end never restarts");
}

#[test]
fn sd03_cleanup_within_its_deadline_drains_everything_and_time_stays_put() {
    let (lab, incarnation, _) = removal_with_queued_messages(Duration::from_secs(5));
    assert_eq!(lab.terminations()[0].1, TerminalCause::Cancelled);
    assert_eq!(adds_of(incarnation), [1, 2, 3]);
    assert_eq!(lab.scheduler.now(), Instant::ORIGIN);
    assert!(lab.restarts().is_empty(), "SD07: a cancelled end never restarts");
}

#[test]
fn sd04_cleanup_that_parks_again_is_forced_at_its_deadline() {
    let mut lab = Lab::new();
    let root = lab.scheduler.start_supervisor(policy(vec![], 3)).unwrap();
    let managed = lab
        .scheduler
        .start_process(root, spec_with(&STUBBORN, Some(COUNTER), 8), Restart::OnDefect)
        .unwrap();
    lab.settle();
    assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Pending));
    assert!(lab.send(named(COUNTER), Msg::Add(1)).is_err(), "unrouted while forcing");
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
            vec![counter_member(COUNTER), Member::Process(zero, Restart::OnDefect)],
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
        assert_eq!(lab.scheduler.remove_process(root, managed), Ok(Progress::Done));
        assert_eq!(lab.lifecycle().len(), events, "removing an ended membership records nothing");
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
    let baseline = (lab.scheduler.process_metrics(), lab.scheduler.metrics().tasks);
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
        (lab.scheduler.process_metrics(), lab.scheduler.metrics().tasks),
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
        .start_supervisor(policy(vec![counter_member(COUNTER), counter_member(PEER)], 1))
        .unwrap();
    lab.deliver(named(COUNTER), Msg::Overflow);
    lab.deliver(named(PEER), Msg::Add(4));
    lab.settle();
    assert_eq!(lab.terminations()[0].1, TerminalCause::Defected);
    assert_eq!(lab.started().len(), 3, "the overflowing member restarted once");
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
            vec![Member::Process(spec_with(&A, Some(COUNTER), 8), Restart::OnDefect)],
            1,
        ))
        .unwrap();
    let taken = lab
        .scheduler
        .start_process(root, spec_with(&B, Some(COUNTER), 8), Restart::Never);
    assert!(taken.is_err());
    assert_eq!(log(|log| log.released_specs.clone()), [b], "a rejected spec is released");
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
    let outcome = lab.scheduler.run_root(stuck, DEADLOCK_DEFECT_CODE);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, DEADLOCK_DEFECT_CODE)
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
        .start_root_with_context(recording_sleeper, deadline.as_nanos() as usize as *mut c_void)
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

unsafe extern "C" fn collect_trace(context: *mut c_void, event: *const SchedulerTraceEvent) {
    let events = unsafe { &mut *context.cast::<Vec<SchedulerTraceEvent>>() };
    events.push(unsafe { std::ptr::read(event) });
}

fn run_c(
    entry: ChimiFiberEntry,
    trace_mode: u32,
) -> (SchedulerOutcome, Vec<SchedulerTraceEvent>) {
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
    };
    let outcome = unsafe { chimi_scheduler_run(&config) };
    (outcome, events)
}

const ES01: &str = "
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
    assert!(full.iter().any(|event| event.family == SCHEDULER_TRACE_TASK));
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
            "@0s process_started supervisor=S1 membership=M1 incarnation=I1
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
    let (outcome, events) = run_c(chimi_fixture_deadlocked_main, 1);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, DEADLOCK_DEFECT_CODE)
    );
    assert!(events.iter().any(|event| {
        event.family == SCHEDULER_TRACE_CLOCK
            && event.kind == ClockEventKind::DeadlockWaiter as u32 + 1
    }));
}

#[test]
fn ob07_the_process_abi_layouts_match_the_c_header() {
    use std::mem::{offset_of, size_of};
    let mut c = [0_usize; 64];
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
    ];
    assert_eq!(&c[..count], &rust[..]);
    assert_eq!(crate::chimi_fiber_abi_version(), 4);
}
