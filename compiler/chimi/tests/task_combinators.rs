//! Conformance for `Task.all`, `Task.results`, and `Task.race`
//! (time-and-outcomes.md, "Task combinators", and the conformance matrix row
//! Combinators, plus `T4345`).
//!
//! Every native run uses the virtual clock, so the order in which children
//! finish is fixed by their sleeps, and `Clock.since` shows whether a call
//! waited for a child it should have cancelled.

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::PathBuf;
use std::process::{Command, Output};

const IMPORTS: &str = concat!(
    "import std/cause as Cause\n",
    "import std/cause.{Suppression}\n",
    "import std/clock as Clock\n",
    "import std/debug as Debug\n",
    "import std/duration as Duration\n",
    "import std/list as List\n",
    "import std/result as Result\n",
    "import std/result.{Result}\n",
    "import std/task as Task\n",
    "import std/task.{Timeout}\n",
    "enum JobError {\n  Broken(code: Int)\n}\n",
    "enum OtherError {\n  Worse\n}\n",
    "fn after(seconds: Int, value: Int) -> Int {\n",
    "  Clock.sleep(Duration.seconds(seconds))\n",
    "  value\n",
    "}\n",
    "fn broken_after(seconds: Int, code: Int) -> Int {\n",
    "  Clock.sleep(Duration.seconds(seconds))\n",
    "  Failure(JobError.Broken(code))\n",
    "}\n",
    "fn elapsed(start: Clock.Instant) -> Int { Duration.to_seconds(Clock.since(start)) }\n",
);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-combinators-{name}-{}-{:?}",
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
    outcome: String,
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
        .expect("build native combinator program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome_path = scratch.join(format!("{name}-outcome.jsonl"));
    let process = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--")
        .output()
        .expect("execute native combinator program");
    let outcome = std::fs::read_to_string(outcome_path).unwrap_or_default();
    std::fs::remove_dir_all(scratch).unwrap();
    Run { process, outcome }
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

fn assert_success(run: &Run, value: &str) {
    assert_eq!(
        run.outcome,
        success(value),
        "{}",
        String::from_utf8_lossy(&run.process.stderr)
    );
}

fn type_project(source: &str) -> chimi::TypeReport {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text("main.chimi", format!("{IMPORTS}{source}")),
    )
    .expect("valid module path");
    Compiler::type_project(vec![module])
}

// ----- Typing -----------------------------------------------------------------

// Rejects contracts that drop a thunk's failures or needs, that make
// `Task.results` fallible, or that refuse a list of thunks whose rows differ.
#[test]
fn combinators_join_the_rows_of_their_thunks() {
    let report = type_project(concat!(
        "fn first() -> Int { after(1, 1) }\n",
        "fn second() -> Int { Failure(OtherError.Worse) }\n",
        "fn joined() { Task.all([first, second]) }\n",
        "fn outcomes() { Task.results([first, second]) }\n",
        "fn winner() { [first, second] |> Task.race() }\n",
    ));
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "joined").as_deref(),
        Some("fn() -> List<Int> errors OtherError needs { clock: Clock }")
    );
    assert_eq!(
        report.contract("main", "outcomes").as_deref(),
        Some("fn() -> List<Result<Int, OtherError>> needs { clock: Clock }")
    );
    assert_eq!(
        report.contract("main", "winner").as_deref(),
        Some("fn() -> Int errors OtherError needs { clock: Clock }")
    );
}

// Rejects a `Task.results` that only works when the program imports
// `std/result` itself.
#[test]
fn task_results_needs_no_import_of_std_result() {
    let module = ProjectSource::new(
        "main",
        SourceSnapshot::from_text(
            "main.chimi",
            concat!(
                "import std/task as Task\n",
                "fn outcomes() { Task.results([fn() { 1 }]) }\n",
            ),
        ),
    )
    .expect("valid module path");
    let report = Compiler::type_project(vec![module]);
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "outcomes").as_deref(),
        Some("fn() -> List<Result<Int, Never>>")
    );
}

// Rejects a join that keeps only one thunk's failures: the failure a case
// leaves unmatched must still reach the caller's row.
#[test]
fn a_failure_left_unmatched_after_task_all_reaches_the_callers_row() {
    let report = type_project(concat!(
        "fn first() -> Int { Failure(JobError.Broken(1)) }\n",
        "fn second() -> Int { Failure(OtherError.Worse) }\n",
        "fn handled() -> Int {\n",
        "  case Task.all([first, second]) {\n",
        "    Success(values) -> List.length(values)\n",
        "    Failure(JobError.Broken(code)) -> code\n",
        "  }\n",
        "}\n",
    ));
    assert!(report.is_success(), "{}", report.render_diagnostics());
    assert_eq!(
        report.contract("main", "handled").as_deref(),
        Some("fn() -> Int errors OtherError")
    );
}

// Rejects a missing check, and one that also fires for a non-empty list or a
// list that is only empty at run time.
#[test]
fn an_empty_list_literal_at_task_race_is_t4345() {
    let report = type_project(concat!(
        "fn none() -> Int { Task.race([]) }\n",
        "fn piped() -> Int { [] |> Task.race() }\n",
        "fn some() -> Int { Task.race([fn() { 1 }]) }\n",
        "fn dynamic(operations: List<fn() -> Int>) -> Int { Task.race(operations) }\n",
    ));
    let rendered = report.render_diagnostics();
    assert_eq!(rendered.matches("T4345").count(), 2, "{rendered}");
    assert!(rendered.contains("main.chimi:"), "{rendered}");
}

// Rejects a combinator that starts a thunk holding a parent-local task
// handle, which `Task.start` refuses.
#[test]
fn a_combinator_refuses_thunks_that_capture_parent_local_values() {
    let report = type_project(concat!(
        "fn leaky() -> List<Int> {\n",
        "  let helper = Task.start(fn() { 1 })\n",
        "  Task.all([fn() { 2 }, fn() { Task.result(helper) }])\n",
        "}\n",
    ));
    let rendered = report.render_diagnostics();
    assert!(rendered.contains("T4240"), "{rendered}");
    assert!(
        rendered.contains("Task.all cannot transfer its thunks"),
        "{rendered}"
    );
}

// ----- Task.all ---------------------------------------------------------------

// Rejects results in finishing order, a call that returns before the slowest
// child, and children that run one after another (6 s instead of 3 s).
#[test]
fn task_all_returns_successes_in_input_order_after_running_children_together() {
    let run = run_virtual(
        "all-order",
        concat!(
            "pub fn main() -> List<Int> {\n",
            "  let start = Clock.now()\n",
            "  let values = Task.all([\n",
            "    fn() { after(3, 10) },\n",
            "    fn() { after(1, 20) },\n",
            "    fn() { after(2, 30) },\n",
            "  ])\n",
            "  List.push(values, elapsed(start))\n",
            "}\n",
        ),
    );
    assert_success(&run, "[10,20,30,3]");
}

// Rejects waiting for the slow sibling (time 10), keeping a later failure as
// the primary one, and dropping the sibling that failed at the same instant
// before the cancellation took effect.
#[test]
fn task_all_cancels_siblings_on_the_first_failure_and_keeps_earlier_ones_as_sibling_causes() {
    let run = run_virtual(
        "all-failure",
        concat!(
            "fn work() -> List<Int> {\n",
            "  Task.all([\n",
            "    fn() { after(10, 1) },\n",
            "    fn() { broken_after(2, 7) },\n",
            "    fn() { broken_after(2, 8) },\n",
            "    fn() { broken_after(5, 9) },\n",
            "  ])\n",
            "}\n",
            "pub fn main() -> List<Int> {\n",
            "  let start = Clock.now()\n",
            "  let captured = Result.capture(work)\n",
            "  let code = case captured {\n",
            "    Result.Err(JobError.Broken(code)) -> code\n",
            "    Result.Ok(_) -> 0\n",
            "  }\n",
            "  let siblings = case Cause.of(captured) {\n",
            "    Some(cause) -> Cause.suppressed(cause)\n",
            "    None -> []\n",
            "  }\n",
            "  let sibling = case List.first(siblings) {\n",
            "    Some(cause) -> case Cause.reason(cause) {\n",
            "      Some(Suppression.Sibling) -> case Cause.summary(cause) {\n",
            "        \"JobError.Broken(8)\" -> 1\n",
            "        other -> Debug.crash(other)\n",
            "      }\n",
            "      _ -> -1\n",
            "    }\n",
            "    None -> 0\n",
            "  }\n",
            "  [code, List.length(siblings), sibling, elapsed(start)]\n",
            "}\n",
        ),
    );
    assert_success(&run, "[7,1,1,2]");
}

// Rejects an empty call that starts nothing but still parks or defects.
#[test]
fn task_all_of_an_empty_list_is_an_empty_list() {
    let run = run_virtual(
        "all-empty",
        concat!(
            "pub fn main() -> Int {\n",
            "  let none: List<fn() -> Int> = []\n",
            "  List.length(Task.all(none)) + List.length(Task.results(none))\n",
            "}\n",
        ),
    );
    assert_success(&run, "0");
}

// Rejects a child defect that the caller survives or turns into a failure.
#[test]
fn a_child_defect_unwinds_the_caller_of_task_all() {
    let run = run_virtual(
        "all-defect",
        concat!(
            "fn divide(by: Int) -> Int {\n",
            "  Clock.sleep(Duration.seconds(1))\n",
            "  10 / by\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let values = Task.all([fn() { after(10, 1) }, fn() { divide(0) }])\n",
            "  List.length(values)\n",
            "}\n",
        ),
    );
    assert_eq!(run.process.status.code(), Some(70), "{}", run.outcome);
    assert!(
        run.outcome.contains("\"outcome\":\"defect\""),
        "{}",
        run.outcome
    );
}

// ----- Task.results -----------------------------------------------------------

// Rejects cancelling a sibling on a failure, outcomes in finishing order, and
// an `Err` that loses its cause.
#[test]
fn task_results_waits_for_every_child_and_stores_each_outcome() {
    let run = run_virtual(
        "results",
        concat!(
            "pub fn main() -> List<Int> {\n",
            "  let start = Clock.now()\n",
            "  let outcomes = Task.results([\n",
            "    fn() { after(3, 1) },\n",
            "    fn() { broken_after(1, 2) },\n",
            "    fn() { after(2, 3) },\n",
            "  ])\n",
            "  let expected = [Result.Ok(1), Result.Err(JobError.Broken(2)), Result.Ok(3)]\n",
            "  let same = case outcomes == expected {\n",
            "    True -> 1\n",
            "    False -> 0\n",
            "  }\n",
            "  let caused = case List.get(outcomes, 1) {\n",
            "    Some(stored) -> case Cause.of(stored) {\n",
            "      Some(_) -> 1\n",
            "      None -> 0\n",
            "    }\n",
            "    None -> -1\n",
            "  }\n",
            "  [same, caused, elapsed(start)]\n",
            "}\n",
        ),
    );
    assert_success(&run, "[1,1,3]");
}

// ----- Task.race --------------------------------------------------------------

// Rejects the input order deciding the winner, waiting for the losers to
// finish (time 9), and a loser left running past the call: it would send 9
// on the channel at time 9, before the root's own 0 at time 10.
#[test]
fn task_race_returns_the_first_outcome_and_drains_the_losers() {
    let run = run_virtual(
        "race",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive, Sender}\n",
            "fn report_after(seconds: Int, report: Sender<Int>) -> Int {\n",
            "  Clock.sleep(Duration.seconds(seconds))\n",
            "  Channel.send(report, 9)\n",
            "  1\n",
            "}\n",
            "pub fn main() -> List<Int> {\n",
            "  let reports = Channel.bounded(4)\n",
            "  let report = reports.sender\n",
            "  let start = Clock.now()\n",
            "  let won = Task.race([\n",
            "    fn() { report_after(9, report) },\n",
            "    fn() { after(2, 2) },\n",
            "    fn() { after(5, 3) },\n",
            "  ])\n",
            "  let failed = case Task.race([fn() { after(4, 1) }, fn() { broken_after(1, 6) }]) {\n",
            "    Success(value) -> value\n",
            "    Failure(JobError.Broken(code)) -> code\n",
            "  }\n",
            "  let spent = elapsed(start)\n",
            "  Clock.sleep(Duration.seconds(7))\n",
            "  Channel.send(reports.sender, 0)\n",
            "  let got = Channel.receive(reports.receiver)\n",
            "  let first = case got {\n",
            "    Receive.Received(value) -> value\n",
            "    Receive.Drained -> -1\n",
            "  }\n",
            "  [won, failed, spent, first]\n",
            "}\n",
        ),
    );
    assert_success(&run, "[2,6,3,0]");
}

// Rejects a combinator that treats a child's own cancellation as a lost
// race: a child that ends cancelled cancels the caller, as `Task.result` of
// it would.
#[test]
fn a_child_that_ends_cancelled_cancels_the_caller_of_task_race() {
    let run = run_virtual(
        "race-self-cancel",
        concat!(
            "fn cancel_own() -> Int {\n",
            "  let inner = Task.start(fn() { after(5, 1) })\n",
            "  Task.cancel(inner)\n",
            "  Task.result(inner)\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  Task.race([cancel_own, fn() { after(3, 2) }])\n",
            "}\n",
        ),
    );
    assert_eq!(run.process.status.code(), Some(130), "{}", run.outcome);
}

// Rejects a cancelled combinator that cancels its children with scheduler
// requests on its way out (CHI-265): the scheduler already ends them, and the
// park left the cancelled task unable to finish, so `Task.result` of it ended
// the program in the deadlock defect instead of the cancellation.
#[test]
fn task_cancel_of_a_task_waiting_in_a_combinator_ends_it_cancelled() {
    for combinator in ["Task.race", "Task.all"] {
        let run = run_virtual(
            "cancel-combinator",
            &format!(
                concat!(
                    "pub fn main() -> Int {{\n",
                    "  let child = Task.start(fn() -> Int {{ case {combinator}([fn() {{ after(1000, 1) }}, fn() {{ after(1000, 2) }}]) {{ _ -> 1 }} }})\n",
                    "  Clock.sleep(Duration.milliseconds(1))\n",
                    "  Task.cancel(child)\n",
                    "  Task.result(child)\n",
                    "}}\n",
                ),
                combinator = combinator
            ),
        );
        assert_eq!(run.process.status.code(), Some(130), "{combinator}: {}", run.outcome);
        assert!(run.outcome.contains("\"outcome\":\"cancellation\""), "{combinator}: {}", run.outcome);
    }
}

// ----- Processes --------------------------------------------------------------

// A stop drains before it cancels (application-lifecycle.md): the body keeps
// working on the message it already took. Rejects a combinator that the stop
// cancels at once (time 0) and one that stays parked until the body is forced
// at `shutdown_within` (time 30); the child finishes at time 5 and the body
// then ends at its next receive.
#[test]
fn a_stopped_process_body_finishes_its_task_all_before_it_ends() {
    let run = run_virtual(
        "process-drain",
        concat!(
            "import std/process as Process\n",
            "import std/process.{Inbox}\n",
            "import std/supervisor as Supervisor\n",
            "pub enum Message {\n  Job(n: Int)\n}\n",
            "fn body(inbox: Inbox<Message>) -> Void {\n",
            "  loop handled = 0 {\n",
            "    case Process.receive(inbox) {\n",
            "      Message.Job(n) -> case n == 2 {\n",
            "        True -> handled + List.length(Task.all([fn() { after(5, 1) }]))\n",
            "        False -> handled + 1\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int needs { clock: Clock.Clock } {\n",
            "  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(1, Duration.seconds(300))))\n",
            "  let managed = Supervisor.start_process(root, Process.spec(body, 4, Duration.seconds(30)), Supervisor.Restart.OnDefect)\n",
            "  let address = Supervisor.address(managed)\n",
            "  Clock.sleep(Duration.milliseconds(1))\n",
            "  let start = Clock.now()\n",
            "  Process.send(address, Message.Job(1))\n",
            "  Process.send(address, Message.Job(2))\n",
            "  Supervisor.remove_process(root, managed)\n",
            "  let spent = elapsed(start)\n",
            "  Supervisor.stop(root)\n",
            "  spent\n",
            "}\n",
        ),
    );
    assert_success(&run, "5");
}

// Rejects an empty race that parks forever, returns a value, or fails.
#[test]
fn task_race_of_an_empty_list_at_run_time_defects() {
    let run = run_virtual(
        "race-empty",
        concat!(
            "pub fn main() -> Int {\n",
            "  let none: List<fn() -> Int> = []\n",
            "  Task.race(none)\n",
            "}\n",
        ),
    );
    assert_eq!(run.process.status.code(), Some(70), "{}", run.outcome);
    assert!(
        run.outcome.contains("\"outcome\":\"defect\""),
        "{}",
        run.outcome
    );
}

// ----- Scopes -----------------------------------------------------------------

// Rejects a combinator that a timeout cannot interrupt (time 10), one that
// turns the scope's cancellation into a child failure, and children left
// running after the scope.
#[test]
fn a_timeout_interrupts_a_waiting_combinator_and_drains_its_children() {
    let run = run_virtual(
        "timeout",
        concat!(
            "fn slow() -> List<Int> { Task.all([fn() { after(10, 1) }, fn() { after(10, 2) }]) }\n",
            "pub fn main() -> List<Int> {\n",
            "  let start = Clock.now()\n",
            "  let timed = case slow |> Task.timeout(Duration.seconds(1)) {\n",
            "    Success(values) -> List.length(values)\n",
            "    Failure(Timeout.TimedOut(_)) -> -1\n",
            "  }\n",
            "  let first = elapsed(start)\n",
            "  Clock.sleep(Duration.seconds(20))\n",
            "  [timed, first, elapsed(start)]\n",
            "}\n",
        ),
    );
    assert_success(&run, "[-1,1,21]");
}
