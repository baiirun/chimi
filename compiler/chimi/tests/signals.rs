//! SIGINT and SIGTERM under the real clock (G22, "Signals" in
//! application-lifecycle.md). The virtual-clock rules, with exact
//! transcripts, are the process lab's SG scenarios; these tests send real
//! signals to show the runner's handler, its wake pipe, and the exit status.
//!
//! No assertion depends on how long anything takes: each test waits for a
//! line the program prints before it signals, and a generous limit only
//! stops a hung program from hanging the suite.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("chimi-signals-{name}-{}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn build_native(scratch: &Path, name: &str, source: &str) -> PathBuf {
    let path = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&path, source).unwrap();
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

/// A program that has started, with its output read line by line.
struct Running {
    child: Child,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
}

impl Running {
    fn start(executable: &Path) -> Self {
        let mut child = Command::new(executable)
            .stdin(Stdio::piped())
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

/// A worker that takes `job_ms` per job and prints each job it finishes;
/// main queues three jobs, prints `ready`, and waits on the root.
fn service(job_ms: u64, shutdown_within_s: u64) -> String {
    format!(
        r#"import std/clock as Clock
import std/clock.{{Clock}}
import std/duration as Duration
import std/int as Int
import std/io.{{Console}}
import std/process as Process
import std/process.{{Inbox}}
import std/supervisor as Supervisor
import std/text as Text

pub enum Job {{
  Work(id: Int)
}}

pub process name worker: Job

fn work(inbox: Inbox<Job>) -> Void needs {{ clock: Clock, console: Console }} {{
  loop done = 0 {{
    case Process.receive(inbox) {{
      Job.Work(id) -> {{
        Clock.sleep(Duration.milliseconds({job_ms}))
        console.write_line("finished " |> Text.append(Int.to_text(id)))
        done + 1
      }}
    }}
  }}
}}

pub fn main() -> Void needs {{ clock: Clock, console: Console }} {{
  let spec = Process.spec(work, 4, Duration.seconds({shutdown_within_s}))
    |> Process.register(worker)
    |> Supervisor.process(Supervisor.Restart.Never)
  let root = Supervisor.start(Supervisor.policy([spec], 1, Supervisor.restarts(0, Duration.seconds(30))))
  Process.send(worker, Job.Work(1))
  Process.send(worker, Job.Work(2))
  Process.send(worker, Job.Work(3))
  console.write_line("ready")
  Supervisor.wait(root)
}}
"#
    )
}

#[test]
fn sigterm_lets_the_worker_drain_its_jobs_then_ends_the_program_by_sigterm() {
    let scratch = scratch_directory("drain");
    let executable = build_native(&scratch, "service", &service(300, 30));
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGTERM);
    let (rest, status) = running.finish();
    // Whatever job was in flight and the ones queued behind it all finish.
    assert_eq!(rest.last().map(String::as_str), Some("finished 3"));
    assert_eq!(status.signal(), Some(SIGTERM), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn an_idle_service_waits_for_a_signal_instead_of_reporting_a_deadlock() {
    let scratch = scratch_directory("idle");
    let executable = build_native(&scratch, "service", &service(1, 30));
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    for job in 1..=3 {
        assert_eq!(running.next_line(), format!("finished {job}"));
    }
    // Every task is now parked with nothing pending. Before G22 the runner
    // reported that as a deadlock (exit 70); a service waits instead. The
    // pause only makes it likely the stall has begun; the test passes either
    // way, and a deadlock report would end the program before the signal.
    std::thread::sleep(Duration::from_millis(200));
    running.signal(SIGINT);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(status.signal(), Some(SIGINT), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn a_second_signal_ends_the_program_at_once() {
    let scratch = scratch_directory("second");
    // A one-minute job under a one-minute deadline: the first signal would
    // wait for it.
    let executable = build_native(&scratch, "service", &service(60_000, 60));
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGTERM);
    running.signal(SIGINT);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "no job finished: {rest:?}");
    assert!(
        matches!(status.signal(), Some(SIGTERM | SIGINT)),
        "ended by a signal: {status}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn sigint_cancels_a_program_without_the_scheduler_while_it_reads_input() {
    let scratch = scratch_directory("reader");
    let executable = build_native(
        &scratch,
        "reader",
        r#"import std/io as IO
import std/io.{Console, FileSystem}

pub fn main() -> Int needs { console: Console, files: FileSystem } {
  console.write_line("reading")
  case files.process_utf8_lines(IO.path("-"), 0, fn(count: Int, line: Text) { count + 1 }) {
    Success(count) -> count
    Failure(_) -> -1
  }
}
"#,
    );
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "reading");
    // Standard input stays open, so the read blocks until the signal.
    running.signal(SIGINT);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(status.signal(), Some(SIGINT), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A scheduled program (it starts a task) whose main reads standard input,
/// while a ticker task sleeps and prints `ticked`.
const SCHEDULED_READER: &str = r#"import std/clock as Clock
import std/clock.{Clock}
import std/duration as Duration
import std/int as Int
import std/io as IO
import std/io.{Console, FileSystem}
import std/task as Task

fn tick() -> Void needs { clock: Clock, console: Console } {
  Clock.sleep(Duration.milliseconds(1))
  console.write_line("ticked")
}

pub fn main() -> Int needs { clock: Clock, console: Console, files: FileSystem } {
  let ticker = Task.start(tick)
  console.write_line("reading")
  let count = case files.process_utf8_lines(IO.path("-"), 0, fn(count: Int, line: Text) { count + 1 }) {
    Success(count) -> count
    Failure(_) -> -1
  }
  Task.result(ticker)
  console.write_line(Int.to_text(count))
  count
}
"#;

#[test]
fn a_task_keeps_running_while_main_waits_for_standard_input() {
    use std::io::Write;
    let scratch = scratch_directory("scheduled-reader");
    let executable = build_native(&scratch, "reader", SCHEDULED_READER);
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "reading");
    // Nothing has been written yet, so main is waiting on its read. Before
    // the reactor that read blocked the whole scheduler, and the ticker
    // could not print until input ended.
    assert_eq!(running.next_line(), "ticked");
    let mut input = running.child.stdin.take().unwrap();
    input.write_all(b"one\ntwo\nthree\n").unwrap();
    drop(input);
    let (rest, status) = running.finish();
    assert_eq!(rest, ["3"]);
    assert!(status.success(), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn the_first_sigint_cancels_a_scheduled_program_while_it_reads_input() {
    let scratch = scratch_directory("scheduled-reader-signal");
    let executable = build_native(&scratch, "reader", SCHEDULED_READER);
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "reading");
    assert_eq!(running.next_line(), "ticked");
    // Standard input stays open. Before the reactor only a second signal
    // ended this read.
    running.signal(SIGINT);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(status.signal(), Some(SIGINT), "{status}");
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn the_first_signal_runs_main_s_release_and_a_second_cuts_it_short() {
    // main has no `shutdown_within`, so its releases have no deadline and
    // only the second signal ends a slow one (application-lifecycle.md,
    // "Signals").
    let scratch = scratch_directory("main-release");
    let executable = build_native(
        &scratch,
        "service",
        r#"import std/clock as Clock
import std/clock.{Clock}
import std/duration as Duration
import std/io.{Console}
import std/resource as Resource
import std/task as Task

fn idle() -> Void needs { clock: Clock } {
  Clock.sleep(Duration.seconds(60))
}

pub fn main() -> Int needs { clock: Clock, console: Console } {
  Resource.scoped(
    fn() { Task.start(idle) },
    fn(held) {
      console.write_line("releasing")
      Clock.sleep(Duration.seconds(60))
      console.write_line("released")
    },
    fn(held) {
      console.write_line("ready")
      Clock.sleep(Duration.seconds(60))
      3
    },
  )
}
"#,
    );
    let mut running = Running::start(&executable);
    assert_eq!(running.next_line(), "ready");
    running.signal(SIGTERM);
    assert_eq!(running.next_line(), "releasing");
    running.signal(SIGINT);
    let (rest, status) = running.finish();
    assert!(rest.is_empty(), "the release was cut short: {rest:?}");
    assert!(
        matches!(status.signal(), Some(SIGTERM | SIGINT)),
        "ended by a signal: {status}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
