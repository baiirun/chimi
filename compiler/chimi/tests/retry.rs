//! Conformance for `std/schedule` and `std/retry` (time-and-outcomes.md,
//! "Retry", and the conformance matrix rows Retry and Retry typing).
//!
//! Every run uses the virtual clock, so the backoff sleeps cost no wall time
//! and each `clock_advanced` event shows exactly when an attempt started.

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::{Path, PathBuf};
use std::process::Command;

const IMPORTS: &str = concat!(
    "import std/clock as Clock\n",
    "import std/duration as Duration\n",
    "import std/retry as Retry\n",
    "import std/schedule as Schedule\n",
    "import std/task as Task\n",
    "import std/task.{Timeout}\n",
    "enum JobError {\n  Transient(at_ms: Int)\n  Rejected(at_ms: Int)\n}\n",
    "fn transient(error: JobError) -> Bool {\n",
    "  case error {\n    JobError.Transient(_) -> True\n    JobError.Rejected(_) -> False\n  }\n",
    "}\n",
    "fn elapsed_ms(start: Clock.Instant) -> Int {\n",
    "  Duration.to_milliseconds(Clock.since(start))\n",
    "}\n",
);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("chimi-retry-{name}-{}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct Run {
    outcome: String,
    /// The virtual times, in milliseconds, the clock advanced to.
    advances_ms: Vec<i64>,
}

/// Builds `body` natively and runs it once under the virtual clock.
fn run_virtual(name: &str, body: &str) -> Run {
    let scratch = scratch_directory(name);
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, format!("{IMPORTS}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native retry program");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let telemetry_path = scratch.join("telemetry.jsonl");
    let outcome_path = scratch.join("outcome.jsonl");
    Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg("--trace=boundaries")
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--")
        .output()
        .expect("run native retry program");
    let telemetry = std::fs::read_to_string(&telemetry_path).unwrap_or_default();
    let outcome = std::fs::read_to_string(&outcome_path).unwrap_or_default();
    std::fs::remove_dir_all(scratch).unwrap();
    let advances_ms = telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_advanced\""))
        .map(|line| {
            let start = line.find("\"time_ns\":").expect("an advance has time_ns") + 10;
            let nanoseconds: i64 = line[start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("time_ns is a count");
            nanoseconds / 1_000_000
        })
        .collect();
    Run { outcome, advances_ms }
}

fn failure(variant: &str, at_ms: i64) -> String {
    format!(
        "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"expected_failure\",\"error\":\
         {{\"type\":\"JobError\",\"variant\":\"{variant}\",\"fields\":{{\"at_ms\":{at_ms}}}}}}}\n"
    )
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

fn type_project(source: &str) -> chimi::TypeReport {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", format!("{IMPORTS}{source}")),
    )
    .expect("valid module path");
    Compiler::type_project(vec![module])
}

fn codes(report: &chimi::TypeReport) -> Vec<String> {
    report.diagnostics().iter().map(|diagnostic| diagnostic.code().to_owned()).collect()
}

// ----- Running -------------------------------------------------------------

// A retried failure sleeps each scheduled delay before the next attempt, and
// exhaustion fails with the last attempt's own failure, not the first one's.
#[test]
fn selected_failures_retry_on_schedule_until_exhausted() {
    let run = run_virtual(
        "exhausted",
        concat!(
            "pub fn main() -> Int {\n",
            "  let start = Clock.now()\n",
            "  let attempt = fn() { Failure(JobError.Transient(elapsed_ms(start))) }\n",
            "  let policy = Retry.policy(\n",
            "    Schedule.exponential(4, Duration.milliseconds(100), Duration.milliseconds(250)),\n",
            "    transient,\n",
            "  )\n",
            "  attempt |> Retry.run(policy)\n",
            "}\n",
        ),
    );
    assert_eq!(run.outcome, failure("Transient", 550));
    assert_eq!(run.advances_ms, [100, 300, 550]);
}

// The first success ends the retry with its value; no further attempt or
// sleep follows it.
#[test]
fn a_success_after_failures_returns_its_value() {
    let run = run_virtual(
        "recovers",
        concat!(
            "pub fn main() -> Int {\n",
            "  let start = Clock.now()\n",
            "  let attempt = fn() {\n",
            "    let at = elapsed_ms(start)\n",
            "    case at < 2000 {\n",
            "      True -> Failure(JobError.Transient(at))\n",
            "      False -> at\n",
            "    }\n",
            "  }\n",
            "  attempt |> Retry.run(Retry.policy(Schedule.fixed(5, Duration.seconds(1)), transient))\n",
            "}\n",
        ),
    );
    assert_eq!(run.outcome, success("2000"));
    assert_eq!(run.advances_ms, [1000, 2000]);
}

// A failure the predicate rejects fails the run on the attempt that raised
// it, with no sleep, even when earlier failures were retried.
#[test]
fn an_unselected_failure_is_not_retried() {
    let run = run_virtual(
        "rejected",
        concat!(
            "pub fn main() -> Int {\n",
            "  let start = Clock.now()\n",
            "  let attempt = fn() {\n",
            "    let at = elapsed_ms(start)\n",
            "    case at < 1000 {\n",
            "      True -> Failure(JobError.Transient(at))\n",
            "      False -> Failure(JobError.Rejected(at))\n",
            "    }\n",
            "  }\n",
            "  attempt |> Retry.run(Retry.policy(Schedule.fixed(5, Duration.seconds(1)), transient))\n",
            "}\n",
        ),
    );
    assert_eq!(run.outcome, failure("Rejected", 1000));
    assert_eq!(run.advances_ms, [1000]);
}

// A defect passes through the retry even when the policy retries everything.
#[test]
fn a_defect_is_not_retried() {
    let run = run_virtual(
        "defect",
        concat!(
            "pub fn main() -> Int {\n",
            "  let attempt = fn() {\n",
            "    let zero = Duration.to_seconds(Duration.zero)\n",
            "    case 1 / zero {\n      0 -> Failure(JobError.Transient(0))\n      value -> value\n    }\n",
            "  }\n",
            "  attempt |> Retry.run(Retry.policy(Schedule.fixed(5, Duration.seconds(1)), fn(_) { True }))\n",
            "}\n",
        ),
    );
    assert!(run.outcome.contains("\"outcome\":\"defect\""), "{}", run.outcome);
    assert!(run.advances_ms.is_empty(), "{:?}", run.advances_ms);
}

// An overall timeout cancels the retry mid-attempt; the cancellation is not
// retried, so the run times out at the overall limit rather than sleeping on.
#[test]
fn cancellation_is_not_retried() {
    let run = run_virtual(
        "cancelled",
        concat!(
            "pub fn main() -> Int {\n",
            "  let attempt = fn() {\n",
            "    Clock.sleep(Duration.seconds(1))\n",
            "    Failure(JobError.Transient(0))\n",
            "  }\n",
            "  let policy = Retry.policy(Schedule.fixed(5, Duration.seconds(1)), fn(_) { True })\n",
            "  let retried = fn() { attempt |> Retry.run(policy) }\n",
            "  retried |> Task.timeout(Duration.milliseconds(2500))\n",
            "}\n",
        ),
    );
    assert!(
        run.outcome.contains("\"type\":\"Timeout\",\"variant\":\"TimedOut\""),
        "{}",
        run.outcome
    );
    assert_eq!(run.advances_ms, [1000, 2000, 2500]);
}

// A per-attempt timeout is an ordinary failure the predicate can select, so
// each attempt times out and the last `Timeout` is the result.
#[test]
fn per_attempt_timeouts_are_retried_when_selected() {
    let run = run_virtual(
        "per_attempt",
        concat!(
            "fn slow_or_transient(error: JobError | Timeout) -> Bool {\n",
            "  case error {\n",
            "    JobError.Transient(_) -> True\n",
            "    JobError.Rejected(_) -> False\n",
            "    Timeout.TimedOut(_) -> True\n",
            "  }\n",
            "}\n",
            "fn charge() -> Int {\n",
            "  Clock.sleep(Duration.seconds(5))\n",
            "  Failure(JobError.Rejected(0))\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let attempt = fn() { charge |> Task.timeout(Duration.seconds(1)) }\n",
            "  let policy = Retry.policy(Schedule.fixed(3, Duration.milliseconds(500)), slow_or_transient)\n",
            "  let outcome = Result.capture(fn() { attempt |> Retry.run(policy) })\n",
            "  case outcome {\n",
            "    Ok(value) -> value\n",
            "    Err(Timeout.TimedOut(limit)) -> Duration.to_milliseconds(limit)\n",
            "    Err(_) -> -1\n",
            "  }\n",
            "}\n",
            "import std/result as Result\n",
            "import std/result.{Ok, Err}\n",
        ),
    );
    assert_eq!(run.outcome, success("1000"));
    assert_eq!(run.advances_ms, [1000, 1500, 2500, 3000, 4000]);
}

// ----- Typing --------------------------------------------------------------

// `Retry.run` keeps the operation's success, failures, and needs, and adds
// the `clock` need its sleeps use.
#[test]
fn run_keeps_the_operation_contract_and_needs_the_clock() {
    let report = type_project(concat!(
        "interface Store { load: fn() -> Int }\n",
        "fn load() -> Int needs { store: Store } {\n",
        "  case store.load() {\n    0 -> Failure(JobError.Transient(0))\n    found -> found\n  }\n",
        "}\n",
        "fn retried() { load |> Retry.run(Retry.policy(Schedule.immediate(3), transient)) }\n",
    ));
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "retried").as_deref(),
        Some("fn() -> Int errors JobError.Transient needs { clock: Clock, store: Store }")
    );
}

// The predicate must take every failure the operation can raise (`T4346`,
// naming what it misses); one that takes more is fine.
#[test]
fn a_predicate_must_cover_the_operation_failures() {
    let operation = concat!(
        "fn call() -> Int {\n  Failure(JobError.Transient(0))\n}\n",
        "fn attempt() { call |> Task.timeout(Duration.seconds(1)) }\n",
    );
    let narrow = type_project(&format!(
        "{operation}fn retried() {{ attempt |> Retry.run(Retry.policy(Schedule.immediate(2), transient)) }}\n"
    ));
    assert_eq!(codes(&narrow), ["T4346"], "{}", narrow.render_diagnostics());
    let message = narrow.diagnostics()[0].message().to_owned();
    assert!(message.contains("`Timeout.TimedOut`"), "{message}");

    let wide = type_project(&format!(
        "{operation}fn any(error: JobError | Timeout) -> Bool {{ True }}\n\
         const policy = Retry.policy(Schedule.immediate(2), any)\n\
         fn retried() {{ attempt |> Retry.run(policy) }}\n\
         fn plain() {{ call |> Retry.run(policy) }}\n"
    ));
    assert!(wide.is_success(), "{}", wide.render_diagnostics());
}

// A schedule constructor whose attempt count is a literal or constant below 1
// always defects, so the compiler rejects it (`T4347`); a computed count is
// left to the run-time check.
#[test]
fn a_constant_attempt_count_below_one_is_rejected() {
    let report = type_project(concat!(
        "const none = 0\n",
        "fn schedules(count: Int) -> List<Schedule.Schedule> {\n",
        "  [\n",
        "    Schedule.immediate(0),\n",
        "    Schedule.fixed(none, Duration.seconds(1)),\n",
        "    Schedule.exponential(-1, Duration.seconds(1), Duration.seconds(2)),\n",
        "    Schedule.immediate(1),\n",
        "    Schedule.immediate(count),\n",
        "  ]\n",
        "}\n",
    ));
    assert_eq!(codes(&report), ["T4347", "T4347", "T4347"], "{}", report.render_diagnostics());
}
