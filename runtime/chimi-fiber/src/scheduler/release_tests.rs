//! Release hooks of `chimi_scheduler_execute_with_config` and
//! `chimi_scheduler_run`: every host word
//! the scheduler takes over leaves it exactly once, by delivery, by return,
//! as the outcome, or through a release hook. The same holds for the entry
//! context of a started task, which its entry or `release_task_context`
//! consumes.

use super::*;
use crate::{ChimiFiberResult, chimi_fiber_request};
use std::cell::{Cell, RefCell};
use std::ptr;

const STACK: usize = 64 * 1024;

#[derive(Default)]
struct Log {
    tasks: Vec<i64>,
    channels: Vec<i64>,
    contexts: Vec<usize>,
    /// The contexts of started tasks a test expects to see released.
    unrun_contexts: Vec<usize>,
    /// Hook counts (task, channel, context) a fiber saw at a named point.
    checkpoints: Vec<(&'static str, usize, usize, usize)>,
    /// The reply status and value a receiver found when resumed to cancel.
    cancelled_receives: Vec<(u32, i64)>,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::new(Log::default());
}

unsafe extern "C" fn release_task(_context: *mut c_void, value: i64) {
    LOG.with(|log| log.borrow_mut().tasks.push(value));
}

unsafe extern "C" fn release_channel(_context: *mut c_void, value: i64) {
    LOG.with(|log| log.borrow_mut().channels.push(value));
}

unsafe extern "C" fn release_context(_context: *mut c_void, task_context: *mut c_void) {
    LOG.with(|log| log.borrow_mut().contexts.push(task_context as usize));
}

fn checkpoint(name: &'static str) {
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        let counts = (log.tasks.len(), log.channels.len(), log.contexts.len());
        log.checkpoints.push((name, counts.0, counts.1, counts.2));
    });
}

/// Runs `entry` through `chimi_scheduler_execute_with_config` and through
/// `chimi_scheduler_run` on the real clock (the native runtime's entry), and
/// requires both to release exactly the same words in the same order. A
/// root that stalls is the exception: see `a_stalled_run_keeps_its_held_values`.
fn execute(entry: ChimiFiberEntry) -> (SchedulerOutcome, Log) {
    let executed = execute_once(entry, Entry::Execute);
    let run = execute_once(entry, Entry::Run);
    assert_ne!(
        run.0.state,
        ChimiFiberState::Parked as u32,
        "a stalled root is tested on its own"
    );
    assert_eq!(
        (
            (
                executed.0.state,
                executed.0.value,
                executed.0.origin,
                executed.0.defect_code
            ),
            &executed.1.tasks,
            &executed.1.channels,
            executed.1.contexts.len(),
            &executed.1.checkpoints,
            &executed.1.cancelled_receives,
        ),
        (
            (run.0.state, run.0.value, run.0.origin, run.0.defect_code),
            &run.1.tasks,
            &run.1.channels,
            run.1.contexts.len(),
            &run.1.checkpoints,
            &run.1.cancelled_receives,
        ),
        "chimi_scheduler_run releases what chimi_scheduler_execute_with_config releases"
    );
    // Contexts are addresses, which differ between the two runs; each run
    // must release the contexts its own tasks left unrun.
    assert_eq!(
        executed.1.contexts == executed.1.unrun_contexts,
        run.1.contexts == run.1.unrun_contexts,
        "both entries release the unrun contexts"
    );
    executed
}

#[derive(Clone, Copy)]
enum Entry {
    Execute,
    Run,
}

fn execute_once(entry: ChimiFiberEntry, through: Entry) -> (SchedulerOutcome, Log) {
    LOG.with(|log| *log.borrow_mut() = Log::default());
    let outcome = match through {
        Entry::Execute => {
            let config = ChimiSchedulerExecuteConfig {
                entry: Some(entry),
                context: ptr::null_mut(),
                stack_bytes: STACK,
                trace_mode: 0,
                trace_sink: None,
                trace_context: ptr::null_mut(),
                release_task_value: Some(release_task),
                release_channel_value: Some(release_channel),
                release_context: ptr::null_mut(),
                release_task_context: Some(release_context),
            };
            unsafe { chimi_scheduler_execute_with_config(&config) }
        }
        Entry::Run => {
            let config = ChimiSchedulerConfig {
                entry: Some(entry),
                context: ptr::null_mut(),
                stack_bytes: STACK,
                trace_mode: 0,
                clock_mode: process_abi::CLOCK_REAL,
                trace_sink: None,
                trace_context: ptr::null_mut(),
                release_message: None,
                release_spec_context: None,
                release_context: ptr::null_mut(),
                deadlock_report: None,
                deadlock_context: ptr::null_mut(),
                release_task_value: Some(release_task),
                release_channel_value: Some(release_channel),
                release_task_context: Some(release_context),
                capability_violation_report: None,
                capability_violation_context: ptr::null_mut(),
                process_defect_report: None,
                process_defect_context: ptr::null_mut(),
                signal: ptr::null_mut(),
                signal_fd: -1,
                injected_signal: 0,
                injected_signal_at: 0,
                copy_channel_value: None,
                user_signals: std::ptr::null_mut(),
                injected_user_signals: std::ptr::null(),
                injected_user_signal_count: 0,
            };
            unsafe { process_abi::chimi_scheduler_run(&config) }
        }
    };
    (
        outcome,
        LOG.with(|log| std::mem::take(&mut *log.borrow_mut())),
    )
}

fn sorted(mut values: Vec<i64>) -> Vec<i64> {
    values.sort_unstable();
    values
}

// ----- What a task does, through the request ABI -------------------------

fn blank(kind: u32) -> SchedulerRequest {
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

/// Issues `request` until it is no longer pending. Returns false if the
/// caller was cancelled.
fn call(request: &mut SchedulerRequest) -> bool {
    loop {
        request.status = 0;
        let command =
            unsafe { chimi_fiber_request((request as *mut SchedulerRequest).cast(), ptr::null()) };
        if command == ChimiFiberCommand::Cancel {
            return false;
        }
        if request.status != RESPONSE_PENDING {
            return true;
        }
    }
}

#[derive(Clone, Copy)]
struct Channel {
    channel: u64,
    owner: u64,
    token: u64,
}

/// What a child task does: send `value` on `channel` (capacity-0 or full,
/// so the send parks), receive on it, or just end with `value`.
#[derive(Clone, Copy)]
enum Child {
    Return(i64),
    /// Park with nothing to wake it, and end with this value from the
    /// cleanup that the cancel runs.
    ReturnFromCleanup(i64),
    Defect(i64),
    Send {
        channel: u64,
        value: i64,
    },
    /// Receive with the capability the parent stores before this task runs.
    Receive(&'static Cell<Option<Channel>>),
}

unsafe extern "C" fn child(context: *mut c_void) -> ChimiFiberResult {
    let behavior = unsafe { *context.cast::<Child>() };
    let value = match behavior {
        Child::Return(value) => value,
        Child::ReturnFromCleanup(value) => {
            unsafe { crate::chimi_fiber_park(ptr::null()) };
            value
        }
        Child::Defect(code) => {
            return ChimiFiberResult {
                state: ChimiFiberState::Defected as u32,
                value: 0,
                defect_code: code,
            };
        }
        Child::Send { channel, value } => {
            let mut request = blank(REQUEST_CHANNEL_SEND);
            request.channel = channel;
            request.value = value;
            if !call(&mut request) {
                return cancelled();
            }
            i64::from(request.status)
        }
        Child::Receive(capability) => {
            let channel = capability
                .get()
                .expect("parent stores the capability first");
            let mut request = blank(REQUEST_CHANNEL_RECEIVE);
            request.channel = channel.channel;
            request.owner = channel.owner;
            request.token = channel.token;
            if !call(&mut request) {
                LOG.with(|log| {
                    log.borrow_mut()
                        .cancelled_receives
                        .push((request.status, request.value))
                });
                return cancelled();
            }
            i64::from(request.status)
        }
    };
    ChimiFiberResult {
        state: ChimiFiberState::Succeeded as u32,
        value,
        defect_code: 0,
    }
}

fn cancelled() -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Cancelled as u32,
        value: 0,
        defect_code: 0,
    }
}

fn start(behavior: Child) -> u64 {
    start_with_context(behavior).0
}

/// Starts a child and returns it with the address of its entry context.
fn start_with_context(behavior: Child) -> (u64, usize) {
    let mut request = blank(REQUEST_TASK_START);
    request.entry = Some(child);
    request.context = Box::into_raw(Box::new(behavior)).cast();
    call(&mut request);
    (request.task, request.context as usize)
}

/// The status and value of a child's result, waiting for it to end; `None`
/// if the caller was cancelled while it waited.
fn result(task: u64) -> Option<(u32, i64)> {
    let mut request = blank(REQUEST_TASK_RESULT);
    request.task = task;
    call(&mut request).then_some((request.status, request.value))
}

fn cancel(task: u64) {
    let mut request = blank(REQUEST_TASK_CANCEL);
    request.task = task;
    call(&mut request);
}

fn bounded(capacity: i64) -> Channel {
    let mut request = blank(REQUEST_CHANNEL_BOUNDED);
    request.value = capacity;
    call(&mut request);
    Channel {
        channel: request.channel,
        owner: request.owner,
        token: request.token,
    }
}

fn send(channel: Channel, value: i64) -> u32 {
    let mut request = blank(REQUEST_CHANNEL_SEND);
    request.channel = channel.channel;
    request.value = value;
    call(&mut request);
    request.status
}

fn receive(channel: Channel) -> Option<(u32, i64)> {
    let mut request = blank(REQUEST_CHANNEL_RECEIVE);
    request.channel = channel.channel;
    request.owner = channel.owner;
    request.token = channel.token;
    call(&mut request).then_some((request.status, request.value))
}

fn release(channel: Channel) -> u32 {
    let mut request = blank(REQUEST_CHANNEL_RELEASE);
    request.channel = channel.channel;
    call(&mut request);
    request.status
}

fn abort(channel: Channel) {
    let mut request = blank(REQUEST_CHANNEL_ABORT);
    request.channel = channel.channel;
    request.owner = channel.owner;
    request.token = channel.token;
    call(&mut request);
}

/// Parks the caller until every task queued ahead of it has run once: a
/// child that only returns is started last and waited on.
fn let_queued_tasks_run() {
    let marker = start(Child::Return(0));
    result(marker);
}

fn succeeded(value: i64) -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Succeeded as u32,
        value,
        defect_code: 0,
    }
}

// ----- Task values --------------------------------------------------------

unsafe extern "C" fn observes_one_child_and_ignores_another(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let observed = start(Child::Return(101));
    let ignored = start(Child::Return(202));
    let first = result(observed);
    let second = result(observed);
    checkpoint("after observing twice");
    let _ = ignored;
    succeeded(
        if first == second && first == Some((RESPONSE_SUCCEEDED, 101)) {
            7
        } else {
            -1
        },
    )
}

#[test]
fn every_task_value_but_the_outcome_is_released_once_after_the_root_ends() {
    let (outcome, log) = execute(observes_one_child_and_ignores_another);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 7)
    );
    // Task.result copies without consuming, so the observed value is still
    // the scheduler's while the root runs; the root's own value passes to
    // the caller.
    assert_eq!(log.checkpoints, vec![("after observing twice", 0, 0, 0)]);
    // 101 was observed twice and 202 never; each is released once.
    assert_eq!(sorted(log.tasks), vec![101, 202]);
    assert!(log.channels.is_empty());
}

unsafe extern "C" fn inherits_a_child_defect(_context: *mut c_void) -> ChimiFiberResult {
    let sibling = start(Child::Return(303));
    let defecting = start(Child::Defect(0x51));
    // The child's defect cancels this task while it waits; a cancelled
    // task ends without a value of its own.
    if result(sibling).is_none() || result(defecting).is_none() {
        return cancelled();
    }
    succeeded(-1)
}

#[test]
fn a_child_defect_code_carried_by_the_outcome_passes_to_the_caller() {
    let (outcome, log) = execute(inherits_a_child_defect);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, 0x51)
    );
    assert_ne!(outcome.origin, 1, "the code originates in the child");
    assert_eq!(log.tasks, vec![303]);
}

unsafe extern "C" fn cancels_a_child_whose_cleanup_returns(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let parked = start(Child::ReturnFromCleanup(404));
    let_queued_tasks_run();
    cancel(parked);
    succeeded(0)
}

#[test]
fn a_value_returned_from_cleanup_after_a_cancel_is_released_once() {
    let (outcome, log) = execute(cancels_a_child_whose_cleanup_returns);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    // 0 is the marker's value; the root's own 0 passes to the caller.
    assert_eq!(sorted(log.tasks), vec![0, 404]);
}

// ----- Channel values -----------------------------------------------------

unsafe extern "C" fn cancels_a_parked_sender(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(0);
    start(Child::Send {
        channel: channel.channel,
        value: 901,
    });
    let delivered = receive(channel);
    let parked = start(Child::Send {
        channel: channel.channel,
        value: 900,
    });
    let_queued_tasks_run();
    cancel(parked);
    checkpoint("after cancelling the parked sender");
    succeeded(if delivered == Some((RESPONSE_SUCCEEDED, 901)) {
        0
    } else {
        -1
    })
}

#[test]
fn a_cancelled_send_releases_its_parked_value_once_at_the_cancel() {
    let (outcome, log) = execute(cancels_a_parked_sender);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
    assert_eq!(log.channels, vec![900]);
    assert_eq!(
        log.checkpoints,
        vec![("after cancelling the parked sender", 0, 1, 0)]
    );
}

/// Starts a receiver child holding `channel`'s receive capability.
fn start_receiver(channel: Channel) -> u64 {
    let capability: &'static Cell<Option<Channel>> = Box::leak(Box::new(Cell::new(None)));
    let receiver = start(Child::Receive(capability));
    let mut transfer = blank(REQUEST_CHANNEL_TRANSFER_RECEIVER);
    transfer.channel = channel.channel;
    transfer.owner = channel.owner;
    transfer.token = channel.token;
    transfer.task = receiver;
    call(&mut transfer);
    capability.set(Some(Channel {
        channel: channel.channel,
        owner: transfer.owner,
        token: transfer.token,
    }));
    receiver
}

unsafe extern "C" fn defects_while_a_receiver_holds_a_message(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let channel = bounded(0);
    start_receiver(channel);
    // Run order: the receiver parks; the sender hands it 970, which wakes
    // it behind the defecting child; that defect cancels this task and its
    // children before the receiver can take 970.
    start(Child::Send {
        channel: channel.channel,
        value: 970,
    });
    let defecting = start(Child::Defect(0x61));
    if result(defecting).is_none() {
        return cancelled();
    }
    succeeded(-1)
}

#[test]
fn a_message_waiting_for_a_cancelled_receiver_is_released_once() {
    let (outcome, log) = execute(defects_while_a_receiver_holds_a_message);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, 0x61)
    );
    assert_eq!(log.channels, vec![970]);
    // The receiver never saw a reply carrying 970.
    assert!(
        log.cancelled_receives
            .iter()
            .all(|reply| reply.0 != RESPONSE_SUCCEEDED)
    );
}

unsafe extern "C" fn cancels_a_receiver_after_its_reply(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(0);
    let receiver = start_receiver(channel);
    // The receiver parks, the marker ends and queues this task, and the
    // sender hands 971 to the receiver. Taking the marker's result queues
    // this task again, behind the receiver, which takes 971 into its reply
    // and is queued behind this task. The cancel below reaches it with the
    // reply written but unread.
    let marker = start(Child::Return(0));
    start(Child::Send {
        channel: channel.channel,
        value: 971,
    });
    result(marker);
    cancel(receiver);
    succeeded(0)
}

#[test]
fn a_written_receive_reply_belongs_to_the_receiver_even_if_it_is_cancelled() {
    let (outcome, log) = execute(cancels_a_receiver_after_its_reply);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
    assert_eq!(log.cancelled_receives, vec![(RESPONSE_SUCCEEDED, 971)]);
    assert!(log.channels.is_empty());
}

unsafe extern "C" fn parks_forever_with_a_queued_value(_context: *mut c_void) -> ChimiFiberResult {
    let queued = bounded(1);
    send(queued, 950);
    let empty = bounded(0);
    receive(empty);
    cancelled()
}

#[test]
fn values_still_held_when_execution_gives_up_are_released_once() {
    let (outcome, log) = execute_once(parks_forever_with_a_queued_value, Entry::Execute);
    assert_eq!(outcome.state, ChimiFiberState::Defected as u32);
    assert_eq!(log.channels, vec![950]);
}

// `chimi_scheduler_run` on the real clock does not give up: a host event may
// still wake the root, so it returns PARKED and the suspended execution keeps
// what it holds. Releasing the value here would hand it out while a parked
// fiber can still be resumed to deliver it.
#[test]
fn a_stalled_run_keeps_its_held_values() {
    let (outcome, log) = execute_once(parks_forever_with_a_queued_value, Entry::Run);
    assert_eq!(outcome.state, ChimiFiberState::Parked as u32);
    assert!(log.channels.is_empty());
}

unsafe extern "C" fn aborts_a_channel_with_queued_values(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let channel = bounded(2);
    send(channel, 962);
    let delivered = receive(channel);
    send(channel, 960);
    send(channel, 961);
    let refused = start(Child::Send {
        channel: channel.channel,
        value: 963,
    });
    let_queued_tasks_run();
    abort(channel);
    checkpoint("after aborting");
    let refusal = result(refused);
    succeeded(
        if delivered == Some((RESPONSE_SUCCEEDED, 962))
            && refusal == Some((RESPONSE_SUCCEEDED, i64::from(RESPONSE_ABORTED)))
        {
            0
        } else {
            -1
        },
    )
}

#[test]
fn an_abort_releases_each_queued_value_once_and_hands_parked_sends_back() {
    let (outcome, log) = execute(aborts_a_channel_with_queued_values);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
    assert_eq!(sorted(log.channels), vec![960, 961]);
    assert_eq!(log.checkpoints, vec![("after aborting", 0, 2, 0)]);
}

// ----- Channel release ----------------------------------------------------

unsafe extern "C" fn releases_a_channel_with_queued_values(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let channel = bounded(3);
    send(channel, 980);
    let delivered = receive(channel);
    send(channel, 981);
    send(channel, 982);
    let status = release(channel);
    checkpoint("after releasing");
    succeeded(
        if delivered == Some((RESPONSE_SUCCEEDED, 980)) && status == RESPONSE_OK {
            0
        } else {
            -1
        },
    )
}

#[test]
fn a_channel_release_frees_each_queued_value_once_at_the_release() {
    let (outcome, log) = execute(releases_a_channel_with_queued_values);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
    // 980 was delivered and is the receiver's; 981 and 982 are released by
    // the request, not later at teardown.
    assert_eq!(sorted(log.channels), vec![981, 982]);
    assert_eq!(log.checkpoints, vec![("after releasing", 0, 2, 0)]);
}

unsafe extern "C" fn releases_channels_with_parked_peers(
    _context: *mut c_void,
) -> ChimiFiberResult {
    let sent_on = bounded(0);
    let sender = start(Child::Send {
        channel: sent_on.channel,
        value: 990,
    });
    let received_on = bounded(0);
    let receiver = start_receiver(received_on);
    let_queued_tasks_run();
    release(sent_on);
    release(received_on);
    let aborted = Some((RESPONSE_SUCCEEDED, i64::from(RESPONSE_ABORTED)));
    succeeded(
        if result(sender) == aborted && result(receiver) == aborted {
            0
        } else {
            -1
        },
    )
}

#[test]
fn a_channel_release_answers_its_parked_peers_aborted() {
    let (outcome, log) = execute(releases_channels_with_parked_peers);
    assert_eq!(
        (outcome.state, outcome.value),
        (ChimiFiberState::Succeeded as u32, 0)
    );
    // The parked send's value went back to its sender with the reply.
    assert!(log.channels.is_empty());
}

unsafe extern "C" fn sends_after_release(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(1);
    release(channel);
    send(channel, 1);
    succeeded(-1)
}

unsafe extern "C" fn receives_after_release(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(1);
    release(channel);
    receive(channel);
    succeeded(-1)
}

unsafe extern "C" fn releases_twice(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(1);
    release(channel);
    release(channel);
    succeeded(-1)
}

#[test]
fn a_released_channel_id_defects_whoever_uses_it_again() {
    for (name, entry) in [
        ("send", sends_after_release as ChimiFiberEntry),
        ("receive", receives_after_release),
        ("release", releases_twice),
    ] {
        let (outcome, log) = execute(entry);
        assert_eq!(
            (outcome.state, outcome.origin, outcome.defect_code),
            (
                ChimiFiberState::Defected as u32,
                1,
                CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE
            ),
            "{name} after release"
        );
        assert!(log.channels.is_empty(), "{name} after release");
    }
}

// ----- Entry contexts of tasks that never run -----------------------------

unsafe extern "C" fn leaves_children_unrun(_context: *mut c_void) -> ChimiFiberResult {
    start(Child::Return(1));
    let_queued_tasks_run();
    let (cancelled, cancelled_context) = start_with_context(Child::Return(2));
    cancel(cancelled);
    checkpoint("after cancelling the unrun child");
    let (_, abandoned_context) = start_with_context(Child::Return(3));
    LOG.with(|log| {
        log.borrow_mut()
            .unrun_contexts
            .extend([cancelled_context, abandoned_context])
    });
    succeeded(0)
}

#[test]
fn a_child_that_never_runs_releases_its_entry_context_once() {
    let (outcome, log) = execute(leaves_children_unrun);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    // The cancelled child's context is released at the cancel and the one
    // the root leaves queued when it ends; the children that ran (the first
    // and the marker) consumed theirs in their entry.
    assert_eq!(
        log.checkpoints,
        vec![("after cancelling the unrun child", 0, 0, 1)]
    );
    assert_eq!(log.contexts, log.unrun_contexts);
}

unsafe extern "C" {
    fn chimi_fixture_execute_config_layout(out: *mut usize) -> usize;
}

#[test]
fn the_execute_config_matches_its_c_layout() {
    use std::mem::{offset_of, size_of};
    let mut c = [0usize; 16];
    let count = unsafe { chimi_fixture_execute_config_layout(c.as_mut_ptr()) };
    let rust = [
        size_of::<ChimiSchedulerExecuteConfig>(),
        offset_of!(ChimiSchedulerExecuteConfig, context),
        offset_of!(ChimiSchedulerExecuteConfig, stack_bytes),
        offset_of!(ChimiSchedulerExecuteConfig, trace_mode),
        offset_of!(ChimiSchedulerExecuteConfig, trace_sink),
        offset_of!(ChimiSchedulerExecuteConfig, trace_context),
        offset_of!(ChimiSchedulerExecuteConfig, release_task_value),
        offset_of!(ChimiSchedulerExecuteConfig, release_channel_value),
        offset_of!(ChimiSchedulerExecuteConfig, release_context),
        offset_of!(ChimiSchedulerExecuteConfig, release_task_context),
        REQUEST_CHANNEL_RELEASE as usize,
    ];
    assert_eq!(&c[..count], &rust[..]);
}
