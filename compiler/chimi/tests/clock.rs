//! Conformance for the `clock: Clock` dependency, `Task.timeout`, and
//! `Task.deadline` (time-and-outcomes.md, "Time sources", "Timeouts and
//! deadlines", and the conformance matrix rows Clock need through Argument
//! order, plus the Tracing row's timer and scope events).
//!
//! Every native run uses the virtual clock, so a test that would wait on the
//! wall clock times the suite out instead of passing slowly.

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMPORTS: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive, SendError}\n",
    "import std/clock as Clock\n",
    "import std/clock.{Instant, Timestamp}\n",
    "import std/duration as Duration\n",
    "import std/task as Task\n",
    "import std/task.{Timeout}\n",
);

/// Receives one value, or 0 once the channel drains.
const TAKE: &str = concat!(
    "fn take(receiver: Channel.Receiver<Int>) -> Int {\n",
    "  let got = Channel.receive(receiver)\n",
    "  case got {\n",
    "    Receive.Received(value) -> value\n",
    "    Receive.Drained -> 0\n",
    "  }\n",
    "}\n",
);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-clock-{name}-{}-{:?}",
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
    std::fs::write(&source, format!("{IMPORTS}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native clock program");
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

fn run(executable: &Path, scratch: &Path, name: &str, trace: &str, runner: &[&str]) -> Run {
    let telemetry_path = scratch.join(format!("{name}-telemetry.jsonl"));
    let outcome_path = scratch.join(format!("{name}-outcome.jsonl"));
    let process = Command::new(executable)
        .arg("--chimi-runner")
        .args(runner)
        .arg(format!("--trace={trace}"))
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--execution-id={name}"))
        .arg("--")
        .output()
        .expect("execute native clock program");
    Run {
        process,
        telemetry: std::fs::read_to_string(telemetry_path).unwrap_or_default(),
        outcome: std::fs::read_to_string(outcome_path).unwrap_or_default(),
    }
}

/// Builds `body` and runs it once under the virtual clock.
fn run_virtual(name: &str, body: &str, trace: &str) -> Run {
    let scratch = scratch_directory(name);
    let executable = build_native(&scratch, name, body);
    let result = run(&executable, &scratch, name, trace, &["--clock=virtual"]);
    std::fs::remove_dir_all(scratch).unwrap();
    result
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

/// The lab projector's lines with task terminals kept. It also checks the key
/// set of every clock event, `clock_scope_fired` included.
fn terminals(telemetry: &str) -> Vec<String> {
    chimi::process_lab::transcript::project(telemetry, &["task_terminal"])
        .unwrap_or_else(|error| panic!("{error}\n{telemetry}"))
}

/// The telemetry's event names, in order.
fn events(telemetry: &str) -> Vec<String> {
    telemetry
        .lines()
        .map(|line| {
            let start = line.find("\"event\":\"").expect("an event has a name") + 9;
            line[start..].split('"').next().unwrap().to_owned()
        })
        .collect()
}

fn type_project(source: &str) -> chimi::TypeReport {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", format!("{IMPORTS}{source}")),
    )
    .expect("valid module path");
    Compiler::type_project(vec![module])
}

// ----- Clock need ---------------------------------------------------------

// Rejects a time function without its `clock` need, a caller whose inferred
// needs drop it, and a timeout that loses the operation's own needs or rows.
#[test]
fn time_functions_surface_the_clock_need() {
    let report = type_project(concat!(
        "interface Store { load: fn() -> Int }\n",
        "enum LoadError { Missing }\n",
        "fn stamp() -> Instant { Clock.now() }\n",
        "fn load() -> Int needs { store: Store } {\n",
        "  let value = store.load()\n",
        "  case value {\n    0 -> Failure(LoadError.Missing)\n    found -> found\n  }\n",
        "}\n",
        "fn bounded() { load |> Task.timeout(Duration.seconds(1)) }\n",
    ));
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "stamp").as_deref(),
        Some("fn() -> Instant needs { clock: Clock }")
    );
    assert_eq!(
        report.contract("main", "bounded").as_deref(),
        Some("fn() -> Int errors LoadError | Timeout needs { clock: Clock, store: Store }")
    );
    for (function, contract) in [
        ("Clock.now", "fn() -> Instant needs { clock: Clock }"),
        (
            "Clock.timestamp",
            "fn() -> Timestamp needs { clock: Clock }",
        ),
        (
            "Clock.since",
            "fn(Instant) -> Duration needs { clock: Clock }",
        ),
        (
            "Clock.sleep_until",
            "fn(Instant) -> Void needs { clock: Clock }",
        ),
        ("Clock.add", "fn(Instant, Duration) -> Instant"),
        ("Clock.between", "fn(Instant, Instant) -> Duration"),
        ("Clock.starting_at", "fn(Clock, Timestamp) -> Clock"),
        ("Clock.timestamp_to_text", "fn(Timestamp) -> Text"),
        ("Clock.unix_nanoseconds", "fn(Timestamp) -> Int"),
        ("Clock.from_unix_nanoseconds", "fn(Int) -> Timestamp"),
        (
            "Clock.timestamp_between",
            "fn(Timestamp, Timestamp) -> Duration",
        ),
    ] {
        assert_eq!(
            report.standard_contract(function).as_deref(),
            Some(contract),
            "{function}"
        );
    }
}

// The runner closes a root `clock` need that `main` only infers.
#[test]
fn the_runner_closes_an_inferred_root_clock_need() {
    let result = run_virtual(
        "inferred-need",
        concat!(
            "fn nap() -> Int {\n  Clock.sleep(Duration.seconds(5))\n  1\n}\n",
            "pub fn main() -> Int {\n",
            "  let start = Clock.now()\n",
            "  let napped = nap()\n",
            "  Duration.to_seconds(Clock.since(start)) * 10 + napped\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(result.outcome, success("51"));
}

// ----- Virtual time ---------------------------------------------------------

// Rejects timers woken in start order, a `since` that measures anything but
// virtual time, and a run that is not repeatable.
#[test]
fn virtual_sleepers_wake_in_deadline_order_and_since_is_exact() {
    let body = format!(
        "{TAKE}pub fn main() -> Int {{\n  \
         let start = Clock.now()\n  \
         let channel = Channel.bounded(2)\n  \
         let sender = channel.sender\n  \
         let slow = Task.start(fn() -> Int {{\n    \
         Clock.sleep(Duration.seconds(5))\n    \
         Channel.send(sender, 5)\n    \
         0\n  }})\n  \
         let fast = Task.start(fn() -> Int {{\n    \
         Clock.sleep(Duration.seconds(2))\n    \
         Channel.send(sender, 2)\n    \
         0\n  }})\n  \
         let first = take(channel.receiver)\n  \
         let second = take(channel.receiver)\n  \
         let total = Task.result(slow) + Task.result(fast)\n  \
         total + first * 1000 + second * 100 + Duration.to_seconds(Clock.since(start))\n}}\n"
    );
    let scratch = scratch_directory("virtual-order");
    let executable = build_native(&scratch, "virtual-order", &body);
    let first = run(
        &executable,
        &scratch,
        "order-1",
        "boundaries",
        &["--clock=virtual"],
    );
    assert_eq!(first.outcome, success("2505"));
    assert_eq!(
        clock_advances(&first.telemetry),
        [2_000_000_000, 5_000_000_000]
    );
    let second = run(
        &executable,
        &scratch,
        "order-2",
        "boundaries",
        &["--clock=virtual"],
    );
    assert_eq!(second.outcome, first.outcome);
    assert_eq!(
        terminals(&second.telemetry),
        terminals(&first.telemetry),
        "a virtual run repeats identically"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Starting timestamp ---------------------------------------------------

// Rejects a `starting_at` that leaves `timestamp` unpinned, stops it from
// advancing, or shifts `now`; and a `--clock-start` that is ignored, accepted
// with the real clock, or accepted when malformed.
#[test]
fn starting_at_pins_the_timestamp_and_leaves_now_alone() {
    let body = concat!(
        "fn stamp() -> Text {\n",
        "  let before = Clock.now()\n",
        "  Clock.sleep(Duration.seconds(1))\n",
        "  let elapsed = Clock.between(before, Clock.now())\n",
        "  \"${Clock.timestamp_to_text(Clock.timestamp())} ${Duration.to_text(elapsed)}\"\n",
        "}\n",
        "fn read() -> Instant { Clock.now() }\n",
        "pub fn main() -> Text needs { clock: Clock.Clock } {\n",
        "  let pinned = Clock.from_unix_nanoseconds(1762767000250000000)\n",
        "  let fixed = Clock.starting_at(clock, pinned)\n",
        "  let inner = stamp using { clock: fixed }\n",
        "  let under = inner()\n",
        "  let outside = stamp()\n",
        "  let unix = Clock.unix_nanoseconds(Clock.timestamp())\n",
        "  let wrapped = read using { clock: fixed }\n",
        "  let drift = Duration.to_text(Clock.between(Clock.now(), wrapped()))\n",
        "  \"${under} | ${outside} | ${unix} | ${drift}\"\n",
        "}\n",
    );
    let scratch = scratch_directory("starting-at");
    let executable = build_native(&scratch, "starting-at", body);
    let default = run(
        &executable,
        &scratch,
        "default",
        "off",
        &["--clock=virtual"],
    );
    assert_eq!(
        default.outcome,
        success("\"2025-11-10T09:30:01.25Z 1s | 1970-01-01T00:00:02Z 1s | 2000000000 | 0s\"")
    );
    let started = run(
        &executable,
        &scratch,
        "started",
        "off",
        &[
            "--clock=virtual",
            "--clock-start=2026-11-10T09:30:00.25+01:00",
        ],
    );
    assert_eq!(
        started.outcome,
        success(
            "\"2025-11-10T09:30:01.25Z 1s | 2026-11-10T08:30:02.25Z 1s | 1794299402250000000 | 0s\""
        )
    );
    let real_only = "--clock-start requires --clock=virtual";
    let malformed = "--clock-start requires an RFC 3339 timestamp";
    for (name, runner, reason) in [
        (
            "real",
            &["--clock-start=2026-11-10T09:30:00Z"][..],
            real_only,
        ),
        (
            "hour",
            &["--clock=virtual", "--clock-start=2026-11-10T25:30:00Z"][..],
            malformed,
        ),
        (
            "leap",
            &["--clock=virtual", "--clock-start=2026-12-31T23:59:60Z"][..],
            malformed,
        ),
        (
            "late",
            &["--clock=virtual", "--clock-start=2263-01-01T00:00:00Z"][..],
            malformed,
        ),
        (
            "early",
            &["--clock=virtual", "--clock-start=1677-12-31T00:00:00Z"][..],
            malformed,
        ),
    ] {
        let rejected = run(&executable, &scratch, name, "off", runner);
        assert_eq!(rejected.process.status.code(), Some(64), "{name}");
        assert!(rejected.outcome.is_empty(), "{name}: {}", rejected.outcome);
        let stderr = String::from_utf8_lossy(&rejected.process.stderr);
        assert!(stderr.contains(reason), "{name}: {stderr}");
    }
    let earliest = run(
        &executable,
        &scratch,
        "earliest",
        "off",
        &["--clock=virtual", "--clock-start=1678-01-01T00:00:00Z"],
    );
    assert_eq!(
        earliest.process.status.code(),
        Some(0),
        "{}",
        earliest.outcome
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Timeout fires --------------------------------------------------------

// Rejects a timeout that waits for the operation, fires at the wrong time,
// carries the wrong limit, or leaves a child of the scope running.
#[test]
fn a_timeout_fires_at_its_limit_and_drains_children_of_the_scope() {
    let result = run_virtual(
        "fires",
        concat!(
            "fn work() -> Int {\n",
            "  let helper = Task.start(fn() -> Int {\n",
            "    Clock.sleep(Duration.seconds(10))\n",
            "    7\n",
            "  })\n",
            "  Clock.sleep(Duration.seconds(5))\n",
            "  Task.result(helper)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let start = Clock.now()\n",
            "  case work |> Task.timeout(Duration.seconds(1)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) ->\n",
            "      Duration.to_seconds(limit) * 100 + Duration.to_seconds(Clock.since(start))\n",
            "  }\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("101"));
    assert_eq!(clock_advances(&result.telemetry), [1_000_000_000]);
    assert_eq!(
        terminals(&result.telemetry),
        [
            "@1s task_terminal task=T2 parent=T1 cause=-",
            "@1s task_terminal task=T1 parent=- cause=-",
        ]
    );
}

// ----- Timeout passes -------------------------------------------------------

// Rejects a timeout that rewrites the operation's own success, failure, or
// defect, or leaves its timer behind (a leftover timer would move the clock to
// 3 s or 4 s before the final sleep).
#[test]
fn an_operation_that_finishes_first_keeps_its_outcome() {
    let result = run_virtual(
        "passes",
        concat!(
            "enum WorkError { Broken(code: Int) }\n",
            "fn quick() -> Int {\n  Clock.sleep(Duration.seconds(1))\n  5\n}\n",
            "fn failing() -> Int {\n",
            "  Clock.sleep(Duration.seconds(1))\n",
            "  Failure(WorkError.Broken(3))\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let kept = case quick |> Task.timeout(Duration.seconds(4)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 0\n",
            "  }\n",
            "  let failed = case failing |> Task.timeout(Duration.seconds(2)) {\n",
            "    Success(value) -> value\n",
            "    Failure(WorkError.Broken(code)) -> code * 10\n",
            "    Failure(Timeout.TimedOut(limit)) -> 0\n",
            "  }\n",
            "  Clock.sleep(Duration.seconds(5))\n",
            "  kept + failed\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("35"));
    assert_eq!(
        clock_advances(&result.telemetry),
        [1_000_000_000, 2_000_000_000, 7_000_000_000]
    );
    assert!(!events(&result.telemetry).contains(&"clock_scope_fired".to_owned()));

    let defect = run_virtual(
        "passes-defect",
        concat!(
            "fn broken() -> Int {\n",
            "  Clock.sleep(Duration.seconds(1))\n",
            "  let zero = Duration.to_seconds(Duration.zero)\n",
            "  10 / zero\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  case broken |> Task.timeout(Duration.seconds(2)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 0\n",
            "  }\n",
            "}\n",
        ),
        "off",
    );
    assert_eq!(defect.process.status.code(), Some(70));
    assert!(
        defect.outcome.contains("\"outcome\":\"defect\"")
            && defect.outcome.contains("integer division by zero"),
        "{}",
        defect.outcome
    );
}

// ----- Timeout scope --------------------------------------------------------

// Rejects an inner timeout that converts its outer scope's cancellation, and
// an outer timeout that converts an inner scope's `TimedOut` as its own.
#[test]
fn nested_timeouts_convert_only_their_own_scope() {
    let result = run_virtual(
        "nested",
        concat!(
            "fn nap() -> Int {\n  Clock.sleep(Duration.seconds(10))\n  1\n}\n",
            "fn inner_long() -> Int {\n",
            "  case nap |> Task.timeout(Duration.seconds(5)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 500\n",
            "  }\n",
            "}\n",
            "fn inner_short() -> Int { nap |> Task.timeout(Duration.seconds(1)) }\n",
            "pub fn main() -> Int {\n",
            "  let outer = case inner_long |> Task.timeout(Duration.seconds(2)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> Duration.to_seconds(limit) * 1000\n",
            "  }\n",
            "  let start = Clock.now()\n",
            "  let through = case inner_short |> Task.timeout(Duration.seconds(3)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) ->\n",
            "      Duration.to_seconds(limit) * 10 + Duration.to_seconds(Clock.since(start))\n",
            "  }\n",
            "  outer + through\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("2011"));
    assert_eq!(
        clock_advances(&result.telemetry),
        [2_000_000_000, 3_000_000_000]
    );
}

// Rejects an inner timeout that converts the task's own cancellation: the
// child would then send 2 and the receive would see it instead of draining.
#[test]
fn task_cancellation_passes_through_a_timeout() {
    let result = run_virtual(
        "cancel-through",
        &format!(
            "{TAKE}pub fn main() -> Int {{\n  \
             let channel = Channel.bounded(1)\n  \
             let sender = channel.sender\n  \
             let child = Task.start(fn() -> Int {{\n    \
             let guarded = fn() -> Int {{\n      \
             Clock.sleep(Duration.seconds(10))\n      \
             1\n    }}\n    \
             let outcome = case guarded |> Task.timeout(Duration.seconds(5)) {{\n      \
             Success(value) -> value\n      \
             Failure(Timeout.TimedOut(limit)) -> 2\n    }}\n    \
             Channel.send(sender, outcome)\n    \
             outcome\n  }})\n  \
             Clock.sleep(Duration.seconds(1))\n  \
             Task.cancel(child)\n  \
             Channel.close(channel.completion)\n  \
             100 + take(channel.receiver)\n}}\n"
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("100"));
    assert_eq!(clock_advances(&result.telemetry), [1_000_000_000]);
    assert!(!events(&result.telemetry).contains(&"clock_scope_fired".to_owned()));
}

// ----- Receive and timeout --------------------------------------------------

// Rejects a timeout that discards a value received before the deadline, and a
// cancelled receive that stays parked: the late send at 3 s would then hand
// its value to the dead receive and succeed (1) instead of failing when the
// channel closes (8).
#[test]
fn a_receive_keeps_its_value_and_a_winning_timeout_takes_none() {
    let result = run_virtual(
        "receive",
        &format!(
            "{TAKE}pub fn main() -> Int {{\n  \
             let early = Channel.bounded(0)\n  \
             let early_receiver = early.receiver\n  \
             let early_sender = early.sender\n  \
             let producer = Task.start(fn() -> Int {{\n    \
             Clock.sleep(Duration.seconds(1))\n    \
             Channel.send(early_sender, 7)\n    \
             1\n  }})\n  \
             let kept = case (fn() -> Int {{ take(early_receiver) }}) |> Task.timeout(Duration.seconds(5)) {{\n    \
             Success(value) -> value\n    \
             Failure(Timeout.TimedOut(limit)) -> 0\n  }}\n  \
             let late = Channel.bounded(0)\n  \
             let late_receiver = late.receiver\n  \
             let late_sender = late.sender\n  \
             let slow = Task.start(fn() -> Int {{\n    \
             Clock.sleep(Duration.seconds(3))\n    \
             case Channel.send(late_sender, 8) {{\n      \
             Success(_) -> 1\n      \
             Failure(error) -> case error {{\n        \
             SendError.Closed(value) -> value\n        \
             SendError.Aborted(value) -> value\n      }}\n    }}\n  }})\n  \
             let lost = case (fn() -> Int {{ take(late_receiver) }}) |> Task.timeout(Duration.seconds(1)) {{\n    \
             Success(value) -> value\n    \
             Failure(Timeout.TimedOut(limit)) -> 0\n  }}\n  \
             Clock.sleep(Duration.seconds(5))\n  \
             Channel.close(late.completion)\n  \
             Task.result(producer) * 1000 + kept * 100 + lost * 10 + Task.result(slow)\n}}\n"
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("1708"));
}

// ----- Non-positive timeout -------------------------------------------------

// `never` has no cancellation point, so any run of it returns 9. Rejects a
// non-positive timeout or a past deadline that starts the operation, and a
// deadline whose `TimedOut` does not carry the time from the call.
#[test]
fn a_non_positive_limit_never_starts_the_operation() {
    let result = run_virtual(
        "non-positive",
        concat!(
            "fn never() -> Int { 9 }\n",
            "pub fn main() -> Int {\n",
            "  let zero = case never |> Task.timeout(Duration.zero) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 1 + Duration.to_seconds(limit)\n",
            "  }\n",
            "  let negative = case never |> Task.timeout(Duration.seconds(-3)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 10 * (5 + Duration.to_seconds(limit))\n",
            "  }\n",
            "  Clock.sleep(Duration.seconds(4))\n",
            "  let past = Clock.add(Clock.now(), Duration.seconds(-2))\n",
            "  let passed = case never |> Task.deadline(past) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 100 * (5 + Duration.to_seconds(limit))\n",
            "  }\n",
            "  let start = Clock.now()\n",
            "  let slow = fn() -> Int {\n    Clock.sleep(Duration.seconds(10))\n    9\n  }\n",
            "  let ahead = case slow |> Task.deadline(Clock.add(start, Duration.seconds(3))) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) ->\n",
            "      1000 * Duration.to_seconds(limit) + 10000 * Duration.to_seconds(Clock.since(start))\n",
            "  }\n",
            "  zero + negative + passed + ahead\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("33321"));
    assert_eq!(
        clock_advances(&result.telemetry),
        [4_000_000_000, 7_000_000_000]
    );
}

// ----- Argument order -------------------------------------------------------

// Rejects a pipeline form that types or runs differently from the call form,
// and a `with_` twin of a combinator in any std module.
#[test]
fn the_pipeline_and_call_forms_are_the_same_timeout() {
    let operation = "fn nap() -> Int {\n  Clock.sleep(Duration.seconds(3))\n  4\n}\n";
    let piped = "fn bounded() { nap |> Task.timeout(Duration.seconds(1)) }\n";
    let called = "fn bounded() { Task.timeout(nap, Duration.seconds(1)) }\n";
    let contract = |form: &str| {
        let report = type_project(&format!("{operation}{form}"));
        assert!(report.is_success(), "{}", report.render_diagnostics());
        report.contract("main", "bounded")
    };
    assert_eq!(contract(piped), contract(called));
    assert_eq!(
        contract(piped).as_deref(),
        Some("fn() -> Int errors Timeout needs { clock: Clock }")
    );
    let traced = |name: &str, form: &str| {
        let result = run_virtual(
            name,
            &format!(
                "{operation}{form}pub fn main() -> Int {{\n  \
                 case bounded() {{\n    \
                 Success(value) -> value\n    \
                 Failure(Timeout.TimedOut(limit)) -> 10\n  }}\n}}\n"
            ),
            "full",
        );
        assert_eq!(result.outcome, success("10"));
        events(&result.telemetry)
    };
    assert_eq!(traced("piped", piped), traced("called", called));
    let std_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("std");
    let mut twins = Vec::new();
    for entry in std::fs::read_dir(&std_dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        twins.extend(
            text.lines()
                .filter(|line| line.contains("fn with_"))
                .map(|line| format!("{}: {line}", path.display())),
        );
    }
    assert!(twins.is_empty(), "{twins:?}");
}

// ----- Providers ------------------------------------------------------------

// A user `Clock` reaches the time functions through `using`. Rejects time
// functions that bypass the bound provider for the execution clock.
#[test]
fn a_user_clock_provider_serves_the_time_functions() {
    let result = run_virtual(
        "provider",
        concat!(
            "fn measure() -> Int {\n",
            "  let start = Clock.now()\n",
            "  Clock.sleep(Duration.seconds(2))\n",
            "  Duration.to_seconds(Clock.since(start))\n",
            "}\n",
            "fn read() -> Instant { Clock.now() }\n",
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
            "  let ahead = read using { clock: shifted }\n",
            "  let offset = Duration.to_seconds(Clock.between(Clock.now(), ahead()))\n",
            "  let measured = measure using { clock: shifted }\n",
            "  offset * 10 + measured()\n",
            "}\n",
        ),
        "boundaries",
    );
    assert_eq!(result.outcome, success("36002"));
    assert_eq!(clock_advances(&result.telemetry), [2_000_000_000]);
}

// ----- Tracing --------------------------------------------------------------

// Scope firing is a boundary event; timer start, fire, and cancel are motion
// events; `off` constructs nothing. Rejects a scope event missing from
// `boundaries`, timer events leaking into it, and events under `off`.
#[test]
fn timeout_scope_firing_is_a_boundary_event() {
    let body = concat!(
        "fn nap() -> Int {\n  Clock.sleep(Duration.seconds(5))\n  1\n}\n",
        "pub fn main() -> Int {\n",
        "  case nap |> Task.timeout(Duration.seconds(1)) {\n",
        "    Success(value) -> value\n",
        "    Failure(Timeout.TimedOut(limit)) -> 2\n",
        "  }\n",
        "}\n",
    );
    let scratch = scratch_directory("tracing");
    let executable = build_native(&scratch, "tracing", body);
    let clock_events = |trace: &str| {
        let result = run(&executable, &scratch, trace, trace, &["--clock=virtual"]);
        assert_eq!(result.outcome, success("2"), "{trace}");
        if trace != "off" {
            terminals(&result.telemetry);
        }
        events(&result.telemetry)
            .into_iter()
            .filter(|event| event.starts_with("clock_"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        clock_events("boundaries"),
        ["clock_advanced", "clock_scope_fired"]
    );
    assert_eq!(
        clock_events("full"),
        [
            "clock_timer_started",
            "clock_timer_started",
            "clock_advanced",
            "clock_timer_fired",
            "clock_scope_fired",
            "clock_timer_cancelled",
        ]
    );
    assert!(clock_events("off").is_empty());
    std::fs::remove_dir_all(scratch).unwrap();
}

/// An integer attribute of a telemetry line.
fn integer_attribute(line: &str, name: &str) -> i64 {
    let key = format!("\"{name}\":");
    let start = line.find(&key).unwrap_or_else(|| panic!("no {name} in {line}")) + key.len();
    line[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '-')
        .collect::<String>()
        .parse()
        .expect("an integer attribute")
}

/// The semantic events, as (event, operation, time_ns), in sequence order.
fn semantic_times(telemetry: &str) -> Vec<(String, String, i64)> {
    telemetry
        .lines()
        .filter(|line| line.contains("\"operation\":"))
        .map(|line| {
            let event = line.split("\"event\":\"").nth(1).unwrap().split('"').next().unwrap();
            let operation = line.split("\"operation\":\"").nth(1).unwrap().split('"').next().unwrap();
            (event.to_owned(), operation.to_owned(), integer_attribute(line, "time_ns"))
        })
        .collect()
}

// Every semantic event carries the execution time it happened at, and the
// computation's start carries the Unix time of the clock's origin. Rejects an
// event without a time, a time that does not include a sleep before it (a
// dependency call's included), a computation that ends before its last
// operation, and a start that ignores `--clock-start` or the real wall clock.
#[test]
fn semantic_events_carry_their_execution_times() {
    let body = concat!(
        "interface Store { load: fn() -> Int }\n",
        "fn nap() -> Int {\n  Clock.sleep(Duration.milliseconds(5))\n  1\n}\n",
        "fn loaded() -> Int needs { clock: Clock.Clock, store: Store } {\n  let first = nap() + store.load()\n  first + nap()\n}\n",
        "pub fn main() -> Int {\n",
        "  let run = loaded using { store: { load: fn() -> Int { 1 } } }\n",
        "  run()\n",
        "}\n",
    );
    let scratch = scratch_directory("event-times");
    let executable = build_native(&scratch, "event-times", body);
    let result = run(&executable, &scratch, "default", "full", &["--clock=virtual"]);
    assert_eq!(result.outcome, success("3"));
    let times = semantic_times(&result.telemetry);
    let first = times.first().expect("events");
    assert_eq!((first.0.as_str(), first.2), ("computation_started", 0));
    let start_line = result.telemetry.lines().find(|line| line.contains("computation_started")).unwrap();
    assert_eq!(integer_attribute(start_line, "start_unix_ns"), 0);
    let last = times.last().expect("events");
    assert_eq!((last.0.as_str(), last.2), ("computation_completed", 10_000_000));
    let completed_calls = times
        .iter()
        .filter(|(event, operation, _)| event == "operation_completed" && operation == "call")
        .map(|(_, _, time)| *time)
        .collect::<Vec<_>>();
    assert!(
        completed_calls.contains(&5_000_000) && completed_calls.contains(&10_000_000),
        "{times:?}"
    );
    let dependency = times
        .iter()
        .find(|(event, _, _)| event == "dependency_called")
        .expect("the store call");
    assert_eq!(dependency.2, 5_000_000, "the store is called after the first nap");
    assert!(times.windows(2).all(|pair| pair[0].2 <= pair[1].2), "{times:?}");

    let started = run(
        &executable,
        &scratch,
        "started",
        "full",
        &["--clock=virtual", "--clock-start=2026-11-10T09:30:00Z"],
    );
    let start_line = started.telemetry.lines().find(|line| line.contains("computation_started")).unwrap();
    assert_eq!(integer_attribute(start_line, "start_unix_ns"), 1_794_303_000_000_000_000);
    assert_eq!(semantic_times(&started.telemetry), times, "the start moves no event");

    let real = run(&executable, &scratch, "real", "full", &[]);
    let start_line = real.telemetry.lines().find(|line| line.contains("computation_started")).unwrap();
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let start_unix = integer_attribute(start_line, "start_unix_ns");
    assert!(start_unix <= wall && wall - start_unix < 120_000_000_000, "{start_unix} vs {wall}");
    let real_times = semantic_times(&real.telemetry);
    assert!(real_times.windows(2).all(|pair| pair[0].2 <= pair[1].2), "{real_times:?}");
    assert!(real_times.last().unwrap().2 >= 10_000_000, "two real naps: {real_times:?}");
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Each semantic event's name and attributes, in order.
fn event_attributes(telemetry: &str) -> Vec<String> {
    telemetry
        .lines()
        .filter(|line| line.contains("\"event\":") && line.contains("\"operation\":"))
        .map(|line| {
            let event = line.split("\"event\":\"").nth(1).unwrap().split('"').next().unwrap();
            let attributes = line.split("\"attributes\":").nth(1).unwrap();
            format!("{event} {attributes}")
        })
        .collect()
}

// The interpreter writes the times the native runner writes under the virtual
// clock for a program that never waits: every attribute, in the same order.
// Rejects an interpreter that leaves either time out or orders them
// differently.
#[test]
fn the_interpreter_writes_the_native_virtual_event_times() {
    let body = concat!(
        "interface Store { load: fn() -> Int }\n",
        "fn loaded() -> Int needs { store: Store } {\n  store.load() + 1\n}\n",
        "pub fn main() -> Int {\n",
        "  let run = loaded using { store: { load: fn() -> Int { 1 } } }\n",
        "  run()\n",
        "}\n",
    );
    let scratch = scratch_directory("interpreter-times");
    let executable = build_native(&scratch, "interpreter-times", body);
    let native = run(&executable, &scratch, "native", "full", &["--clock=virtual"]);
    assert_eq!(native.outcome, success("2"));
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(scratch.join("interpreter-times.chimi"))
        .output()
        .expect("run the reference interpreter");
    let interpreted = event_attributes(&String::from_utf8_lossy(&output.stdout));
    assert!(interpreted.first().unwrap().contains("\"start_unix_ns\":0"), "{interpreted:?}");
    assert!(interpreted.iter().any(|line| line.starts_with("dependency_called")), "{interpreted:?}");
    assert_eq!(interpreted, event_attributes(&native.telemetry));
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Interpreter ----------------------------------------------------------

// The interpreter has no scheduler clock. Rejects one that runs a timeout or
// reads time as if no time passed instead of saying so.
#[test]
fn the_interpreter_reports_the_clock_operations_as_native_only() {
    let scratch = scratch_directory("interpreter");
    for (name, call) in [
        ("Clock.now", "Clock.now()\n  1"),
        (
            "Task.timeout",
            "case (fn() -> Int { 1 }) |> Task.timeout(Duration.seconds(1)) {\n    \
             Success(value) -> value\n    \
             Failure(Timeout.TimedOut(limit)) -> 0\n  }",
        ),
    ] {
        let path = scratch.join("main.chimi");
        std::fs::write(
            &path,
            format!("{IMPORTS}pub fn main() -> Int {{\n  {call}\n}}\n"),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .arg("run")
            .arg(&path)
            .output()
            .expect("run the reference interpreter");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let outcome = stdout.lines().last().unwrap_or_default();
        assert!(
            outcome.contains("\"outcome\":\"defect\"")
                && outcome.contains(&format!(
                    "{name} requires the native runtime; build the program with `chimi build`"
                )),
            "{name}: {outcome}"
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Real clock -----------------------------------------------------------

// Every loop pass is a cancellation point. Under the real clock a loop that
// never parks gives the scheduler no chance to fire timers, so it must poll.
// Rejects a timeout that never fires around a busy loop, with or without
// `Clock.now()` in it (the run would hang until the 60 s guard kills it), and
// nested scopes that both fire at one poll delivering the inner one's
// cancellation: the inner would then convert it (100) instead of the outer
// (1000).
#[test]
fn a_real_clock_timeout_stops_a_loop_that_never_parks() {
    let scratch = scratch_directory("busy-loop");
    let executable = build_native(
        &scratch,
        "busy-loop",
        concat!(
            "fn spin() -> Int {\n",
            "  loop n = 0 {\n    case n {\n      m if m < 0 -> break m\n      m -> m + 1\n    }\n  }\n",
            "}\n",
            "fn reading() -> Int {\n",
            "  loop n = 0 {\n    let at = Clock.now()\n",
            "    case n {\n      m if m < 0 -> break m\n      m -> m + 1\n    }\n  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let spun = case spin |> Task.timeout(Duration.milliseconds(20)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 1\n",
            "  }\n",
            "  let read = case reading |> Task.timeout(Duration.milliseconds(20)) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 10\n",
            "  }\n",
            "  let at = Clock.add(Clock.now(), Duration.milliseconds(20))\n",
            "  let inner = fn() -> Int {\n",
            "    case spin |> Task.deadline(at) {\n",
            "      Success(value) -> value\n",
            "      Failure(Timeout.TimedOut(limit)) -> 100\n",
            "    }\n",
            "  }\n",
            "  let nested = case inner |> Task.deadline(at) {\n",
            "    Success(value) -> value\n",
            "    Failure(Timeout.TimedOut(limit)) -> 1000\n",
            "  }\n",
            "  spun + read + nested\n",
            "}\n",
        ),
    );
    let outcome_path = scratch.join("busy-loop-outcome.jsonl");
    let mut child = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--")
        .spawn()
        .expect("start the busy-loop program");
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > std::time::Duration::from_secs(60) {
            child.kill().unwrap();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(status.is_some(), "the timeout never stopped the loop");
    assert_eq!(
        std::fs::read_to_string(&outcome_path).unwrap(),
        success("1011")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Forced process shutdown ----------------------------------------------

// A process body forced to stop while inside a timeout must unwind without
// issuing the scope exit: the scheduler abandons a forced body at its next
// park, which would leak the body's stack and heap. Rejects that leak: the
// run that wraps the body's receive loop in `Task.timeout` must end with as
// many live heap chunks as the run that does not.
#[test]
fn a_forced_process_inside_a_timeout_releases_its_heap() {
    let program = |body: &str| {
        format!(
            "import std/channel.{{Sender}}\n\
             import std/process as Process\n\
             import std/process.{{Inbox}}\n\
             import std/supervisor as Supervisor\n\
             pub enum Message {{\n  Add(amount: Int, report: Sender<Int>)\n}}\n\
             fn report_each(inbox: Inbox<Message>) -> Int {{\n  \
             loop reported = 0 {{\n    \
             case Process.receive(inbox) {{\n      \
             Message.Add(amount, report) -> {{\n        \
             Channel.send(report, amount)\n        \
             reported + 1\n      }}\n    }}\n  }}\n}}\n\
             fn body(inbox: Inbox<Message>) -> Void {{\n{body}}}\n\
             {TAKE}\
             pub fn main() -> Int needs {{ clock: Clock.Clock }} {{\n  \
             let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(30))))\n  \
             let managed = Supervisor.start_process(root, Process.spec(body, 3, Duration.zero), Supervisor.Restart.OnDefect)\n  \
             let address = Supervisor.address(managed)\n  \
             let reports = Channel.bounded(4)\n  \
             Clock.sleep(Duration.milliseconds(1))\n  \
             Process.send(address, Message.Add(1, reports.sender))\n  \
             Supervisor.remove_process(root, managed)\n  \
             Channel.send(reports.sender, 9)\n  \
             let first = take(reports.receiver)\n  \
             Supervisor.stop(root)\n  \
             first\n}}\n"
        )
    };
    let scratch = scratch_directory("forced-scope");
    let live_chunks = |name: &str, body: &str| {
        let executable = build_native(&scratch, name, &program(body));
        let memory = scratch.join(format!("{name}.memory.json"));
        let outcome = scratch.join(format!("{name}.outcome.jsonl"));
        let process = Command::new(&executable)
            .arg("--chimi-runner")
            .arg("--clock=virtual")
            .arg("--trace=off")
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg(format!("--memory-report-file={}", memory.display()))
            .arg("--")
            .output()
            .expect("run the forced-shutdown program");
        assert_eq!(process.status.code(), Some(0), "{name}");
        assert_eq!(std::fs::read_to_string(&outcome).unwrap(), success("9"));
        let report = std::fs::read_to_string(&memory).unwrap();
        let start = report.find("\"heap_chunks_live\":").expect("live chunks") + 19;
        report[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
    };
    let plain = live_chunks("plain", "  let n = report_each(inbox)\n  Void\n");
    let scoped = live_chunks(
        "scoped",
        concat!(
            "  case (fn() -> Int { report_each(inbox) }) |> Task.timeout(Duration.seconds(100)) {\n",
            "    Success(value) -> Void\n",
            "    Failure(Timeout.TimedOut(limit)) -> Void\n",
            "  }\n",
        ),
    );
    assert_eq!(scoped, plain);
    std::fs::remove_dir_all(scratch).unwrap();
}
