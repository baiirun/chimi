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
        self.chimi_with_path(arguments, &std::env::var_os("PATH").unwrap())
    }

    fn chimi_with_path(&self, arguments: &[&str], path: &std::ffi::OsStr) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .env("PATH", path)
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
  // Text reaches C with its length, so U+0000 does not end it.
  Result.capture(fn() { BindingProbe.parse("1\u{0}2") })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Invalid(\"not a decimal digit\"))")
}

test "a defect raised in C" {
  Test.capture_defect(fn() { BindingProbe.crash("boom") }) |> Test.assert_equal("boom")
  Test.capture_defect(fn() { BindingProbe.add(9223372036854775807, 1) })
    |> Test.assert_equal("Integer overflow")
}

// C hands back Text with its length, so U+0000 stays in it; bytes that are
// not UTF-8 never become Text, nor a failure's or a defect's message.
test "Text C builds is UTF-8" {
  let not_utf8 = Bytes.append(Bytes.from_text("a"), Bytes.from_int(255, Bytes.IntLayout.U8))
  BindingProbe.decode(Bytes.from_text("a\u{0}é")) |> Test.assert_equal(Some(("a\u{0}é", ["a\u{0}é"])))
  Test.capture_defect(fn() { BindingProbe.decode(not_utf8) })
    |> Test.assert_equal("a C binding built an invalid value")
  Result.capture(fn() { BindingProbe.refuse(Bytes.from_text("no\u{0}pe"), False) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Invalid(\"no\"))")
  Test.capture_defect(fn() { BindingProbe.refuse(Bytes.from_text("halt"), True) })
    |> Test.assert_equal("halt")
  Test.capture_defect(fn() { BindingProbe.refuse(not_utf8, False) })
    |> Test.assert_equal("a C binding failed with a message that is not UTF-8")
  Test.capture_defect(fn() { BindingProbe.refuse(not_utf8, True) })
    |> Test.assert_equal("a C binding defected with a message that is not UTF-8")
}
"#,
    );
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("4 passed, 0 failed"), "{}", run.stdout);
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
  // The new pipe takes the closed one's slot and descriptors, so a second
  // release here would close the new pipe's descriptors.
  let reused = BindingProbe.open()
  BindingProbe.close(pipe)
  Result.capture(fn() { BindingProbe.read(pipe) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Closed)")
  Result.capture(fn() { BindingProbe.write(pipe, Bytes.from_text("x")) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Closed)")
  BindingProbe.write(reused, Bytes.from_text("open"))
  BindingProbe.read(reused) |> Test.assert_equal(Some(Bytes.from_text("open")))
}

test "closing a handle ends another task's wait on it" {
  let pipe = BindingProbe.open()
  let reader = Task.start(fn() { Result.capture(fn() { BindingProbe.read(pipe) }) })
  Clock.sleep(Duration.milliseconds(20))
  BindingProbe.close(pipe)
  Task.result(reader) |> Debug.render |> Test.assert_equal("Result.Err(ProbeError.Closed)")
}

test "closing a handle ends another task's wait even when its descriptors are reused" {
  let pipe = BindingProbe.open()
  let reader = Task.start(fn() { Result.capture(fn() { BindingProbe.read(pipe) }) })
  Clock.sleep(Duration.milliseconds(20))
  BindingProbe.close(pipe)
  // The new pipe gets the closed one's descriptor numbers at once.
  let reused = BindingProbe.open()
  Task.timeout(fn() { Task.result(reader) }, Duration.seconds(5))
    |> Debug.render
    |> Test.assert_equal("Result.Err(ProbeError.Closed)")
  BindingProbe.close(reused)
}
"#,
    );
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("5 passed, 0 failed"), "{}", run.stdout);
}

/// A forced process body parked in a binding call lets the call's handles
/// go even when the binding issues one more request on its way out, which
/// abandons the body's fiber: the handle stays usable by other calls. Were
/// the call still running on the handle, the next call on it would wait
/// forever, so the run has a deadline.
#[test]
fn a_forced_body_parked_in_a_binding_call_leaves_its_handle_to_other_calls() {
    let source = format!(
        "{IMPORTS}import std/process as Process
import std/process.{{Inbox}}
import std/supervisor as Supervisor
{}",
        r#"
enum Message {
  Ping
}

test "a forced reader leaves the pipe usable" {
  let pipe = BindingProbe.open()
  let root = Supervisor.start(Supervisor.policy([], 1, Supervisor.restarts(0, Duration.seconds(1))))
  let spec = Process.handoff_spec(fn(inbox: Inbox<Message>, held: BindingProbe.Pipe) {
    let _ = Result.capture(fn() { BindingProbe.read(held) })
    Void
  }, 1, Duration.milliseconds(20))
  let _ = Supervisor.start_handoff(root, spec, pipe)
  Clock.sleep(Duration.milliseconds(20))
  Supervisor.stop(root)
  BindingProbe.write(pipe, Bytes.from_text("after"))
  BindingProbe.read(pipe) |> Test.assert_equal(Some(Bytes.from_text("after")))
}
"#
    );
    let scratch = Scratch::new("forced-binding", &source);
    let mut child = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch.path())
        .args(["test", "cases.chimi", "--clock=real"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run chimi");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while child.try_wait().expect("poll chimi").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the run hung: a call still runs on the forced body's handle");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let output = child.wait_with_output().expect("chimi output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed, 0 failed"),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The build report of the test build in `scratch`.
fn build_report(scratch: &Scratch) -> String {
    std::fs::read_to_string(scratch.path().join("target/chimi/cases/test/build.json"))
        .expect("the test build writes build.json")
}

/// The `[...]` list named `field` in a build report's cache object.
fn cache_list(report: &str, field: &str) -> String {
    report
        .split(&format!("\"{field}\":["))
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .to_owned()
}

// Guards: a sanitized build compiles the binding packages it reaches itself,
// with the sanitizers, rather than take the prebuilt (unsanitized) archive.
#[cfg(target_os = "linux")]
#[test]
fn a_sanitized_build_compiles_its_binding_packages() {
    let scratch = Scratch::new(
        "sanitized",
        &suite("test \"adds\" {\n  BindingProbe.add(1, 2) |> Test.assert_equal(3)\n}\n"),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch.path())
        .env("CHIMI_TEST_RUNTIME_SANITIZE", "address,undefined")
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .args(["test", "cases.chimi"])
        .output()
        .expect("run chimi");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = build_report(&scratch);
    assert!(
        !cache_list(&report, "prebuilt").contains("libchimi_bind_probe.a"),
        "{report}"
    );
    assert!(
        cache_list(&report, "misses").contains("libchimi_bind_probe.a"),
        "{report}"
    );
    let archive = scratch.path().join("target/chimi/cases/test/libchimi_bind_probe.a");
    let symbols = Command::new("nm").arg(&archive).output().expect("run nm");
    assert!(
        String::from_utf8_lossy(&symbols.stdout).contains("__asan_"),
        "the probe archive was compiled without AddressSanitizer"
    );
}

#[test]
fn a_program_links_a_binding_package_only_when_it_reaches_it() {
    // A sanitized build (CHIMI_TEST_RUNTIME_SANITIZE) compiles every binding
    // package itself rather than use the prebuilt archives this test checks.
    if std::env::var("CHIMI_TEST_RUNTIME_SANITIZE").is_ok_and(|value| !value.trim().is_empty()) {
        eprintln!("skipped: a sanitized build never uses prebuilt binding packages");
        return;
    }
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
    // build.rs prebuilt the package (its nested header and source, with the
    // package's own flags), so no project compiles it.
    assert!(
        cache_list(&report, "prebuilt").contains("libchimi_bind_probe.a"),
        "precondition: build.rs prebuilt the probe package (no `binding package probe \
         not prebuilt` cargo warning) with today's `cc`; {report}"
    );
    assert!(
        !cache_list(&report, "misses").contains("libchimi_bind_probe.a"),
        "{report}"
    );

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

// With a `cc` unlike the one build.rs used, the prebuilt archive's key no
// longer matches: the project compiles the package once, nested files and
// flags included, and takes it from its cache after that.
#[cfg(unix)]
#[test]
fn a_c_compiler_unlike_the_prebuilding_one_compiles_a_package_per_project() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new(
        "fallback",
        &suite("test \"adds\" {\n  BindingProbe.add(1, 2) |> Test.assert_equal(3)\n}\n"),
    );
    let real_cc = Command::new("sh")
        .args(["-c", "command -v cc"])
        .output()
        .expect("locate cc");
    let real_cc = String::from_utf8(real_cc.stdout).unwrap();
    let bin = scratch.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let wrapper = bin.join("cc");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'updated cc 99'; fi\nexec {} \"$@\"\n",
            real_cc.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = bin.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap());

    let cold = scratch.chimi_with_path(&["test", "cases.chimi"], &path);
    assert_eq!(cold.status, Some(0), "{}{}", cold.stdout, cold.stderr);
    let report = build_report(&scratch);
    assert!(
        cache_list(&report, "misses").contains("libchimi_bind_probe.a"),
        "{report}"
    );
    assert!(
        !cache_list(&report, "prebuilt").contains("libchimi_bind_probe.a"),
        "{report}"
    );
    let warm = scratch.chimi_with_path(&["test", "cases.chimi"], &path);
    assert_eq!(warm.status, Some(0), "{}", warm.stderr);
    let report = build_report(&scratch);
    assert!(
        cache_list(&report, "hits").contains("libchimi_bind_probe.a"),
        "{report}"
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

#[test]
fn a_constant_cannot_call_a_binding() {
    let scratch = Scratch::new(
        "constant",
        &suite(
            "const sum = BindingProbe.add(1, 2)\n\ntest \"adds\" {\n  Test.assert_equal(sum, 3)\n}\n",
        ),
    );
    let run = scratch.chimi(&["test", "cases.chimi"]);
    assert_ne!(run.status, Some(0));
    let output = format!("{}{}", run.stdout, run.stderr);
    assert!(
        output.contains("E5201") && output.contains("a C binding operation"),
        "{output}"
    );
}

#[test]
fn a_handle_is_neither_equatable_nor_ordered() {
    let scratch = Scratch::new(
        "equality",
        &suite(
            "test \"compares\" {\n  Test.assert_equal(BindingProbe.open(), BindingProbe.open())\n}\n",
        ),
    );
    let run = scratch.chimi(&["test", "cases.chimi"]);
    let output = format!("{}{}", run.stdout, run.stderr);
    assert!(
        output.contains("T4300") && output.contains("`Pipe` is not equatable"),
        "{output}"
    );
}
