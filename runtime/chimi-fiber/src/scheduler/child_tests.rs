//! `CHIMI_REQUEST_CHILD_WAIT` through the request ABI: a wait answers at once
//! for an exited child without reaping it, parks while the child runs and
//! other tasks go on, holds virtual time, wakes on the SIGCHLD pipe byte or a
//! busy scheduler's recheck, and is
//! withdrawn by a timeout scope.

use super::*;
use crate::{ChimiFiberResult, chimi_fiber_request};
use std::cell::{Cell, RefCell};
use std::process::{Child, Command};
use std::ptr;
use std::time::{Duration, Instant as RealInstant};

thread_local! {
    static SEEN: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static WOKEN: Cell<bool> = const { Cell::new(false) };
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

/// The child wait's final status, or `None` when cancelled.
fn wait_child(pid: i64) -> Option<u32> {
    loop {
        let mut request = blank(REQUEST_CHILD_WAIT);
        request.value = pid;
        if !issue(&mut request) {
            return None;
        }
        if request.status != RESPONSE_PENDING {
            return Some(request.status);
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

/// A child that runs `script` under /bin/sh. Spawned on the test thread:
/// a fiber's stack is too small for `Command::spawn`.
fn shell(script: &str) -> Child {
    Command::new("/bin/sh")
        .args(["-c", script])
        .spawn()
        .expect("spawn sh")
}

fn pid_context(child: &Child) -> *mut c_void {
    child.id() as usize as *mut c_void
}

fn pipe() -> (i32, i32) {
    let mut ends = [0; 2];
    assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
    for end in ends {
        unsafe { libc::fcntl(end, libc::F_SETFL, libc::O_NONBLOCK) };
    }
    (ends[0], ends[1])
}

/// Asserts the child is still unreaped, then reaps it.
fn reap(mut child: Child) -> std::process::ExitStatus {
    child
        .wait()
        .expect("the scheduler left the child for the host to reap")
}

fn run(
    entry: ChimiFiberEntry,
    clock_mode: u32,
    context: *mut c_void,
) -> (SchedulerOutcome, Vec<i64>) {
    run_with_signal(entry, clock_mode, context, -1)
}

fn run_with_signal(
    entry: ChimiFiberEntry,
    clock_mode: u32,
    context: *mut c_void,
    signal_fd: i32,
) -> (SchedulerOutcome, Vec<i64>) {
    SEEN.with(|seen| seen.borrow_mut().clear());
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
        signal: ptr::null_mut(),
        signal_fd,
        injected_signal: 0,
        injected_signal_at: 0,
        copy_channel_value: None,
        user_signals: ptr::null_mut(),
        injected_user_signals: std::ptr::null(),
        injected_user_signal_count: 0,
        carriers: 0,
        carrier_enter: None,
        carrier_context: ptr::null_mut(),
    };
    let outcome = unsafe { process_abi::chimi_scheduler_run(&config) };
    (outcome, SEEN.with(|seen| seen.borrow().clone()))
}

unsafe extern "C" fn wait_for_context_pid(context: *mut c_void) -> ChimiFiberResult {
    let pid = context as usize as i64;
    note(i64::from(wait_child(pid).unwrap_or(0)));
    // The child stays a zombie, so asking again answers again.
    note(i64::from(wait_child(pid).unwrap_or(0)));
    note(i64::from(wait_child(0).unwrap_or(0)));
    note(i64::from(wait_child(-1).unwrap_or(0)));
    // pid 1 is never this process's child.
    note(i64::from(wait_child(1).unwrap_or(0)));
    succeeded(0)
}

#[test]
fn an_exited_child_answers_at_once_and_stays_unreaped_and_a_non_child_is_an_error() {
    let child = shell("exit 3");
    while matches!(
        {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            unsafe { info.si_pid() }
        },
        0
    ) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let (outcome, seen) = run(
        wait_for_context_pid,
        process_abi::CLOCK_VIRTUAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    let (ok, error) = (i64::from(RESPONSE_OK), i64::from(RESPONSE_ERROR));
    assert_eq!(seen, [ok, ok, error, error, error]);
    assert_eq!(reap(child).code(), Some(3));
}

unsafe extern "C" fn count_while_parked(_context: *mut c_void) -> ChimiFiberResult {
    for _ in 0..3 {
        note(1);
        sleep(1_000_000);
    }
    succeeded(0)
}

unsafe extern "C" fn wait_while_a_sibling_runs(context: *mut c_void) -> ChimiFiberResult {
    let sibling = start(count_while_parked, ptr::null_mut());
    note(i64::from(wait_child(context as usize as i64).unwrap_or(0)));
    result(sibling);
    succeeded(0)
}

#[test]
fn a_running_child_parks_the_wait_while_other_tasks_run_and_its_exit_wakes_it() {
    // No SIGCHLD source is connected: the idle recheck finds the exit.
    let child = shell("exec sleep 0.2");
    let (outcome, seen) = run(
        wait_while_a_sibling_runs,
        process_abi::CLOCK_REAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1, 1, 1, i64::from(RESPONSE_OK)]);
    assert!(reap(child).success());
}

unsafe extern "C" fn sleep_one_second(_context: *mut c_void) -> ChimiFiberResult {
    sleep(1_000_000_000);
    note(now());
    succeeded(0)
}

unsafe extern "C" fn wait_beside_a_sleeper(context: *mut c_void) -> ChimiFiberResult {
    let sleeper = start(sleep_one_second, ptr::null_mut());
    // The sleeper's timer is the only one pending, but the wait is a host
    // operation in flight: virtual time must not jump past it.
    note(i64::from(wait_child(context as usize as i64).unwrap_or(0)));
    note(now());
    result(sleeper);
    succeeded(0)
}

#[test]
fn a_child_wait_holds_virtual_time() {
    let child = shell("exec sleep 0.05");
    let (outcome, seen) = run(
        wait_beside_a_sleeper,
        process_abi::CLOCK_VIRTUAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [i64::from(RESPONSE_OK), 0, 1_000_000_000]);
    assert!(reap(child).success());
}

unsafe extern "C" fn wait_and_time_it(context: *mut c_void) -> ChimiFiberResult {
    let started = RealInstant::now();
    note(i64::from(wait_child(context as usize as i64).unwrap_or(0)));
    note(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
    succeeded(0)
}

/// Blocks until `pid` has exited, without reaping it.
fn await_exit(pid: u32) {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if unsafe { info.si_pid() } != 0 {
            return;
        }
    }
}

#[test]
fn the_sigchld_pipe_byte_ends_an_idle_wait_before_the_backed_off_recheck() {
    // Without the byte the rechecks back off (10, 20, 40 ms ...) and the
    // first one after the exit at 1.5 s comes at about 2.27 s.
    let child = shell("exec sleep 1.5");
    let (read, write) = pipe();
    let pid = child.id();
    // Plays the runner's SIGCHLD handler.
    let handler = std::thread::spawn(move || {
        await_exit(pid);
        assert_eq!(unsafe { libc::write(write, b"x".as_ptr().cast(), 1) }, 1);
    });
    let (outcome, seen) = run_with_signal(
        wait_and_time_it,
        process_abi::CLOCK_REAL,
        pid_context(&child),
        read,
    );
    handler.join().unwrap();
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen[0], i64::from(RESPONSE_OK));
    assert!(seen[1] < 2100, "woken after {} ms", seen[1]);
    assert!(reap(child).success());
    unsafe {
        libc::close(read);
        libc::close(write);
    }
}

unsafe extern "C" fn finish_at_once(_context: *mut c_void) -> ChimiFiberResult {
    succeeded(0)
}

unsafe extern "C" fn wait_and_flag(context: *mut c_void) -> ChimiFiberResult {
    let status = wait_child(context as usize as i64).unwrap_or(0);
    WOKEN.with(|woken| woken.set(true));
    succeeded(i64::from(status))
}

unsafe extern "C" fn stay_busy_beside_a_waiter(context: *mut c_void) -> ChimiFiberResult {
    let waiter = start(wait_and_flag, context);
    let started = RealInstant::now();
    // Starting and awaiting a task never leaves the run queue empty, so the
    // scheduler never goes idle while this loops.
    while !WOKEN.with(Cell::get) && started.elapsed() < Duration::from_secs(5) {
        result(start(finish_at_once, ptr::null_mut()));
    }
    note(i64::from(WOKEN.with(Cell::get)));
    result(waiter);
    succeeded(0)
}

#[test]
fn a_busy_scheduler_still_wakes_a_waiter_whose_child_exited() {
    WOKEN.with(|woken| woken.set(false));
    let child = shell("exec sleep 0.05");
    let (outcome, seen) = run(
        stay_busy_beside_a_waiter,
        process_abi::CLOCK_REAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1], "the waiter woke while the scheduler was busy");
    assert!(reap(child).success());
}

#[test]
fn a_child_reaped_by_someone_else_while_waited_on_is_an_error() {
    let child = shell("exec sleep 0.05");
    let pid = child.id();
    let reaper = std::thread::spawn(move || {
        let mut status = 0;
        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) }
    });
    let (outcome, seen) = run(wait_for_one, process_abi::CLOCK_REAL, pid_context(&child));
    assert_eq!(reaper.join().unwrap(), pid as libc::pid_t);
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [i64::from(RESPONSE_ERROR)]);
    std::mem::forget(child);
}

unsafe extern "C" fn wait_for_one(context: *mut c_void) -> ChimiFiberResult {
    note(i64::from(wait_child(context as usize as i64).unwrap_or(0)));
    succeeded(0)
}

unsafe extern "C" fn time_out_a_child_wait(context: *mut c_void) -> ChimiFiberResult {
    let mut enter = blank(REQUEST_SCOPE_ENTER);
    enter.value = now() + 20_000_000;
    call(&mut enter);
    note(i64::from(wait_child(context as usize as i64).is_none()));
    let mut exit = blank(REQUEST_SCOPE_EXIT);
    exit.token = enter.token;
    exit.value = 1;
    call(&mut exit);
    note(exit.value);
    succeeded(0)
}

#[test]
fn a_timeout_scope_withdraws_a_parked_child_wait() {
    let mut child = shell("exec sleep 30");
    let (outcome, seen) = run(
        time_out_a_child_wait,
        process_abi::CLOCK_REAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1, 1]);
    child.kill().unwrap();
    assert!(!reap(child).success());
}

unsafe extern "C" fn wait_until_cancelled(context: *mut c_void) -> ChimiFiberResult {
    if wait_child(context as usize as i64).is_none() {
        return cancelled();
    }
    succeeded(1)
}

unsafe extern "C" fn cancel_a_waiter_then_sleep(context: *mut c_void) -> ChimiFiberResult {
    let started = RealInstant::now();
    let waiter = start(wait_until_cancelled, context);
    // Let the waiter park.
    sleep(0);
    let mut cancel = blank(REQUEST_TASK_CANCEL);
    cancel.task = waiter;
    call(&mut cancel);
    result(waiter);
    // The withdrawn wait no longer holds virtual time, so this sleep jumps.
    sleep(1_000_000_000);
    note(now());
    note(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
    succeeded(0)
}

#[test]
fn a_cancelled_waiter_stops_holding_virtual_time() {
    let child = shell("exec sleep 0.5");
    let (outcome, seen) = run(
        cancel_a_waiter_then_sleep,
        process_abi::CLOCK_VIRTUAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen[0], 1_000_000_000);
    assert!(seen[1] < 400, "time was held for {} ms", seen[1]);
    assert!(reap(child).success());
}

fn cancelled() -> ChimiFiberResult {
    ChimiFiberResult {
        state: ChimiFiberState::Cancelled as u32,
        value: 0,
        defect_code: 0,
    }
}

unsafe extern "C" fn wait_inside_an_expired_scope(context: *mut c_void) -> ChimiFiberResult {
    let mut enter = blank(REQUEST_SCOPE_ENTER);
    enter.value = now();
    call(&mut enter);
    // The child has exited, but the scope's deadline has passed (real time
    // moved on since `now`): a cancellation point refuses the wait first.
    note(i64::from(wait_child(context as usize as i64).is_none()));
    let mut exit = blank(REQUEST_SCOPE_EXIT);
    exit.token = enter.token;
    exit.value = 1;
    call(&mut exit);
    succeeded(0)
}

#[test]
fn a_child_wait_is_a_cancellation_point() {
    let child = shell("exit 0");
    await_exit(child.id());
    let (outcome, seen) = run(
        wait_inside_an_expired_scope,
        process_abi::CLOCK_REAL,
        pid_context(&child),
    );
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [1]);
    assert!(reap(child).success());
}

/// The resource key the child wait below names.
const CHILD_RESOURCE: u64 = 7 << 32 | 1;

unsafe extern "C" fn wait_on_a_resource(context: *mut c_void) -> ChimiFiberResult {
    let mut request = blank(REQUEST_CHILD_WAIT);
    request.value = context as usize as i64;
    request.channel = CHILD_RESOURCE;
    issue(&mut request);
    note(i64::from(request.status));
    succeeded(0)
}

unsafe extern "C" fn withdraw_a_child_wait(context: *mut c_void) -> ChimiFiberResult {
    let waiter = start(wait_on_a_resource, context);
    // Let the waiter park (a zero sleep does not yield to a new task).
    sleep(10_000_000);
    let mut other = blank(REQUEST_IO_WITHDRAW);
    other.channel = CHILD_RESOURCE + 1;
    call(&mut other);
    note(other.value);
    let mut withdraw = blank(REQUEST_IO_WITHDRAW);
    withdraw.channel = CHILD_RESOURCE;
    call(&mut withdraw);
    note(withdraw.value);
    result(waiter);
    succeeded(0)
}

/// A withdraw of a child wait's resource wakes it while the child still runs,
/// as it does an IO wait, so a host that closes the resource can wake a task
/// waiting on its child; a withdraw of another key wakes nothing.
#[test]
fn a_withdraw_wakes_a_child_wait_on_its_resource() {
    let mut child = shell("exec sleep 30");
    let (outcome, seen) = run(withdraw_a_child_wait, process_abi::CLOCK_REAL, pid_context(&child));
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [0, 1, i64::from(RESPONSE_PENDING)]);
    child.kill().unwrap();
    assert!(!reap(child).success());
}

unsafe extern "C" fn wait_after_a_withdraw(context: *mut c_void) -> ChimiFiberResult {
    let mut withdraw = blank(REQUEST_IO_WITHDRAW);
    withdraw.channel = CHILD_RESOURCE;
    call(&mut withdraw);
    let started = now();
    let mut request = blank(REQUEST_CHILD_WAIT);
    request.value = context as usize as i64;
    request.channel = CHILD_RESOURCE;
    issue(&mut request);
    note(i64::from(request.status));
    note(i64::from(now() - started < 1_000_000_000));
    succeeded(0)
}

/// A child wait on a withdrawn key answers PENDING at once, while the child
/// still runs, as an IO wait does (see reactor.rs).
#[test]
fn a_child_wait_on_a_withdrawn_key_answers_at_once() {
    let mut child = shell("exec sleep 5");
    let (outcome, seen) = run(wait_after_a_withdraw, process_abi::CLOCK_REAL, pid_context(&child));
    assert_eq!(outcome.state, ChimiFiberState::Succeeded as u32);
    assert_eq!(seen, [i64::from(RESPONSE_PENDING), 1]);
    child.kill().unwrap();
    assert!(!reap(child).success());
}
