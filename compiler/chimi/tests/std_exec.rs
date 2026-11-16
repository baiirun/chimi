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
import std/bytes as Bytes
import std/duration as Duration
import std/exec as Exec
import std/exec.{Commands, ExecError, Status, Stdio}
import std/map as Map
import std/option as Option
import std/path as Path
import std/result as Result
import std/result.{Err, Ok}
import std/stream as Stream
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
// twice, seen in the shell's own environment block on Linux), a relative program looked for in the child's directory, a file the
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
  let command = Exec.variable(Exec.variable(Exec.command("/bin/sh", ["-c", "test \"$X\" = second && { test ! -r /proc/$$/environ || test \"$(tr '\\0' '\\n' < /proc/$$/environ | grep -c '^X=')\" = 1; } && test -n \"$PATH\""]), "X", "first"), "X", "second")
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

// A test's own standard input is already /dev/null, so stdin is told apart
// in `standard_input_to_nothing_reads_nothing`, from a built program.
test "standard output and error to nothing" {
  let output = Exec.stdout(Exec.command("/bin/sh", ["-c", "test -c /dev/fd/1 && ! test -c /dev/fd/2"]), Stdio.Null)
  let error = Exec.stderr(Exec.command("/bin/sh", ["-c", "! test -c /dev/fd/1 && test -c /dev/fd/2"]), Stdio.Null)
  let inherited = Exec.command("/bin/sh", ["-c", "! test -c /dev/fd/1 && ! test -c /dev/fd/2"])
  let statuses = (Exec.run(output), Exec.run(error), Exec.run(inherited)) using { commands: Exec.host_for_tests() }
  statuses |> Test.assert_equal((Status.Exited(0), Status.Exited(0), Status.Exited(0)))
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

test "a wait after a cancelled one still gets the status" {
  let both = Exec.start(Exec.command("/bin/sh", ["-c", "sleep 0.3; exit 9"]), fn(child) {
    let first = attempt(fn() { Task.timeout(fn() { Exec.wait(child) }, Duration.milliseconds(50)) })
    (first, Exec.wait(child))
  }) using { commands: Exec.host_for_tests() }
  both |> Test.assert_equal(("Err(Timeout.TimedOut(50ms))", Status.Exited(9)))
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
    assert_passed(&run, 7);
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

/// Builds `main` from `body` (after the shared imports) into `runner`.
fn build_main(name: &str, body: &str) -> Scratch {
    let scratch = Scratch::new(name, &format!("{IMPORTS}\n{body}"));
    let built = scratch.chimi(&["build", "cases.chimi", "-o", "runner"]);
    assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
    scratch
}

/// Runs the built `runner` with `input` on a piped standard input.
fn run_main(scratch: &Scratch, input: &str) -> std::process::Output {
    use std::io::Write;
    let mut child = Command::new(scratch.0.join("runner"))
        .current_dir(&scratch.0)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run the program");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().expect("wait for the program")
}

// Rejects `Stdio.Null` on stdin (also `Exec.capture`'s) that leaves the
// parent's input connected, or is opened for writing: the program's own input holds a line, which a child
// that inherits it reads and one given nothing does not (and `cat` fails on
// a descriptor opened only for writing).
#[test]
fn standard_input_to_nothing_reads_nothing() {
    let scratch = build_main(
        "stdin",
        r#"pub fn main() -> Void needs { commands: Commands } {
  let nothing = Exec.run(Exec.stdin(Exec.command("/bin/sh", ["-c", "cat > got && ! test -s got && test -c /dev/fd/0"]), Stdio.Null))
  let captured = Exec.capture(Exec.command("/bin/cat", []))
  let inherited = Exec.run(Exec.command("/bin/sh", ["-c", "read line && test \"$line\" = hello"]))
  case (nothing, Bytes.length(captured.output), inherited) {
    (Status.Exited(0), 0, Status.Exited(0)) -> Void
    other -> Debug.crash("the children ended with ${Debug.render(other)}")
  }
}
"#,
    );
    let output = run_main(&scratch, "hello\n");
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
}

// Rejects a child that inherits the parent's blocked signals: the program is
// started with SIGUSR1 blocked, and its child's mask is still empty. The
// child is grep, not a shell, since dash clears its own mask at startup.
#[cfg(target_os = "linux")]
#[test]
fn a_child_starts_with_no_signal_blocked() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: no python3 to block a signal with");
        return;
    }
    let scratch = build_main(
        "mask",
        r#"pub fn main() -> Void needs { commands: Commands } {
  let ended = Exec.run(Exec.command("grep", ["-q", "^SigBlk:[[:space:]]*0*$", "/proc/self/status"]))
  case ended {
    Status.Exited(0) -> Void
    other -> Debug.crash("the child ended with ${Debug.render(other)}")
  }
}
"#,
    );
    let blocked = Command::new("python3")
        .current_dir(&scratch.0)
        .args([
            "-c",
            "import os, signal; signal.pthread_sigmask(signal.SIG_BLOCK, [signal.SIGUSR1]); os.execv('./runner', ['runner'])",
        ])
        .output()
        .expect("run the program");
    assert_eq!(blocked.status.code(), Some(0), "{}", String::from_utf8_lossy(&blocked.stderr));
}

// Rejects a capture that loses a stream, mixes them up, reads them one after
// the other (each child writes 200 000 bytes to both, more than a pipe holds,
// so reading one to its end first would never finish), or treats a nonzero
// exit as a failure.
#[test]
fn capture_collects_both_streams_and_the_status() {
    let run = run_tests(
        "capture",
        r#"
test "both streams and the status" {
  let got = Exec.capture(Exec.command("/bin/sh", ["-c", "printf out; printf err >&2; exit 3"])) using { commands: Exec.host_for_tests() }
  (got.status, Bytes.to_text(got.output), Bytes.to_text(got.error_output)) |> Test.assert_equal((Status.Exited(3), "out", "err"))
}

test "more than a pipe holds, on both at once" {
  let script = "head -c 200000 /dev/zero; head -c 200000 /dev/zero >&2; head -c 200000 /dev/zero"
  let got = Exec.capture(Exec.command("/bin/sh", ["-c", script])) using { commands: Exec.host_for_tests() }
  (Bytes.length(got.output), Bytes.length(got.error_output), got.status) |> Test.assert_equal((400000, 200000, Status.Exited(0)))
}

test "capture reads no input" {
  let got = Exec.capture(Exec.command("/bin/sh", ["-c", "cat"])) using { commands: Exec.host_for_tests() }
  (Bytes.length(got.output), got.status) |> Test.assert_equal((0, Status.Exited(0)))
}
"#,
    );
    assert_passed(&run, 3);
}

// Rejects pipes that do not carry bytes both ways, a wait that closes the
// input under a writer task (wc would count fewer than 262 144 bytes), a write to a child that
// stopped reading that kills the runner with SIGPIPE instead of failing with
// `BrokenPipe`, a stopped output that leaves the child writing forever (it
// dies of SIGPIPE, the default it starts with), a pipe end another child
// inherits (a sibling holding cat's input open would keep cat from exiting
// after the input is closed), and a read that a cancellation cannot stop.
#[test]
fn pipes_carry_a_childs_input_and_output() {
    let run = run_tests(
        "pipes",
        r#"
fn cat() -> Exec.Command {
  Exec.stdout(Exec.stdin(Exec.command("/bin/cat", []), Stdio.Pipe), Stdio.Pipe)
}

test "a round trip through cat" {
  let got = Exec.start(cat(), fn(child) {
    let writer = Task.start(fn() {
      Stream.write(Exec.input(child), Bytes.from_text("hello, "))
      Stream.write(Exec.input(child), Bytes.from_text("cat"))
      Stream.close(Exec.input(child))
    })
    let all = Stream.read_all(Exec.output(child), 100)
    Task.result(writer)
    (Bytes.to_text(all), Exec.wait(child))
  }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal(("hello, cat", Status.Exited(0)))
}

test "wait leaves the input to a writer task" {
  let command = Exec.stdout(Exec.stdin(Exec.command("/bin/sh", ["-c", "wc -c | tr -d ' '"]), Stdio.Pipe), Stdio.Pipe)
  let got = Exec.start(command, fn(child) {
    let input = Exec.input(child)
    let writer = Task.start(fn() {
      loop count = 0 {
        case count == 32768 {
          True -> break Void
          False -> {
            Stream.write(input, Bytes.from_text("12345678"))
            count + 1
          }
        }
      }
      Stream.close(input)
    })
    let reader = Task.start(fn() { Stream.read_all(Exec.output(child), 100) })
    let status = Exec.wait(child)
    Task.result(writer)
    (Bytes.to_text(Task.result(reader)), status)
  }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal(("262144\n", Status.Exited(0)))
}

test "a write to a child that stopped reading" {
  let command = Exec.stdout(Exec.stdin(Exec.command("/bin/sh", ["-c", "exec 0<&-; echo closed; exec sleep 5"]), Stdio.Pipe), Stdio.Pipe)
  let got = Exec.start(command, fn(child) {
    // The child says when its input is closed.
    let said = Stream.read(Exec.output(child))
    attempt(fn() { Stream.write(Exec.input(child), Bytes.from_text("nobody reads this")) })
  }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal("Err(ExecError.BrokenPipe(\"/bin/sh\"))")
}

test "a write after the input was closed" {
  let got = Exec.start(cat(), fn(child) {
    Stream.close(Exec.input(child))
    attempt(fn() { Stream.write(Exec.input(child), Bytes.from_text("late")) })
  }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal("Err(ExecError.Closed(\"/bin/cat\"))")
}

test "stopping the output ends a child that keeps writing" {
  let status = Exec.start(Exec.stdout(Exec.command("yes", []), Stdio.Pipe), fn(child) {
    let first = Stream.read(Exec.output(child))
    Stream.stop(Exec.output(child))
    (Option.is_some(first), Exec.wait(child))
  }) using { commands: Exec.host_for_tests() }
  status |> Test.assert_equal((True, Status.Signaled(13)))
}

test "a sibling does not hold a child's pipe" {
  let waited = Exec.start(Exec.stdin(Exec.command("/bin/cat", []), Stdio.Pipe), fn(child) {
    Exec.start(Exec.command("/bin/sh", ["-c", "exec sleep 3"]), fn(_later) {
      let start = Clock.now()
      Stream.close(Exec.input(child))
      (Exec.wait(child), Clock.since(start) < Duration.seconds(2))
    })
  }) using { commands: Exec.host_for_tests() }
  waited |> Test.assert_equal((Status.Exited(0), True))
}

test "a cancelled read returns at once" {
  let got = Exec.start(Exec.stdout(Exec.command("/bin/sh", ["-c", "exec sleep 5"]), Stdio.Pipe), fn(child) {
    attempt(fn() { Task.timeout(fn() { Stream.read(Exec.output(child)) }, Duration.milliseconds(100)) })
  }) using { commands: Exec.host_for_tests() }
  got |> Test.assert_equal("Err(Timeout.TimedOut(100ms))")
}
"#,
    );
    assert_passed(&run, 7);
}

// Rejects a release that leaves the parent's pipe ends open: the runner holds
// as many descriptors after a piped child's start as before it.
#[cfg(target_os = "linux")]
#[test]
fn a_released_child_leaves_no_descriptor_open() {
    let run = run_tests(
        "descriptors",
        r#"
test "pipes are closed with the child" {
  let counted = (fn() {
    let before = Exec.capture(Exec.command("/bin/sh", ["-c", "ls /proc/$PPID/fd | wc -l"]))
    let piped = Exec.stderr(Exec.stdout(Exec.stdin(Exec.command("/bin/sh", ["-c", "exit 0"]), Stdio.Pipe), Stdio.Pipe), Stdio.Pipe)
    Exec.start(piped, fn(child) { Exec.wait(child) })
    let after = Exec.capture(Exec.command("/bin/sh", ["-c", "ls /proc/$PPID/fd | wc -l"]))
    (Bytes.to_text(before.output), Bytes.to_text(after.output))
  })() using { commands: Exec.host_for_tests() }
  let (before, after) = counted
  after |> Test.assert_equal(before)
}
"#,
    );
    assert_passed(&run, 1);
}

// Rejects fakes that cannot pipe: a fake child's streams come from
// `Exec.piped`, `Exec.capture` reads them, a fake input
// whose reader stopped fails with `BrokenPipe`, and asking for a stream the
// command did not pipe is a defect that names it. Native only: std/stream's
// pipes need the bounded channel runtime, which the interpreter lacks.
#[test]
fn a_fake_child_pipes_streams() {
    let source = suite(
        r#"
fn version() -> Text needs { commands: Commands } {
  let got = Exec.capture(Exec.command("tool", ["--version"]))
  "${Bytes.to_text(got.output)}|${Bytes.to_text(got.error_output)}|${Debug.render(got.status)}"
}

fn fake_child() -> Exec.Child {
  Exec.child(fn() { Status.Exited(0) }, fn() { Void }, fn() { Void }, fn() { Void })
}

// A fake makes its streams inside `start`: the provider goes to every task
// its caller starts, and a channel end it held would belong to one of them.
fn fake(output: Text, stopped: Bool) {
  {
    start: fn(_command) {
      let made = Result.capture(fn() {
        let (input, unread) = Stream.pipe(1)
        case stopped {
          True -> Stream.stop(unread)
          False -> Void
        }
        Exec.piped(fake_child(), input, Stream.from_bytes(Bytes.from_text(output)), Stream.from_bytes(Bytes.from_text("")))
      })
      case made {
        Ok(child) -> child
        Err(_) -> Failure(ExecError.Failed("tool", "the fake's streams"))
      }
    },
  }
}

test "a fake's output is captured" {
  let got = version() using { commands: fake("tool 1.0", False) }
  got |> Test.assert_equal("tool 1.0||Status.Exited(0)")
}

// A fake `cat`: what is written to its input comes out of its output.
fn fake_cat() {
  {
    start: fn(_command) {
      let made = Result.capture(fn() {
        let (input, output) = Stream.pipe(4)
        Exec.piped(fake_child(), input, output, Stream.from_bytes(Bytes.from_text("")))
      })
      case made {
        Ok(child) -> child
        Err(_) -> Failure(ExecError.Failed("cat", "the fake's streams"))
      }
    },
  }
}

test "a fake round trip with a writer task" {
  let got = Exec.start(Exec.command("cat", []), fn(child) {
    let input = Exec.input(child)
    let writer = Task.start(fn() {
      Stream.write(input, Bytes.from_text("through "))
      Stream.write(input, Bytes.from_text("the fake"))
      Stream.close(input)
    })
    let all = Stream.read_all(Exec.output(child), 100)
    Task.result(writer)
    (Bytes.to_text(all), Exec.wait(child))
  }) using { commands: fake_cat() }
  got |> Test.assert_equal(("through the fake", Status.Exited(0)))
}

test "a fake read after a stop is Closed" {
  let got = Exec.start(Exec.command("tool", []), fn(child) {
    Stream.stop(Exec.output(child))
    attempt(fn() { Stream.read(Exec.output(child)) })
  }) using { commands: fake("unread", False) }
  got |> Test.assert_equal("Err(ExecError.Closed(\"tool\"))")
}

test "a fake input whose reader stopped" {
  let got = Exec.start(Exec.command("tool", []), fn(child) {
    attempt(fn() { Stream.write(Exec.input(child), Bytes.from_text("x")) })
  }) using { commands: fake("", True) }
  got |> Test.assert_equal("Err(ExecError.BrokenPipe(\"tool\"))")
}
"#,
    );
    let run = Scratch::new("fake-pipes", &source).chimi(&["test", "cases.chimi"]);
    assert_passed(&run, 4);
    let unpiped = run_tests(
        "unpiped",
        "test \"no pipe\" {\n  Exec.start(Exec.command(\"/bin/sh\", [\"-c\", \"exit 0\"]), fn(child) {\n    let _ = Stream.read(Exec.output(child))\n    Void\n  }) using { commands: Exec.host_for_tests() }\n}\n",
    );
    assert_ne!(unpiped.status, Some(0));
    assert!(
        unpiped.stdout.contains("Exec: /bin/sh was started without Stdio.Pipe for stdout"),
        "{}{}",
        unpiped.stdout,
        unpiped.stderr
    );
}

// Rejects a release that kills at once, waits past the exit, blocks the
// scheduler while the child takes its time, or never sends SIGKILL. A child
// that traps SIGTERM leaves a note and exits; one that ignores SIGTERM is
// killed once `stop_within` passes; zero kills at once, so the trap never
// runs; a piped input is closed before SIGTERM, so a child that exits at its
// end needs no signal; a start cancelled from outside still stops its child
// gently; a task started before the release keeps running through it; and a
// negative `stop_within` is a defect where it is set.
#[test]
fn a_child_left_running_gets_sigterm_then_sigkill() {
    let run = run_tests(
        "stopping-gracefully",
        r#"
fn ready(command: Exec.Command) -> Exec.Command {
  Exec.stdout(command, Stdio.Pipe)
}

// Starts `script`, waits for its first line, and ends the start.
fn leave_running(script: Text, within: Duration.Duration) -> Void needs { commands: Commands } {
  Exec.start(Exec.stop_within(ready(Exec.command("/bin/sh", ["-c", script])), within), fn(child) {
    let _ = Stream.read(Exec.output(child))
    Void
  })
}

fn exists(file: Text) -> Bool needs { commands: Commands } {
  Exec.succeeded(sh("test -s ${file}"))
}

test "a child that traps SIGTERM exits on its own" {
  let noted = (fn() {
    leave_running("trap 'echo bye > term.txt; exit 0' TERM; echo ready; while :; do sleep 0.02; done", Duration.seconds(5))
    exists("term.txt")
  })() using { commands: Exec.host_for_tests() }
  noted |> Test.assert_equal(True)
}

test "a child that ignores SIGTERM is killed after stop_within" {
  let report = (fn() {
    let start = Clock.now()
    leave_running("trap '' TERM; echo $$ > ignored.pid; echo ready; while :; do sleep 0.02; done", Duration.milliseconds(300))
    let took = Clock.since(start)
    let gone = Exec.succeeded(sh("! kill -0 \"$(cat ignored.pid)\" 2>/dev/null"))
    (took >= Duration.milliseconds(300), took < Duration.seconds(3), gone)
  })() using { commands: Exec.host_for_tests() }
  report |> Test.assert_equal((True, True, True))
}

test "zero sends SIGKILL at once" {
  let noted = (fn() {
    leave_running("trap 'echo bye > zero.txt; exit 0' TERM; echo ready; while :; do sleep 0.02; done", Duration.seconds(0))
    exists("zero.txt")
  })() using { commands: Exec.host_for_tests() }
  noted |> Test.assert_equal(False)
}

test "a piped input's end comes before SIGTERM" {
  let report = (fn() {
    let start = Clock.now()
    let command = Exec.stdin(ready(Exec.command("/bin/sh", ["-c", "trap '' TERM; echo ready; cat > /dev/null; echo done > eof.txt"])), Stdio.Pipe)
    Exec.start(Exec.stop_within(command, Duration.seconds(5)), fn(child) {
      let _ = Stream.read(Exec.output(child))
      Void
    })
    (Clock.since(start) < Duration.seconds(3), exists("eof.txt"))
  })() using { commands: Exec.host_for_tests() }
  report |> Test.assert_equal((True, True))
}

test "a cancelled start still stops its child gently" {
  let noted = (fn() {
    let _ = Result.capture(fn() {
      Task.timeout(fn() {
        Exec.start(Exec.command("/bin/sh", ["-c", "trap 'echo bye > cancelled.txt; exit 0' TERM; while :; do sleep 0.02; done"]), fn(child) {
          Exec.wait(child)
        })
      }, Duration.milliseconds(200))
    })
    exists("cancelled.txt")
  })() using { commands: Exec.host_for_tests() }
  noted |> Test.assert_equal(True)
}

test "other tasks run while a child is stopped" {
  let order = (fn() {
    let ticker = Task.start(fn() {
      Clock.sleep(Duration.milliseconds(200))
      Clock.now()
    })
    leave_running("trap '' TERM; echo ready; while :; do sleep 0.02; done", Duration.seconds(1))
    let released = Clock.now()
    Task.result(ticker) < released
  })() using { commands: Exec.host_for_tests() }
  order |> Test.assert_equal(True)
}
"#,
    );
    assert_passed(&run, 6);
    let negative = run_tests(
        "negative-stop",
        "test \"negative\" {\n  let _ = Exec.stop_within(Exec.command(\"/bin/sh\", []), Duration.milliseconds(-5))\n  Void\n}\n",
    );
    assert_ne!(negative.status, Some(0));
    assert!(
        negative.stdout.contains("Exec: stop_within must not be negative, got -5ms"),
        "{}{}",
        negative.stdout,
        negative.stderr
    );
}

// Rejects a runner that leaves a child behind when the program dies: a
// defect inside `use` still stops the child, and a child the program holds
// outside any `Exec.start` (its provider called directly) is killed at the
// program's end. Each child's process is gone once the runner has exited.
#[test]
fn no_child_outlives_the_program() {
    let scratch = build_main(
        "backstop",
        r#"pub fn main() -> Void needs { commands: Commands } {
  Exec.start(Exec.command("/bin/sh", ["-c", "echo $$ > child.pid; exec sleep 30"]), fn(child) {
    let waited = sh("while ! test -s child.pid; do sleep 0.01; done")
    case Exec.succeeded(waited) {
      True -> Debug.crash("a defect with a child running")
      False -> Exec.wait(child)
    }
  })
  Void
}
"#,
    );
    let output = run_main(&scratch, "");
    assert_ne!(output.status.code(), Some(0));
    let pid: i32 = std::fs::read_to_string(scratch.0.join("child.pid")).unwrap().trim().parse().unwrap();
    assert!(!exists(pid), "the child {pid} outlived the program's defect");

    let scratch = build_main(
        "backstop_unscoped",
        r#"pub fn main() -> Void needs { commands: Commands } {
  let _held = commands.start(Exec.command("/bin/sh", ["-c", "echo $$ > child.pid; exec sleep 30"]))
  let _ = sh("while ! test -s child.pid; do sleep 0.01; done")
  Void
}
"#,
    );
    let started = std::time::Instant::now();
    let output = run_main(&scratch, "");
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "the runner waited out the child");
    let pid: i32 = std::fs::read_to_string(scratch.0.join("child.pid")).unwrap().trim().parse().unwrap();
    assert!(!exists(pid), "the held child {pid} outlived the program");
}
