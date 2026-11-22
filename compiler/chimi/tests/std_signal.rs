//! `std/signal` (CHI-249): SIGHUP, SIGUSR1 and SIGUSR2 as events a task
//! waits for. The virtual-clock tests inject signals with the runner's
//! `--signal=HUP@...` option and read the outcome and the
//! `clock_signal_received` events; the real-signal tests send signals with
//! `kill` and wait for a line the program prints first, so no assertion
//! depends on how long anything takes.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

const SIGHUP: i32 = 1;
const SIGTERM: i32 = 15;
#[cfg(target_os = "linux")]
const SIGUSR1: i32 = 10;
#[cfg(target_os = "macos")]
const SIGUSR1: i32 = 30;

const IMPORTS: &str = "\
import std/clock as Clock
import std/clock.{Clock}
import std/duration as Duration
import std/io.{Console}
import std/select as Select
import std/signal as Signal
import std/signal.{Signal, Signals}
import std/task as Task
import std/text as Text

fn name(signal: Signal) -> Text {
  case signal {
    Signal.Hangup -> \"hangup\"
    Signal.User1 -> \"user1\"
    Signal.User2 -> \"user2\"
  }
}

fn waited(wanted: List<Signal>) -> Text needs { signals: Signals } {
  name(Signal.next(wanted))
}
";

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("chimi-std-signal-{name}-{}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn build_native(scratch: &Path, body: &str) -> PathBuf {
    let path = scratch.join("main.chimi");
    let executable = scratch.join("main");
    std::fs::write(&path, format!("{IMPORTS}\n{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    executable
}

/// A virtual-clock run: its process output, outcome record, and the
/// `(time in ms, signal number)` of each `clock_signal_received` event.
struct Run {
    output: Output,
    outcome: String,
    signals: Vec<(u64, i64)>,
    advanced_to: Vec<u64>,
}

fn run_virtual(name: &str, body: &str, signals: &[&str]) -> Run {
    let scratch = scratch_directory(name);
    let executable = build_native(&scratch, body);
    let telemetry = scratch.join("telemetry.jsonl");
    let outcome = scratch.join("outcome.jsonl");
    let mut command = Command::new(&executable);
    command
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg("--trace=boundaries")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()));
    for signal in signals {
        command.arg(format!("--signal={signal}"));
    }
    let output = output_within(command.arg("--"), Duration::from_secs(60));
    let telemetry = std::fs::read_to_string(telemetry).unwrap_or_default();
    let field = |line: &str, name: &str| -> i64 {
        let start = line.find(&format!("\"{name}\":")).unwrap() + name.len() + 3;
        line[start..]
            .split(|character: char| !character.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let millis = |line: &str| u64::try_from(field(line, "time_ns")).unwrap() / 1_000_000;
    let signals = telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_signal_received\""))
        .map(|line| (millis(line), field(line, "request_kind")))
        .collect();
    let advanced_to = telemetry
        .lines()
        .filter(|line| line.contains("\"event\":\"clock_advanced\""))
        .map(millis)
        .collect();
    let outcome = std::fs::read_to_string(outcome).unwrap_or_default();
    std::fs::remove_dir_all(scratch).unwrap();
    Run {
        output,
        outcome,
        signals,
        advanced_to,
    }
}

fn success(value: &str) -> String {
    format!("{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{value}}}\n")
}

// ----- Virtual clock ----------------------------------------------------------

// One program walks the rules in order: a waiter wakes when its signal
// arrives; signals that arrive while nobody waits are kept, and repeats
// coalesce into one; the first listed pending signal wins; a `next` that
// loses a `Select.first` race and is cancelled takes nothing, so a later
// `next` still gets the next signal; and a signal no waiter wants stays
// latched until one asks for it.
#[test]
fn signals_wake_waiters_coalesce_and_wait_their_turn() {
    let run = run_virtual(
        "rules",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  let first = waited([Signal.Hangup])
  Clock.sleep(Duration.seconds(1))
  let coalesced = waited([Signal.Hangup])
  let user = waited([Signal.Hangup, Signal.User1])
  let racer = Task.start(fn() { waited([Signal.Hangup]) })
  let raced = Select.first([
    Select.task(racer, fn(got: Text) { got }),
    Select.after(Duration.seconds(1), \"timeout\"),
  ])
  Task.cancel(racer)
  let later = waited([Signal.Hangup])
  let latched = waited([Signal.User2])
  Text.join([first, coalesced, user, raced, later, latched], \" \")
}
",
        &[
            "HUP@1s",
            "HUP@1500ms",
            "HUP@1600ms",
            "USR1@3s",
            "USR2@3500ms",
            "HUP@5s",
        ],
    );
    assert!(run.output.status.success(), "{:?}", run.output);
    assert_eq!(
        run.outcome,
        success("\"hangup hangup user1 timeout hangup user2\"")
    );
    let times: Vec<u64> = run.signals.iter().map(|(time, _)| *time).collect();
    assert_eq!(times, [1000, 1500, 1600, 3000, 3500, 5000]);
    assert_eq!(run.signals[0].1, i64::from(SIGHUP));
    assert_eq!(run.signals[3].1, i64::from(SIGUSR1));
}

// When several listed signals are pending, `next` returns the first in its
// list, not in any fixed order of the signals.
#[test]
fn the_first_listed_pending_signal_is_returned() {
    let run = run_virtual(
        "list-order",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  Clock.sleep(Duration.seconds(2))
  let first = waited([Signal.User2, Signal.Hangup])
  let second = waited([Signal.Hangup, Signal.User2])
  Text.join([first, second], \" \")
}
",
        &["HUP@1s", "USR2@1s"],
    );
    assert_eq!(run.outcome, success("\"user2 hangup\""), "{:?}", run.output);
}

// When several tasks wait for one signal, each signal wakes one of them, the
// one that has waited longest.
#[test]
fn each_signal_wakes_the_oldest_waiter() {
    let run = run_virtual(
        "fifo",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  let a = Task.start(fn() { Text.concat([\"a:\", waited([Signal.Hangup])]) })
  Clock.sleep(Duration.milliseconds(10))
  let b = Task.start(fn() { Text.concat([\"b:\", waited([Signal.User1, Signal.Hangup])]) })
  Clock.sleep(Duration.milliseconds(10))
  let first = Select.first([Select.task(a, fn(got: Text) { got }), Select.task(b, fn(got: Text) { got })])
  let second = Select.first([Select.task(b, fn(got: Text) { got })])
  Text.join([first, second], \" \")
}
",
        &["HUP@1s", "HUP@2s"],
    );
    assert_eq!(
        run.outcome,
        success("\"a:hangup b:hangup\""),
        "{:?}",
        run.output
    );
}

// A waiter woken for a signal but cancelled before it runs again (here by
// its timeout, due at the same instant) takes nothing: the signal goes to the
// next waiter.
#[test]
fn a_woken_waiter_that_is_cancelled_passes_its_signal_on() {
    let run = run_virtual(
        "woken-cancelled",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  let a = Task.start(fn() {
    case fn() { waited([Signal.Hangup]) } |> Task.timeout(Duration.seconds(1)) {
      Success(got) -> Text.concat([\"a:\", got])
      Failure(_) -> \"a:timeout\"
    }
  })
  Clock.sleep(Duration.milliseconds(10))
  let b = Task.start(fn() { Text.concat([\"b:\", waited([Signal.Hangup])]) })
  let first = Select.first([Select.task(a, fn(got: Text) { got })])
  let second = Select.first([Select.task(b, fn(got: Text) { got })])
  Text.join([first, second], \" \")
}
",
        &["HUP@1s"],
    );
    assert_eq!(
        run.outcome,
        success("\"a:timeout b:hangup\""),
        "{:?}",
        run.output
    );
}

// A signal handed to a waiter is its own until it runs again: a task that
// asks for the same signal in the meantime waits for the next one.
#[test]
fn a_new_wait_does_not_take_a_signal_handed_to_an_older_waiter() {
    let run = run_virtual(
        "reserved",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  let a = Task.start(fn() {
    Clock.sleep(Duration.milliseconds(10))
    Text.concat([\"a:\", waited([Signal.Hangup])])
  })
  let first = waited([Signal.User1, Signal.Hangup])
  let second = waited([Signal.Hangup])
  let older = Select.first([
    Select.task(a, fn(got: Text) { got }),
    Select.after(Duration.milliseconds(1), \"a:pending\"),
  ])
  Text.join([first, second, older], \" \")
}
",
        &["USR1@1s", "HUP@1s", "HUP@3s"],
    );
    assert_eq!(
        run.outcome,
        success("\"user1 hangup a:hangup\""),
        "{:?}",
        run.output
    );
}

// SIGHUP never stops the program: after a reload, SIGTERM still drains and
// stops it as in any program, and the exit status is SIGTERM's.
#[test]
fn a_hangup_reloads_and_a_later_terminate_still_drains() {
    let run = run_virtual(
        "reload-then-stop",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  let reload = waited([Signal.Hangup])
  Clock.sleep(Duration.seconds(10))
  reload
}
",
        &["HUP@1s", "TERM@2s"],
    );
    assert_eq!(run.output.status.code(), Some(143), "{:?}", run.output);
    assert!(
        run.outcome.contains("\"outcome\":\"cancellation\""),
        "{}",
        run.outcome
    );
    assert_eq!(
        run.signals,
        [(1000, i64::from(SIGHUP)), (2000, i64::from(SIGTERM))]
    );
}

// A task waiting for a signal nobody will send is stuck: under the virtual
// clock that is a deadlock, and the report names `Signal.next`.
#[test]
fn a_waiter_with_no_signal_coming_is_a_deadlock() {
    let run = run_virtual(
        "deadlock",
        "pub fn main() -> Text needs { signals: Signals } {
  waited([Signal.Hangup])
}
",
        &[],
    );
    assert_eq!(run.output.status.code(), Some(70), "{:?}", run.output);
    assert!(run.outcome.contains("Signal.next"), "{}", run.outcome);
}

// A signal due after main ends is never latched and never moves virtual
// time, like an injected SIGTERM (SG03).
#[test]
fn a_signal_due_after_main_ends_changes_nothing() {
    let run = run_virtual(
        "after-main",
        "pub fn main() -> Text needs { signals: Signals, clock: Clock } {
  Clock.sleep(Duration.seconds(1))
  \"done\"
}
",
        &["HUP@10s", "USR2@20s"],
    );
    assert_eq!(run.outcome, success("\"done\""), "{:?}", run.output);
    assert!(run.signals.is_empty(), "{:?}", run.signals);
    assert_eq!(run.advanced_to, [1000]);
}

// `next` with nothing to wait for is a mistake in the program.
#[test]
fn waiting_for_no_signal_defects() {
    let run = run_virtual(
        "empty",
        "pub fn main() -> Text needs { signals: Signals } {
  waited([])
}
",
        &[],
    );
    assert!(
        run.outcome
            .contains("Signal.next needs at least one signal to wait for"),
        "{}",
        run.outcome
    );
}

// Injected signals need the virtual clock, and the option names the signals
// it takes.
#[test]
fn the_signal_option_takes_the_three_signals_under_the_virtual_clock_only() {
    let scratch = scratch_directory("option");
    let executable = build_native(
        &scratch,
        "pub fn main() -> Text needs { signals: Signals } {\n  waited([Signal.Hangup])\n}\n",
    );
    // Bounded: a runner that accepted the option would wait for a real
    // signal forever.
    let real = output_within(
        Command::new(&executable).args(["--chimi-runner", "--signal=HUP@1s", "--"]),
        Duration::from_secs(30),
    );
    assert_eq!(real.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&real.stderr).contains("--signal requires --clock=virtual"));
    let unknown = Command::new(&executable)
        .args([
            "--chimi-runner",
            "--clock=virtual",
            "--signal=WINCH@1s",
            "--",
        ])
        .output()
        .unwrap();
    assert_eq!(unknown.status.code(), Some(64));
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("INT, TERM, HUP, USR1 or USR2"),
        "{unknown:?}"
    );
    // The option repeats, up to 64 std/signal signals.
    let repeated = |count: usize| {
        let mut command = Command::new(&executable);
        command.args(["--chimi-runner", "--clock=virtual"]);
        for _ in 0..count {
            command.arg("--signal=HUP@1s");
        }
        output_within(command.arg("--"), Duration::from_secs(30))
    };
    assert_eq!(repeated(64).status.code(), Some(0));
    let too_many = repeated(65);
    assert_eq!(too_many.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&too_many.stderr).contains("at most 64"));
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The output of `command`, killing it and failing after `limit`.
fn output_within(command: &mut Command, limit: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start native program");
    let deadline = Instant::now() + limit;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the program did not end within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

// ----- Real signals -----------------------------------------------------------

/// A program that has started, with its output read line by line.
struct Running {
    child: Child,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
}

impl Running {
    fn start(executable: &Path) -> Self {
        let mut child = Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start native program");
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self { child, lines }
    }

    fn next_line(&mut self) -> String {
        self.lines
            .next()
            .expect("the program printed another line")
            .unwrap()
    }

    fn signal(&self, signal: i32) {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(self.child.id().to_string())
            .status()
            .unwrap();
        assert!(status.success(), "kill -{signal}");
    }

    /// The rest of the output and the exit status.
    fn finish(mut self) -> (Vec<String>, ExitStatus) {
        let limit = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > limit {
                let _ = self.child.kill();
                panic!("the program did not end within 60 s");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let rest = self.lines.map(Result::unwrap).collect();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
        (rest, status)
    }
}

// A service reloads on each SIGHUP. It waits with nothing else to do, so the
// runner must wait for the signal rather than report a deadlock.
#[test]
fn a_service_reloads_on_each_hangup() {
    let scratch = scratch_directory("reload");
    let executable = build_native(
        &scratch,
        "pub fn main() -> Void needs { signals: Signals, console: Console } {
  console.write_line(\"ready\")
  loop reloads = 0 {
    case reloads == 2 {
      True -> break Void
      False -> {
        console.write_line(waited([Signal.Hangup]))
        reloads + 1
      }
    }
  }
}
",
    );
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGHUP);
    assert_eq!(running.next_line(), "hangup");
    running.signal(SIGHUP);
    let (rest, status) = running.finish();
    assert_eq!(rest, ["hangup"]);
    assert_eq!(status.code(), Some(0), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// The handlers are installed when the runner builds `main`'s provider, before
// main's first line, and cover all three signals: a program that never waits
// survives SIGHUP and SIGUSR1, and SIGTERM still drains and ends it.
#[test]
fn a_program_that_needs_signals_survives_them_from_its_first_line() {
    let scratch = scratch_directory("survive");
    let executable = build_native(
        &scratch,
        "pub fn main() -> Void needs { signals: Signals, clock: Clock, console: Console } {
  console.write_line(\"ready\")
  Clock.sleep(Duration.seconds(60))
}
",
    );
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGHUP);
    running.signal(SIGUSR1);
    running.signal(SIGTERM);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(status.signal(), Some(SIGTERM), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// A program that does not need `signals` keeps the default actions: SIGHUP
// ends it.
#[test]
fn a_program_without_signals_is_ended_by_a_hangup() {
    let scratch = scratch_directory("default");
    let executable = build_native(
        &scratch,
        "pub fn main() -> Void needs { clock: Clock, console: Console } {
  console.write_line(\"ready\")
  Clock.sleep(Duration.seconds(60))
}
",
    );
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGHUP);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(status.signal(), Some(SIGHUP), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// SIGTERM after a reload, while main waits for SIGHUP again, cancels the wait
// and drains, and the program dies by SIGTERM. That the second stop signal
// ends a program at once is tests/signals.rs's; std/signal leaves that path
// alone.
#[test]
fn stop_signals_keep_their_meaning_after_a_hangup() {
    let scratch = scratch_directory("stop");
    let executable = build_native(
        &scratch,
        "pub fn main() -> Void needs { signals: Signals, clock: Clock, console: Console } {
  console.write_line(\"ready\")
  console.write_line(waited([Signal.Hangup]))
  Task.start(fn() { Clock.sleep(Duration.seconds(60)) })
  console.write_line(waited([Signal.Hangup]))
}
",
    );
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGHUP);
    assert_eq!(running.next_line(), "hangup");
    running.signal(SIGTERM);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(status.signal(), Some(SIGTERM), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// ----- Interpreter and tests --------------------------------------------------

// The interpreter has no signal handlers: the runner's provider says so when
// a program waits.
#[test]
fn the_interpreter_needs_the_native_runtime_to_wait() {
    let scratch = scratch_directory("interpreter");
    let program = scratch.join("main.chimi");
    std::fs::write(
        &program,
        format!("{IMPORTS}\npub fn main() -> Text needs {{ signals: Signals }} {{\n  waited([Signal.Hangup])\n}}\n"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(&program)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Signal.next requires the native runtime"),
        "{stdout}"
    );
    // An empty list defects with the native runtime's message.
    std::fs::write(
        &program,
        format!(
            "{IMPORTS}\npub fn main() -> Text needs {{ signals: Signals }} {{\n  waited([])\n}}\n"
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(&program)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Signal.next needs at least one signal to wait for"),
        "{stdout}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// A test supplies `signals` as a fake record, and a test that does not is
// rejected before it runs.
#[test]
fn a_test_supplies_signals_as_a_fake() {
    let scratch = scratch_directory("fake");
    let program = scratch.join("main.chimi");
    let source = format!(
        "{IMPORTS}
import std/test as Test

fn reload_count(wanted: Int) -> Int needs {{ signals: Signals }} {{
  loop count = 0 {{
    case count == wanted {{
      True -> break count
      False -> {{
        let _ = Signal.next([Signal.Hangup])
        count + 1
      }}
    }}
  }}
}}

test \"the fake hands out hangups\" {{
  let fake = {{ next: fn(wanted: List<Signal>) {{ Signal.Hangup }} }}
  Test.assert_equal(reload_count(3) using {{ signals: fake }}, 3)
  Test.assert_equal(waited([Signal.User1]) using {{ signals: fake }}, \"hangup\")
}}
"
    );
    std::fs::write(&program, source).unwrap();
    for backend in ["native", "interpreter"] {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .arg("test")
            .arg(format!("--backend={backend}"))
            .arg(&program)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed, 0 failed"),
            "{backend}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let unsupplied = format!(
        "{IMPORTS}\nimport std/test as Test\n\ntest \"waits on the machine\" {{\n  Test.assert_equal(waited([Signal.Hangup]), \"hangup\")\n}}\n"
    );
    std::fs::write(&program, unsupplied).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("test")
        .arg(&program)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("T4400"), "{stderr}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// A stop signal cancels `main` like any other task (scope-owned-resources.md,
// "Cancelling the task"): a `main` inside `Resource.scoped` runs its release,
// which sleeps 2 s, before it ends. Rejects ending `main` at
// once, which skipped the release, so an `Exec.start` written in `main` lost
// its graceful stop on Ctrl-C.
#[test]
fn a_stop_signal_runs_the_releases_of_main_itself() {
    let run = run_virtual(
        "main-release",
        "import std/resource as Resource

pub fn main() -> Int needs { clock: Clock, console: Console } {
  Resource.scoped(
    fn() { 3 },
    fn(held) {
      Clock.sleep(Duration.seconds(2))
      console.write_line(\"released\")
    },
    fn(held) {
      Clock.sleep(Duration.seconds(10))
      held
    },
  )
}
",
        &["TERM@1s"],
    );
    assert_eq!(run.output.status.code(), Some(143), "{:?}", run.output);
    assert!(
        run.outcome.contains("\"outcome\":\"cancellation\""),
        "{}",
        run.outcome
    );
    assert_eq!(String::from_utf8_lossy(&run.output.stdout), "released\n");
}
