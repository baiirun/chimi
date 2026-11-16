//! Conformance for `std/exec` part 2 (CHI-250): starting real programs and
//! waiting for them, the failures a command that cannot start reports, a child
//! stopped by `kill`, `terminate`, a cancelled wait or the end of its
//! `Exec.start`, fakes on both backends, and who may start programs (`main` by
//! declaring the need, a test only through `Exec.host_for_tests`).
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI there.
//! Tests that run real programs use `--clock=real`, except the one that shows
//! a child wait holds virtual time.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-exec-{name}-{}-{:?}",
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
        self.chimi_with(arguments, &[])
    }

    /// Runs the CLI with `variables` set over the inherited environment.
    fn chimi_with(&self, arguments: &[&str], variables: &[(&str, String)]) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_chimi"));
        command.current_dir(&self.0).args(arguments);
        for (name, value) in variables {
            command.env(name, value);
        }
        let output = command.output().expect("run chimi");
        Run {
            status: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
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

const IMPORTS: &str = r#"import std/clock as Clock
import std/debug as Debug
import std/duration as Duration
import std/exec as Exec
import std/exec.{Commands, ExecError, Status, Stdio}
import std/map as Map
import std/path as Path
import std/result as Result
import std/result.{Err, Ok}
import std/task as Task
import std/test as Test

fn attempt<T, E, R>(operation: fn() -> T errors E needs R) -> Text needs R {
  case Result.capture(operation) {
    Ok(value) -> "Ok(${Debug.render(value)})"
    Err(error) -> "Err(${Debug.render(error)})"
  }
}

fn sh(script: Text) -> Status needs { commands: Commands } {
  Exec.run(Exec.command("/bin/sh", ["-c", script]))
}
"#;

fn suite(tests: &str) -> String {
    format!("{IMPORTS}\n{tests}")
}

fn run_tests(name: &str, tests: &str) -> Run {
    Scratch::new(name, &suite(tests)).chimi(&["test", "cases.chimi", "--clock=real"])
}

fn assert_passed(run: &Run, count: usize) {
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(&format!("{count} passed, 0 failed")),
        "{}",
        run.stdout
    );
}

/// True while `pid` names a process, a zombie included.
fn exists(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run kill")
        .success()
}

// Rejects a status that loses the exit code or the signal, a nonzero exit
// treated as a failure, and arguments that pass through a shell: an argument
// with spaces and quotes reaches the program as one word.
#[test]
fn a_child_reports_how_it_ended() {
    let run = run_tests(
        "status",
        r#"
test "exit codes and signals" {
  let codes = (sh("exit 0"), sh("exit 3"), sh("kill -TERM $$")) using { commands: Exec.host_for_tests() }
  Debug.render(codes) |> Test.assert_equal("(Status.Exited(0), Status.Exited(3), Status.Signaled(15))")
  Exec.succeeded(Status.Exited(0)) |> Test.assert_equal(True)
  Exec.succeeded(Status.Exited(1)) |> Test.assert_equal(False)
  Exec.succeeded(Status.Signaled(9)) |> Test.assert_equal(False)
}

test "arguments reach the program as given" {
  let command = Exec.command("/bin/sh", ["-c", "test $# = 1 && test \"$1\" = 'a \"b\" c'", "sh", "a \"b\" c"])
  let status = Exec.run(command) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(0))
}

test "a second wait answers the same status" {
  let both = Exec.start(Exec.command("/bin/sh", ["-c", "exit 7"]), fn(child) {
    (Exec.wait(child), Exec.wait(child))
  }) using { commands: Exec.host_for_tests() }
  both |> Test.assert_equal((Status.Exited(7), Status.Exited(7)))
}
"#,
    );
    assert_passed(&run, 3);
}

// Rejects a lookup that ignores PATH or the parent's PATH, a failure that
// names the wrong program, a missing directory reported only by the child, an
// environment that is not replaced or not extended (or holds a variable
// twice), a relative program looked for in the child's directory, a file the
// kernel cannot run reported as a permission, and `Stdio.Null` that leaves a
// stream connected. A directory with no search permission is also invalid,
// but tests run as root in CI, where `access` grants it, so none covers it.
#[test]
fn a_command_that_cannot_start_fails_with_its_reason() {
    let run = run_tests(
        "failures",
        r#"
test "not found" {
  let got1 = attempt(fn() { Exec.run(Exec.command("no-such-program-chimi", [])) }) using { commands: Exec.host_for_tests() }
  got1 |> Test.assert_equal("Err(ExecError.NotFound(\"no-such-program-chimi\"))")
  let got2 = attempt(fn() { Exec.run(Exec.command("/no/such/program", [])) }) using { commands: Exec.host_for_tests() }
  got2 |> Test.assert_equal("Err(ExecError.NotFound(\"/no/such/program\"))")
  let got3 = attempt(fn() { Exec.run(Exec.command("", [])) }) using { commands: Exec.host_for_tests() }
  got3 |> Test.assert_equal("Err(ExecError.NotFound(\"\"))")
}

test "not executable" {
  let got1 = attempt(fn() { Exec.run(Exec.command("/etc/passwd", [])) }) using { commands: Exec.host_for_tests() }
  got1 |> Test.assert_equal("Err(ExecError.PermissionDenied(\"/etc/passwd\"))")
  let got2 = attempt(fn() { Exec.run(Exec.command("/tmp", [])) }) using { commands: Exec.host_for_tests() }
  got2 |> Test.assert_equal("Err(ExecError.PermissionDenied(\"/tmp\"))")
}

test "a bad directory" {
  let command = Exec.directory(Exec.command("/bin/sh", ["-c", "exit 0"]), Path.from("/no/such/directory"))
  let got = attempt(fn() { Exec.run(command) }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal("Err(ExecError.InvalidDirectory(\"/no/such/directory\"))")
}

test "a name is found on PATH" {
  let status = Exec.run(Exec.command("sh", ["-c", "exit 4"])) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(4))
}

test "variables are added to the inherited environment" {
  let command = Exec.variable(Exec.variable(Exec.command("/bin/sh", ["-c", "test \"$X\" = second && test \"$(env | grep -c '^X=')\" = 1 && test -n \"$PATH\""]), "X", "first"), "X", "second")
  let status = Exec.run(command) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(0))
}

test "a replaced environment holds only its variables" {
  let command = Exec.variable(Exec.environment(Exec.command("/bin/sh", ["-c", "test -z \"$HOME\" && test \"$A\" = 1 && test \"$B\" = 2"]), Map.from([("A", "1")])), "B", "2")
  let status = Exec.run(command) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(0))
}

test "a working directory" {
  let command = Exec.directory(Exec.command("/bin/sh", ["-c", "test \"$(pwd -P)\" = \"$(cd / && pwd -P)\""]), Path.from("/"))
  let status = Exec.run(command) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(0))
}

// A test's own standard input is already /dev/null, so only the output
// streams can tell `Stdio.Null` from `Stdio.Inherit` here; stdin goes through
// the same code.
test "standard streams to nothing" {
  let script = "test -c /dev/fd/0 && test -c /dev/fd/1 && ! test -c /dev/fd/2"
  let nothing = Exec.stdin(Exec.stdout(Exec.command("/bin/sh", ["-c", script]), Stdio.Null), Stdio.Null)
  let inherited = Exec.command("/bin/sh", ["-c", "! test -c /dev/fd/1"])
  let statuses = (Exec.run(nothing), Exec.run(inherited)) using { commands: Exec.host_for_tests() }
  statuses |> Test.assert_equal((Status.Exited(0), Status.Exited(0)))
}

test "a relative program is found from the parent's directory" {
  let made = sh("mkdir -p tools && printf '#!/bin/sh\\nexit 5\\n' > tools/five && chmod +x tools/five") using { commands: Exec.host_for_tests() }
  made |> Test.assert_equal(Status.Exited(0))
  let command = Exec.directory(Exec.command("tools/five", []), Path.from("/"))
  let status = Exec.run(command) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(5))
}

test "a file the kernel cannot run" {
  let made = sh("printf 'not a program' > junk && chmod +x junk") using { commands: Exec.host_for_tests() }
  made |> Test.assert_equal(Status.Exited(0))
  let got = Result.capture(fn() { Exec.run(Exec.command("./junk", [])) }) using { commands: Exec.host_for_tests() }
  let reason = case got {
    Err(ExecError.Failed(program, _)) -> "Failed(${program})"
    other -> Debug.render(other)
  }
  reason |> Test.assert_equal("Failed(./junk)")
}
"#,
    );
    assert_passed(&run, 10);
}

// Rejects a kill or terminate that is not sent, one sent after the child was
// reaped, and a child that outlives its `Exec.start`: the child writes its
// pid, and once the test ends that process no longer exists (not even as a
// zombie, which `kill(pid, 0)` would still see).
#[test]
fn a_child_is_stopped_and_never_outlives_its_start() {
    let scratch = Scratch::new(
        "stopping",
        &suite(
            r#"
test "kill" {
  let status = Exec.start(Exec.command("/bin/sh", ["-c", "exec sleep 30"]), fn(child) {
    Exec.kill(child)
    Exec.wait(child)
  }) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Signaled(9))
}

test "terminate" {
  let status = Exec.start(Exec.command("/bin/sh", ["-c", "exec sleep 30"]), fn(child) {
    Exec.terminate(child)
    let ended = Exec.wait(child)
    // Both are no-ops once the child has ended.
    Exec.terminate(child)
    Exec.kill(child)
    ended
  }) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Signaled(15))
}

test "a child left running when start ends is killed and reaped" {
  Exec.start(Exec.command("/bin/sh", ["-c", "echo $$ > left.pid; exec sleep 30"]), fn(_child) {
    // Wait until the child wrote its pid.
    loop tries = 0 {
      let written = Exec.run(Exec.command("/bin/sh", ["-c", "test -s left.pid"]))
      case written {
        Status.Exited(0) -> break Void
        _ -> {
          Clock.sleep(Duration.milliseconds(10))
          tries + 1
        }
      }
    }
  }) using { commands: Exec.host_for_tests() }
  // Gone, not even a zombie, as soon as start returns.
  let gone = sh("! kill -0 \"$(cat left.pid)\" 2>/dev/null") using { commands: Exec.host_for_tests() }
  gone |> Test.assert_equal(Status.Exited(0))
}

test "two tasks wait for one child" {
  let both = Exec.start(Exec.command("/bin/sh", ["-c", "sleep 0.2; exit 6"]), fn(child) {
    let first = Task.start(fn() { Exec.wait(child) })
    let second = Task.start(fn() { Exec.wait(child) })
    (Task.result(first), Task.result(second))
  }) using { commands: Exec.host_for_tests() }
  both |> Test.assert_equal((Status.Exited(6), Status.Exited(6)))
}

test "a cancelled wait kills and reaps the child" {
  let got = attempt(fn() {
    Exec.start(Exec.command("/bin/sh", ["-c", "echo $$ > cancelled.pid; exec sleep 30"]), fn(child) {
      Task.timeout(fn() { Exec.wait(child) }, Duration.milliseconds(300))
    })
  }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal("Err(Timeout.TimedOut(300ms))")
}

test "a wait after release is Closed" {
  let child = Exec.start(Exec.command("/bin/sh", ["-c", "exec sleep 30"]), fn(child) { child }) using { commands: Exec.host_for_tests() }
  attempt(fn() { Exec.wait(child) }) |> Test.assert_equal("Err(ExecError.Closed(\"/bin/sh\"))")
}
"#,
        ),
    );
    let started = Instant::now();
    let run = scratch.chimi(&["test", "cases.chimi", "--clock=real"]);
    assert_passed(&run, 6);
    assert!(started.elapsed() < Duration::from_secs(20), "a sleep 30 child was waited out");
    for file in ["left.pid", "cancelled.pid"] {
        let text = std::fs::read_to_string(scratch.0.join(file)).expect(file);
        let pid: i32 = text.trim().parse().expect("a pid");
        assert!(!exists(pid), "{file}: process {pid} outlived its start");
    }
}

// Rejects a child wait that lets virtual time run past a pending timer: the
// sleeper's one-second timer has not fired when the child's exit is seen, so
// the clock still reads its start.
#[test]
fn a_child_wait_holds_virtual_time() {
    let run = Scratch::new(
        "virtual",
        &suite(
            r#"
test "virtual time waits for the child" {
  let report = Exec.start(Exec.command("/bin/sh", ["-c", "exec sleep 0.2"]), fn(child) {
    let sleeper = Task.start(fn() { Clock.sleep(Duration.seconds(1)) })
    let start = Clock.now()
    let status = Exec.wait(child)
    let waited = Clock.since(start)
    Task.result(sleeper)
    (status, waited)
  }) using { commands: Exec.host_for_tests() }
  report |> Test.assert_equal((Status.Exited(0), Duration.seconds(0)))
}
"#,
        ),
    )
    .chimi(&["test", "cases.chimi", "--clock=virtual"]);
    assert_passed(&run, 1);
}

// Rejects fakes that need the native runtime: a fake `Commands` built from
// `Exec.child` runs under the interpreter too, and the command it receives
// names the program and arguments the caller built.
#[test]
fn a_fake_stands_in_for_real_programs_on_both_backends() {
    let source = suite(
        r#"
fn deploy() -> Status needs { commands: Commands } {
  Exec.run(Exec.command("kubectl", ["apply", "-f", "app.yaml"]))
}

test "a fake" {
  let fake = {
    start: fn(command) {
      let code = case Exec.program(command) == "kubectl" and Exec.arguments(command) == ["apply", "-f", "app.yaml"] {
        True -> 0
        False -> 1
      }
      Exec.child(fn() { Status.Exited(code) }, fn() { Void }, fn() { Void }, fn() { Void })
    },
  }
  let status = deploy() using { commands: fake }
  status |> Test.assert_equal(Status.Exited(0))
}

test "a fake that fails to start" {
  let fake = { start: fn(command) { Failure(ExecError.NotFound(Exec.program(command))) } }
  let got = attempt(fn() { deploy() }) using { commands: fake }
  got |> Test.assert_equal("Err(ExecError.NotFound(\"kubectl\"))")
}
"#,
    );
    for backend in [&["test", "cases.chimi"][..], &["test", "cases.chimi", "--backend=interpreter"][..]] {
        let run = Scratch::new("fake", &source).chimi(backend);
        assert_passed(&run, 2);
    }
}

// Rejects a runner that does not close `main`'s `commands` need, and a back
// door: `Exec.host_for_tests` is test-only, and a test that does not supply
// `commands` is refused before it runs.
#[test]
fn main_gets_real_programs_by_declaring_the_need_and_nothing_else_does() {
    let scratch = Scratch::new(
        "main",
        &format!(
            "{IMPORTS}\npub fn main() -> Void needs {{ commands: Commands }} {{\n  let ended = sh(\"exit 0\")\n  case ended {{\n    Status.Exited(0) -> Void\n    other -> Debug.crash(\"the child ended with ${{Debug.render(other)}}\")\n  }}\n}}\n"
        ),
    );
    let built = scratch.chimi(&["build", "cases.chimi", "-o", "runner"]);
    assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
    let output = Command::new(scratch.0.join("runner")).output().expect("run the program");
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));

    let from_main = Scratch::new(
        "from-main",
        &format!("{IMPORTS}\npub fn main() -> Status {{\n  sh(\"exit 0\") using {{ commands: Exec.host_for_tests() }}\n}}\n"),
    )
    .chimi(&["build", "cases.chimi"]);
    assert_ne!(from_main.status, Some(0));
    assert!(from_main.stderr.contains("T4401"), "{}", from_main.stderr);

    let unsupplied = run_tests("unsupplied", "test \"reaches out\" {\n  sh(\"exit 0\") |> Test.assert_equal(Status.Exited(0))\n}\n");
    assert_ne!(unsupplied.status, Some(0));
    assert!(unsupplied.stderr.contains("T4400"), "{}", unsupplied.stderr);
}

// Rejects a real provider that pretends to work under the interpreter: a
// binding call there is a defect that says so.
#[test]
fn the_interpreter_cannot_start_real_programs() {
    let run = Scratch::new(
        "interpreter",
        &suite("test \"real\" {\n  let status = sh(\"exit 0\") using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(0))\n}\n"),
    )
    .chimi(&["test", "cases.chimi", "--backend=interpreter"]);
    assert_ne!(run.status, Some(0));
    assert!(run.stdout.contains("E5204"), "{}{}", run.stdout, run.stderr);
}

// Rejects a child whose signal actions are reset: a program started under
// `nohup`-style SIG_IGN for SIGHUP passes that on, as a shell would, so a
// child that sends itself SIGHUP lives on and exits 0.
#[test]
fn a_child_keeps_the_signals_its_parent_ignores() {
    let scratch = Scratch::new(
        "defaults",
        &format!(
            "{IMPORTS}\npub fn main() -> Void needs {{ commands: Commands }} {{\n  let ended = sh(\"kill -HUP $$; exit 0\")\n  case ended {{\n    Status.Exited(0) -> Void\n    other -> Debug.crash(\"the child ended with ${{Debug.render(other)}}\")\n  }}\n}}\n"
        ),
    );
    let built = scratch.chimi(&["build", "cases.chimi", "-o", "runner"]);
    assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
    let output = Command::new("/bin/sh")
        .current_dir(&scratch.0)
        .args(["-c", "trap '' HUP; exec ./runner"])
        .output()
        .expect("run the program");
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
}

// Rejects a lookup that takes a directory on PATH for the program: a
// directory named `sh` earlier on PATH is passed over for the real one.
#[test]
fn a_directory_on_path_is_not_the_program() {
    let scratch = Scratch::new(
        "path",
        &suite(
            r#"
test "a directory named like the program" {
  let status = Exec.run(Exec.command("sh", ["-c", "exit 4"])) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(4))
}
"#,
        ),
    );
    std::fs::create_dir_all(scratch.0.join("bin/sh")).unwrap();
    let path = format!("{}:{}", scratch.0.join("bin").display(), std::env::var("PATH").unwrap());
    let run = scratch.chimi_with(&["test", "cases.chimi", "--clock=real"], &[("PATH", path)]);
    assert_passed(&run, 1);
}

// Rejects a child wait that holds the scheduler thread, and one that finds
// an exit only by polling. The child exits once another task has touched a
// file, which that task can do only while the wait is parked; and a child
// that exits after 1.3 s is seen at once, where the rechecks that back a
// missed SIGCHLD up (10 ms doubling to 1 s) would first look again at 2.27 s.
#[test]
fn a_child_wait_lets_other_tasks_run_and_ends_at_the_exit() {
    let run = run_tests(
        "parked",
        r#"
test "other tasks run while one waits" {
  let script = "for i in $(seq 100); do test -e go && exit 0; sleep 0.02; done; exit 1"
  let status = Exec.start(Exec.command("/bin/sh", ["-c", script]), fn(child) {
    let toucher = Task.start(fn() {
      Clock.sleep(Duration.milliseconds(50))
      sh("touch go")
    })
    let ended = Exec.wait(child)
    Task.result(toucher)
    ended
  }) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal(Status.Exited(0))
}

test "the exit wakes the wait" {
  let waited = Exec.start(Exec.command("/bin/sh", ["-c", "exec sleep 1.3"]), fn(child) {
    let start = Clock.now()
    Exec.wait(child)
    Clock.since(start)
  }) using { commands: Exec.host_for_tests() }
  (waited < Duration.milliseconds(2000)) |> Test.assert_equal(True)
}
"#,
    );
    assert_passed(&run, 2);
}
