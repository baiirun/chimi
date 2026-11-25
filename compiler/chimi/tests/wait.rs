//! Conformance for `std/wait` (time-and-outcomes.md, "Choosing among
//! events", and the conformance rows Wait commit, Wait cancellation, and
//! Wait timer, plus the Tracing row's wait park and commit).
//!
//! Every native run uses the virtual clock, so a select that would wait on
//! the wall clock times the suite out instead of passing slowly.

#[path = "support/interpreter.rs"]
mod interpreter;

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMPORTS: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{ChannelError, Receive, Receiver}\n",
    "import std/clock as Clock\n",
    "import std/clock.{Instant, Timestamp}\n",
    "import std/duration as Duration\n",
    "import std/wait as Wait\n",
    "import std/task as Task\n",
    "import std/task.{Timeout}\n",
);

/// Maps a receive to its value, or 0 once the channel drains.
const VALUE: &str = concat!(
    "fn value(next: Receive<Int>) -> Int {\n",
    "  case next {\n",
    "    Receive.Received(value) -> value\n",
    "    Receive.Drained -> 0\n",
    "  }\n",
    "}\n",
);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-select-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn build_native(scratch: &Path, name: &str, body: &str) -> PathBuf {
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, format!("{IMPORTS}{VALUE}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native select program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    executable
}

struct Run {
    process: Output,
    telemetry: String,
    outcome: String,
}

/// Builds `body` and runs it once under the virtual clock.
fn run_virtual(name: &str, body: &str, trace: &str) -> Run {
    let scratch = scratch_directory(name);
    let executable = build_native(&scratch, name, body);
    let telemetry_path = scratch.join("telemetry.jsonl");
    let outcome_path = scratch.join("outcome.jsonl");
    let child = Command::new(&executable)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg(format!("--trace={trace}"))
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--execution-id={name}"))
        .arg("--")
        .spawn()
        .expect("execute native select program");
    let process = wait_bounded(child, &scratch);
    let result = Run {
        process,
        telemetry: std::fs::read_to_string(telemetry_path).unwrap_or_default(),
        outcome: std::fs::read_to_string(outcome_path).unwrap_or_default(),
    };
    std::fs::remove_dir_all(scratch).unwrap();
    result
}

/// Waits for `child`, killing it after 15 s. A select that livelocks under
/// `--trace=boundaries` writes telemetry without end, so it must fail rather
/// than fill the disk.
fn wait_bounded(mut child: std::process::Child, scratch: &Path) -> Output {
    let started = std::time::Instant::now();
    while child.try_wait().expect("poll select program").is_none() {
        if started.elapsed() > std::time::Duration::from_secs(15) {
            child.kill().expect("kill a select program that never ends");
            let _ = child.wait();
            std::fs::remove_dir_all(scratch).unwrap();
            panic!("the select program ran for more than 15 s");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    child
        .wait_with_output()
        .expect("collect select program output")
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

/// The virtual times, in nanoseconds, that `clock_advanced` moves to.
fn clock_advances(telemetry: &str) -> Vec<i64> {
    telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_advanced\""))
        .map(|line| {
            let start = line.find("\"time_ns\":").expect("an advance has time_ns") + 10;
            line[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("time_ns is a count")
        })
        .collect()
}

fn type_codes(source: &str) -> Vec<String> {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", format!("{IMPORTS}{VALUE}{source}")),
    )
    .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect()
}

const SECOND: i64 = 1_000_000_000;

// ----- Wait commit ------------------------------------------------------------

// With two ready channels only the first in list order delivers; the other
// keeps its value for the next select. Rejects random or last-wins choice,
// and a select that consumes a losing channel's value.
#[test]
fn among_ready_channels_the_first_in_list_order_delivers() {
    let result = run_virtual(
        "commit",
        concat!(
            "pub fn main() -> Int {\n",
            "  let first = Channel.bounded(1)\n",
            "  let second = Channel.bounded(1)\n",
            "  Channel.send(first.sender, 1)\n",
            "  Channel.send(second.sender, 2)\n",
            "  let a = Wait.first([Wait.receive(second.receiver, value), Wait.receive(first.receiver, value)])\n",
            "  let b = Wait.first([Wait.receive(second.receiver, value), Wait.receive(first.receiver, value)])\n",
            "  Channel.close(second.completion)\n",
            "  let c = Wait.first([Wait.receive(second.receiver, value), Wait.receive(first.receiver, value)])\n",
            "  a * 100 + b * 10 + c\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("210"));
}

// A parked select commits to whichever source turns ready first, and the
// mapping function runs in the selecting task. Rejects a select that only
// polls once, and one that wakes on the wrong channel.
#[test]
fn a_parked_select_takes_the_first_value_to_arrive() {
    let result = run_virtual(
        "parked",
        concat!(
            "fn later(sender: Channel.Sender<Int>, seconds: Int, value: Int) -> Int needs { clock: Clock.Clock } {\n",
            "  Clock.sleep(Duration.seconds(seconds))\n",
            "  Channel.send(sender, value)\n",
            "  0\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let slow = Channel.bounded(0)\n",
            "  let fast = Channel.bounded(0)\n",
            "  let start = Clock.now()\n",
            "  let slow_sender = slow.sender\n",
            "  let fast_sender = fast.sender\n",
            "  let s = Task.start(fn() { later(slow_sender, 9, 5) })\n",
            "  let f = Task.start(fn() { later(fast_sender, 3, 7) })\n",
            "  let got = Wait.first([Wait.receive(slow.receiver, value), Wait.receive(fast.receiver, value)])\n",
            "  let waited = Duration.to_seconds(Clock.since(start))\n",
            "  let rest = value(Channel.receive(slow.receiver))\n",
            "  got * 1000 + waited * 10 + rest\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("7035"));
    assert_eq!(clock_advances(&result.telemetry), [3 * SECOND, 9 * SECOND]);
}

// A committed receive on an aborted channel fails with `Channel.Aborted`
// and never calls the mapping function. Rejects a mapping call with no
// value, and an abort swallowed as `Drained`.
#[test]
fn an_aborted_channel_fails_the_select_without_mapping() {
    let result = run_virtual(
        "aborted",
        concat!(
            "fn mapped(next: Receive<Int>) -> Int { 99 }\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.abort(channel.receiver)\n",
            "  case Wait.first([Wait.receive(channel.receiver, mapped)]) {\n",
            "    Success(value) -> value\n",
            "    Failure(ChannelError.Aborted) -> 42\n",
            "  }\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("42"));
}

// A receive on a closed channel is ready and delivers `Drained`.
#[test]
fn a_closed_channel_is_ready_and_drained() {
    let result = run_virtual(
        "closed",
        concat!(
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.close(channel.completion)\n",
            "  Wait.first([\n",
            "    Wait.after(Duration.seconds(1), 7),\n",
            "    Wait.receive(channel.receiver, fn(next) { value(next) + 40 }),\n",
            "  ])\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("40"));
}

// The same receiver in two events: the earlier one commits, and the value is
// received once.
#[test]
fn the_same_receiver_twice_commits_the_earlier_event() {
    let result = run_virtual(
        "same-receiver",
        concat!(
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(2)\n",
            "  Channel.send(channel.sender, 5)\n",
            "  Channel.send(channel.sender, 6)\n",
            "  let first = Wait.first([\n",
            "    Wait.receive(channel.receiver, fn(next) { value(next) + 100 }),\n",
            "    Wait.receive(channel.receiver, value),\n",
            "  ])\n",
            "  first * 10 + value(Channel.receive(channel.receiver))\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("1056"));
}

// A select nothing can wake is a virtual deadlock that names `Wait.first`.
#[test]
fn a_select_nothing_can_wake_is_a_deadlock_naming_select() {
    let result = run_virtual(
        "deadlock",
        concat!(
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  let got = Wait.first([Wait.receive(channel.receiver, value)])\n",
            "  Channel.send(channel.sender, 1)\n",
            "  got\n",
            "}\n",
        ),
        "off",
    );
    let report = format!(
        "{}{}",
        result.outcome,
        String::from_utf8_lossy(&result.process.stderr)
    );
    assert!(report.contains("deadlock"), "{report}");
    assert!(report.contains("Wait.first"), "{report}");
}

// A mapping function's failure is the select's failure.
#[test]
fn a_mapping_failure_is_the_select_failure() {
    let result = run_virtual(
        "mapping-failure",
        concat!(
            "enum Odd { Found(value: Int) }\n",
            "fn even(next: Receive<Int>) -> Int {\n",
            "  let got = value(next)\n",
            "  case got % 2 == 1 {\n    True -> Failure(Odd.Found(got))\n    False -> got\n  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.send(channel.sender, 3)\n",
            "  case Wait.first([Wait.receive(channel.receiver, even)]) {\n",
            "    Success(value) -> value\n",
            "    Failure(Odd.Found(value)) -> value * 10\n",
            "    Failure(ChannelError.Aborted) -> 0\n",
            "  }\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("30"));
}

// ----- Tasks ------------------------------------------------------------------

// A task event is ready once the task ends; its value goes through the
// mapping and its failure is the select's failure.
#[test]
fn a_task_event_waits_for_the_task_and_bubbles_its_failure() {
    let result = run_virtual(
        "task",
        concat!(
            "enum Broke { Down }\n",
            "fn slow(seconds: Int, fails: Bool) -> Int needs { clock: Clock.Clock } {\n",
            "  Clock.sleep(Duration.seconds(seconds))\n",
            "  case fails {\n    True -> Failure(Broke.Down)\n    False -> seconds\n  }\n",
            "}\n",
            "fn tenfold(value: Int) -> Int { value * 10 }\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let a = Task.start(fn() { slow(4, False) })\n",
            "  let b = Task.start(fn() { slow(6, True) })\n",
            "  let first = Wait.first([Wait.task(b, tenfold), Wait.task(a, tenfold)])\n",
            "  let second = case Wait.first([Wait.task(b, tenfold)]) {\n",
            "    Success(value) -> value\n",
            "    Failure(Broke.Down) -> 7\n",
            "  }\n",
            "  first * 10 + second\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("407"));
}

// ----- Inbox ------------------------------------------------------------------

// A process body waits on its inbox and a timer at once: each message
// arrives through the inbox event, and the timer commits once the inbox
// stays empty. Rejects an inbox event that never wakes, and one that drops
// or reorders messages.
#[test]
fn a_process_waits_on_its_inbox_or_a_timer() {
    let result = run_virtual(
        "inbox",
        concat!(
            "import std/channel.{Sender}\n",
            "import std/process as Process\n",
            "import std/process.{Inbox}\n",
            "import std/supervisor as Supervisor\n",
            "pub enum Message {\n  Add(amount: Int, report: Sender<Int>)\n}\n",
            "enum Wake {\n  Got(message: Message)\n  Idle\n}\n",
            "fn body(inbox: Inbox<Message>) -> Void needs { clock: Clock.Clock } {\n",
            "  let total = loop total = 0 {\n",
            "    let wake = Wait.first([\n",
            "      Wait.inbox(inbox, fn(message: Message) -> Wake { Wake.Got(message) }),\n",
            "      Wait.after(Duration.seconds(10), Wake.Idle),\n",
            "    ])\n",
            "    case wake {\n",
            "      Wake.Got(Message.Add(amount, report)) -> {\n",
            "        Channel.send(report, total * 10 + amount)\n",
            "        total * 10 + amount\n",
            "      }\n",
            "      Wake.Idle -> break total\n",
            "    }\n",
            "  }\n",
            "  Void\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))\n",
            "  let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.seconds(1)), Supervisor.Restart.Never)\n",
            "  let address = Supervisor.address(managed)\n",
            "  let reports = Channel.bounded(4)\n",
            "  Process.send(address, Message.Add(1, reports.sender))\n",
            "  Clock.sleep(Duration.seconds(4))\n",
            "  Process.send(address, Message.Add(2, reports.sender))\n",
            "  let first = value(Channel.receive(reports.receiver))\n",
            "  let second = value(Channel.receive(reports.receiver))\n",
            "  Clock.sleep(Duration.seconds(30))\n",
            "  Supervisor.stop(root)\n",
            "  first * 100 + second\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("112"));
    // The second message wakes the body at 4 s; its idle timer then commits
    // at 14 s, before main's sleep ends at 34 s.
    assert_eq!(
        clock_advances(&result.telemetry),
        [4 * SECOND, 14 * SECOND, 34 * SECOND]
    );
}

// ----- Wait timer -------------------------------------------------------------

// `Wait.after` measures from the start of the `Wait.first` call, not
// from when the event was built, and the earliest timer commits. Rejects a
// deadline fixed at construction (that run would end at 10 s) and a timer
// list that commits the last one.
#[test]
fn after_measures_from_the_select_call() {
    let result = run_virtual(
        "after",
        concat!(
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let start = Clock.now()\n",
            "  let events = [Wait.after(Duration.seconds(10), 1), Wait.after(Duration.seconds(4), 2)]\n",
            "  Clock.sleep(Duration.seconds(5))\n",
            "  let got = Wait.first(events)\n",
            "  got * 100 + Duration.to_seconds(Clock.since(start))\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("209"));
    assert_eq!(clock_advances(&result.telemetry), [5 * SECOND, 9 * SECOND]);
}

// `Wait.at` with a passed deadline is ready at once; ties keep list order.
#[test]
fn a_passed_deadline_is_ready_and_ties_keep_list_order() {
    let result = run_virtual(
        "at",
        concat!(
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let now = Clock.now()\n",
            "  let past = Wait.first([Wait.at(Clock.add(now, Duration.seconds(1)), 1), Wait.at(now, 2), Wait.at(now, 3)])\n",
            "  let later = Clock.add(now, Duration.seconds(2))\n",
            "  let tie = Wait.first([Wait.at(later, 4), Wait.at(later, 5)])\n",
            "  past * 10 + tie\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("24"));
}

// A timer on a user `Clock` goes through that clock's `sleep_until`.
// Rejects a timer that reads the execution clock instead: the shifted
// clock's deadline is an hour ahead of the execution clock's, so that run
// would wait an hour.
#[test]
fn a_user_clock_timer_works_through_its_sleep_until() {
    let result = run_virtual(
        "user-clock",
        concat!(
            "fn wait() -> Int { Wait.first([Wait.after(Duration.seconds(3), 8)]) }\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let base = clock\n",
            "  let shift = Duration.hours(1)\n",
            "  let shifted = {\n",
            "    now: fn() -> Instant { Clock.add(base.now(), shift) },\n",
            "    timestamp: fn() -> Timestamp { base.timestamp() },\n",
            "    sleep_until: fn(at: Instant) -> Void {\n",
            "      base.sleep_until(Clock.add(at, Duration.negate(shift)))\n",
            "    },\n",
            "  }\n",
            "  let start = Clock.now()\n",
            "  let waited = wait using { clock: shifted }\n",
            "  let got = waited()\n",
            "  got * 100 + Duration.to_seconds(Clock.since(start))\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("803"));
    assert_eq!(clock_advances(&result.telemetry), [3 * SECOND]);
}

// A commit is atomic with a user-clock timer in the list too: the losing
// timer child is cancelled only after the receive took its value, so the
// task that cancels the rendezvous sender at the same instant runs too late.
#[test]
fn a_user_clock_timer_does_not_break_the_commit() {
    let result = run_virtual(
        "user-clock-commit",
        concat!(
            "fn middle(sender: Channel.Sender<Int>) -> Int needs { clock: Clock.Clock } {\n",
            "  let offer = Task.start(fn() {\n    Channel.send(sender, 7)\n    0\n  })\n",
            "  Clock.sleep(Duration.seconds(1))\n",
            "  Task.cancel(offer)\n",
            "  0\n",
            "}\n",
            "fn wait(receiver: Receiver<Int>) -> Int {\n",
            "  Wait.first([Wait.receive(receiver, value), Wait.after(Duration.seconds(10), 99)])\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let base = clock\n",
            "  let passing = {\n",
            "    now: fn() -> Instant { base.now() },\n",
            "    timestamp: fn() -> Timestamp { base.timestamp() },\n",
            "    sleep_until: fn(at: Instant) -> Void { base.sleep_until(at) },\n",
            "  }\n",
            "  let canceller = Task.start(fn() { middle(sender) })\n",
            "  Clock.sleep(Duration.seconds(1))\n",
            "  let waited = wait using { clock: passing }\n",
            "  let got = waited(receiver)\n",
            "  Clock.sleep(Duration.seconds(20))\n",
            "  got\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(
        result.outcome,
        success("7"),
        "{}",
        String::from_utf8_lossy(&result.process.stderr)
    );
    // The losing 10 s timer child was cancelled: time steps from 1 s
    // straight to the 21 s sleep, never stopping at 11 s.
    assert_eq!(clock_advances(&result.telemetry), [SECOND, 21 * SECOND]);
}

// A user-clock timer that is already due is ready at once, so list order
// decides against a ready receive, as on the execution clock.
#[test]
fn a_due_user_clock_timer_keeps_list_order() {
    let result = run_virtual(
        "user-clock-due",
        concat!(
            "fn wait(receiver: Receiver<Int>) -> Int {\n",
            "  Wait.first([Wait.after(Duration.seconds(0), 1), Wait.receive(receiver, fn(next) { 2 })])\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let channel = Channel.bounded(1)\n",
            "  let _ = Channel.send(channel.sender, 5)\n",
            "  let base = clock\n",
            "  let passing = {\n",
            "    now: fn() -> Instant { base.now() },\n",
            "    timestamp: fn() -> Timestamp { base.timestamp() },\n",
            "    sleep_until: fn(at: Instant) -> Void { base.sleep_until(at) },\n",
            "  }\n",
            "  let direct = wait(channel.receiver)\n",
            "  let waited = wait using { clock: passing }\n",
            "  direct * 100 + waited(channel.receiver) * 10 + value(Channel.receive(channel.receiver))\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(
        result.outcome,
        success("115"),
        "{}",
        String::from_utf8_lossy(&result.process.stderr)
    );
}

// A parked select wakes when a value is queued on a buffered channel.
#[test]
fn a_buffered_send_wakes_a_parked_select() {
    let result = run_virtual(
        "buffered-wake",
        concat!(
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let channel = Channel.bounded(1)\n",
            "  let sender = channel.sender\n",
            "  let feeder = Task.start(fn() {\n",
            "    Clock.sleep(Duration.seconds(1))\n",
            "    let _ = Channel.send(sender, 4)\n",
            "    0\n",
            "  })\n",
            "  Wait.first([Wait.receive(channel.receiver, value)])\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(
        result.outcome,
        success("4"),
        "{}",
        String::from_utf8_lossy(&result.process.stderr)
    );
}

// A timeout withdraws a parked select instead of waiting for it.
#[test]
fn a_timeout_withdraws_a_parked_select() {
    let result = run_virtual(
        "timeout-withdraws",
        concat!(
            "fn wait() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  let got = Wait.first([Wait.receive(channel.receiver, value)])\n",
            "  let _ = Channel.send(channel.sender, 1)\n",
            "  got\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let start = Clock.now()\n",
            "  let got = case Task.timeout(wait, Duration.seconds(1)) {\n",
            "    Success(got) -> got\n",
            "    Failure(Timeout.TimedOut(_)) -> 100\n",
            "  }\n",
            "  got + Duration.to_seconds(Clock.since(start))\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(
        result.outcome,
        success("101"),
        "{}",
        String::from_utf8_lossy(&result.process.stderr)
    );
}

// ----- Mixed events ---------------------------------------------------------

// Events of different failure sets join in one list: a receive (which may
// fail with `Channel.Aborted`) and a timer (which never fails). A worker
// loop takes jobs until the channel drains and beats on a timer while idle,
// as in the spec's example. Rejects a list that drops the receive's failure,
// and a heartbeat that fires while jobs keep arriving.
#[test]
fn a_worker_takes_jobs_and_beats_while_idle() {
    let result = run_virtual(
        "worker",
        concat!(
            "enum Wake {\n  Next(next: Receive<Int>)\n  Beat\n}\n",
            "fn feed(sender: Channel.Sender<Int>, completion: Channel.Completion<Int>) -> Int needs { clock: Clock.Clock } {\n",
            "  Channel.send(sender, 1)\n",
            "  Clock.sleep(Duration.seconds(1))\n",
            "  Channel.send(sender, 2)\n",
            "  Clock.sleep(Duration.seconds(25))\n",
            "  Channel.send(sender, 3)\n",
            "  Channel.close(completion)\n",
            "  0\n",
            "}\n",
            "fn work(jobs: Receiver<Int>) -> Int needs { clock: Clock.Clock } {\n",
            "  let period = Duration.seconds(10)\n",
            "  loop state = (0, 0, Clock.add(Clock.now(), period)) {\n",
            "    let (sum, beats, next_beat) = state\n",
            "    let wake = case Wait.first([\n",
            "      Wait.receive(jobs, fn(next: Receive<Int>) -> Wake { Wake.Next(next) }),\n",
            "      Wait.at(next_beat, Wake.Beat),\n",
            "    ]) {\n",
            "      Success(wake) -> wake\n",
            "      Failure(ChannelError.Aborted) -> break -1\n",
            "    }\n",
            "    case wake {\n",
            "      Wake.Next(Receive.Received(job)) -> (sum + job, beats, Clock.add(Clock.now(), period))\n",
            "      Wake.Next(Receive.Drained) -> break sum * 100 + beats\n",
            "      Wake.Beat -> (sum, beats + 1, Clock.add(next_beat, period))\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let jobs = Channel.bounded(0)\n",
            "  let sender = jobs.sender\n",
            "  let completion = jobs.completion\n",
            "  let feeder = Task.start(fn() { feed(sender, completion) })\n",
            "  work(jobs.receiver)\n",
            "}\n",
        ),
        "boundaries",
    );
    // Jobs at 0 s and 1 s; beats at 11 s and 21 s; the last job at 26 s.
    assert_eq!(result.outcome, success("602"));
    assert_eq!(
        clock_advances(&result.telemetry),
        [SECOND, 11 * SECOND, 21 * SECOND, 26 * SECOND]
    );
}

// ----- Wait cancellation ------------------------------------------------------

// A timeout that cancels a parked select delivers nothing and leaves the
// channel as it was: a sender that arrives afterwards is never received, so
// closing hands its value back. Rejects a withdrawn select that left a
// registration behind and took the late value.
#[test]
fn cancelling_a_parked_select_leaves_the_channel_unchanged() {
    let result = run_virtual(
        "cancel",
        concat!(
            "import std/channel.{SendError}\n",
            "fn wait(receiver: Receiver<Int>) -> Int {\n",
            "  Wait.first([Wait.receive(receiver, value)])\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let late = Channel.bounded(0)\n",
            "  let receiver = late.receiver\n",
            "  let sender = late.sender\n",
            "  let slow = Task.start(fn() -> Int {\n",
            "    Clock.sleep(Duration.seconds(3))\n",
            "    case Channel.send(sender, 8) {\n",
            "      Success(_) -> 1\n",
            "      Failure(error) -> case error {\n",
            "        SendError.Closed(value) -> value\n",
            "        SendError.Aborted(value) -> value\n",
            "      }\n",
            "    }\n",
            "  })\n",
            "  let timed = case (fn() -> Int { wait(receiver) }) |> Task.timeout(Duration.seconds(1)) {\n",
            "    Success(got) -> got\n",
            "    Failure(ChannelError.Aborted) -> 50\n",
            "    Failure(Timeout.TimedOut(limit)) -> 100\n",
            "  }\n",
            "  Clock.sleep(Duration.seconds(5))\n",
            "  Channel.close(late.completion)\n",
            "  timed + Task.result(slow)\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("108"));
}

// ----- Empty lists ------------------------------------------------------------

// An empty list literal is `T4345`; an empty list built at run time defects.
#[test]
fn an_empty_event_list_is_rejected_or_defects() {
    assert_eq!(
        type_codes("pub fn main() -> Int { Wait.first([]) }\n"),
        ["T4345"]
    );
    let result = run_virtual(
        "empty",
        concat!(
            "import std/list as List\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let events = [Wait.after(Duration.seconds(1), 1)]\n",
            "  Wait.first(List.slice(events, 1..1))\n",
            "}\n",
        ),
        "off",
    );
    assert!(
        result.outcome.contains("\"outcome\":\"defect\"")
            && result.outcome.contains("needs at least one event"),
        "{}",
        result.outcome
    );
}

// ----- Task-local events ------------------------------------------------------

// An event cannot leave its task: not through a started task's capture, and
// not through a channel. Rejects an event treated as plain data.
#[test]
fn an_event_is_task_local() {
    let captured = type_codes(concat!(
        "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
        "  let event = Wait.after(Duration.seconds(1), 1)\n",
        "  let task = Task.start(fn() { Wait.first([event]) })\n",
        "  Task.result(task)\n",
        "}\n",
    ));
    assert_eq!(
        captured,
        ["T4240"],
        "a started task cannot capture an event"
    );
    let sent = type_codes(concat!(
        "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
        "  let channel = Channel.bounded(1)\n",
        "  Channel.send(channel.sender, Wait.after(Duration.seconds(1), 1))\n",
        "  1\n",
        "}\n",
    ));
    assert_eq!(sent, ["T4243"], "an event cannot be sent");
}

// ----- Tracing ----------------------------------------------------------------

// Wait park and commit are boundary events, the commit carrying the
// event's index; `off` constructs none.
#[test]
fn wait_park_and_commit_are_boundary_events() {
    let body = concat!(
        "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
        "  Wait.first([Wait.after(Duration.seconds(5), 1), Wait.after(Duration.seconds(2), 2)])\n",
        "}\n",
    );
    let traced = run_virtual("trace", body, "boundaries");
    assert_eq!(traced.outcome, success("2"));
    let wait_lines: Vec<_> = traced
        .telemetry
        .lines()
        .filter(|line| line.contains("clock_wait"))
        .collect();
    assert_eq!(wait_lines.len(), 2, "{}", traced.telemetry);
    assert!(wait_lines[0].contains("\"event\":\"clock_wait_parked\""));
    assert!(wait_lines[1].contains("\"event\":\"clock_wait_committed\""));
    assert!(
        wait_lines[1].contains("\"index\":1"),
        "{}",
        wait_lines[1]
    );
    let quiet = run_virtual("trace-off", body, "off");
    assert_eq!(quiet.outcome, success("2"));
    assert!(!quiet.telemetry.contains("clock_wait"));
}

// ----- Interpreter --------------------------------------------------------------

#[test]
fn the_interpreter_reports_select_as_native_only() {
    let scratch = scratch_directory("interpreter");
    let path = scratch.join("main.chimi");
    std::fs::write(
        &path,
        format!(
            "{IMPORTS}{VALUE}pub fn main() -> Int needs {{ clock: Clock.Clock }} {{\n  \
             Wait.first([Wait.after(Duration.seconds(1), 1)])\n}}\n"
        ),
    )
    .unwrap();
    let output = interpreter::traced(
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .arg("run")
            .arg(&path),
    )
    .expect("run the reference interpreter");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let outcome = stdout.lines().last().unwrap_or_default();
    assert!(
        outcome.contains("\"outcome\":\"defect\"")
            && outcome.contains("Wait.after requires the native runtime"),
        "{outcome}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
