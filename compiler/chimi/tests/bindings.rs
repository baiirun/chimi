//! Conformance for C bindings (G20, standard-modules-and-constants.md "Native
//! bindings for std and vendored modules"), through the test-only
//! `std/binding_probe` package: values and failures across the boundary, a
//! defect raised in C, handles that wait through the scheduler and stay closed,
//! linking only what a program reaches, and the interpreter's refusal.
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI there.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-bindings-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("cases.chimi"), source).unwrap();
        Self(path)
    }

    fn chimi(&self, arguments: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("run chimi");
        Run {
            status: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

const IMPORTS: &str = "import std/binding_probe as BindingProbe
import std/bytes as Bytes
import std/clock as Clock
import std/debug as Debug
import std/duration as Duration
import std/result as Result
import std/task as Task
import std/test as Test
";

fn suite(tests: &str) -> String {
    format!("{IMPORTS}\n{tests}")
}

/// `chimi test --clock=real`: a descriptor wait holds virtual time, so a test
/// that waits on a pipe and a timer needs the real clock.
fn run_tests(name: &str, tests: &str) -> Run {
    Scratch::new(name, &suite(tests)).chimi(&["test", "cases.chimi", "--clock=real"])
}

#[test]
fn values_failures_and_defects_cross_the_boundary() {
    let run = run_tests(
        "values",
        r#"
test "values" {
  BindingProbe.add(40, 2) |> Test.assert_equal(42)
  BindingProbe.echo(Bytes.from_text("a\u{0}b")) |> Test.assert_equal(Bytes.from_text("a\u{0}b"))
  BindingProbe.echo(Bytes.from_text("")) |> Test.assert_equal(Bytes.from_text(""))
  BindingProbe.parse("-9223372036854775808") |> Test.assert_equal(-9223372036854775808)
}

test "a failure becomes the module's enum" {
  Result.capture(fn() { BindingProbe.parse("12x") })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Invalid(\"not a decimal digit\"))")
  Result.capture(fn() { BindingProbe.parse("9223372036854775808") })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Invalid(\"out of range\"))")
}

test "a defect raised in C" {
  Test.capture_defect(fn() { BindingProbe.crash("boom") }) |> Test.assert_equal("boom")
  Test.capture_defect(fn() { BindingProbe.add(9223372036854775807, 1) })
    |> Test.assert_equal("Integer overflow")
}
"#,
    );
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("3 passed, 0 failed"), "{}", run.stdout);
}

#[test]
fn a_handle_waits_through_the_scheduler_crosses_tasks_and_stays_closed() {
    let run = run_tests(
        "handles",
        r#"
test "a read parks while another task writes" {
  let pipe = BindingProbe.open()
  let reader = Task.start(fn() { BindingProbe.read(pipe) })
  let writer = Task.start(fn() {
    Clock.sleep(Duration.milliseconds(20))
    BindingProbe.write(pipe, Bytes.from_text("ping"))
  })
  Task.result(reader) |> Test.assert_equal(Some(Bytes.from_text("ping")))
  Task.result(writer)
  Debug.render(pipe) |> Test.assert_equal("<Pipe>")
}

test "a timeout cancels a parked read" {
  let pipe = BindingProbe.open()
  Result.capture(fn() { Task.timeout(fn() { BindingProbe.read(pipe) }, Duration.milliseconds(20)) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(Timeout.TimedOut(20ms))")
  BindingProbe.write(pipe, Bytes.from_text("after"))
  BindingProbe.read(pipe) |> Test.assert_equal(Some(Bytes.from_text("after")))
}

test "a closed handle stays closed, and closing again does nothing" {
  let pipe = BindingProbe.open()
  BindingProbe.close(pipe)
  BindingProbe.close(pipe)
  let reused = BindingProbe.open()
  Result.capture(fn() { BindingProbe.read(pipe) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Closed)")
  Result.capture(fn() { BindingProbe.write(pipe, Bytes.from_text("x")) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Closed)")
  BindingProbe.write(reused, Bytes.from_text("open"))
  BindingProbe.read(reused) |> Test.assert_equal(Some(Bytes.from_text("open")))
}
"#,
    );
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("3 passed, 0 failed"), "{}", run.stdout);
}

/// The build report of the test build in `scratch`.
fn build_report(scratch: &Scratch) -> String {
    std::fs::read_to_string(scratch.path().join("target/chimi/cases/test/build.json"))
        .expect("the test build writes build.json")
}

#[test]
fn a_program_links_a_binding_package_only_when_it_reaches_it() {
    let reaching = Scratch::new(
        "reaching",
        &suite("test \"adds\" {\n  BindingProbe.add(1, 2) |> Test.assert_equal(3)\n}\n"),
    );
    let run = reaching.chimi(&["test", "cases.chimi"]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    let report = build_report(&reaching);
    assert!(
        report.contains("\"binding_packages\":[{\"name\":\"probe\""),
        "{report}"
    );
    assert!(report.contains("\"misses\":[") && report.contains("libchimi_bind_probe.a"));

    // A second build takes the package archive from the cache.
    let again = reaching.chimi(&["test", "cases.chimi"]);
    assert_eq!(again.status, Some(0), "{}", again.stderr);
    let report = build_report(&reaching);
    let hits = report
        .split("\"hits\":[")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    assert!(hits.contains("libchimi_bind_probe.a"), "{report}");

    // Importing the module is not reaching it.
    let importing = Scratch::new(
        "importing",
        &suite("test \"adds\" {\n  Test.assert_equal(1 + 2, 3)\n}\n"),
    );
    let run = importing.chimi(&["test", "cases.chimi"]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    let report = build_report(&importing);
    assert!(report.contains("\"binding_packages\":[]"), "{report}");
    assert!(
        !importing
            .path()
            .join("target/chimi/cases/test/libchimi_bind_probe.a")
            .exists()
    );
}

#[test]
fn the_interpreter_refuses_a_binding_call() {
    let scratch = Scratch::new(
        "interpreter",
        &suite("test \"adds\" {\n  BindingProbe.add(1, 2) |> Test.assert_equal(3)\n}\n"),
    );
    let run = scratch.chimi(&["test", "cases.chimi", "--backend=interpreter"]);
    assert_eq!(run.status, Some(1), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout
            .contains("E5204: the interpreter cannot call C bindings"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_program_cannot_reach_the_test_only_probe() {
    let scratch = Scratch::new(
        "main",
        &format!("{IMPORTS}\npub fn main() -> Int {{\n  BindingProbe.add(1, 2)\n}}\n"),
    );
    let run = scratch.chimi(&["build", "cases.chimi"]);
    assert_ne!(run.status, Some(0));
    assert!(
        run.stderr.contains("T4401") && run.stderr.contains("BindingProbe.native_add"),
        "{}",
        run.stderr
    );
}

#[test]
fn an_application_module_cannot_declare_a_binding() {
    let scratch = Scratch::new(
        "application",
        "native fn add(left: Int, right: Int) -> Int\n",
    );
    let run = scratch.chimi(&["check", "cases.chimi"]);
    assert_ne!(run.status, Some(0));
    assert!(
        run.stderr.contains("N3101") || run.stdout.contains("N3101"),
        "{}{}",
        run.stdout,
        run.stderr
    );
}
