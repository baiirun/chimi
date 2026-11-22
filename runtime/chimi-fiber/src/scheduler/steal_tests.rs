//! Stealing (parallel-scheduler.md, step 4 PR 4c), driven on one thread: a
//! carrier with nothing to run takes the oldest ready task of a carrier
//! that is parked asleep, when that task has run before, and becomes its
//! home. Work a carrier is awake for, new work, and carrier 0's work stay
//! where they are.

use super::carrier::IdleStep;
use super::ready::Carrier;
use super::*;
use crate::{ChimiFiberResult, FIXTURE_LOCK};

unsafe extern "C" {
    fn chimi_fixture_generated_many_parks(context: *mut std::ffi::c_void) -> ChimiFiberResult;
}

fn virtual_kernel(carriers: u32) -> Scheduler {
    let mut scheduler =
        Scheduler::with_options(SchedulerOptions::new(64 * 1024, TraceMode::Off, ClockMode::Virtual));
    scheduler.set_carriers(carriers);
    scheduler
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

/// Parks `carrier`, as its thread does once it has nothing to run.
fn park(scheduler: &mut Scheduler, carrier: u32) {
    scheduler.enter_carrier(Carrier(carrier));
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "carrier {carrier} parks");
}

/// Forgets the wakes marked or written so far.
fn clear_wakes(scheduler: &mut Scheduler) {
    drop(scheduler.take_wakes());
    for carrier in 0..scheduler.ready.carriers() {
        signal::drain(wake_fd(scheduler, carrier));
    }
}

fn wake_fd(scheduler: &Scheduler, carrier: u32) -> i32 {
    scheduler.wakes.read_fd(Carrier(carrier)).expect("a wake pipe")
}

/// True when `carrier`'s wake pipe holds a byte: its idle wait would end.
fn woken(scheduler: &Scheduler, carrier: u32) -> bool {
    let mut poll = libc::pollfd {
        fd: wake_fd(scheduler, carrier),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    unsafe { libc::poll(&mut poll, 1, 0) > 0 }
}

/// A task woken for a carrier that sleeps runs on the carrier that has
/// nothing to do, which becomes its home; the sleeping carrier is not woken
/// for the work it lost.
#[test]
fn an_idle_carrier_takes_a_started_task_from_a_sleeping_carrier() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    let task = start_parked(&mut scheduler, 2);
    park(&mut scheduler, 1);
    park(&mut scheduler, 2);
    scheduler.enter_carrier(Carrier(0));
    clear_wakes(&mut scheduler);
    scheduler.wake(task).expect("wake");
    assert!(scheduler.wakes.is_marked(Carrier(2)), "the wake is for carrier 2");
    // `drive` writes the wakes still marked when it releases the kernel.
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].carrier, Carrier(0), "carrier 0 is its home now");
    assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Parked, "it ran here and parked again");
    assert!(scheduler.ready.is_empty_for(Carrier(2)));
    assert!(!woken(&scheduler, 2), "carrier 2 sleeps on");
    scheduler.cancel(None, task).expect("cancel");
    assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Cancelled), "its unwind ran at its new home");
}

/// A steal that leaves the sleeping carrier other work keeps its wake: the
/// mark is its only way to learn of that work.
#[test]
fn a_steal_that_leaves_work_behind_keeps_the_wake() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(3);
    let task = start_parked(&mut scheduler, 2);
    park(&mut scheduler, 1);
    park(&mut scheduler, 2);
    scheduler.enter_carrier(Carrier(0));
    clear_wakes(&mut scheduler);
    scheduler.wake(task).expect("wake");
    let child = scheduler
        .start_child(task, chimi_fixture_generated_many_parks)
        .expect("child");
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].carrier, Carrier(0), "the started task was taken");
    assert_eq!(scheduler.ready.front(Carrier(2)), Some(child), "the new child waits at home");
    assert!(woken(&scheduler, 2), "carrier 2 is woken for it");
    scheduler.enter_carrier(Carrier(2));
    scheduler.drive();
    scheduler.cancel(None, task).expect("cancel");
}

/// A carrier that is awake runs its own ready task: nothing is taken from it.
#[test]
fn a_carrier_that_is_awake_keeps_its_ready_task() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let task = start_parked(&mut scheduler, 1);
    scheduler.enter_carrier(Carrier(0));
    scheduler.wake(task).expect("wake");
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].carrier, Carrier(1));
    assert_eq!(scheduler.ready.front(Carrier(1)), Some(task));
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    scheduler.cancel(None, task).expect("cancel");
}

/// A task that never ran is new work: it starts where it was placed, even
/// when that carrier sleeps, so a fan-out still spreads over the carriers.
#[test]
fn a_task_that_never_ran_waits_for_its_own_carrier() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    park(&mut scheduler, 1);
    let task = scheduler
        .start_root(chimi_fixture_generated_many_parks)
        .expect("task");
    scheduler.enter_carrier(Carrier(0));
    scheduler.drive();
    assert!(!scheduler.tasks[&task].ran);
    assert_eq!(scheduler.ready.front(Carrier(1)), Some(task));
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].carrier, Carrier(1), "it started at home");
    scheduler.cancel(None, task).expect("cancel");
}

/// Carrier 0 owns the root, signals and descriptor waits, so no carrier
/// takes its work, even while it waits for the others.
#[test]
fn no_carrier_takes_carrier_zeros_work() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let busy = start_parked(&mut scheduler, 1);
    scheduler.wake(busy).expect("carrier 1 has work");
    let task = start_parked(&mut scheduler, 0);
    assert!(matches!(scheduler.idle_step(), IdleStep::Park(_)), "carrier 0 waits for carrier 1");
    scheduler.enter_carrier(Carrier(1));
    scheduler.wake(task).expect("wake");
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].carrier, Carrier(0));
    assert_eq!(scheduler.ready.front(Carrier(0)), Some(task));
    scheduler.enter_carrier(Carrier(0));
    scheduler.drive();
    scheduler.cancel(None, task).expect("cancel");
    scheduler.cancel(None, busy).expect("cancel");
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
}

/// An entry whose task was cancelled while it waited is dropped, not moved:
/// the unwind stays a job for the sleeping home, which is still woken for it.
#[test]
fn a_cancelled_entry_is_not_moved_and_its_unwind_waits_for_its_home() {
    let _fixture = FIXTURE_LOCK.lock().expect("fixture lock");
    let mut scheduler = virtual_kernel(2);
    let task = start_parked(&mut scheduler, 1);
    park(&mut scheduler, 1);
    scheduler.enter_carrier(Carrier(0));
    clear_wakes(&mut scheduler);
    scheduler.wake(task).expect("wake");
    scheduler.cancel(None, task).expect("cancel");
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].carrier, Carrier(1));
    assert!(scheduler.ready.is_empty_for(Carrier(1)), "the stale entry was dropped");
    assert!(!scheduler.jobs.is_empty_for(Carrier(1)), "carrier 1 owes the unwind");
    assert!(woken(&scheduler, 1), "and is woken for it");
    scheduler.enter_carrier(Carrier(1));
    scheduler.drive();
    assert_eq!(scheduler.tasks[&task].phase, TaskPhase::Terminal(TaskOutcome::Cancelled));
}
