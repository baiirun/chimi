//! Conformance for `Resource.scoped` under the native scheduler
//! (`std/resource.chimi`; `runtime/chimi-fiber/src/scheduler/scope.rs`): a
//! release runs whatever ends its use, no cancellation interrupts it, a
//! cancellation it held off is delivered when it returns, and a timed-out
//! scope aborts the channels it created (CHI-215). The outcome rules shared
//! with the interpreter are in `std-tests/resource`.
//!
//! Every run uses the virtual clock. Each program logs what its releases saw
//! through a channel, and folds the log, the elapsed seconds, and the outcome
//! into one number, so an assertion names every fact it guards.

use std::path::PathBuf;
use std::process::{Command, Output};

const PRELUDE: &str = r#"import std/channel as Channel
import std/channel.{Receive, Receiver, SendError, Sender}
import std/clock as Clock
import std/duration as Duration
import std/resource as Resource
import std/task as Task
import std/task.{Timeout}
import std/select as Select
import std/process as Process
import std/process.{Inbox}
import std/supervisor as Supervisor

fn take(receiver: Channel.Receiver<Int>) -> Int {
  let got = Channel.receive(receiver)
  case got {
    Receive.Received(value) -> value
    Receive.Drained -> 0
  }
}

"#;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-resource-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct Run {
    process: Output,
    telemetry: String,
    outcome: String,
}

/// Builds `body` after the prelude and runs it once under the virtual clock
/// with boundary tracing.
fn run_virtual(name: &str, body: &str) -> Run {
    run_traced(name, body, "boundaries")
}

/// Builds and runs `body` under the virtual clock with `--trace=<trace>`.
fn run_traced(name: &str, body: &str, trace: &str) -> Run {
    let scratch = scratch_directory(name);
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, format!("{PRELUDE}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native resource program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let telemetry_path = scratch.join("telemetry.jsonl");
    let outcome_path = scratch.join("outcome.jsonl");
    let process = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg(format!("--trace={trace}"))
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--")
        .output()
        .expect("execute native resource program");
    let run = Run {
        process,
        telemetry: std::fs::read_to_string(telemetry_path).unwrap_or_default(),
        outcome: std::fs::read_to_string(outcome_path).unwrap_or_default(),
    };
    std::fs::remove_dir_all(scratch).unwrap();
    run
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

/// The telemetry's `channel_send_accepted` events by task, as `T<id>`; they
/// appear only under `--trace=full`.
fn accepted_sends(telemetry: &str) -> Vec<String> {
    telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"channel_send_accepted\""))
        .map(|line| {
            let start = line.find("\"task_id\":").expect("a send names its task") + 10;
            let id: String = line[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            format!("T{id}")
        })
        .collect()
}

// Rejects a release that a firing timeout skips or cuts short: the release sleeps 5 s inside the fired scope and still logs its handle, and only then does the timeout return `TimedOut` (6 s, logged 3, code 100).
#[test]
fn release_runs_masked_after_a_timeout_fires_in_use() {
    let result = run_virtual(
        "release_runs_masked_after_a_timeout_fires_in_use",
        r#"fn operation(sender: Channel.Sender<Int>) -> Int {
  Resource.scoped(
    fn() { 3 },
    fn(held) {
      Clock.sleep(Duration.seconds(5))
      Channel.send(sender, held)
    },
    fn(held) {
      Clock.sleep(Duration.seconds(10))
      held
    },
  )
}

pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let start = Clock.now()
  let code = case fn() { operation(sender) } |> Task.timeout(Duration.seconds(1)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 100
  }
  code + take(log.receiver) * 10 + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("6130"), "{}", result.outcome);
}

// Rejects a `Task.cancel` that tears the fiber down without its release, or interrupts the release's own park (logged 4 at 3 s).
#[test]
fn task_cancel_runs_the_release_of_a_task_parked_in_use() {
    let result = run_virtual(
        "task_cancel_runs_the_release_of_a_task_parked_in_use",
        r#"pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let start = Clock.now()
  let child = Task.start(fn() {
    Resource.scoped(
      fn() { 4 },
      fn(held) {
        Clock.sleep(Duration.seconds(2))
        Channel.send(sender, held)
      },
      fn(held) {
        Clock.sleep(Duration.seconds(10))
        held
      },
    )
  })
  Clock.sleep(Duration.seconds(1))
  Task.cancel(child)
  take(log.receiver) * 10 + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("3040"), "{}", result.outcome);
}

// Rejects a cancellation that interrupts a running release: it arrives at 1 s, the release sleeps until 3 s and logs 5.
#[test]
fn cancel_while_masked_waits_for_the_release() {
    let result = run_virtual(
        "cancel_while_masked_waits_for_the_release",
        r#"pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let start = Clock.now()
  let child = Task.start(fn() {
    Resource.scoped(
      fn() { 5 },
      fn(held) {
        Clock.sleep(Duration.seconds(3))
        Channel.send(sender, held)
      },
      fn(held) { held },
    )
  })
  Clock.sleep(Duration.seconds(1))
  Task.cancel(child)
  take(log.receiver) * 10 + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("3050"), "{}", result.outcome);
}

// Rejects a mask that also holds off a timeout written inside the release: the outer timeout fired at 1 s, the release's own 2 s timeout ends it at 3 s.
#[test]
fn a_timeout_inside_a_release_still_fires() {
    let result = run_virtual(
        "a_timeout_inside_a_release_still_fires",
        r#"fn nap() -> Void {
  Clock.sleep(Duration.seconds(10))
}

fn operation() -> Int {
  Resource.scoped(
    fn() { 6 },
    fn(held) {
      case nap |> Task.timeout(Duration.seconds(2)) {
        Success(_) -> Void
        Failure(Timeout.TimedOut(_)) -> Void
      }
    },
    fn(held) {
      Clock.sleep(Duration.seconds(10))
      held
    },
  )
}

pub fn main() -> Int {
  let start = Clock.now()
  let code = case operation |> Task.timeout(Duration.seconds(1)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 100
  }
  code + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("3100"), "{}", result.outcome);
}

// Rejects an outer timeout that cancels a helper task the release started: it fires at 2 s while the release waits on its helper until 4 s.
#[test]
fn a_release_helper_task_survives_the_outer_timeout() {
    let result = run_virtual(
        "a_release_helper_task_survives_the_outer_timeout",
        r#"fn operation() -> Int {
  Resource.scoped(
    fn() { 7 },
    fn(held) {
      let helper = Task.start(fn() {
        Clock.sleep(Duration.seconds(3))
        held
      })
      let got = Task.result(helper)
      Void
    },
    fn(held) {
      Clock.sleep(Duration.seconds(1))
      held
    },
  )
}

pub fn main() -> Int {
  let start = Clock.now()
  let code = case operation |> Task.timeout(Duration.seconds(2)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 100
  }
  code + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("4100"), "{}", result.outcome);
}

// CHI-215. Rejects leaving a channel made inside a timed-out operation open until the task ends: a task outside holds its sender and sees `Aborted` (3).
#[test]
fn a_timed_out_scope_aborts_channels_it_created() {
    let result = run_virtual(
        "a_timed_out_scope_aborts_channels_it_created",
        r#"fn operation(handoff: Channel.Sender<Channel.Sender<Int>>) -> Int {
  let inner = Channel.bounded(1)
  Channel.send(handoff, inner.sender)
  Clock.sleep(Duration.seconds(10))
  take(inner.receiver)
}

pub fn main() -> Int {
  let handoff = Channel.bounded(1)
  let handoff_sender = handoff.sender
  let handoff_receiver = handoff.receiver
  let watcher = Task.start(fn() {
    let got = Channel.receive(handoff_receiver)
    case got {
      Receive.Received(inner) -> {
        Clock.sleep(Duration.seconds(2))
        case Channel.send(inner, 1) {
          Success(_) -> 1
          Failure(SendError.Closed(_)) -> 2
          Failure(SendError.Aborted(_)) -> 3
        }
      }
      Receive.Drained -> 0
    }
  })
  let code = case fn() { operation(handoff_sender) } |> Task.timeout(Duration.seconds(1)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 100
  }
  code + Task.result(watcher)
}
"#,
    );
    assert_eq!(result.outcome, success("103"), "{}", result.outcome);
}

// Rejects aborting a receiver a child started before the scope hands back while the scope runs: it is read through `Task.result` after the timeout (42).
#[test]
fn a_child_result_handed_back_inside_a_timed_out_scope_stays_open() {
    let result = run_virtual(
        "a_child_result_handed_back_inside_a_timed_out_scope_stays_open",
        r#"fn wait() -> Int {
  Clock.sleep(Duration.seconds(10))
  0
}

pub fn main() -> Int {
  let child = Task.start(fn() {
    let made = Channel.bounded(1)
    let sent = Channel.send(made.sender, 42)
    Channel.close(made.completion)
    Clock.sleep(Duration.seconds(1))
    made.receiver
  })
  let code = case wait |> Task.timeout(Duration.seconds(2)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 100
  }
  let receiver = Task.result(child)
  code + take(receiver)
}
"#,
    );
    assert_eq!(result.outcome, success("142"), "{}", result.outcome);
}

// Rejects releases that run out of order or only once under `Task.cancel`: the inner logs 2, then the outer logs 1.
#[test]
fn nested_releases_run_innermost_first_under_cancel() {
    let result = run_virtual(
        "nested_releases_run_innermost_first_under_cancel",
        r#"pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let child = Task.start(fn() {
    Resource.scoped(
      fn() { 1 },
      fn(held) { Channel.send(sender, held) },
      fn(_outer) {
        Resource.scoped(
          fn() { 2 },
          fn(held) {
            Clock.sleep(Duration.seconds(1))
            Channel.send(sender, held)
          },
          fn(held) {
            Clock.sleep(Duration.seconds(10))
            held
          },
        )
      },
    )
  })
  Clock.sleep(Duration.seconds(1))
  Task.cancel(child)
  take(log.receiver) * 10 + take(log.receiver)
}
"#,
    );
    assert_eq!(result.outcome, success("21"), "{}", result.outcome);
}

// Rejects cancelling a release's helper task with its owner, or leaving the release parked forever on it: the helper's 3 reaches the log.
#[test]
fn a_release_waiting_on_its_helper_is_not_cut_short_by_cancel() {
    let result = run_virtual(
        "a_release_waiting_on_its_helper_is_not_cut_short_by_cancel",
        r#"pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let child = Task.start(fn() {
    Resource.scoped(
      fn() { 3 },
      fn(held) {
        let helper = Task.start(fn() {
          Clock.sleep(Duration.seconds(2))
          held
        })
        Channel.send(sender, Task.result(helper))
      },
      fn(held) { held },
    )
  })
  Clock.sleep(Duration.seconds(1))
  Task.cancel(child)
  take(log.receiver)
}
"#,
    );
    assert_eq!(result.outcome, success("3"), "{}", result.outcome);
}

// Rejects a release that keeps a cancelled task alive forever: a release that
// can never return ends in the deadlock defect instead of hanging the program.
#[test]
fn a_release_that_never_returns_ends_in_the_deadlock_defect() {
    let result = run_virtual(
        "deadlock",
        r#"
pub fn main() -> Int {
  let never = Channel.bounded(1)
  let receiver = never.receiver
  let child = Task.start(fn() {
    Resource.scoped(
      fn() { 1 },
      fn(_held) {
        let _ = take(receiver)
        Void
      },
      fn(held) {
        Clock.sleep(Duration.seconds(10))
        held
      },
    )
  })
  Clock.sleep(Duration.seconds(1))
  Task.cancel(child)
  let keep = never.sender
  Clock.sleep(Duration.seconds(1))
  Task.result(child)
}
"#,
    );
    assert_eq!(result.process.status.code(), Some(70));
    assert!(result.outcome.contains("deadlock"), "{}", result.outcome);
}

// Rejects a child defect that tears down a parent waiting in `use` without its
// release: the release's send is accepted (task 1), and the program still ends
// in the child's defect.
#[test]
fn a_child_defect_runs_the_parent_release_and_stays_the_outcome() {
    let result = run_traced(
        "child-defect",
        r#"
fn divide(left: Int, right: Int) -> Int {
  left / right
}

pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let receiver = log.receiver
  let watcher = Task.start(fn() {
    take(receiver)
  })
  Resource.scoped(
    fn() { 9 },
    fn(held) { Channel.send(sender, held) },
    fn(held) {
      let child = Task.start(fn() {
        Clock.sleep(Duration.seconds(1))
        divide(held, 0)
      })
      Task.result(child)
    },
  )
}
"#,
        "full",
    );
    assert_eq!(result.process.status.code(), Some(70));
    assert!(
        result
            .outcome
            .contains("integer division by zero in `/`: 9 / 0"),
        "{}",
        result.outcome
    );
    assert_eq!(accepted_sends(&result.telemetry), ["T1"]);
}

// Rejects a select inside a release that a defecting helper never wakes: the
// helper's defect reaches the release, which is not left parked, so the
// program ends in that defect rather than the deadlock defect.
#[test]
fn a_release_selecting_on_a_helper_that_defects_is_woken() {
    let result = run_virtual(
        "select-helper-defect",
        r#"fn divide(left: Int, right: Int) -> Int {
  left / right
}

fn tenfold(value: Int) -> Int {
  value * 10
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let log = Channel.bounded(4)
  let sender = log.sender
  let receiver = log.receiver
  let watcher = Task.start(fn() { take(receiver) })
  Resource.scoped(
    fn() { 9 },
    fn(held) {
      let helper = Task.start(fn() {
        Clock.sleep(Duration.seconds(1))
        divide(held, 0)
      })
      let _ = Select.first([Select.task(helper, tenfold)])
      Channel.send(sender, held)
    },
    fn(held) { held },
  )
}
"#,
    );
    assert_eq!(result.process.status.code(), Some(70));
    assert!(
        result
            .outcome
            .contains("integer division by zero in `/`: 9 / 0"),
        "{}",
        result.outcome
    );
}

// Rejects a graceful process stop that cuts short a release it reached during
// use: the release sleeps 2 s and still reports 7, and the stop takes 2 s, not
// the 10 s deadline (702).
#[test]
fn a_process_stopped_in_use_finishes_its_release_before_the_deadline() {
    let result = run_virtual(
        "process-stop-in-use",
        r#"pub enum Message {
  Start(report: Sender<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(report) -> Resource.scoped(
      fn() { 7 },
      fn(held) {
        Clock.sleep(Duration.seconds(2))
        Channel.send(report, held)
      },
      fn(_held) {
        let _ = Process.receive(inbox)
        Void
      },
    )
  }
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(10)), Supervisor.Restart.Never)
  let reports = Channel.bounded(4)
  Process.send(Supervisor.address(managed), Message.Start(reports.sender))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  take(reports.receiver) * 100 + elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("702"), "{}", result.outcome);
}

// Rejects a graceful process stop delivered into running work: stopped 1 s
// into a 2 s use of the resource, the body drains (G22), so it reports 7 1 s
// later, `scoped` returns normally and its 99 is sent, and the next receive
// ends the body; the removal waits the 1 s (7991).
#[test]
fn a_process_stopped_in_its_release_lets_the_release_finish() {
    let result = run_virtual(
        "process-stop-in-release",
        r#"pub enum Message {
  Start(report: Sender<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(report) -> {
      Resource.scoped(
        fn() { 7 },
        fn(held) {
          Clock.sleep(Duration.seconds(2))
          Channel.send(report, held)
        },
        fn(_held) { Void },
      )
      Channel.send(report, 99)
    }
  }
  let _ = Process.receive(inbox)
  Void
}

fn read(next: Receive<Int>) -> Int {
  case next {
    Receive.Received(value) -> value
    Receive.Drained -> 0
  }
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(10)), Supervisor.Restart.Never)
  let reports = Channel.bounded(4)
  Process.send(Supervisor.address(managed), Message.Start(reports.sender))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  let first = take(reports.receiver)
  let second = Select.first([Select.receive(reports.receiver, read), Select.after(Duration.seconds(1), 0)])
  first * 1000 + second * 10 + elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("7991"), "{}", result.outcome);
}

// Rejects a shutdown that waits forever for a child's release: the child's
// release can never return, and the stop still ends at the 3 s deadline.
#[test]
fn a_process_child_stuck_in_its_release_is_forced_at_the_deadline() {
    let result = run_virtual(
        "process-child-forced",
        r#"pub enum Message {
  Start(never: Receiver<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(never) -> {
      let _child = Task.start(fn() {
        Resource.scoped(
          fn() { 1 },
          fn(_held) {
            let _ = take(never)
            Void
          },
          fn(held) {
            Clock.sleep(Duration.seconds(100))
            held
          },
        )
      })
      let _ = Process.receive(inbox)
      Void
    }
  }
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(3)), Supervisor.Restart.Never)
  let never = Channel.bounded(1)
  Process.send(Supervisor.address(managed), Message.Start(never.receiver))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  let keep = never.sender
  elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("3"), "{}", result.outcome);
}

// Rejects a process body torn down by a child's defect without its release:
// the release gives the handle back (7) before the incarnation ends.
#[test]
fn a_process_body_runs_its_release_when_a_child_defects() {
    let result = run_virtual(
        "process-child-defect",
        r#"pub enum Message {
  Start(report: Sender<Int>)
}

fn divide(left: Int, right: Int) -> Int {
  left / right
}

fn body(inbox: Inbox<Message>) -> Void {
  case Process.receive(inbox) {
    Message.Start(report) -> Resource.scoped(
      fn() { 7 },
      fn(held) { Channel.send(report, held) },
      fn(held) {
        let child = Task.start(fn() { divide(held, 0) })
        let _ = Task.result(child)
        Void
      },
    )
  }
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(10)), Supervisor.Restart.Never)
  let reports = Channel.bounded(4)
  Process.send(Supervisor.address(managed), Message.Start(reports.sender))
  let got = take(reports.receiver)
  Supervisor.stop(root)
  got
}
"#,
    );
    assert_eq!(result.outcome, success("7"), "{}", result.outcome);
}

// Rejects a graceful stop that cancels the helper a running release waits on:
// the helper still returns 7 to the release 1 s after the stop (701).
#[test]
fn a_process_stop_leaves_a_release_helper_running() {
    let result = run_virtual(
        "process-stop-release-helper",
        r#"pub enum Message {
  Start(report: Sender<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(report) -> Resource.scoped(
      fn() { 7 },
      fn(held) {
        let helper = Task.start(fn() {
          Clock.sleep(Duration.seconds(2))
          held
        })
        Channel.send(report, Task.result(helper))
      },
      fn(_held) { Void },
    )
  }
  let _ = Process.receive(inbox)
  Void
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(10)), Supervisor.Restart.Never)
  let reports = Channel.bounded(4)
  Process.send(Supervisor.address(managed), Message.Start(reports.sender))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  take(reports.receiver) * 100 + elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("701"), "{}", result.outcome);
}

// Rejects a shutdown deadline that misses a grandchild: the body's child waits
// on a grandchild stuck in its release, and the stop still ends at 3 s.
#[test]
fn a_process_grandchild_stuck_in_its_release_is_forced_at_the_deadline() {
    let result = run_virtual(
        "process-grandchild-forced",
        r#"pub enum Message {
  Start(never: Receiver<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(never) -> {
      let _child = Task.start(fn() {
        let grandchild = Task.start(fn() {
          Resource.scoped(
          fn() { 1 },
          fn(_held) {
            let _ = take(never)
            Void
          },
          fn(held) {
            Clock.sleep(Duration.seconds(100))
            held
          },
        )
        })
        Task.result(grandchild)
      })
      let _ = Process.receive(inbox)
      Void
    }
  }
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(3)), Supervisor.Restart.Never)
  let never = Channel.bounded(1)
  Process.send(Supervisor.address(managed), Message.Start(never.receiver))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  let keep = never.sender
  elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("3"), "{}", result.outcome);
}

// Rejects a release parked in `Process.receive` that a stop never wakes: the
// receive answers the stop at once, so the stop takes no time.
#[test]
fn a_process_receive_in_a_release_answers_the_stop() {
    let result = run_virtual(
        "process-receive-in-release",
        r#"pub enum Message {
  Start(report: Sender<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(report) -> Resource.scoped(
      fn() { 7 },
      fn(held) {
        let _ = Process.receive(inbox)
        Channel.send(report, held)
      },
      fn(_held) { Void },
    )
  }
  let _ = Process.receive(inbox)
  Void
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(10)), Supervisor.Restart.Never)
  let reports = Channel.bounded(4)
  Process.send(Supervisor.address(managed), Message.Start(reports.sender))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("0"), "{}", result.outcome);
}

// Rejects a stopping body whose release selects on a helper that defects and
// is never woken: the helper defects 1 s after the stop and the stop ends
// then, not at the 10 s deadline.
#[test]
fn a_stopping_release_selecting_on_a_defecting_helper_is_woken() {
    let result = run_virtual(
        "process-select-helper-defect",
        r#"pub enum Message {
  Start(report: Sender<Int>)
}

fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {
  case Process.receive(inbox) {
    Message.Start(report) -> Resource.scoped(
      fn() { 7 },
      fn(held) {
        let helper = Task.start(fn() {
          Clock.sleep(Duration.seconds(2))
          held / 0
        })
        let _ = Select.first([Select.task(helper, fn(value: Int) -> Int { value })])
        Channel.send(report, held)
      },
      fn(_held) { Void },
    )
  }
  let _ = Process.receive(inbox)
  Void
}

pub fn main() -> Int needs { clock: Clock.Clock } {
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))
  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(10)), Supervisor.Restart.Never)
  let reports = Channel.bounded(4)
  Process.send(Supervisor.address(managed), Message.Start(reports.sender))
  Clock.sleep(Duration.seconds(1))
  let start = Clock.now()
  Supervisor.remove_process(root, managed)
  let elapsed = Duration.to_seconds(Clock.since(start))
  Supervisor.stop(root)
  elapsed
}
"#,
    );
    assert_eq!(result.outcome, success("1"), "{}", result.outcome);
}

// Rejects an acquire that a cancellation can leave after its wait ended: the
// grant reaches the acquiring task just before `Task.cancel`, and the
// cancellation is held off past the acquire's later `Clock.now` until use
// begins, so use does not run and release logs 7 at once, at 1 s (CHI-268).
#[test]
fn a_cancel_after_acquire_stops_waiting_still_runs_the_release() {
    let result = run_virtual(
        "a_cancel_after_acquire_stops_waiting_still_runs_the_release",
        r#"pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let grants = Channel.bounded(1)
  let receiver = grants.receiver
  let child = Task.start(fn() {
    Resource.scoped(
      fn() {
        let granted = take(receiver)
        let _ = Clock.now()
        granted
      },
      fn(held) { Channel.send(sender, held) },
      fn(held) {
        Clock.sleep(Duration.seconds(10))
        held
      },
    )
  })
  let start = Clock.now()
  Clock.sleep(Duration.seconds(1))
  Channel.send(grants.sender, 7)
  Task.cancel(child)
  take(log.receiver) + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("1007"), "{}", result.outcome);
}

// Rejects an acquire masked even while it waits: a timeout written around the
// whole call still ends a wait inside acquire, at 1 s rather than 10 s, and no
// release runs because nothing was acquired (code 100, nothing logged, 2 s
// with the 1 s wait for the log).
#[test]
fn a_timeout_ends_a_wait_inside_acquire() {
    let result = run_virtual(
        "a_timeout_ends_a_wait_inside_acquire",
        r#"fn operation(sender: Channel.Sender<Int>) -> Int {
  Resource.scoped(
    fn() {
      Clock.sleep(Duration.seconds(10))
      3
    },
    fn(held) { Channel.send(sender, held) },
    fn(held) { held },
  )
}

pub fn main() -> Int {
  let log = Channel.bounded(4)
  let sender = log.sender
  let start = Clock.now()
  let code = case fn() { operation(sender) } |> Task.timeout(Duration.seconds(1)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 100
  }
  let receiver = log.receiver
  let logged = case fn() { take(receiver) } |> Task.timeout(Duration.seconds(1)) {
    Success(value) -> value
    Failure(Timeout.TimedOut(limit)) -> 0
  }
  code + logged * 10 + Duration.to_seconds(Clock.since(start)) * 1000
}
"#,
    );
    assert_eq!(result.outcome, success("2100"), "{}", result.outcome);
}
