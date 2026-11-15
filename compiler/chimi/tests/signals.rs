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
    // reported that as a deadlock (exit 70); a service waits instead.
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
