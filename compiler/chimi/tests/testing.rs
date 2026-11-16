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
    let interpreted = chimi_test(
        &format!("{name}-interpreter"),
        source,
        &interpreter_arguments,
    );
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
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("{\"schema\":\"chimi.test.v1\","))
    );
    assert!(lines[0].contains(
        "\"event\":\"test_finished\",\"module\":\"main\",\"name\":\"passes\",\"line\":7,\"outcome\":\"passed\""
    ));
    assert!(lines[1].contains(concat!(
        "\"name\":\"charges once\",\"line\":11,\"outcome\":\"failed\",\"kind\":\"assertion\",",
        "\"message\":\"Test.assert_equal: the values differ\",",
        "\"location\":{\"path\":\"cases.chimi\",\"line\":12,\"column\":3},",
        "\"actual\":\"[\\n  2500,\\n  2500,\\n]\",\"expected\":\"[\\n  2500,\\n]\",\"output\":[],",
        "\"seed\":0,"
    )));
    assert_eq!(
        lines[2],
        "{\"schema\":\"chimi.test.v1\",\"event\":\"run_finished\",\"passed\":1,\"failed\":1,\"seed\":0}"
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
    assert!(
        one.stdout
            .starts_with("test main \"number 0\" ... ok\ntest main \"number 1\" ... FAILED\n")
    );
    assert!(one.stdout.ends_with("\n4 passed, 8 failed\n"));
}

#[test]
fn filters_select_tests_by_module_and_name() {
    let run = both_backends("filter", LISTS, &["--filter=main \"pa"]);
    assert_eq!(run.status, Some(0), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        "test main \"passes\" ... ok\n\n1 passed, 0 failed\n"
    );
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
        &[
            "--backend=interpreter",
            "--clock-start=2026-11-10T09:30:00Z",
        ],
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
    assert_eq!(
        lines[0],
        "test main \"virtual time passes only by waiting\" ... ok"
    );
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

/// Constant evaluation reports its own stops as defects, so a constant may not
/// capture one (`E5201`, as for any operation that needs the running program).
#[test]
fn a_constant_cannot_capture_a_defect() {
    let source = concat!(
        "import std/debug as Debug\n",
        "import std/test as Test\n",
        "\n",
        "const CAUGHT: Text = Test.capture_defect(fn() { Debug.crash(\"boom\") })\n",
        "\n",
        "test \"reads a constant\" {\n",
        "  Test.assert_equal(CAUGHT, \"boom\")\n",
        "}\n",
    );
    let run = chimi_test("capture-constant", source, &[]);
    assert_eq!(run.status, Some(2));
    assert!(
        run.stderr.contains("error[E5201]") && run.stderr.contains("`Test.capture_defect`"),
        "{}",
        run.stderr
    );
}

/// A native defect message is a C string; both backends capture it up to the
/// first NUL, so a test sees the same value on each.
#[test]
fn a_captured_message_is_the_same_on_both_backends_when_it_holds_a_nul() {
    let source = concat!(
        "import std/debug as Debug\n",
        "import std/test as Test\n",
        "\n",
        "test \"keeps the text before a NUL\" {\n",
        "  Test.capture_defect(fn() { Debug.crash(\"a\\u{0}b\") }) |> Test.assert_equal(\"a\")\n",
        "}\n",
    );
    let run = both_backends("capture-nul", source, &[]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
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

/// A fake `Payments` provider that records each charge and replies from a script.
const RECORDERS: &str = concat!(
    "import std/debug as Debug\n",
    "import std/int as Int\n",
    "import std/list as List\n",
    "import std/test as Test\n",
    "\n",
    "interface Payments {\n",
    "  charge: fn(Int) -> Text\n",
    "}\n",
    "\n",
    "fn checkout(cents: List<Int>) -> List<Text> needs { payments: Payments } {\n",
    "  List.map(cents, fn(each) { payments.charge(each) })\n",
    "}\n",
    "\n",
    "test \"records each call in order\" {\n",
    "  let calls = Test.recorder()\n",
    "  let payments = {\n",
    "    charge: fn(cents: Int) -> Text {\n",
    "      Test.record(calls, { cents: cents, label: Int.to_text(cents) })\n",
    "      \"ok\"\n",
    "    },\n",
    "  }\n",
    "  let receipts = checkout([5, 7]) using { payments: payments }\n",
    "  let expected = [{ cents: 5, label: \"5\" }, { cents: 7, label: \"7\" }]\n",
    "  Test.assert_equal(Test.recorded(calls), expected)\n",
    "  Test.assert_equal(Test.recorded(calls), expected)\n",
    "  Test.assert_equal(Debug.render(calls), \"<Recorder>\")\n",
    "}\n",
    "\n",
    "test \"replies from a script\" {\n",
    "  let replies = Test.script([\"first\", \"second\"])\n",
    "  let payments = { charge: fn(_cents: Int) -> Text { Test.next(replies) } }\n",
    "  Test.assert_equal(checkout([1, 2]) using { payments: payments }, [\"first\", \"second\"])\n",
    "  Test.assert_equal(Debug.render(replies), \"<Script>\")\n",
    "}\n",
    "\n",
    "test \"an empty recorder records nothing\" {\n",
    "  Test.assert_equal(Test.recorded(Test.recorder()), [])\n",
    "}\n",
    "\n",
    "test \"a call past the script fails the test\" {\n",
    "  let replies = Test.script([\"only\"])\n",
    "  let payments = { charge: fn(_cents: Int) -> Text { Test.next(replies) } }\n",
    "  let receipts = checkout([1, 2]) using { payments: payments }\n",
    "  Void\n",
    "}\n",
);

#[test]
fn recorders_keep_calls_in_order_and_scripts_reply_until_exhausted() {
    let run = both_backends("recorders", RECORDERS, &[]);
    assert_eq!(run.status, Some(1), "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.stdout,
        concat!(
            "test main \"records each call in order\" ... ok\n",
            "test main \"replies from a script\" ... ok\n",
            "test main \"an empty recorder records nothing\" ... ok\n",
            "test main \"a call past the script fails the test\" ... FAILED\n",
            "  assertion failed at cases.chimi:42:54\n",
            "  Test.next: script exhausted after 1 values\n",
            "\n",
            "3 passed, 1 failed\n",
        )
    );
}

/// Native only: the interpreter runs no tasks alongside a test.
#[test]
fn tasks_record_into_one_log_in_the_order_they_ran() {
    let source = concat!(
        "import std/channel as Channel\n",
        "import std/int as Int\n",
        "import std/task as Task\n",
        "import std/test as Test\n",
        "\n",
        "test \"tasks record into one log in the order they ran\" {\n",
        "  let events = Test.recorder()\n",
        "  let channel = Channel.bounded(1)\n",
        "  let sender = channel.sender\n",
        "  let receiver = channel.receiver\n",
        "  Test.record(events, \"before\")\n",
        "  let child = Task.start(fn() -> Int {\n",
        "    Test.record(events, Int.to_text(42))\n",
        "    Channel.send(sender, 1)\n",
        "    1\n",
        "  })\n",
        "  let received = Channel.receive(receiver)\n",
        "  Test.record(events, \"after\")\n",
        "  Test.assert_equal(Task.result(child), 1)\n",
        "  Test.assert_equal(Test.recorded(events), [\"before\", \"42\", \"after\"])\n",
        "}\n",
    );
    let run = chimi_test("recorder-tasks", source, &[]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
}

#[test]
fn a_recorder_or_script_must_hold_shareable_values() {
    let source = concat!(
        "import std/channel as Channel\n",
        "import std/test as Test\n",
        "\n",
        "test \"keeps a receiver\" {\n",
        "  let channel = Channel.bounded(1)\n",
        "  Test.record(Test.recorder(), channel.receiver)\n",
        "  let replies = Test.script([channel.receiver])\n",
        "  Void\n",
        "}\n",
    );
    let run = chimi_test("recorder-shareable", source, &[]);
    assert_eq!(run.status, Some(2), "{}", run.stdout);
    let diagnostics: Vec<&str> = run.stderr.lines().filter(|line| line.contains("T4403")).collect();
    assert_eq!(diagnostics.len(), 2, "{}", run.stderr);
    assert!(diagnostics[0].starts_with("cases.chimi:6:32: error[T4403] type: Test.record keeps"), "{}", run.stderr);
    assert!(diagnostics[1].starts_with("cases.chimi:7:29: error[T4403] type: Test.script keeps"), "{}", run.stderr);
}

#[test]
fn a_recorder_reachable_from_main_fails_the_build() {
    let source = concat!(
        "import std/test as Test\n",
        "\n",
        "pub fn main() -> Int {\n",
        "  Test.record(Test.recorder(), 1)\n",
        "  0\n",
        "}\n",
    );
    let run = chimi("recorder-main", source, &["run", "cases.chimi"]);
    assert_eq!(run.status, Some(1), "{}", run.stdout);
    assert!(
        run.stderr.contains("error[T4401] execution-setup: `Test.record` is test-only"),
        "{}",
        run.stderr
    );
}

/// Fake process addresses: a send to a full mailbox is refused, `received`
/// empties it, and a real process can send to the address.
const MAILBOXES: &str = r#"import std/clock as Clock
import std/clock.{Clock}
import std/debug as Debug
import std/duration as Duration
import std/process as Process
import std/process.{Inbox, SendError}
import std/supervisor as Supervisor
import std/test as Test

enum Message {
  Ping(number: Int)
  Note(text: Text, tags: List<Text>)
}

fn send_all(to: Process.Address<Message>, count: Int) -> Int {
  loop state = (1, 0) {
    let (index, refused) = state
    case index > count {
      True -> break refused
      False -> case Process.send(to, Message.Ping(index)) {
        Success(_) -> (index + 1, refused)
        Failure(error) -> case error {
          SendError.Full(Message.Ping(number)) -> (index + 1, refused + number)
          SendError.Full(_) -> (index + 1, refused + 1000)
          SendError.Unavailable(_) -> (index + 1, refused + 100)
        }
      }
    }
  }
}

fn reporter(to: Process.Address<Message>) -> fn(Inbox<Message>) -> Void {
  fn(inbox: Inbox<Message>) -> Void {
    case Process.send(to, Message.Ping(7)) {
      Success(_) -> Void
      Failure(_) -> Void
    }
  }
}

fn pause() -> Void needs { clock: Clock } {
  Clock.sleep(Duration.milliseconds(10))
}

test "a full mailbox refuses and received frees it" {
  let (address, mailbox) = Test.mailbox(2)
  Test.assert_equal(send_all(address, 3), 3)
  Test.assert_equal(Test.received(mailbox), [Message.Ping(1), Message.Ping(2)])
  Test.assert_equal(Test.received(mailbox), [])
  Test.assert_equal(send_all(address, 2), 0)
  Test.assert_equal(Test.received(mailbox), [Message.Ping(1), Message.Ping(2)])
  let note = Message.Note("held", ["a", "b"])
  let _ = Process.send(address, note)
  Test.assert_equal(Test.received(mailbox), [note])
  Test.assert_equal(Debug.render(address), "<address test>")
  Test.assert_equal(Debug.render(mailbox), "<Mailbox>")
}

test "a process sends to a test address" {
  let (address, mailbox) = Test.mailbox(4)
  let root = Supervisor.start(Supervisor.policy([], 2, Supervisor.restarts(0, Duration.seconds(1))))
  let spec = Process.spec(reporter(address), 1, Duration.seconds(1))
  let _ = Supervisor.start_process(root, spec, Supervisor.Restart.Never)
  pause()
  Test.assert_equal(Test.received(mailbox), [Message.Ping(7)])
}

test "a capacity below one defects" {
  let (_, mailbox) = Test.mailbox(0)
  Test.assert_equal(Test.received(mailbox), [])
}
"#;

/// Native only, as processes are.
#[test]
fn a_test_mailbox_holds_what_is_sent_to_its_address() {
    let run = chimi_test("mailboxes", MAILBOXES, &[]);
    assert_eq!(run.status, Some(1), "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("test main \"a full mailbox refuses and received frees it\" ... ok"), "{}", run.stdout);
    assert!(run.stdout.contains("test main \"a process sends to a test address\" ... ok"), "{}", run.stdout);
    assert!(
        run.stdout.contains("Test.mailbox: the capacity must be at least 1, not 0"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("2 passed, 1 failed"), "{}", run.stdout);
}

#[test]
fn a_test_mailbox_refuses_a_receiver_and_needs_the_native_backend() {
    let source = concat!(
        "import std/channel as Channel\n",
        "import std/channel.{Receiver}\n",
        "import std/process as Process\n",
        "import std/test as Test\n",
        "\n",
        "enum Message {\n",
        "  Hand(replies: List<Receiver<Int>>)\n",
        "}\n",
        "\n",
        "test \"hands over a receiver\" {\n",
        "  let (address, _) = Test.mailbox(1)\n",
        "  let pair = Channel.bounded(1)\n",
        "  case Process.send(address, Message.Hand([pair.receiver])) {\n",
        "    Success(_) -> Void\n",
        "    Failure(_) -> Void\n",
        "  }\n",
        "}\n",
    );
    let run = chimi_test("mailbox-receiver", source, &[]);
    assert_eq!(run.status, Some(1), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(
            "defect at cases.chimi:13:8:\n    Process.send: a test mailbox cannot hold a message with a Receiver or Completion"
        ),
        "{}",
        run.stdout
    );
    let run = chimi_test("mailbox-interpreter", source, &["--backend=interpreter"]);
    assert_eq!(run.status, Some(1), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(
            "defect at cases.chimi:11:22:\n    Test.mailbox requires the native backend, as processes do"
        ),
        "{}",
        run.stdout
    );
}

#[test]
fn a_mailbox_is_not_an_inbox_and_main_cannot_reach_one() {
    let source = concat!(
        "import std/process as Process\n",
        "import std/process.{Inbox}\n",
        "import std/test as Test\n",
        "\n",
        "enum Message {\n",
        "  Ping\n",
        "}\n",
        "\n",
        "fn take(inbox: Inbox<Message>) -> Message {\n",
        "  Process.receive(inbox)\n",
        "}\n",
        "\n",
        "test \"receives from a mailbox\" {\n",
        "  let (_, mailbox) = Test.mailbox(1)\n",
        "  let message = take(mailbox)\n",
        "  Void\n",
        "}\n",
    );
    let run = chimi_test("mailbox-inbox", source, &[]);
    assert_eq!(run.status, Some(2), "{}", run.stdout);
    assert!(
        run.stderr.contains("cases.chimi:15:22: error[T4203] type: call argument has type `Mailbox<_>`, expected `Inbox<Message>`"),
        "{}",
        run.stderr
    );
    let source = concat!(
        "import std/test as Test\n",
        "\n",
        "pub fn main() -> Int {\n",
        "  let (_, mailbox) = Test.mailbox(1)\n",
        "  0\n",
        "}\n",
    );
    let run = chimi("mailbox-main", source, &["run", "cases.chimi"]);
    assert_eq!(run.status, Some(1), "{}", run.stdout);
    assert!(
        run.stderr.contains("error[T4401] execution-setup: `Test.mailbox` is test-only"),
        "{}",
        run.stderr
    );
}

/// `draws` prints one of each draw; its numbers come from the run seed and its
/// identity, so the test before it and a filter must not change them.
const RANDOM: &str = concat!(
    "import std/debug as Debug\n",
    "import std/io.{Console}\n",
    "import std/random as Random\n",
    "import std/test as Test\n",
    "\n",
    "fn show(line: Text) -> Void needs { console: Console } {\n",
    "  console.write_line(line)\n",
    "}\n",
    "\n",
    "test \"draws first\" {\n",
    "  show(Debug.render(Random.int(0..1000)))\n",
    "}\n",
    "\n",
    "test \"draws\" {\n",
    "  show(Debug.render([Random.int(1..=6), Random.int(1..=6), Random.int(-5..5)]))\n",
    "  show(Debug.render(Random.float()))\n",
    "  show(Debug.render(Random.pick([\"a\", \"b\", \"c\"])))\n",
    "  show(Debug.render(Random.shuffle([1, 2, 3, 4, 5])))\n",
    "}\n",
    "\n",
    "test \"fails after a draw\" {\n",
    "  Test.assert_equal(Random.int(1..=6), 0)\n",
    "}\n",
    "\n",
    "test \"fails without a draw\" {\n",
    "  Test.assert_equal(1, 0)\n",
    "}\n",
);

/// The draws of `draws` at run seed 0, which an independent model of
/// xoshiro256** seeded through SplitMix64 reproduces.
const DRAWS_AT_SEED_0: &str = concat!(
    "test main \"draws\" ... ok\n",
    "  output:\n",
    "    [4, 6, 1]\n",
    "    0.2469708770566883\n",
    "    Some(\"a\")\n",
    "    [1, 2, 5, 4, 3]\n",
);

#[test]
fn a_test_draws_the_same_numbers_alone_and_among_others_on_both_backends() {
    let all = both_backends("random-all", RANDOM, &["--show-output"]);
    assert!(all.stdout.contains(DRAWS_AT_SEED_0), "{}", all.stdout);
    let alone = both_backends("random-alone", RANDOM, &["--show-output", "--filter=main \"draws\""]);
    assert_eq!(alone.stdout, format!("{DRAWS_AT_SEED_0}\n1 passed, 0 failed\n"));
    let reseeded = both_backends("random-reseeded", RANDOM, &["--show-output", "--seed=1"]);
    assert!(!reseeded.stdout.contains(DRAWS_AT_SEED_0), "{}", reseeded.stdout);
}

#[test]
fn a_failure_names_the_seed_only_when_the_test_drew_numbers() {
    let run = both_backends("random-seed-report", RANDOM, &["--seed=7", "--filter=fails"]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    let after_draw = run.stdout.split("test main \"fails without a draw\"").collect::<Vec<_>>();
    assert!(
        after_draw[0].contains("  random seed: 7; replay with --seed=7\n"),
        "{}",
        run.stdout
    );
    assert!(!after_draw[1].contains("random seed"), "{}", run.stdout);
    let json = both_backends("random-seed-json", RANDOM, &["--seed=7", "--json"]);
    assert!(json.stdout.lines().all(|line| line.contains("\"seed\":7")), "{}", json.stdout);
}

#[test]
fn a_provided_random_source_replaces_the_seeded_one_and_an_empty_range_defects() {
    let source = concat!(
        "import std/random as Random\n",
        "import std/test as Test\n",
        "\n",
        "test \"uses the provided source\" {\n",
        "  let fixed = { next_int: fn() -> Int { -9223372036854775805 } }\n",
        "  let draws = [Random.int(0..10), Random.int(10..=11)] using { random: fixed }\n",
        "  Test.assert_equal(draws, [3, 11])\n",
        "  let zero = { next_int: fn() -> Int { 0 } }\n",
        "  Test.assert_equal(Random.float() using { random: zero }, 0.0)\n",
        "  Test.assert_equal(Random.pick([]) using { random: zero }, None)\n",
        "  Test.assert_equal(Random.shuffle([1, 2]) using { random: zero }, [2, 1])\n",
        "  let lowest = { next_int: fn() -> Int { 1024 } }\n",
        "  Test.assert_equal(Random.float() using { random: lowest }, 0.00000000000000011102230246251565)\n",
        "}\n",
        "\n",
        "test \"defects on an empty range\" {\n",
        "  let roll = Random.int(3..3)\n",
        "  Void\n",
        "}\n",
    );
    let run = both_backends("random-provided", source, &[]);
    assert_eq!(run.status, Some(1), "{}", run.stderr);
    assert!(run.stdout.contains("test main \"uses the provided source\" ... ok\n"), "{}", run.stdout);
    assert!(run.stdout.contains("    Random.int: the range 3..3 is empty\n"), "{}", run.stdout);
}

#[test]
fn main_gets_a_random_source_only_under_the_name_random() {
    let source = concat!(
        "import std/debug as Debug\n",
        "import std/io.{Console}\n",
        "import std/random as Random\n",
        "\n",
        "pub fn main() -> Int needs { console: Console } {\n",
        "  let roll = Random.int(1..=6)\n",
        "  console.write_line(Debug.render(roll >= 1 and roll <= 6))\n",
        "  0\n",
        "}\n",
    );
    let scratch = scratch_directory("random-main");
    std::fs::write(scratch.join("cases.chimi"), source).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "cases.chimi", "-o", "program"])
        .output()
        .expect("run chimi build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let program = Command::new(scratch.join("program")).output().expect("run the program");
    assert_eq!(String::from_utf8_lossy(&program.stdout), "True\n");
    std::fs::remove_dir_all(scratch).unwrap();
    let renamed = source.replace(
        "pub fn main() -> Int needs { console: Console } {\n",
        "import std/random.{Random}\n\nfn roll() -> Int needs { dice: Random } {\n  dice.next_int()\n}\n\npub fn main() -> Int needs { console: Console } {\n  let rolled = roll()\n",
    );
    let run = chimi("random-main-renamed", &renamed, &["build", "cases.chimi", "-o", "program"]);
    assert_eq!(run.status, Some(1), "{}", run.stdout);
    assert!(run.stderr.contains("error[E5002]"), "{}", run.stderr);
}

/// `chimi test <directory>` runs every module under it, reported in module path
/// order, and skips hidden directories and `target`, where builds write.
#[test]
fn a_directory_input_runs_every_module_under_it() {
    let scratch = scratch_directory("directory-input");
    let write = |path: &str, text: &str| {
        let path = scratch.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "app/shapes.chimi",
        "pub fn area(width: Int, height: Int) -> Int {\n  width * height\n}\n",
    );
    write(
        "app/shapes_test.chimi",
        concat!(
            "import app/shapes as Shapes\n",
            "import std/test as Test\n",
            "\n",
            "test \"a rectangle\" {\n",
            "  Test.assert_equal(Shapes.area(2, 3), 6)\n",
            "}\n",
        ),
    );
    write(
        "lib/text_test.chimi",
        concat!(
            "import std/test as Test\n",
            "\n",
            "test \"adds\" {\n",
            "  Test.assert_equal(1 + 1, 2)\n",
            "}\n",
        ),
    );
    write("aardvark.chimi", "import std/test as Test\n\ntest \"first\" {\n  Test.assert(True, \"holds\")\n}\n");
    write(".hidden/broken.chimi", "this is not chimi\n");
    write("target/broken.chimi", "this is not chimi\n");
    write(".broken.chimi", "this is not chimi\n");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(scratch.join("app"), scratch.join("alias")).unwrap();
        std::os::unix::fs::symlink(scratch.join("aardvark.chimi"), scratch.join("link.chimi")).unwrap();
    }
    for backend in ["--backend=native", "--backend=interpreter"] {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .args(["test", ".", backend])
            .output()
            .expect("run chimi");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{backend}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stdout,
            concat!(
                "test aardvark \"first\" ... ok\n",
                "test app/shapes_test \"a rectangle\" ... ok\n",
                "test lib/text_test \"adds\" ... ok\n",
                "\n",
                "3 passed, 0 failed\n",
            ),
            "{backend}"
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn a_directory_without_modules_or_with_a_bad_name_fails_the_run() {
    let scratch = scratch_directory("directory-errors");
    std::fs::create_dir_all(scratch.join("empty/.hidden")).unwrap();
    std::fs::write(scratch.join("empty/.hidden/a.chimi"), "").unwrap();
    std::fs::create_dir_all(scratch.join("named")).unwrap();
    std::fs::write(scratch.join("named/my-test.chimi"), "").unwrap();
    let run = |directory: &str| {
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .args(["test", directory])
            .output()
            .expect("run chimi")
    };
    let empty = run("empty");
    assert_eq!(empty.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&empty.stderr),
        "chimi: no .chimi modules under empty\n"
    );
    let named = run("named/");
    assert_eq!(named.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&named.stderr).contains("my-test"),
        "{}",
        String::from_utf8_lossy(&named.stderr)
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Doc-comment examples run as tests named after the item they document,
/// under that item's module, and report locations in the doc comment.
const DOCUMENTED: &str = r#"/// Doubles `value`.
///
/// ```chimi
/// import main as Main
/// import std/test as Test
///
/// Test.assert_equal(Main.double(2), 4)
/// ```
pub fn double(value: Int) -> Int {
  value * 2
}

/// Halves `value`, rounding toward zero.
///
/// ```chimi
/// import main as Main
/// import std/test as Test
///
/// Test.assert_equal(Main.half(7), 4)
/// ```
///
/// ```chimi fragment
/// Main.half(
/// ```
///
/// ```text
/// not code
/// ```
pub fn half(value: Int) -> Int {
  value / 2
}

fn secret() -> Int {
  1
}
"#;

#[test]
fn doc_comment_examples_run_as_tests_on_both_backends() {
    for backend in ["--backend=native", "--backend=interpreter"] {
        let run = chimi_test("doc-examples", DOCUMENTED, &[backend]);
        assert_eq!(run.status, Some(1), "{backend}: {}{}", run.stdout, run.stderr);
        assert_eq!(
            run.stdout,
            concat!(
                "test main \"double example 1\" ... ok\n",
                "test main \"half example 1\" ... FAILED\n",
                "  assertion failed at cases.chimi:19:5\n",
                "  actual:\n",
                "    3\n",
                "  expected:\n",
                "    4\n",
                "\n",
                "1 passed, 1 failed\n",
            ),
            "{backend}"
        );
    }
    let run = chimi_test("doc-examples-filter", DOCUMENTED, &["--filter=double example"]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
}

#[test]
fn a_doc_example_sees_only_public_items_and_chimi_check_reads_it() {
    let source = DOCUMENTED.replace(
        "/// Test.assert_equal(Main.double(2), 4)",
        "/// Test.assert_equal(Main.secret(), 1)",
    );
    let expected = "cases.chimi:7:28: error[E_PRIVATE_SYMBOL] name: Main.secret is module-private";
    let run = chimi_test("doc-examples-private", &source, &[]);
    assert_eq!(run.status, Some(2), "{}", run.stdout);
    assert!(run.stderr.contains(expected), "{}", run.stderr);
    let run = chimi("doc-examples-check", &source, &["check", "cases.chimi"]);
    assert_eq!(run.status, Some(1), "{}", run.stdout);
    assert!(run.stderr.contains(expected), "{}", run.stderr);
    // JSON names the documented module and offsets into its own text.
    let start = source.find("secret(), 1)").unwrap();
    let run = chimi("doc-examples-json", &source, &["check", "--json", "cases.chimi"]);
    assert_eq!(run.status, Some(1), "{}", run.stdout);
    assert!(
        run.stdout.contains(&format!(
            "\"primary\":{{\"module\":\"main\",\"path\":\"cases.chimi\",\"start\":{start},"
        )),
        "{}",
        run.stdout
    );
}

const TRACED: &str = r#"import std/test as Test

fn add(a: Int, b: Int) -> Int {
  a + b
}

test "adds" {
  Test.assert_equal(add(1, 2), 3)
}

test "fails" {
  Test.assert_equal(add(1, 2), 4)
}
"#;

/// `--trace` writes each test's telemetry to its own file, named by the test
/// and identical on both backends apart from times; a new traced run clears
/// the last one's files, and `--trace=off` writes none.
#[test]
fn trace_writes_one_telemetry_file_per_test_on_both_backends() {
    let scratch = scratch_directory("trace");
    std::fs::write(scratch.join("cases.chimi"), TRACED).unwrap();
    let traces = scratch.join("target/chimi/cases/test/traces");
    let run = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .arg("test")
            .arg("cases.chimi")
            .args(arguments)
            .output()
            .expect("run chimi");
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    let files = |names: &[&str]| {
        names
            .iter()
            .map(|name| {
                let text = std::fs::read_to_string(traces.join(name))
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                text.lines()
                    .map(|line| {
                        let at = line.find(",\"time_ns\":");
                        at.map_or_else(
                            || line.to_owned(),
                            |at| {
                                let end = line[at + 11..]
                                    .find(|c: char| !c.is_ascii_digit())
                                    .map_or(line.len(), |end| at + 11 + end);
                                format!("{}{}", &line[..at], &line[end..])
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let names = ["0-main-adds.jsonl", "1-main-fails.jsonl"];

    std::fs::create_dir_all(&traces).unwrap();
    std::fs::write(traces.join("stale.jsonl"), "").unwrap();
    let (status, stdout, stderr) = run(&["--trace=full", "--json"]);
    assert_eq!(status, Some(1), "{stdout}{stderr}");
    assert!(!traces.join("stale.jsonl").exists(), "a traced run clears the last one");
    for name in names {
        let path = format!("\"trace\":\"target/chimi/cases/test/traces/{name}\"");
        assert!(stdout.contains(&path), "{path} in {stdout}");
    }
    let native = files(&names);
    for (index, (lines, test)) in native.iter().zip(["adds", "fails"]).enumerate() {
        let first = lines.first().expect("a trace has events");
        let last = lines.last().unwrap();
        let computation = format!("\"computation\":\"test \\\"{test}\\\"\"");
        let execution = format!("\"execution_id\":\"test-{index}\"");
        for line in lines {
            assert!(line.contains(&computation) && line.contains(&execution), "{line}");
        }
        assert!(first.contains("\"event\":\"computation_started\""), "{first}");
        assert!(last.contains("\"event\":\"computation_completed\""), "{last}");
        assert!(lines.iter().any(|line| line.contains("\"event\":\"operation_started\"")));
    }
    assert!(native[0].last().unwrap().contains("\"outcome\":\"success\""));
    assert!(native[1].last().unwrap().contains("\"outcome\":\"defect\""));

    let (status, stdout, stderr) = run(&["--trace=full", "--json", "--backend=interpreter"]);
    assert_eq!(status, Some(1), "{stdout}{stderr}");
    assert_eq!(files(&names), native, "both backends write the same events");

    let (status, stdout, stderr) = run(&["--trace=boundaries"]);
    assert_eq!(status, Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("  trace: target/chimi/cases/test/traces/1-main-fails.jsonl\n"),
        "the report names a failing test's trace: {stdout}"
    );
    assert!(!stdout.contains("0-main-adds.jsonl"), "{stdout}");
    let boundaries = files(&names);
    assert!(boundaries.iter().all(|lines| {
        lines.len() == 2 && !lines.iter().any(|line| line.contains("operation_started"))
    }));

    std::fs::remove_dir_all(&traces).unwrap();
    for arguments in [&["--json"][..], &["--json", "--trace=off"]] {
        let (status, stdout, stderr) = run(arguments);
        assert_eq!(status, Some(1), "{stdout}{stderr}");
        assert!(!stdout.contains("\"trace\""), "{stdout}");
        assert!(!traces.exists(), "--trace=off writes no traces");
    }

    let (status, _, stderr) = run(&["--trace=some"]);
    assert_eq!(status, Some(2));
    assert!(stderr.contains("--trace must be off, boundaries, or full"), "{stderr}");
    std::fs::remove_dir_all(scratch).unwrap();
}
