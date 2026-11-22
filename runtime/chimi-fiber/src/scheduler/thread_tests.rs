//! Carriers on real threads (parallel-scheduler.md, "Step 3 plan", PR 5c-6):
//! `process_abi::run_root_on_threads` runs carrier 0 on the test's thread
//! and every other carrier on a thread of its own. Each test loops under a
//! watchdog, since a lost wake shows up as a hang, and its fibers check that
//! they never change thread and never run with the kernel lock held.

use super::process_abi::{
    CLOCK_REAL, CLOCK_VIRTUAL, ChimiSchedulerConfig, EnterHook, chimi_scheduler_run, run_on_carriers,
    run_root_on_threads, test_hooks,
};
use super::ready::Carrier;
use super::*;
use crate::{ChimiFiberResult, FIXTURE_LOCK, chimi_fiber_request};
use std::collections::BTreeSet;
use std::ptr;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

unsafe extern "C" {
    fn chimi_fixture_generated_many_parks(context: *mut std::ffi::c_void) -> ChimiFiberResult;
    fn chimi_fixture_two_crashes_main(context: *mut std::ffi::c_void) -> ChimiFiberResult;
    fn chimi_fixture_waits_on_a_member_main(context: *mut std::ffi::c_void) -> ChimiFiberResult;
}

/// A fiber that finds itself on another thread, or holding the kernel.
const MOVED: i64 = 77;
const HELD: i64 = 78;

/// Every thread a fiber has run on, across a test.
static THREADS: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

fn this_thread() -> u64 {
    // SAFETY: `pthread_self` has no preconditions.
    unsafe { libc::pthread_self() as u64 }
}

fn request(kind: u32, value: i64) -> SchedulerRequest {
    SchedulerRequest {
        kind,
        status: 0,
        task: 0,
        channel: 0,
        owner: 0,
        token: 0,
        value,
        entry: None,
        context: ptr::null_mut(),
    }
}

/// Issues `request` until it is no longer pending; false if cancelled.
fn call(request: &mut SchedulerRequest) -> bool {
    loop {
        request.status = 0;
        // SAFETY: a request block that outlives the call, on this fiber.
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

fn ended(state: ChimiFiberState, defect_code: i64) -> ChimiFiberResult {
    ChimiFiberResult {
        state: state as u32,
        value: 0,
        defect_code,
    }
}

/// Sleeps one microsecond as many times as its context says, checking
/// after every park that it is on the thread it started on and that the
/// carrier resumed it without the kernel.
unsafe extern "C" fn hops(context: *mut std::ffi::c_void) -> ChimiFiberResult {
    let home = this_thread();
    THREADS.lock().expect("threads").insert(home);
    for _ in 0..context as usize {
        let mut now = request(REQUEST_TIMER_NOW, 0);
        let mut sleep = request(REQUEST_TIMER_SLEEP_UNTIL, 0);
        if !call(&mut now) || {
            sleep.value = now.value + 1_000;
            !call(&mut sleep)
        } {
            return ended(ChimiFiberState::Cancelled, 0);
        }
        if this_thread() != home {
            return ended(ChimiFiberState::Defected, MOVED);
        }
        if carrier::kernel_held() {
            return ended(ChimiFiberState::Defected, HELD);
        }
    }
    ended(ChimiFiberState::Succeeded, 0)
}

/// Sleeps one microsecond as many times as its context says and returns
/// how many of its parks resumed on another thread than the park before,
/// checking after every park that the carrier resumed it without the kernel.
unsafe extern "C" fn wanders(context: *mut std::ffi::c_void) -> ChimiFiberResult {
    let mut thread = this_thread();
    let mut moves = 0;
    let mut parked = |request: &mut SchedulerRequest| -> Option<bool> {
        if !call(request) {
            return None;
        }
        if this_thread() != thread {
            thread = this_thread();
            moves += 1;
        }
        Some(carrier::kernel_held())
    };
    for _ in 0..context as usize {
        let mut now = request(REQUEST_TIMER_NOW, 0);
        let Some(held) = parked(&mut now) else {
            return ended(ChimiFiberState::Cancelled, 0);
        };
        let mut sleep = request(REQUEST_TIMER_SLEEP_UNTIL, now.value + 1_000);
        let Some(held_too) = parked(&mut sleep) else {
            return ended(ChimiFiberState::Cancelled, 0);
        };
        if held || held_too {
            return ended(ChimiFiberState::Defected, HELD);
        }
    }
    ChimiFiberResult {
        state: ChimiFiberState::Succeeded as u32,
        value: moves,
        defect_code: 0,
    }
}

/// Runs `test` on another thread and aborts the test binary if it has not
/// finished within `limit`: a carrier that waits forever is a lost wake,
/// and a hang would otherwise only time the whole suite out.
pub(super) fn under_watchdog(limit: Duration, test: impl FnOnce() + Send) {
    let (done, finished) = mpsc::channel();
    std::thread::scope(|scope| {
        let runner = scope.spawn(move || {
            test();
            let _ = done.send(());
        });
        if finished.recv_timeout(limit) == Err(mpsc::RecvTimeoutError::Timeout) {
            eprintln!("a run on carrier threads hung for {limit:?}");
            std::process::abort();
        }
        if let Err(panic) = runner.join() {
            std::panic::resume_unwind(panic);
        }
    });
}

/// Starts `per_carrier` hopping roots on every carrier and runs the first
/// as the root on threads; every task must succeed, and the fibers must
/// have run on one thread per carrier. Runs twice on the same kernel, so
/// the second run's carriers start after the first run ended.
fn run_hoppers(clock: ClockMode, carriers: u32, per_carrier: u32, hops_each: usize) {
    let mut scheduler = Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, clock));
    scheduler.set_carriers(carriers);
    for _ in 0..2 {
        let mut tasks = Vec::new();
        for carrier in 0..carriers {
            scheduler.enter_carrier(Carrier(carrier));
            for _ in 0..per_carrier {
                tasks.push(
                    scheduler
                        .start_root_with_context(hops, hops_each as *mut std::ffi::c_void)
                        .expect("task"),
                );
            }
        }
        scheduler.enter_carrier(Carrier(0));
        THREADS.lock().expect("threads").clear();
        let run = run_root_on_threads(&mut scheduler, tasks[0], 1);
        // Threads alive at once have distinct ids, and every carrier has
        // fibers homed on it.
        assert_eq!(THREADS.lock().expect("threads").len(), carriers as usize, "one thread per carrier");
        assert_eq!(run.outcome.state, ChimiFiberState::Succeeded as u32, "{:?}", run.outcome.defect_code);
        for task in tasks {
            assert!(
                matches!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Succeeded(_))),
                "task {task:?}: {:?}",
                scheduler.tasks[&task].phase
            );
        }
        assert_eq!(scheduler.run_end, Some(carrier::RunEnd::Finished));
    }
}

/// Fibers homed on four carriers run on four threads under virtual time,
/// each staying on its own thread, and the run ends with every carrier
/// joined, again and again.
#[test]
fn fibers_on_four_carrier_threads_stay_on_their_thread() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(60), || {
        for _ in 0..40 {
            run_hoppers(ClockMode::Virtual, 4, 3, 20);
        }
    });
}

/// The same under the real clock, where an idle carrier sleeps until the
/// earliest timer or a wake on its pipe.
#[test]
fn fibers_on_carrier_threads_under_the_real_clock() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(60), || {
        for _ in 0..10 {
            run_hoppers(ClockMode::Real, 3, 2, 10);
        }
    });
}

/// A parked task the kernel moves to the next carrier each time it becomes
/// ready resumes on that carrier's thread, parks, and moves again: every
/// wanderer changes thread at every park, under both clocks, and ends.
/// (parallel-scheduler.md, step 4 PR 4b.)
#[test]
fn a_parked_task_moved_to_another_carrier_resumes_on_its_thread() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(60), || {
        for clock in [ClockMode::Virtual, ClockMode::Real] {
            for _ in 0..10 {
                let mut scheduler = Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, clock));
                scheduler.set_carriers(3);
                scheduler.enter_carrier(Carrier(0));
                let parks = 20;
                // The root outlives the wanderers: a run ends with its root,
                // and what is left then finishes on carrier 0's thread.
                let root = scheduler
                    .start_root_with_context(hops, (4 * parks) as *mut std::ffi::c_void)
                    .expect("root");
                let wanderers: Vec<TaskId> = (0..6)
                    .map(|_| {
                        scheduler
                            .start_root_with_context(wanders, parks as *mut std::ffi::c_void)
                            .expect("wanderer")
                    })
                    .collect();
                scheduler.rotating.extend(wanderers.iter().copied());
                let run = run_root_on_threads(&mut scheduler, root, 1);
                assert_eq!(run.outcome.state, ChimiFiberState::Succeeded as u32);
                for task in wanderers {
                    assert!(
                        matches!(
                            scheduler.tasks[&task].phase,
                            TaskPhase::Terminal(TaskOutcome::Succeeded(moves)) if moves == 2 * parks
                        ),
                        "task {task:?}: {:?}",
                        scheduler.tasks[&task].phase
                    );
                }
            }
        }
    });
}

/// One carrier on threads is the plain run: no carrier thread is spawned,
/// and every fiber runs on the one thread.
#[test]
fn one_carrier_on_threads_runs_on_one_thread() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(30), || run_hoppers(ClockMode::Virtual, 1, 4, 5));
}

/// A run on several threads that parks is refused once its carriers are
/// joined: the root waits on the host, and fibers homed on a joined thread
/// could never be resumed or destroyed.
#[test]
fn a_run_on_several_threads_that_parks_is_refused() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler =
        Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Real));
    scheduler.set_carriers(2);
    scheduler.enter_carrier(Carrier(0));
    let root = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("root");
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_root_on_threads(&mut scheduler, root, 1)
    }));
    let message = refused.err().and_then(|panic| panic.downcast::<&str>().ok());
    assert!(
        message.is_some_and(|message| message.contains("parked")),
        "the parked run is refused"
    );
    assert_eq!(scheduler.run_end, Some(carrier::RunEnd::Parked));
    scheduler.enter_carrier(Carrier(0));
    scheduler.cancel(None, root).expect("cancel");
}

#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(depth: u64) -> u64 {
    let frame = std::hint::black_box([depth; 64]);
    recurse(frame[0] + 1) + frame[1]
}

/// Parks once, so its carrier's thread is its home, then overflows its
/// stack there.
unsafe extern "C" fn overflows(_context: *mut std::ffi::c_void) -> ChimiFiberResult {
    // SAFETY: `hops` with one hop, on this fiber.
    unsafe { hops(ptr::without_provenance_mut(1)) };
    std::hint::black_box(recurse(0));
    ended(ChimiFiberState::Succeeded, 0)
}

/// A fiber that overflows its stack on a carrier thread ends in a stack
/// overflow, caught on that thread's own signal stack, and the run goes on.
#[test]
fn a_stack_overflow_on_a_carrier_thread_ends_only_its_task() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(30), || {
        let mut scheduler =
            Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Virtual));
        scheduler.set_carriers(2);
        scheduler.enter_carrier(Carrier(0));
        let root = scheduler
            .start_root_with_context(hops, 5 as *mut std::ffi::c_void)
            .expect("root");
        scheduler.enter_carrier(Carrier(1));
        let overflowing = scheduler.start_root(overflows).expect("overflowing root");
        scheduler.enter_carrier(Carrier(0));
        let run = run_root_on_threads(&mut scheduler, root, 1);
        assert_eq!(run.outcome.state, ChimiFiberState::Succeeded as u32);
        assert!(
            matches!(
                scheduler.tasks[&overflowing].phase,
                TaskPhase::Terminal(TaskOutcome::StackOverflow { .. })
            ),
            "{:?}",
            scheduler.tasks[&overflowing].phase
        );
    });
}

/// A carrier that cannot get a thread fails the run before any carrier
/// touches the kernel: the carriers already spawned leave at the gate, the
/// run defects with its roots unstarted, and the same kernel runs them all
/// once threads can be had.
#[test]
fn a_carrier_without_a_thread_fails_the_run_before_it_starts() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(30), || {
        let mut scheduler =
            Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Virtual));
        scheduler.set_carriers(3);
        let mut tasks = Vec::new();
        for carrier in 0..3 {
            scheduler.enter_carrier(Carrier(carrier));
            tasks.push(
                scheduler
                    .start_root_with_context(hops, 3 as *mut std::ffi::c_void)
                    .expect("task"),
            );
        }
        scheduler.enter_carrier(Carrier(0));
        test_hooks::FAIL_SPAWN_OF.with(|carrier| carrier.set(Some(2)));
        let failed = run_root_on_threads(&mut scheduler, tasks[0], 1);
        test_hooks::FAIL_SPAWN_OF.with(|carrier| carrier.set(None));
        assert_eq!(failed.outcome.state, ChimiFiberState::Defected as u32);
        assert_eq!(failed.outcome.defect_code, SCHEDULER_DEFECT_CODE);
        for task in &tasks {
            assert!(scheduler.tasks[task].phase.is_live(), "task {task:?} ran: {:?}", scheduler.tasks[task].phase);
        }
        THREADS.lock().expect("threads").clear();
        let run = run_root_on_threads(&mut scheduler, tasks[0], 1);
        assert_eq!(run.outcome.state, ChimiFiberState::Succeeded as u32);
        for task in &tasks {
            assert!(matches!(scheduler.tasks[task].phase, TaskPhase::Terminal(TaskOutcome::Succeeded(_))));
        }
        assert_eq!(THREADS.lock().expect("threads").len(), 3);
    });
}

/// The carriers `record_enter` saw entered, each with its thread.
static ENTERED: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());

unsafe extern "C" fn record_enter(context: *mut std::ffi::c_void, carrier: u32) {
    assert_eq!(context as usize, 0xC0, "the hook gets carrier_context");
    assert!(!carrier::kernel_held(), "the enter hook runs outside the lock");
    ENTERED.lock().expect("entered").push((carrier, this_thread()));
}

/// Every trace event's family and kind, without its time, sequence or the
/// records it names: what a run's trace says whichever member crashes
/// first.
unsafe extern "C" fn collect_kinds(context: *mut std::ffi::c_void, event: *const SchedulerTraceEvent) {
    // SAFETY: the context is the test's Mutex, and the event is valid for the call.
    let (events, event) = unsafe { (&*context.cast::<Mutex<Vec<(u32, u32)>>>(), &*event) };
    events.lock().expect("events").push((event.family, event.kind));
}

fn abi_run(
    entry: ChimiFiberEntry,
    clock_mode: u32,
    carriers: u32,
    events: &Mutex<Vec<(u32, u32)>>,
) -> SchedulerOutcome {
    let config = ChimiSchedulerConfig {
        entry: Some(entry),
        context: ptr::null_mut(),
        stack_bytes: 64 * 1024,
        trace_mode: 1,
        clock_mode,
        trace_sink: Some(collect_kinds),
        trace_context: (events as *const Mutex<Vec<(u32, u32)>>).cast_mut().cast(),
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
        user_signals: ptr::null_mut(),
        injected_user_signals: ptr::null(),
        injected_user_signal_count: 0,
        take_release: None,
        carriers,
        carrier_enter: Some(record_enter),
        carrier_context: ptr::without_provenance_mut(0xC0),
    };
    ENTERED.lock().expect("entered").clear();
    // SAFETY: a valid config whose hooks and contexts outlive the call.
    unsafe { chimi_scheduler_run(&config) }
}

/// Through the ABI, `carriers` above 1 runs the supervisor fixture's
/// members on carrier threads the scheduler starts: the enter hook runs
/// once on each, never on the caller's, and the run escalates as on one
/// carrier, with the same kinds of trace events. 0 and 1 both mean one carrier and
/// call no hook.
#[test]
fn the_abi_runs_a_supervisor_on_the_carriers_its_config_asks_for() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(60), || {
        let one = Mutex::new(Vec::new());
        let outcome = abi_run(chimi_fixture_two_crashes_main, CLOCK_REAL, 1, &one);
        assert_eq!(outcome.defect_code, RESTART_LIMIT_DEFECT_CODE);
        assert!(ENTERED.lock().expect("entered").is_empty(), "one carrier starts no thread");
        let zero = Mutex::new(Vec::new());
        assert_eq!(abi_run(chimi_fixture_two_crashes_main, CLOCK_REAL, 0, &zero).defect_code, RESTART_LIMIT_DEFECT_CODE);
        assert!(ENTERED.lock().expect("entered").is_empty(), "0 is one carrier");
        let mut expected = one.into_inner().expect("events");
        expected.sort_unstable();
        for _ in 0..50 {
            let three = Mutex::new(Vec::new());
            let outcome = abi_run(chimi_fixture_two_crashes_main, CLOCK_REAL, 3, &three);
            assert_eq!(
                (outcome.state, outcome.defect_code),
                (ChimiFiberState::Defected as u32, RESTART_LIMIT_DEFECT_CODE)
            );
            let mut entered = ENTERED.lock().expect("entered").clone();
            entered.sort_unstable();
            assert_eq!(entered.iter().map(|&(carrier, _)| carrier).collect::<Vec<_>>(), [1, 2]);
            assert!(entered[0].1 != entered[1].1, "each carrier on its own thread");
            assert!(entered.iter().all(|&(_, thread)| thread != this_thread()), "not on the caller's");
            let mut events = three.into_inner().expect("events");
            events.sort_unstable();
            assert_eq!(events, expected, "the same trace events as one carrier");
        }
    });
}

/// More than one carrier under the virtual clock is refused before any
/// task runs: virtual time needs every carrier idle to jump, which a
/// carrier on its own thread cannot promise.
#[test]
fn the_abi_refuses_several_carriers_under_virtual_time() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let events = Mutex::new(Vec::new());
    let outcome = abi_run(chimi_fixture_two_crashes_main, CLOCK_VIRTUAL, 2, &events);
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, SCHEDULER_DEFECT_CODE)
    );
    assert!(events.into_inner().expect("events").is_empty(), "no task ran");
    assert!(ENTERED.lock().expect("entered").is_empty());
}

/// A run on two carriers that nothing can wake returns parked through the
/// ABI, as one carrier's does: the kernel is left in place, with the
/// member's fiber parked on carrier 1's thread, which has been joined.
/// Destroying that fiber from the caller's thread would trip its home
/// check.
#[test]
fn a_parked_run_on_two_carriers_returns_parked_through_the_abi() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(30), || {
        let events = Mutex::new(Vec::new());
        let outcome = abi_run(chimi_fixture_waits_on_a_member_main, CLOCK_REAL, 2, &events);
        assert_eq!(outcome.state, ChimiFiberState::Parked as u32);
        assert_eq!(ENTERED.lock().expect("entered").iter().map(|&(carrier, _)| carrier).collect::<Vec<_>>(), [1]);
    });
}


/// A fiber on a carrier thread whose enter hook had not run there.
const UNBOUND: i64 = 79;

/// The thread the run under test was called on, and whether this thread's
/// carrier was entered.
static CALLER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
thread_local! {
    static BOUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "C" fn bind_carrier(_context: *mut std::ffi::c_void, _carrier: u32) {
    BOUND.with(|bound| bound.set(true));
}

/// `hops`, first checking that a fiber on a carrier thread finds the
/// thread entered, as a C fiber needs its carrier bound before it runs.
unsafe extern "C" fn bound_hops(context: *mut std::ffi::c_void) -> ChimiFiberResult {
    let caller = CALLER.load(std::sync::atomic::Ordering::Relaxed);
    if this_thread() != caller && !BOUND.with(std::cell::Cell::get) {
        return ended(ChimiFiberState::Defected, UNBOUND);
    }
    // SAFETY: on this fiber, with the context `hops` takes.
    unsafe { hops(context) }
}

/// The enter hook runs on each carrier thread before that carrier runs its
/// first fiber.
#[test]
fn a_carrier_thread_is_entered_before_its_first_fiber() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(30), || {
        CALLER.store(this_thread(), std::sync::atomic::Ordering::Relaxed);
        let mut scheduler =
            Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Virtual));
        scheduler.set_carriers(3);
        let mut tasks = Vec::new();
        for carrier in 0..3 {
            scheduler.enter_carrier(Carrier(carrier));
            for _ in 0..2 {
                tasks.push(
                    scheduler
                        .start_root_with_context(bound_hops, 3 as *mut std::ffi::c_void)
                        .expect("task"),
                );
            }
        }
        scheduler.enter_carrier(Carrier(0));
        THREADS.lock().expect("threads").clear();
        let enter = EnterHook {
            hook: bind_carrier,
            context: ptr::null_mut(),
        };
        let (run, scheduler) = run_on_carriers(scheduler, tasks[0], 1, Some(enter));
        assert_eq!(run.outcome.state, ChimiFiberState::Succeeded as u32, "{}", run.outcome.defect_code);
        for task in &tasks {
            assert!(
                matches!(scheduler.tasks[task].phase, TaskPhase::Terminal(TaskOutcome::Succeeded(_))),
                "{:?}",
                scheduler.tasks[task].phase
            );
        }
        assert_eq!(THREADS.lock().expect("threads").len(), 3);
    });
}

/// A carrier whose wake pipe cannot be opened fails the run through the ABI
/// as a scheduler defect, before any task or hook runs.
#[test]
fn the_abi_defects_when_a_carrier_gets_no_wake_pipe() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let events = Mutex::new(Vec::new());
    super::wake::test_hooks::FAIL_OPEN_AFTER.with(|left| left.set(Some(1)));
    let outcome = abi_run(chimi_fixture_two_crashes_main, CLOCK_REAL, 3, &events);
    super::wake::test_hooks::FAIL_OPEN_AFTER.with(|left| left.set(None));
    assert_eq!(
        (outcome.state, outcome.defect_code),
        (ChimiFiberState::Defected as u32, SCHEDULER_DEFECT_CODE)
    );
    assert!(events.into_inner().expect("events").is_empty(), "no task ran");
    assert!(ENTERED.lock().expect("entered").is_empty(), "no carrier was entered");
}

static ENTERS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn count_enter(_context: *mut std::ffi::c_void, _carrier: u32) {
    ENTERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A run whose carrier cannot get a thread enters no carrier: the carriers
/// already spawned leave at the gate before their hook.
#[test]
fn a_failed_start_enters_no_carrier() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    under_watchdog(Duration::from_secs(30), || {
        let mut scheduler =
            Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Virtual));
        scheduler.set_carriers(3);
        scheduler.enter_carrier(Carrier(0));
        let root = scheduler
            .start_root_with_context(hops, ptr::without_provenance_mut(1))
            .expect("root");
        ENTERS.store(0, std::sync::atomic::Ordering::Relaxed);
        let enter = EnterHook {
            hook: count_enter,
            context: ptr::null_mut(),
        };
        test_hooks::FAIL_SPAWN_OF.with(|carrier| carrier.set(Some(2)));
        let (run, scheduler) = run_on_carriers(scheduler, root, 1, Some(enter));
        test_hooks::FAIL_SPAWN_OF.with(|carrier| carrier.set(None));
        assert_eq!(run.outcome.defect_code, SCHEDULER_DEFECT_CODE);
        assert_eq!(ENTERS.load(std::sync::atomic::Ordering::Relaxed), 0);
        drop(scheduler);
    });
}
