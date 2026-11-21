//! Carriers on real threads (parallel-scheduler.md, "Step 3 plan", PR 5c-6):
//! `process_abi::run_root_on_threads` runs carrier 0 on the test's thread
//! and every other carrier on a thread of its own. Each test loops under a
//! watchdog, since a lost wake shows up as a hang, and its fibers check that
//! they never change thread and never run with the kernel lock held.

use super::process_abi::run_root_on_threads;
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
