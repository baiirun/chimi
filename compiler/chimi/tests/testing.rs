//! Conformance for `chimi test` and `std/test` (testing.md, conformance matrix
//! rows Pass and fail, Escaped failure, Assertion in a fake, Capture defect,
//! Test-only from main, Virtual clock,
//! Deadlock, Isolation, Report order, Exit status, and JSON).
//!
//! Each test writes a module to a scratch directory and runs the CLI there, so
//! the report names the file by its relative path.

use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-testing-{name}-{}-{:?}",
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
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

fn chimi_test(name: &str, source: &str, arguments: &[&str]) -> Run {
    let mut command = vec!["test", "cases.chimi"];
    command.extend_from_slice(arguments);
    chimi(name, source, &command)
}

/// Runs `chimi <arguments>` in a scratch directory holding `source` as `cases.chimi`.
fn chimi(name: &str, source: &str, arguments: &[&str]) -> Run {
    let scratch = scratch_directory(name);
    std::fs::write(scratch.join("cases.chimi"), source).unwrap();
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(arguments)
        .output()
        .expect("run chimi");
    std::fs::remove_dir_all(scratch).unwrap();
    Run {
        status: status.code(),
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

/// The same report from both backends, which must agree.
fn both_backends(name: &str, source: &str, arguments: &[&str]) -> Run {
    let native = chimi_test(&format!("{name}-native"), source, arguments);
    let mut interpreter_arguments = arguments.to_vec();
    interpreter_arguments.push("--backend=interpreter");
    let interpreted = chimi_test(&format!("{name}-interpreter"), source, &interpreter_arguments);
    assert_eq!(
        without_wall_time(&native.stdout),
        without_wall_time(&interpreted.stdout),
        "{}",
        native.stderr
    );
    assert_eq!(native.status, interpreted.status);
    native
}

/// `text` with each JSON `wall_ns` value, the one field runs may differ in, as 0.
fn without_wall_time(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("\"wall_ns\":") {
        let digits = start + "\"wall_ns\":".len();
        output.push_str(&rest[..digits]);
        output.push('0');
        rest = rest[digits..].trim_start_matches(|character: char| character.is_ascii_digit());
    }
    output.push_str(rest);
    output
}

const LISTS: &str = concat!(
    "import std/test as Test\n",
    "\n",
    "fn charges() -> List<Int> {\n",
    "  [2500, 2500]\n",
    "}\n",
    "\n",
    "test \"passes\" {\n",
    "  Test.assert_equal(charges(), [2500, 2500])\n",
    "}\n",
    "\n",
    "test \"charges once\" {\n",
    "  Test.assert_equal(charges(), [2500])\n",
    "}\n",
);

#[test]
fn a_failed_assertion_reports_its_location_both_values_and_a_diff() {
    let run = both_backends("report", LISTS, &[]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"passes\" ... ok\n",
            "test main \"charges once\" ... FAILED\n",
            "  assertion failed at cases.chimi:12:3\n",
            "  actual:\n",
            "    [\n",
            "      2500,\n",
            "      2500,\n",
            "    ]\n",
            "  expected:\n",
            "    [\n",
            "      2500,\n",
            "    ]\n",
            "  diff:\n",
            "      [\n",
            "        2500,\n",
            "    -   2500,\n",
            "      ]\n",
            "\n",
            "1 passed, 1 failed\n",
        )
    );
}

const KINDS: &str = concat!(
    "import std/debug as Debug\n",
    "import std/test as Test\n",
    "\n",
    "enum ChargeError {\n",
    "  Declined(reason: Text)\n",
    "}\n",
    "\n",
    "interface Payments {\n",
    "  charge: fn(Int) -> Text errors ChargeError\n",
    "}\n",
    "\n",
    "fn checkout(cents: Int) -> Text needs { payments: Payments } {\n",
    "  case cents > 0 {\n",
    "    True -> payments.charge(cents)\n",
    "    False -> Debug.crash(\"checkout: cents must be positive, got ${cents}\")\n",
    "  }\n",
    "}\n",
    "\n",
    "test \"escapes a declined charge\" {\n",
    "  let payments = {\n",
    "    charge: fn(_cents: Int) -> Text { Failure(ChargeError.Declined(\"expired\")) },\n",
    "  }\n",
    "  let receipt = checkout(100) using { payments: payments }\n",
    "  Void\n",
    "}\n",
    "\n",
    "test \"defects\" {\n",
    "  let payments = { charge: fn(_cents: Int) -> Text { \"receipt\" } }\n",
    "  let receipt = checkout(0) using { payments: payments }\n",
    "  Void\n",
    "}\n",
    "\n",
    "test \"runs after a defect\" {\n",
    "  Test.assert_not_equal(1, 2)\n",
    "}\n",
    "\n",
    "test \"asserts inside a fake\" {\n",
    "  let payments = {\n",
    "    charge: fn(_cents: Int) -> Text { Test.fail(\"charge must not be called\") },\n",
    "  }\n",
    "  let receipt = checkout(5) using { payments: payments }\n",
    "  Void\n",
    "}\n",
    "\n",
    "test \"checks a condition\" {\n",
    "  Test.assert(1 + 1 == 3, \"one plus one is three\")\n",
    "}\n",
    "\n",
    "test \"floats compare exactly\" {\n",
    "  Test.assert_equal(0.1 + 0.2, 0.3)\n",
    "}\n",
);

#[test]
fn failures_are_classified_by_kind_and_a_defect_does_not_reach_the_next_test() {
    let run = both_backends("kinds", KINDS, &[]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"escapes a declined charge\" ... FAILED\n",
            "  an expected failure escaped the test:\n",
            "    ChargeError.Declined(\n",
            "      \"expired\",\n",
            "    )\n",
            "test main \"defects\" ... FAILED\n",
            "  defect at cases.chimi:15:14:\n",
            "    checkout: cents must be positive, got 0\n",
            "test main \"runs after a defect\" ... ok\n",
            "test main \"asserts inside a fake\" ... FAILED\n",
            "  assertion failed at cases.chimi:39:39\n",
            "  charge must not be called\n",
            "test main \"checks a condition\" ... FAILED\n",
            "  assertion failed at cases.chimi:46:3\n",
            "  one plus one is three\n",
            "test main \"floats compare exactly\" ... FAILED\n",
            "  assertion failed at cases.chimi:50:3\n",
            "  actual:\n",
            "    0.30000000000000004\n",
            "  expected:\n",
            "    0.3\n",
            "\n",
            "1 passed, 5 failed\n",
        )
    );
}

#[test]
fn json_records_one_test_per_line_then_the_run() {
    let run = both_backends("json", LISTS, &["--json"]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    let lines = run.stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert!(lines.iter().all(|line| line.starts_with("{\"schema\":\"chimi.test.v1\",")));
    assert!(lines[0].contains(
        "\"event\":\"test_finished\",\"module\":\"main\",\"name\":\"passes\",\"line\":7,\"outcome\":\"passed\""
    ));
    assert!(lines[1].contains(concat!(
        "\"name\":\"charges once\",\"line\":11,\"outcome\":\"failed\",\"kind\":\"assertion\",",
        "\"message\":\"Test.assert_equal: the values differ\",",
        "\"location\":{\"path\":\"cases.chimi\",\"line\":12,\"column\":3},",
        "\"actual\":\"[\\n  2500,\\n  2500,\\n]\",\"expected\":\"[\\n  2500,\\n]\",\"output\":[]"
    )));
    assert_eq!(
        lines[2],
        "{\"schema\":\"chimi.test.v1\",\"event\":\"run_finished\",\"passed\":1,\"failed\":1}"
    );
}

#[test]
fn reports_keep_suite_order_however_many_tests_run_at_once() {
    let source = (0..12)
        .map(|index| {
            format!("test \"number {index}\" {{\n  Test.assert_equal({index} % 3, 0)\n}}\n\n")
        })
        .collect::<String>();
    let source = format!("import std/test as Test\n\n{source}");
    let one = chimi_test("order-one", &source, &["--jobs=1"]);
    let many = chimi_test("order-many", &source, &["--jobs=8"]);
    assert_eq!(one.status, Some(1), "{}", one.stderr);
    assert_eq!(one.stdout, many.stdout);
    assert!(one.stdout.starts_with("test main \"number 0\" ... ok\ntest main \"number 1\" ... FAILED\n"));
    assert!(one.stdout.ends_with("\n4 passed, 8 failed\n"));
}

#[test]
fn filters_select_tests_by_module_and_name() {
    let run = both_backends("filter", LISTS, &["--filter=main \"pa"]);
    assert_eq!(run.status, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "test main \"passes\" ... ok\n\n1 passed, 0 failed\n");
    // The spec's form is `<module> <name>`, without the quotes reports print.
    let plain = both_backends("filter-plain", LISTS, &["--filter=main pa"]);
    assert_eq!(plain.stdout, run.stdout, "{}", plain.stderr);
}

#[test]
fn the_exit_status_is_two_when_the_run_cannot_start() {
    let unmatched = chimi_test("unmatched", LISTS, &["--filter=nothing"]);
    assert_eq!(unmatched.status, Some(2));
    assert!(unmatched.stderr.contains("no test matches the filter"));
    let option = chimi_test("option", LISTS, &["--jobs=0"]);
    assert_eq!(option.status, Some(2));
    for start in ["garbage", "2026-02-30T00:00:00Z", "2263-01-01T00:00:00Z"] {
        let clock = chimi_test("clock-start", LISTS, &[&format!("--clock-start={start}")]);
        assert_eq!(clock.status, Some(2), "{start}: {}", clock.stdout);
        assert!(clock.stderr.contains("RFC 3339"), "{}", clock.stderr);
    }
    let interpreted = chimi_test(
        "clock-start-interpreter",
        LISTS,
        &["--backend=interpreter", "--clock-start=2026-11-10T09:30:00Z"],
    );
    assert_eq!(interpreted.status, Some(2));
    let broken = chimi_test(
        "broken",
        "import std/test as Test\n\ntest \"typed\" {\n  Test.assert_equal(1, \"one\")\n}\n",
        &[],
    );
    assert_eq!(broken.status, Some(2));
    assert!(broken.stderr.contains("error[T"), "{}", broken.stderr);
    assert!(broken.stdout.is_empty());
    let none = chimi_test("none", "pub fn main() -> Int {\n  0\n}\n", &[]);
    assert_eq!(none.status, Some(2));
    assert!(none.stderr.contains("E5009"), "{}", none.stderr);
}

const TIME: &str = concat!(
    "import std/channel as Channel\n",
    "import std/channel.{Receive}\n",
    "import std/clock as Clock\n",
    "import std/clock.{Clock}\n",
    "import std/duration as Duration\n",
    "import std/io.{Console}\n",
    "import std/test as Test\n",
    "\n",
    "fn wait_twice() -> Void needs { clock: Clock } {\n",
    "  Clock.sleep(Duration.seconds(30))\n",
    "  Clock.sleep(Duration.seconds(30))\n",
    "}\n",
    "\n",
    "fn greet() -> Void needs { console: Console } {\n",
    "  console.write_line(\"hello from a test\")\n",
    "}\n",
    "\n",
    "test \"virtual time passes only by waiting\" {\n",
    "  let start = Clock.now()\n",
    "  wait_twice()\n",
    "  Test.assert_equal(Clock.since(start), Duration.minutes(1))\n",
    "}\n",
    "\n",
    "test \"waits forever\" {\n",
    "  let channel = Channel.bounded(1)\n",
    "  let got = Channel.receive(channel.receiver)\n",
    "  Void\n",
    "}\n",
    "\n",
    "test \"shows its output when it fails\" {\n",
    "  greet()\n",
    "  Test.assert_equal(1, 2)\n",
    "}\n",
    "\n",
    "test \"keeps its output when it passes\" {\n",
    "  greet()\n",
    "}\n",
);

#[test]
fn native_tests_run_on_virtual_time_and_report_deadlocks_and_output() {
    let started = std::time::Instant::now();
    let run = chimi_test("time", TIME, &[]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(50),
        "a minute of sleeps must take no real time"
    );
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    let lines = run.stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "test main \"virtual time passes only by waiting\" ... ok");
    assert_eq!(lines[1], "test main \"waits forever\" ... FAILED");
    assert!(lines[2].starts_with("  deadlock"), "{}", run.stdout);
    assert!(
        run.stdout.contains(concat!(
            "test main \"shows its output when it fails\" ... FAILED\n",
            "  assertion failed at cases.chimi:32:3\n",
            "  actual:\n    1\n  expected:\n    2\n",
            "  output:\n    hello from a test\n",
            "test main \"keeps its output when it passes\" ... ok\n",
            "\n2 passed, 2 failed\n",
        )),
        "{}",
        run.stdout
    );
    let shown = chimi_test("time-shown", TIME, &["--show-output", "--filter=keeps"]);
    assert_eq!(
        shown.stdout,
        concat!(
            "test main \"keeps its output when it passes\" ... ok\n",
            "  output:\n    hello from a test\n",
            "\n1 passed, 0 failed\n",
        )
    );
}

#[test]
fn a_test_past_its_time_limit_fails_without_stopping_the_run() {
    let source = concat!(
        "import std/test as Test\n",
        "\n",
        "test \"spins\" {\n",
        "  let total = loop count = 0 {\n",
        "    case count < 0 {\n",
        "      True -> break count\n",
        "      False -> count + 1\n",
        "    }\n",
        "  }\n",
        "  Test.assert(total > 0, \"spun\")\n",
        "}\n",
        "\n",
        "test \"finishes\" {\n",
        "  Test.assert(True, \"done\")\n",
        "}\n",
    );
    let run = both_backends("limit", source, &["--time-limit=500ms"]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"spins\" ... FAILED\n",
            "  exceeded the time limit\n",
            "test main \"finishes\" ... ok\n",
            "\n1 passed, 1 failed\n",
        )
    );
}

#[test]
fn assert_not_equal_fails_on_equal_values_on_both_backends() {
    let source = concat!(
        "import std/test as Test\n",
        "\n",
        "test \"differs\" {\n",
        "  Test.assert_not_equal([1, 2], [1, 2])\n",
        "}\n",
    );
    let run = both_backends("not-equal", source, &[]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"differs\" ... FAILED\n",
            "  assertion failed at cases.chimi:4:3\n",
            "  Test.assert_not_equal: the values are equal\n",
            "  actual:\n",
            "    [\n",
            "      1,\n",
            "      2,\n",
            "    ]\n",
            "  expected:\n",
            "    [\n",
            "      1,\n",
            "      2,\n",
            "    ]\n",
            "\n",
            "0 passed, 1 failed\n",
        )
    );
}

const CAPTURES: &str = concat!(
    "import std/clock as Clock\n",
    "import std/clock.{Clock}\n",
    "import std/debug as Debug\n",
    "import std/duration as Duration\n",
    "import std/int as Int\n",
    "import std/io.{Console}\n",
    "import std/task as Task\n",
    "import std/test as Test\n",
    "\n",
    "fn checkout(cents: Int) -> Int {\n",
    "  case cents > 0 {\n",
    "    True -> cents\n",
    "    False -> Debug.crash(\"checkout: cents must be positive, got ${cents}\")\n",
    "  }\n",
    "}\n",
    "\n",
    "test \"captures a precondition\" {\n",
    "  Test.capture_defect(fn() { checkout(0) })\n",
    "  |> Test.assert_equal(\"checkout: cents must be positive, got 0\")\n",
    "}\n",
    "\n",
    "test \"a success fails the test\" {\n",
    "  Test.capture_defect(fn() { checkout(5) }) |> Test.assert_equal(\"\")\n",
    "}\n",
    "\n",
    "test \"an expected failure fails the test\" {\n",
    "  Test.capture_defect(fn() { Int.parse(\"x\") }) |> Test.assert_equal(\"\")\n",
    "}\n",
    "\n",
    "test \"an inner assertion is not captured\" {\n",
    "  Test.capture_defect(fn() { Test.assert_equal(1, 2) }) |> Test.assert_equal(\"\")\n",
    "}\n",
);

#[test]
fn capture_defect_returns_the_message_and_fails_on_any_other_outcome() {
    let run = both_backends("capture", CAPTURES, &[]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"captures a precondition\" ... ok\n",
            "test main \"a success fails the test\" ... FAILED\n",
            "  assertion failed at cases.chimi:23:3\n",
            "  Test.capture_defect: the operation returned instead of defecting:\n",
            "  5\n",
            "test main \"an expected failure fails the test\" ... FAILED\n",
            "  assertion failed at cases.chimi:27:3\n",
            "  Test.capture_defect: the operation failed with an expected failure instead of defecting:\n",
            "  ParseError.InvalidDigit\n",
            "test main \"an inner assertion is not captured\" ... FAILED\n",
            "  assertion failed at cases.chimi:31:30\n",
            "  actual:\n",
            "    1\n",
            "  expected:\n",
            "    2\n",
            "\n",
            "1 passed, 3 failed\n",
        )
    );
}

/// Native only, as tasks are: the child would print after 10 virtual seconds
/// if the defect left it running.
#[test]
fn capture_defect_cancels_the_tasks_its_operation_started() {
    let source = format!(
        "{CAPTURES}{}",
        concat!(
            "\n",
            "fn start_then_crash() -> Int needs { clock: Clock, console: Console } {\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Clock.sleep(Duration.seconds(10))\n",
            "    console.write_line(\"the child outlived the defect\")\n",
            "    1\n",
            "  })\n",
            "  Debug.crash(\"crashed with a child running\")\n",
            "}\n",
            "\n",
            "test \"cancels the operation's tasks\" {\n",
            "  let start = Clock.now()\n",
            "  Test.capture_defect(fn() { start_then_crash() })\n",
            "  |> Test.assert_equal(\"crashed with a child running\")\n",
            "  Clock.sleep(Duration.seconds(20))\n",
            "  Test.assert_equal(Clock.since(start), Duration.seconds(20))\n",
            "}\n",
        )
    );
    let run = chimi_test("capture-tasks", &source, &["--filter=tasks", "--show-output"]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.stdout,
        "test main \"cancels the operation's tasks\" ... ok\n\n1 passed, 0 failed\n"
    );
}

/// The helper's call is the last assertion node of the program, which a
/// lookup of the native table must still find.
#[test]
fn an_assertion_in_a_later_helper_is_not_captured_either() {
    let source = concat!(
        "import std/test as Test\n",
        "\n",
        "test \"a lone inner assertion\" {\n",
        "  let message = Test.capture_defect(fn() { check() })\n",
        "  Void\n",
        "}\n",
        "\n",
        "fn check() -> Void {\n",
        "  Test.fail(\"inner\")\n",
        "}\n",
    );
    let run = both_backends("capture-last", source, &[]);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"a lone inner assertion\" ... FAILED\n",
            "  assertion failed at cases.chimi:9:3\n",
            "  inner\n",
            "\n",
            "0 passed, 1 failed\n",
        )
    );
}

/// With no deadline, the scope adds no timer for virtual time to jump to.
#[test]
fn a_deadlock_inside_capture_defect_is_still_a_deadlock() {
    let source = concat!(
        "import std/channel as Channel\n",
        "import std/test as Test\n",
        "\n",
        "test \"waits forever\" {\n",
        "  Test.capture_defect(fn() {\n",
        "    let channel = Channel.bounded(1)\n",
        "    Channel.receive(channel.receiver)\n",
        "  })\n",
        "  |> Test.assert_equal(\"\")\n",
        "}\n",
    );
    let run = chimi_test("capture-deadlock", source, &["--json"]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert!(run.stdout.contains("\"kind\":\"deadlock\""), "{}", run.stdout);
}

const TEST_ONLY: &str = concat!(
    "import std/debug as Debug\n",
    "import std/test as Test\n",
    "\n",
    "fn guarded() -> Text {\n",
    "  Test.capture_defect(fn() { Debug.crash(\"boom\") })\n",
    "}\n",
    "\n",
    "fn helper() -> Text {\n",
    "  guarded()\n",
    "}\n",
    "\n",
    "pub fn main() -> Int {\n",
    "  let message = helper()\n",
    "  0\n",
    "}\n",
    "\n",
    "test \"helpers may use it\" {\n",
    "  guarded() |> Test.assert_equal(\"boom\")\n",
    "}\n",
);

#[test]
fn a_test_only_operation_reachable_from_main_fails_the_build_but_not_the_tests() {
    let expected = concat!(
        "cases.chimi:5:8: error[T4401] execution-setup: `Test.capture_defect` is test-only, ",
        "but the program reaches it: main -> helper -> guarded -> Test.capture_defect; ",
        "call it only from `test` items and the helpers they use\n",
    );
    for command in [&["build", "cases.chimi", "-o", "program"][..], &["run", "cases.chimi"]] {
        let run = chimi("test-only", TEST_ONLY, command);
        assert_eq!(run.status, Some(1), "{command:?}: {}", run.stdout);
        assert_eq!(run.stderr, expected, "{command:?}");
    }
    let tests = both_backends("test-only-tests", TEST_ONLY, &[]);
    assert_eq!(tests.status, Some(0), "{}", tests.stderr);
}
