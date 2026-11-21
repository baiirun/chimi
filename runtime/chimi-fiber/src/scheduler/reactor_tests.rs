//! `CHIMI_REQUEST_IO_WAIT` through the request ABI: a wait answers at once
//! for a ready descriptor, parks otherwise while other tasks run, holds
//! virtual time, and is withdrawn by a timeout scope or a signal.

use super::*;
use crate::{ChimiFiberResult, chimi_fiber_request};
use std::cell::{Cell, RefCell};
use std::ptr;

thread_local! {
    static SEEN: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static DONE: Cell<bool> = const { Cell::new(false) };
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

/// Issues `request` once; false if the task was cancelled.
fn issue(request: &mut SchedulerRequest) -> bool {
    request.status = 0;
    let command =
        unsafe { chimi_fiber_request((request as *mut SchedulerRequest).cast(), ptr::null()) };
    command != ChimiFiberCommand::Cancel
}

/// Issues `request` until it is no longer pending; false if cancelled.
fn call(request: &mut SchedulerRequest) -> bool {
    loop {
        if !issue(request) {
            return false;
        }
        if request.status != RESPONSE_PENDING {
            return true;
        }
    }
}

/// The ready conditions, or `None` when cancelled. A non-OK status is
/// returned negated.
fn wait_io(fd: i32, interest: u32) -> Option<i64> {
    loop {
        let mut request = blank(REQUEST_IO_WAIT);
        request.value = i64::from(fd);
        request.token = u64::from(interest);
        if !issue(&mut request) {
            return None;
        }
        match request.status {
            RESPONSE_PENDING => continue,
            RESPONSE_OK => return Some(request.value),
            status => return Some(-i64::from(status)),
        }
    }
}

fn now() -> i64 {
    let mut request = blank(REQUEST_TIMER_NOW);
    call(&mut request);
    request.value
}

fn sleep(nanos: i64) {
    let mut request = blank(REQUEST_TIMER_SLEEP_UNTIL);
    request.value = now() + nanos;
    call(&mut request);
}

fn start(entry: ChimiFiberEntry, context: *mut c_void) -> u64 {
    let mut request = blank(REQUEST_TASK_START);
    request.entry = Some(entry);
    request.context = context;
    call(&mut request);
    request.task
}

fn result(task: u64) {
    let mut request = blank(REQUEST_TASK_RESULT);
    request.task = task;
    call(&mut request);
}

fn succeeded(value: i64) -> ChimiFiberResult {
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

/// A pipe as (read end, write end).
fn pipe() -> (i32, i32) {
    let mut ends = [0; 2];
    assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
    (ends[0], ends[1])
}

fn write_byte(fd: i32) {
    assert_eq!(unsafe { libc::write(fd, b"x".as_ptr().cast(), 1) }, 1);
}

fn close(fd: i32) {
    unsafe { libc::close(fd) };
}

fn run(entry: ChimiFiberEntry, clock_mode: u32) -> (SchedulerOutcome, Vec<i64>) {
    run_with_signal(entry, clock_mode, ptr::null_mut(), ptr::null_mut(), -1)
}

fn run_with_signal(
    entry: ChimiFiberEntry,
    clock_mode: u32,
    context: *mut c_void,
    signal: *mut i32,
    signal_fd: i32,
) -> (SchedulerOutcome, Vec<i64>) {
    SEEN.with(|seen| seen.borrow_mut().clear());
    DONE.with(|done| done.set(false));
    let config = ChimiSchedulerConfig {
        entry: Some(entry),
        context,
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
        signal,
        signal_fd,
        injected_signal: 0,
        injected_signal_at: 0,
        copy_channel_value: None,
        user_signals: std::ptr::null_mut(),
        injected_user_signals: std::ptr::null(),
        injected_user_signal_count: 0,
        take_release: None,
    };
    let outcome = unsafe { process_abi::chimi_scheduler_run(&config) };
    (outcome, SEEN.with(|seen| seen.borrow().clone()))
}

unsafe extern "C" fn ready_and_invalid(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    write_byte(write);
    note(wait_io(read, IO_READABLE).unwrap_or(0));
    note(wait_io(write, IO_WRITABLE).unwrap_or(0));
    note(wait_io(read, 0).unwrap_or(0));
    note(wait_io(read, 4).unwrap_or(0));
    close(read);
    close(write);
    // Not `read`: tests run in parallel and another one may reuse it.
    // Descriptors are allocated lowest first, so this one is never open.
    note(wait_io(4095, IO_READABLE).unwrap_or(0));
    note(wait_io(-1, IO_READABLE).unwrap_or(0));
    succeeded(0)
}

#[test]
fn a_ready_descriptor_answers_at_once_and_a_closed_one_or_a_bad_interest_is_an_error() {
    let (outcome, seen) = run(ready_and_invalid, process_abi::CLOCK_VIRTUAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    let error = -i64::from(RESPONSE_ERROR);
    assert_eq!(
        seen,
        [
            i64::from(IO_READABLE),
            i64::from(IO_WRITABLE),
            error,
            error,
            error,
            error
        ]
    );
}

unsafe extern "C" fn write_after_a_sleep(context: *mut c_void) -> ChimiFiberResult {
    let fd = context as usize as i32;
    sleep(2_000_000);
    note(1);
    write_byte(fd);
    succeeded(0)
}

unsafe extern "C" fn read_while_a_sibling_runs(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    let writer = start(write_after_a_sleep, write as usize as *mut c_void);
    // Nothing is in the pipe until the sibling, which sleeps first, writes.
    note(wait_io(read, IO_READABLE).unwrap_or(0));
    result(writer);
    close(read);
    close(write);
    succeeded(0)
}

#[test]
fn a_parked_wait_lets_other_tasks_run_and_wakes_when_the_descriptor_turns_ready() {
    let (outcome, seen) = run(read_while_a_sibling_runs, process_abi::CLOCK_REAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    // The writer ran (and its timer fired) while the reader was parked.
    assert_eq!(seen, [1, i64::from(IO_READABLE)]);
}

unsafe extern "C" fn sleep_one_second(_context: *mut c_void) -> ChimiFiberResult {
    sleep(1_000_000_000);
    note(now());
    succeeded(0)
}

unsafe extern "C" fn read_from_another_thread(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    let sleeper = start(sleep_one_second, ptr::null_mut());
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_byte(write);
    });
    // The sleeper's timer is the only one pending, but the wait is a host
    // operation in flight: virtual time must not jump past it.
    note(wait_io(read, IO_READABLE).unwrap_or(0));
    note(now());
    result(sleeper);
    writer.join().unwrap();
    close(read);
    close(write);
    succeeded(0)
}

#[test]
fn a_descriptor_wait_holds_virtual_time() {
    let (outcome, seen) = run(read_from_another_thread, process_abi::CLOCK_VIRTUAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [i64::from(IO_READABLE), 0, 1_000_000_000]);
}

unsafe extern "C" fn time_out_a_wait(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    let mut enter = blank(REQUEST_SCOPE_ENTER);
    enter.value = now() + 2_000_000;
    call(&mut enter);
    // The pipe never turns ready; the scope's deadline withdraws the wait.
    note(i64::from(wait_io(read, IO_READABLE).is_none()));
    let mut exit = blank(REQUEST_SCOPE_EXIT);
    exit.token = enter.token;
    exit.value = 1;
    call(&mut exit);
    note(exit.value);
    // The descriptor still works for a later wait.
    write_byte(write);
    note(wait_io(read, IO_READABLE).unwrap_or(0));
    close(read);
    close(write);
    succeeded(0)
}

#[test]
fn a_timeout_scope_withdraws_a_parked_wait() {
    let (outcome, seen) = run(time_out_a_wait, process_abi::CLOCK_REAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1, 1, i64::from(IO_READABLE)]);
}

unsafe extern "C" fn time_out_at_once(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    let mut enter = blank(REQUEST_SCOPE_ENTER);
    enter.value = now();
    call(&mut enter);
    // The deadline is the current instant: firing it moves no time, so it
    // fires although a descriptor wait holds the clock.
    note(i64::from(wait_io(read, IO_READABLE).is_none()));
    let mut exit = blank(REQUEST_SCOPE_EXIT);
    exit.token = enter.token;
    exit.value = 1;
    call(&mut exit);
    note(now());
    close(read);
    close(write);
    succeeded(0)
}

#[test]
fn a_deadline_already_due_fires_during_a_virtual_clock_wait() {
    let (outcome, seen) = run(time_out_at_once, process_abi::CLOCK_VIRTUAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1, 0]);
}

/// The signal word and wake pipe of a run, as the runner's handler sees them.
#[derive(Clone, Copy)]
struct Handler {
    word: usize,
    wake: i32,
}

unsafe extern "C" fn wait_until_signalled(context: *mut c_void) -> ChimiFiberResult {
    let handler = unsafe { *context.cast::<Handler>() };
    let (read, _write) = pipe();
    // Plays the runner's signal handler on another thread, as a real signal
    // arrives: only the wake pipe can end the scheduler's idle wait.
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        unsafe { ptr::write_volatile(handler.word as *mut i32, 15) };
        write_byte(handler.wake);
    });
    // Like a read of a terminal nobody types into: only the signal ends it.
    let waited = wait_io(read, IO_READABLE);
    raiser.join().unwrap();
    if waited.is_none() {
        return cancelled();
    }
    note(-1);
    succeeded(0)
}

#[test]
fn the_first_signal_cancels_a_root_parked_in_a_descriptor_wait() {
    let (wake_read, wake_write) = pipe();
    for fd in [wake_read, wake_write] {
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) };
    }
    let mut word = 0_i32;
    let mut handler = Handler {
        word: &mut word as *mut i32 as usize,
        wake: wake_write,
    };
    let (outcome, seen) = run_with_signal(
        wait_until_signalled,
        process_abi::CLOCK_REAL,
        (&mut handler as *mut Handler).cast(),
        &mut word,
        wake_read,
    );
    assert_eq!(outcome.state, ChimiFiberState::Cancelled as u32);
    assert!(seen.is_empty(), "{seen:?}");
    close(wake_read);
    close(wake_write);
}

/// The rendezvous channel's id and the pipe's write end.
#[derive(Clone, Copy)]
struct Busy {
    channel: u64,
    write: i32,
}

unsafe extern "C" fn ping(context: *mut c_void) -> ChimiFiberResult {
    let busy = unsafe { *context.cast::<Busy>() };
    let mut wrote = false;
    // Keeps the run queue busy until it is cancelled.
    loop {
        let mut send = blank(REQUEST_CHANNEL_SEND);
        send.channel = busy.channel;
        send.value = 1;
        if !call(&mut send) {
            return cancelled();
        }
        if !wrote {
            // The reader is parked by now: the byte arrives mid-run.
            write_byte(busy.write);
            wrote = true;
        }
    }
}

unsafe extern "C" fn read_while_others_never_idle(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    let mut bounded = blank(REQUEST_CHANNEL_BOUNDED);
    bounded.value = 0;
    call(&mut bounded);
    let reader = start(read_then_finish, read as usize as *mut c_void);
    let busy = Box::into_raw(Box::new(Busy {
        channel: bounded.channel,
        write,
    }));
    let sender = start(ping, busy.cast());
    // The root and the sender hand values back and forth, so the scheduler
    // is never idle; only the periodic poll can wake the reader.
    while !DONE.with(Cell::get) {
        let mut receive = blank(REQUEST_CHANNEL_RECEIVE);
        receive.channel = bounded.channel;
        receive.owner = bounded.owner;
        receive.token = bounded.token;
        call(&mut receive);
        assert_eq!(receive.status, RESPONSE_SUCCEEDED);
    }
    let mut cancel = blank(REQUEST_TASK_CANCEL);
    cancel.task = sender;
    call(&mut cancel);
    result(sender);
    result(reader);
    close(read);
    close(write);
    drop(unsafe { Box::from_raw(busy) });
    succeeded(0)
}

unsafe extern "C" fn read_then_finish(context: *mut c_void) -> ChimiFiberResult {
    note(wait_io(context as usize as i32, IO_READABLE).unwrap_or(0));
    DONE.with(|done| done.set(true));
    succeeded(0)
}

#[test]
fn a_ready_descriptor_wakes_its_waiter_even_when_other_tasks_never_idle() {
    let (outcome, seen) = run(read_while_others_never_idle, process_abi::CLOCK_VIRTUAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [i64::from(IO_READABLE)]);
}

/// Waits on `fd` for `resource` until woken once: the status of the reissue
/// after the first wake, or of the first answer when it does not park.
fn wait_once(fd: i32, interest: u32, resource: u64) -> i64 {
    let mut request = blank(REQUEST_IO_WAIT);
    request.value = i64::from(fd);
    request.token = u64::from(interest);
    request.channel = resource;
    if !issue(&mut request) {
        return -100;
    }
    match request.status {
        RESPONSE_PENDING => {}
        RESPONSE_OK => return request.value,
        status => return -i64::from(status),
    }
    // Woken: ask again, as a host does.
    request.value = i64::from(fd);
    if !issue(&mut request) {
        return -100;
    }
    match request.status {
        RESPONSE_OK => request.value,
        status => -i64::from(status),
    }
}

fn withdraw(resource: u64) -> (u32, i64) {
    let mut request = blank(REQUEST_IO_WITHDRAW);
    request.channel = resource;
    call(&mut request);
    (request.status, request.value)
}

unsafe extern "C" fn wait_on_resource(context: *mut c_void) -> ChimiFiberResult {
    let fd = (context as usize >> 8) as i32;
    let resource = (context as usize & 0xff) as u64;
    let answer = wait_once(fd, IO_READABLE, resource);
    note(resource as i64 * 1000 + answer);
    succeeded(0)
}

unsafe extern "C" fn withdraw_one_resource(_context: *mut c_void) -> ChimiFiberResult {
    let (first, first_write) = pipe();
    let (second, second_write) = pipe();
    let a = start(wait_on_resource, ((first as usize) << 8 | 7) as *mut c_void);
    let b = start(
        wait_on_resource,
        ((second as usize) << 8 | 8) as *mut c_void,
    );
    sleep(2_000_000);
    note(i64::from(withdraw(0).0));
    let (status, woken) = withdraw(7);
    note(i64::from(status));
    note(woken);
    // A wakes and reissues on a descriptor still empty, so it parks again;
    // then write to let it finish, and to B, which the withdraw left parked.
    sleep(2_000_000);
    write_byte(first_write);
    write_byte(second_write);
    result(a);
    result(b);
    for fd in [first, first_write, second, second_write] {
        close(fd);
    }
    succeeded(0)
}

#[test]
fn a_withdraw_wakes_only_the_waits_on_its_resource() {
    let (outcome, seen) = run(withdraw_one_resource, process_abi::CLOCK_REAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    let readable = i64::from(IO_READABLE);
    assert_eq!(
        seen[..3],
        [i64::from(RESPONSE_ERROR), i64::from(RESPONSE_OK), 1]
    );
    // A's wake came from the withdraw, so its reissue parked again until the
    // write (still pending when it returned); B's came from its descriptor.
    let mut waiters = seen[3..].to_vec();
    waiters.sort();
    assert_eq!(
        waiters,
        [7000 - i64::from(RESPONSE_PENDING), 8000 + readable]
    );
}

unsafe extern "C" fn wait_through_a_failed_poll(_context: *mut c_void) -> ChimiFiberResult {
    let (read, write) = pipe();
    reactor::FAIL_NEXT_POLL.with(|fail| fail.set(true));
    // The scheduler goes idle, its poll fails, and the reissue answers ERROR.
    note(wait_once(read, IO_READABLE, 0));
    // The failure is spent: the next wait works.
    write_byte(write);
    note(wait_once(read, IO_READABLE, 0));
    close(read);
    close(write);
    succeeded(0)
}

#[test]
fn a_failed_poll_fails_the_waits_in_flight_instead_of_spinning() {
    let (outcome, seen) = run(wait_through_a_failed_poll, process_abi::CLOCK_REAL);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [-i64::from(RESPONSE_ERROR), i64::from(IO_READABLE)]);
}
