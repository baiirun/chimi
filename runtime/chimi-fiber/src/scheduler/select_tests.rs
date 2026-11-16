//! `CHIMI_REQUEST_SELECT` through the request ABI, under the virtual clock:
//! list order decides among ready sources, a parked select commits to the
//! first source that turns ready, and nothing is consumed by the select
//! itself, so a source that lost keeps its value.

use super::*;
use crate::{ChimiFiberResult, chimi_fiber_request};
use std::cell::RefCell;
use std::ptr;

thread_local! {
    static SEEN: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

fn note(value: i64) {
    SEEN.with(|seen| seen.borrow_mut().push(value));
}

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

/// Issues `request` until it is no longer pending; false if cancelled.
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

fn receive(channel: Channel) -> (u32, i64) {
    let mut request = blank(REQUEST_CHANNEL_RECEIVE);
    request.channel = channel.channel;
    request.owner = channel.owner;
    request.token = channel.token;
    call(&mut request);
    (request.status, request.value)
}

fn now() -> i64 {
    let mut request = blank(REQUEST_TIMER_NOW);
    call(&mut request);
    request.value
}

fn on_channel(channel: Channel) -> ChimiSelectSource {
    ChimiSelectSource {
        kind: SELECT_RECEIVE,
        channel: channel.channel,
        owner: channel.owner,
        token: channel.token,
        task: 0,
        deadline: 0,
    }
}

fn on_task(task: u64) -> ChimiSelectSource {
    ChimiSelectSource {
        kind: SELECT_TASK,
        channel: 0,
        owner: 0,
        token: 0,
        task,
        deadline: 0,
    }
}

fn at(deadline: i64) -> ChimiSelectSource {
    ChimiSelectSource {
        kind: SELECT_TIMER,
        channel: 0,
        owner: 0,
        token: 0,
        task: 0,
        deadline,
    }
}

/// The committed index, or `None` when cancelled; the reply status is
/// noted when it is not OK.
fn select(sources: &[ChimiSelectSource]) -> Option<i64> {
    let mut request = blank(REQUEST_SELECT);
    request.context = sources.as_ptr().cast_mut().cast();
    request.value = sources.len() as i64;
    if !call(&mut request) {
        return None;
    }
    if request.status != RESPONSE_OK {
        note(-i64::from(request.status));
    }
    Some(request.value)
}

/// A child that sends `value` on the channel at `context` after sleeping
/// `delay` nanoseconds.
#[derive(Clone, Copy)]
struct Sender {
    channel: Channel,
    delay: i64,
    value: i64,
}

unsafe extern "C" fn delayed_sender(context: *mut c_void) -> ChimiFiberResult {
    let sender = unsafe { *context.cast::<Sender>() };
    if sender.delay > 0 {
        let mut sleep = blank(REQUEST_TIMER_SLEEP_UNTIL);
        sleep.value = now() + sender.delay;
        call(&mut sleep);
    }
    send(sender.channel, sender.value);
    succeeded(0)
}

fn start_sender(channel: Channel, delay: i64, value: i64) -> u64 {
    let mut request = blank(REQUEST_TASK_START);
    request.entry = Some(delayed_sender);
    request.context = Box::into_raw(Box::new(Sender {
        channel,
        delay,
        value,
    }))
    .cast();
    call(&mut request);
    request.task
}

fn succeeded(value: i64) -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Succeeded as u32,
        value,
        defect_code: 0,
    }
}

/// A child's answer to a cancellation: it must issue no further request.
fn cancelled() -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Cancelled as u32,
        value: 0,
        defect_code: 0,
    }
}

fn run(entry: ChimiFiberEntry) -> (SchedulerOutcome, Vec<i64>) {
    run_on(entry, process_abi::CLOCK_VIRTUAL)
}

fn run_on(entry: ChimiFiberEntry, clock_mode: u32) -> (SchedulerOutcome, Vec<i64>) {
    SEEN.with(|seen| seen.borrow_mut().clear());
    let config = ChimiSchedulerConfig {
        entry: Some(entry),
        context: ptr::null_mut(),
        stack_bytes: 64 * 1024,
        trace_mode: 0,
        clock_mode,
        trace_sink: None,
        trace_context: ptr::null_mut(),
        release_message: None,
        release_spec_context: None,
        release_context: ptr::null_mut(),
        deadlock_report: None,
        deadlock_context: ptr::null_mut(),
        release_task_value: None,
        release_channel_value: None,
        release_task_context: None,
        capability_violation_report: None,
        capability_violation_context: ptr::null_mut(),
        process_defect_report: None,
        process_defect_context: ptr::null_mut(),
        signal: ptr::null_mut(),
        signal_fd: -1,
        injected_signal: 0,
        injected_signal_at: 0,
        copy_channel_value: None,
    };
    let outcome = unsafe { process_abi::chimi_scheduler_run(&config) };
    (outcome, SEEN.with(|seen| seen.borrow().clone()))
}

unsafe extern "C" fn two_ready_channels(_context: *mut c_void) -> ChimiFiberResult {
    let first = bounded(1);
    let second = bounded(1);
    send(second, 20);
    send(first, 10);
    // Both are ready: list order decides, and the loser keeps its value.
    note(select(&[on_channel(second), on_channel(first)]).unwrap_or(-1));
    note(receive(second).1);
    note(select(&[on_channel(first), on_channel(second)]).unwrap_or(-1));
    note(receive(first).1);
    succeeded(0)
}

#[test]
fn among_ready_sources_the_first_in_list_order_commits_and_the_rest_keep_their_values() {
    let (outcome, seen) = run(two_ready_channels);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [0, 20, 0, 10]);
}

unsafe extern "C" fn parked_select_on_a_later_sender(_context: *mut c_void) -> ChimiFiberResult {
    let quiet = bounded(1);
    let busy = bounded(0);
    let start = now();
    start_sender(busy, 5, 7);
    // Nothing is ready; the rendezvous sender arriving at +5 wakes the
    // select before the +100 timer, and the receive then takes its value.
    note(select(&[on_channel(quiet), on_channel(busy), at(start + 100)]).unwrap_or(-1));
    note(receive(busy).1);
    note(now() - start);
    // The quiet channel was never touched: a value sent now is still the
    // first one it holds.
    send(quiet, 3);
    note(receive(quiet).1);
    succeeded(0)
}

#[test]
fn a_parked_select_commits_to_the_first_source_that_turns_ready() {
    let (outcome, seen) = run(parked_select_on_a_later_sender);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1, 7, 5, 3]);
}

unsafe extern "C" fn timer_wins_then_channel_is_intact(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(1);
    let start = now();
    start_sender(channel, 50, 9);
    note(select(&[on_channel(channel), at(start + 10), at(start + 20)]).unwrap_or(-1));
    note(now() - start);
    // The select's registration on the channel is gone: the later send is
    // received normally.
    note(receive(channel).1);
    note(now() - start);
    succeeded(0)
}

#[test]
fn the_earliest_timer_commits_and_leaves_the_other_sources_untouched() {
    let (outcome, seen) = run(timer_wins_then_channel_is_intact);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1, 10, 9, 50]);
}

unsafe extern "C" fn waits_for_a_child(_context: *mut c_void) -> ChimiFiberResult {
    let idle = bounded(1);
    let child = start_sender(idle, 30, 4);
    let start = now();
    // The child's send makes the channel ready before the child ends; the
    // channel is first in the list, so it commits.
    note(select(&[on_channel(idle), on_task(child)]).unwrap_or(-1));
    note(receive(idle).1);
    // Now only the terminal of the child remains.
    note(select(&[on_channel(idle), on_task(child), at(start + 1_000)]).unwrap_or(-1));
    note(now() - start);
    succeeded(0)
}

#[test]
fn a_task_source_is_ready_once_the_child_is_terminal() {
    let (outcome, seen) = run(waits_for_a_child);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [0, 4, 1, 30]);
}

unsafe extern "C" fn selects_on_nothing_ready_forever(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(1);
    note(select(&[on_channel(channel)]).unwrap_or(-1));
    succeeded(0)
}

#[test]
fn a_select_nothing_can_wake_is_a_virtual_deadlock() {
    let (outcome, _) = run(selects_on_nothing_ready_forever);
    assert_eq!(outcome.state, ChimiFiberState::Defected as u32);
    assert_eq!(outcome.defect_code, DEADLOCK_DEFECT_CODE);
}

unsafe extern "C" fn malformed_selects(_context: *mut c_void) -> ChimiFiberResult {
    select(&[]);
    let unknown = ChimiSelectSource { kind: 99, ..at(0) };
    select(&[unknown]);
    succeeded(0)
}

#[test]
fn an_empty_or_unknown_source_list_is_an_error_reply() {
    let (outcome, seen) = run(malformed_selects);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    let error = -i64::from(RESPONSE_ERROR);
    assert_eq!(seen, [error, error]);
}

unsafe extern "C" fn select_with_a_stale_receiver(_context: *mut c_void) -> ChimiFiberResult {
    let channel = bounded(1);
    let stale = Channel {
        token: channel.token + 1,
        ..channel
    };
    note(select(&[on_channel(stale)]).unwrap_or(-1));
    succeeded(0)
}

#[test]
fn a_receiver_that_is_not_the_callers_defects_it() {
    let (outcome, _) = run(select_with_a_stale_receiver);
    assert_eq!(outcome.state, ChimiFiberState::Defected as u32);
    assert_eq!(
        outcome.defect_code,
        CHANNEL_CAPABILITY_VIOLATION_DEFECT_CODE
    );
}

/// Sends on `a` after 5 ms.
unsafe extern "C" fn wake_at_five(context: *mut c_void) -> ChimiFiberResult {
    let a = unsafe { *context.cast::<Channel>() };
    let mut sleep = blank(REQUEST_TIMER_SLEEP_UNTIL);
    sleep.value = now() + 5_000_000;
    if !call(&mut sleep) {
        return cancelled();
    }
    send(a, 1);
    succeeded(0)
}

/// Wakes after 5 ms too, behind the sender, and then holds the processor
/// for 40 ms, so the select woken by the send reissues only after its
/// scope's 20 ms deadline.
unsafe extern "C" fn hog_after_five(_context: *mut c_void) -> ChimiFiberResult {
    let mut sleep = blank(REQUEST_TIMER_SLEEP_UNTIL);
    sleep.value = now() + 5_000_000;
    if !call(&mut sleep) {
        return cancelled();
    }
    let until = std::time::Instant::now() + std::time::Duration::from_millis(40);
    while std::time::Instant::now() < until {}
    succeeded(0)
}

/// Sends on `b` (the select's stale watch, if it survived) and then on `c`.
unsafe extern "C" fn send_b_then_c(context: *mut c_void) -> ChimiFiberResult {
    let [b, c] = unsafe { *context.cast::<[Channel; 2]>() };
    send(b, 2);
    send(c, 3);
    succeeded(0)
}

fn start(entry: ChimiFiberEntry, context: *mut c_void) {
    let mut request = blank(REQUEST_TASK_START);
    request.entry = Some(entry);
    request.context = context;
    call(&mut request);
}

unsafe extern "C" fn scope_refuses_a_woken_select(_context: *mut c_void) -> ChimiFiberResult {
    let a = bounded(1);
    let b = bounded(1);
    let c = bounded(1);
    let mut enter = blank(REQUEST_SCOPE_ENTER);
    enter.value = now() + 20_000_000;
    call(&mut enter);
    start(wake_at_five, Box::into_raw(Box::new(a)).cast());
    start(hog_after_five, ptr::null_mut());
    // Woken by `a`, but the scope fires before the select is asked again.
    note(select(&[on_channel(b), on_channel(a)]).unwrap_or(-1));
    let mut exit = blank(REQUEST_SCOPE_EXIT);
    exit.token = enter.token;
    call(&mut exit);
    note(exit.value);
    start(send_b_then_c, Box::into_raw(Box::new([b, c])).cast());
    // A select registration left on `b` would wake this receive early.
    note(receive(c).1);
    succeeded(0)
}

/// On a slow host the scope may fire before the send; the select is then
/// withdrawn while parked, and the result is the same.
#[test]
fn a_select_a_scope_refuses_leaves_no_registration_behind() {
    let (outcome, seen) = run_on(scope_refuses_a_woken_select, process_abi::CLOCK_REAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [-1, 1, 3]);
}
