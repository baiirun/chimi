//! Global idle with several carriers, driven on one thread (parallel-
//! scheduler.md, "Step 3 plan", PR 5b-6): a carrier other than 0 with
//! nothing to do parks on its wake pipe, and carrier 0 moves virtual time or
//! gives a verdict only once every other carrier is parked and no carrier
//! has a ready task or a job. `carrier::round_robin` stands in for the
//! carriers' threads and fails if a wake is lost.

use super::carrier::{IdleStep, KernelAccess, RunEnd, round_robin, run_carrier};
use super::ready::Carrier;
use super::*;
use crate::{ChimiFiberResult, FIXTURE_LOCK, chimi_fiber_request};
use std::ptr;

unsafe extern "C" {
    fn chimi_fixture_generated_many_parks(context: *mut std::ffi::c_void) -> ChimiFiberResult;
}

const SECOND: i64 = 1_000_000_000;

fn virtual_kernel(carriers: u32) -> Scheduler {
    let mut scheduler =
        Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Full, ClockMode::Virtual));
    scheduler.set_carriers(carriers);
    scheduler
}

/// Sleeps until the virtual time in its context, then succeeds.
unsafe extern "C" fn sleeper(context: *mut std::ffi::c_void) -> ChimiFiberResult {
    let mut request = SchedulerRequest {
        kind: REQUEST_TIMER_SLEEP_UNTIL,
        status: 0,
        task: 0,
        channel: 0,
        owner: 0,
        token: 0,
        value: context as usize as i64,
        entry: None,
        context: ptr::null_mut(),
    };
    loop {
        request.status = 0;
        // SAFETY: a request block that outlives the call, on this fiber.
        let command =
            unsafe { chimi_fiber_request((&raw mut request).cast(), ptr::null()) };
        if command == ChimiFiberCommand::Cancel {
            return ChimiFiberResult {
                state: ChimiFiberState::Cancelled as u32,
                value: 0,
                defect_code: 0,
            };
        }
        if request.status != RESPONSE_PENDING {
            return ChimiFiberResult {
                state: ChimiFiberState::Succeeded as u32,
                value: 0,
                defect_code: 0,
            };
        }
    }
}

fn start_sleeper(scheduler: &mut Scheduler, home: u32, until: i64) -> TaskId {
    scheduler.enter_carrier(Carrier(home));
    scheduler
        .start_root_with_context(sleeper, until as usize as *mut std::ffi::c_void)
        .expect("sleeper")
}

fn succeeded(scheduler: &Scheduler, task: TaskId) -> bool {
    matches!(
        scheduler.tasks.get(&task).map(|record| record.phase),
        Some(TaskPhase::Terminal(TaskOutcome::Succeeded(_))) | None
    )
}

fn readable(fd: i32) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    unsafe { libc::poll(&mut poll, 1, 0) > 0 }
}

/// Sleepers on carriers 1 and 2 each wake at their own deadline: carrier 0
/// jumps time only when both have parked, first to the earlier deadline,
/// and reports the program quiescent once both have ended.
#[test]
fn time_moves_only_once_every_other_carrier_has_parked() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    let late = start_sleeper(&mut scheduler, 1, 5 * SECOND);
    let early = start_sleeper(&mut scheduler, 2, 3 * SECOND);
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);
    let jumps: Vec<i64> = scheduler
        .clock_events()
        .iter()
        .filter(|event| event.kind == ClockEventKind::Advanced)
        .map(|event| event.time.as_nanos())
        .collect();
    assert_eq!(jumps, [3 * SECOND, 5 * SECOND]);
    assert!(succeeded(&scheduler, late) && succeeded(&scheduler, early));
}

/// A task parked for good on carrier 1 is a deadlock, but only once the
/// sleeper on carrier 2 has run to its end: carrier 0 waited for it.
#[test]
fn a_deadlock_is_reported_only_once_every_carrier_is_idle() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    scheduler.enter_carrier(Carrier(1));
    let stuck = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    let sleeper = start_sleeper(&mut scheduler, 2, 4 * SECOND);
    let Idle::Deadlock(parked) = round_robin(&mut scheduler) else {
        panic!("the parked task can never wake");
    };
    assert_eq!(parked.iter().map(|parked| parked.task).collect::<Vec<_>>(), [stuck]);
    assert_eq!(scheduler.now(), Instant::from_nanos(4 * SECOND));
    assert!(succeeded(&scheduler, sleeper));
}

/// The deadlock report and its trace events list the parked tasks by id,
/// whatever order the task table keeps them in.
#[test]
fn a_deadlock_lists_its_parked_tasks_by_id() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(1);
    let stuck: Vec<TaskId> = (0..12)
        .map(|_| {
            scheduler
                .start_root(chimi_fixture_generated_many_parks)
                .expect("task")
        })
        .collect();
    let Idle::Deadlock(parked) = round_robin(&mut scheduler) else {
        panic!("the parked tasks can never wake");
    };
    assert_eq!(parked.iter().map(|parked| parked.task).collect::<Vec<_>>(), stuck);
    let waiters: Vec<TaskId> = scheduler
        .clock_events()
        .iter()
        .filter(|event| event.kind == ClockEventKind::DeadlockWaiter)
        .filter_map(|event| event.task)
        .collect();
    assert_eq!(waiters, stuck);
}

/// Carrier 0 neither moves time nor gives a verdict while another carrier
/// runs a fiber, even with every queue empty: it parks, until that carrier
/// parks too.
#[test]
fn carrier_zero_waits_for_a_carrier_running_a_fiber() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let sleeper = start_sleeper(&mut scheduler, 1, 5 * SECOND);
    let mut resumes = 0;
    let runnable = scheduler.next_runnable(&mut resumes).expect("the sleeper's first step");
    assert!(scheduler.shards.no_ready() && scheduler.shards.no_jobs());
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "carrier 1 runs a fiber");
    scheduler.enter_carrier(Carrier(1));
    let event = resume_fiber(runnable.fiber, runnable.command);
    scheduler.complete(runnable, event);
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)));
    // A parked carrier taking the kernel again is no longer parked.
    scheduler.enter_carrier(Carrier(1));
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "carrier 1 is back");
    scheduler.enter_carrier(Carrier(1));
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)));
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.idle_step(), IdleStep::Continue), "time jumps once it parks");
    assert_eq!(scheduler.now(), Instant::from_nanos(5 * SECOND));
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);
    assert!(succeeded(&scheduler, sleeper));
}

/// Carrier 0 gives no verdict while a parked carrier has a ready task or a
/// job that another carrier handed it: it parks too, and the work still
/// runs.
#[test]
fn carrier_zero_waits_while_another_carrier_has_work() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    scheduler.enter_carrier(Carrier(1));
    let woken = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)));
    scheduler.enter_carrier(Carrier(0));
    scheduler.wake(woken).expect("wake");
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "a ready task on carrier 1");
    let Idle::Deadlock(parked) = round_robin(&mut scheduler) else {
        panic!("the woken task parks again for good");
    };
    assert_eq!(parked.iter().map(|parked| parked.task).collect::<Vec<_>>(), [woken]);
    scheduler.cancel(None, woken).expect("cancel");
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);

    scheduler.enter_carrier(Carrier(1));
    let parked = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)));
    scheduler.enter_carrier(Carrier(0));
    scheduler.cancel(None, parked).expect("cancel");
    assert!(!scheduler.shards.no_jobs(), "the unwind waits for carrier 1");
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "a job on carrier 1");
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);
    assert_eq!(
        scheduler.tasks.get(&parked).map(|record| record.phase),
        Some(TaskPhase::Terminal(TaskOutcome::Cancelled))
    );
}

/// Only the last carrier to park wakes carrier 0.
#[test]
fn the_last_carrier_to_park_wakes_carrier_zero() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    let zero = scheduler.wakes.read_fd(Carrier(0)).expect("a wake pipe");
    scheduler.enter_carrier(Carrier(1));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    assert!(!readable(zero), "carrier 2 has not parked");
    scheduler.enter_carrier(Carrier(2));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    assert!(readable(zero), "the last to park woke carrier 0");
    signal::drain(zero);
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.idle_step(), IdleStep::Done(Idle::Quiescent)));
}

/// An idle carrier other than 0 fires a timer already due, which may give
/// it work; it never waits on descriptors itself, which are carrier 0's.
#[test]
fn an_idle_carrier_fires_due_timers_and_leaves_descriptors_to_carrier_zero() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    scheduler.enter_carrier(Carrier(1));
    let task = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    let now = scheduler.now();
    scheduler.start_wake_timer(task, now).expect("timer");
    assert!(
        matches!(scheduler.idle_step(), IdleStep::Continue),
        "the due timer woke the task"
    );
    assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Runnable);
    scheduler.drive();

    let (read, write) = {
        let mut ends = [0; 2];
        // SAFETY: room for the two descriptors pipe(2) writes.
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        (ends[0], ends[1])
    };
    let mut io = SchedulerRequest {
        kind: REQUEST_IO_WAIT,
        status: 0,
        task: 0,
        channel: 0,
        owner: 0,
        token: u64::from(IO_READABLE),
        value: i64::from(read),
        entry: None,
        context: ptr::null_mut(),
    };
    scheduler.handle_io_wait(task, &mut io);
    assert_eq!(io.status, RESPONSE_PENDING);
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "carrier 1 parks");
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.idle_step(), IdleStep::Io(_)), "carrier 0 waits on it");
    scheduler.clear_io_wait(task);
    drop(scheduler);
    // SAFETY: the test opened both ends and closes each once.
    unsafe {
        libc::close(read);
        libc::close(write);
    }
}

/// Under the real clock, a carrier waiting with nothing to do sleeps only
/// until the earliest timer, so the timer fires on time: carrier 0 while
/// another carrier runs, and a parked carrier other than 0.
#[test]
fn carrier_zero_waiting_for_others_sleeps_until_the_earliest_timer() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = Scheduler::with_carriers(64 * 1024, TraceMode::Off, 2);
    let _busy = start_sleeper(&mut scheduler, 1, 0);
    scheduler.enter_carrier(Carrier(0));
    let task = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    let soon = scheduler.now().saturating_add(std::time::Duration::from_millis(5));
    scheduler.start_wake_timer(task, soon).expect("timer");
    let IdleStep::Sleep(wait, _) = scheduler.idle_step() else {
        panic!("a timer is pending, so carrier 0 sleeps until it");
    };
    assert!(wait <= std::time::Duration::from_millis(5));
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    let IdleStep::Sleep(wait, _) = scheduler.idle_step() else {
        panic!("a timer is pending, so carrier 1 parks only until it");
    };
    assert!(wait <= std::time::Duration::from_millis(5));
}

/// The last carrier to park wakes carrier 0 even with a timer pending, so
/// carrier 0, parked while that carrier still ran, jumps to the timer.
#[test]
fn the_last_carrier_to_park_wakes_carrier_zero_with_a_timer_pending() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let zero = scheduler.wakes.read_fd(Carrier(0)).expect("a wake pipe");
    let sleeper = start_sleeper(&mut scheduler, 1, 5 * SECOND);
    scheduler.drive();
    signal::drain(zero);
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    scheduler.enter_carrier(Carrier(1));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    assert!(readable(zero), "the last to park woke carrier 0");
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);
    assert!(succeeded(&scheduler, sleeper));
}

/// Carrier 0 waiting for another carrier still fires its own due timer and
/// runs the task it wakes, rather than parking with that task ready.
#[test]
fn carrier_zero_waiting_for_others_runs_its_own_due_timer() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let _busy = start_sleeper(&mut scheduler, 1, 0);
    scheduler.enter_carrier(Carrier(0));
    let task = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    let now = scheduler.now();
    scheduler.start_wake_timer(task, now).expect("timer");
    assert!(matches!(scheduler.idle_step(), IdleStep::Continue));
    assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Runnable);
}

/// While another carrier runs, carrier 0 still waits on descriptors, but
/// returns to the host for a host operation only once every other carrier
/// has parked; an ended operation wakes it.
#[test]
fn carrier_zero_serves_descriptors_while_others_run_and_the_host_after() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let _busy = start_sleeper(&mut scheduler, 1, 0);
    scheduler.enter_carrier(Carrier(0));
    let task = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    let (read, write) = {
        let mut ends = [0; 2];
        // SAFETY: room for the two descriptors pipe(2) writes.
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        (ends[0], ends[1])
    };
    let mut io = SchedulerRequest {
        kind: REQUEST_IO_WAIT,
        status: 0,
        task: 0,
        channel: 0,
        owner: 0,
        token: u64::from(IO_READABLE),
        value: i64::from(read),
        entry: None,
        context: ptr::null_mut(),
    };
    scheduler.handle_io_wait(task, &mut io);
    assert_eq!(io.status, RESPONSE_PENDING);
    assert!(matches!(scheduler.idle_step(), IdleStep::Io(_)));
    scheduler.begin_host_operation();
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "carrier 1 still runs");
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    // Parked, but handed a task it has not run yet: still not idle.
    scheduler.enter_carrier(Carrier(1));
    let parker = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    scheduler.enter_carrier(Carrier(0));
    let own = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    scheduler.wake(parker).expect("wake");
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)), "carrier 1 has a task");
    // Waiting for the others, carrier 0 still runs its own due timer (and
    // its next drive takes the task carrier 1 sleeps on).
    let now = scheduler.now();
    scheduler.start_wake_timer(own, now).expect("timer");
    assert!(matches!(scheduler.idle_step(), IdleStep::Continue), "its own timer fired");
    scheduler.drive();
    // Once carrier 1 has parked again with nothing left, the host is served.
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Park(_)));
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Done(Idle::AwaitingHost)));
    let zero = scheduler.wakes.read_fd(Carrier(0)).expect("a wake pipe");
    signal::drain(zero);
    scheduler.enter_carrier(Carrier(1));
    scheduler.with(Scheduler::end_host_operation);
    assert!(readable(zero), "an operation ended on carrier 1 wakes carrier 0");
    scheduler.clear_io_wait(task);
    drop(scheduler);
    // SAFETY: the test opened both ends and closes each once.
    unsafe {
        libc::close(read);
        libc::close(write);
    }
}

/// The end of a run marks every carrier but 0, and each one leaves its loop
/// at its next idle step instead of parking.
#[test]
fn the_end_of_a_run_wakes_every_other_carrier_and_each_leaves() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    let fds: Vec<i32> = (0..3)
        .map(|carrier| scheduler.wakes.read_fd(Carrier(carrier)).expect("a wake pipe"))
        .collect();
    let late = start_sleeper(&mut scheduler, 2, 0);
    scheduler.enter_carrier(Carrier(0));
    scheduler.with(|kernel| kernel.stop_carriers(RunEnd::Finished));
    assert!(!readable(fds[0]), "carrier 0 ended the run");
    assert!(readable(fds[1]) && readable(fds[2]));
    for carrier in [2, 1] {
        scheduler.enter_carrier(Carrier(carrier));
        run_carrier(&mut scheduler);
        assert!(matches!(scheduler.idle_step(), IdleStep::Exit));
        assert!(!scheduler.shards.is_parked(Carrier(carrier)), "it left without parking");
    }
    assert!(succeeded(&scheduler, late), "a carrier runs what it has before it leaves");
    scheduler.enter_carrier(Carrier(0));
    scheduler.begin_run(late);
    assert_eq!(scheduler.run_end, None, "the next run starts the carriers again");
}

/// The root's last idle step ends the run in the hold that sees it finish,
/// so another carrier taking the kernel next leaves; a parked root ends it
/// too, and the next run starts it again.
#[test]
fn a_finished_or_parked_root_ends_the_run_for_every_carrier() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let one = scheduler.wakes.read_fd(Carrier(1)).expect("a wake pipe");
    let root = start_sleeper(&mut scheduler, 0, 0);
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);
    assert!(succeeded(&scheduler, root));
    signal::drain(one);
    let mut deadlock = Vec::new();
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(
        scheduler.with(|kernel| kernel.root_step(root, Idle::Quiescent, 1, &mut deadlock)),
        process_abi::RootStep::Finished
    ));
    assert!(readable(one));
    scheduler.enter_carrier(Carrier(1));
    assert!(matches!(scheduler.idle_step(), IdleStep::Exit));

    scheduler.run_end = None;
    scheduler.enter_carrier(Carrier(0));
    let parked = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    signal::drain(one);
    assert!(matches!(
        scheduler.with(|kernel| kernel.root_step(parked, Idle::Stalled(Vec::new()), 1, &mut deadlock)),
        process_abi::RootStep::Parked
    ));
    assert!(
        scheduler.run_end == Some(RunEnd::Parked) && readable(one),
        "a parked root ends the run too"
    );
    scheduler.cancel(None, parked).expect("cancel");
}

/// Starts a task on `home` that parks until it is cancelled, and runs it to
/// its first park there.
fn start_parked(scheduler: &mut Scheduler, home: u32) -> TaskId {
    scheduler.enter_carrier(Carrier(home));
    let task = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.drive();
    task
}

fn holds_fiber(scheduler: &Scheduler, task: TaskId) -> bool {
    scheduler.tasks[&task].fiber.is_some()
}

/// A carrier leaving a finished run checks that the run left it nothing:
/// a fiber still homed on it could only be destroyed off its home, by
/// `Drop` on carrier 0's thread.
#[test]
fn a_carrier_leaving_a_finished_run_checks_it_holds_no_fiber() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    let left = start_parked(&mut scheduler, 1);
    let own = start_parked(&mut scheduler, 0);
    scheduler.enter_carrier(Carrier(0));
    scheduler.with(|kernel| kernel.stop_carriers(RunEnd::Finished));
    scheduler.enter_carrier(Carrier(2));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Exit), "carrier 2 holds none");
    scheduler.enter_carrier(Carrier(1));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scheduler.idle_step()));
    let message = caught.err().and_then(|panic| panic.downcast::<String>().ok());
    assert!(
        message.is_some_and(|message| message.contains("holding its fiber on carrier 1")),
        "carrier 1 still holds a fiber"
    );
    assert!(holds_fiber(&scheduler, left) && holds_fiber(&scheduler, own));
    scheduler.enter_carrier(Carrier(1));
    scheduler.cancel(None, left).expect("cancel");
    scheduler.drive();
    assert!(!holds_fiber(&scheduler, left));
    assert!(matches!(scheduler.idle_step(), IdleStep::Exit), "carrier 1 holds none now");
    scheduler.enter_carrier(Carrier(0));
    scheduler.cancel(None, own).expect("cancel");
}

/// A finished run leaves no job behind either.
#[test]
fn a_carrier_leaving_a_finished_run_checks_it_owes_no_job() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let ended = start_parked(&mut scheduler, 1);
    scheduler.enter_carrier(Carrier(0));
    scheduler.cancel(None, ended).expect("cancel");
    assert!(!scheduler.shards.jobs_empty_for(Carrier(1)), "carrier 1 owes the unwind");
    scheduler.with(|kernel| kernel.stop_carriers(RunEnd::Finished));
    scheduler.enter_carrier(Carrier(1));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scheduler.idle_step()));
    let message = caught.err().and_then(|panic| panic.downcast::<String>().ok());
    assert!(message.is_some_and(|message| message.contains("left a job on carrier 1")));
    scheduler.drive();
    assert!(scheduler.shards.no_jobs() && !holds_fiber(&scheduler, ended));
}

/// A carrier leaving a parked run keeps its fibers: the root waits on the
/// host, and a later run resumes them.
#[test]
fn a_carrier_leaving_a_parked_run_keeps_its_fibers() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let kept = start_parked(&mut scheduler, 1);
    scheduler.enter_carrier(Carrier(0));
    scheduler.with(|kernel| kernel.stop_carriers(RunEnd::Parked));
    scheduler.enter_carrier(Carrier(1));
    assert!(matches!(scheduler.with(Scheduler::idle_step), IdleStep::Exit));
    assert!(holds_fiber(&scheduler, kept));
    scheduler.cancel(None, kept).expect("cancel");
}

/// A parked task's request points into its fiber's stack, which kernel code
/// on any carrier writes through; entering the fiber clears it under the
/// lock, so no carrier writes into a stack while its fiber runs.
#[test]
fn entering_a_fiber_clears_its_parked_request() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let task = start_sleeper(&mut scheduler, 1, 5 * SECOND);
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    assert!(!scheduler.tasks[&task].request.is_null(), "parked on its sleep");
    scheduler.wake(task).expect("wake");
    let runnable = scheduler.prepare_run(task).expect("runnable");
    assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Running);
    assert!(scheduler.tasks[&task].request.is_null(), "no request while it runs");
    let event = resume_fiber(runnable.fiber, runnable.command);
    scheduler.complete(runnable, event);
    scheduler.cancel(None, task).expect("cancel");
}

/// A root step that leaves the run going does not end it: a root still
/// running, or one that has finished while another carrier still has work.
#[test]
fn a_root_still_running_leaves_the_carriers_running() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let root = start_sleeper(&mut scheduler, 0, 5 * SECOND);
    scheduler.drive();
    let mut deadlock = Vec::new();
    assert!(matches!(
        scheduler.root_step(root, Idle::AwaitingHost, 1, &mut deadlock),
        process_abi::RootStep::Continue
    ));
    assert_eq!(scheduler.run_end, None);
    let done = start_sleeper(&mut scheduler, 0, 0);
    scheduler.drive();
    assert!(succeeded(&scheduler, done));
    let _queued = start_sleeper(&mut scheduler, 1, 0);
    scheduler.enter_carrier(Carrier(0));
    assert!(matches!(
        scheduler.root_step(done, Idle::Quiescent, 1, &mut deadlock),
        process_abi::RootStep::Continue
    ));
    assert_eq!(scheduler.run_end, None, "carrier 1 still has a task");
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)));
    assert_eq!(round_robin(&mut scheduler), Idle::Quiescent);
}

/// A carrier about to wait drains the host releases queued for it first
/// (here, one a release hook queued after its last drive), and looks again
/// instead of waiting with what they woke: carrier 0 waiting for the
/// others, and a carrier other than 0 about to park.
#[test]
fn a_carrier_drains_its_host_releases_before_it_waits() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    crate::handles::begin_run();
    let _busy = start_sleeper(&mut scheduler, 1, 0);
    for carrier in [0, 1] {
        let done = start_sleeper(&mut scheduler, carrier, 0);
        scheduler.drive();
        assert!(succeeded(&scheduler, done));
        crate::handles::queue(done.0 | crate::handles::TASK_HANDLE_KEY);
        assert!(matches!(scheduler.idle_step(), IdleStep::Continue), "carrier {carrier}");
        assert_eq!(crate::handles::take(), None);
        assert!(!scheduler.tasks.contains_key(&done), "the release removed the ended task");
    }
}
