//! Console writes to a pipe nobody is reading (CHI-266). A full pipe parks the
//! writing task in the IO reactor instead of stopping every task, and lines
//! from different tasks never interleave.
//!
//! No assertion depends on how long anything takes: each test waits for the
//! output it needs, and a generous limit only stops a hung program from
//! hanging the suite.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(60);

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("chimi-console-{name}-{}", std::process::id()));
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

/// Two writers each print 100 lines of 3000 bytes (about 600 KB, far more than
/// a pipe holds), while a third task prints `ticked` on standard error once
/// both have started.
const FLOOD: &str = r#"import std/clock as Clock
import std/clock.{Clock}
import std/duration as Duration
import std/io.{Console}
import std/task as Task
import std/text as Text

fn flood(letter: Text) -> Void needs { console: Console } {
  let line = Text.repeat(letter, 3000)
  loop count = 0 {
    case count == 100 {
      True -> break
      False -> {
        console.write_line(line)
        count + 1
      }
    }
  }
}

fn tick() -> Void needs { clock: Clock, console: Console } {
  Clock.sleep(Duration.milliseconds(200))
  console.write_error("ticked")
}

pub fn main() -> Int needs { clock: Clock, console: Console } {
  let a = Task.start(fn() { flood("a") })
  let b = Task.start(fn() { flood("b") })
  let ticker = Task.start(tick)
  Task.result(ticker)
  Task.result(a)
  Task.result(b)
  0
}
"#;

#[test]
fn a_task_keeps_running_while_another_waits_on_a_full_stdout_pipe() {
    let scratch = scratch_directory("flood");
    let executable = build_native(&scratch, "flood", FLOOD);
    let mut child = Command::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start native program");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Standard output is not read yet, so both writers fill the pipe and wait.
    // Before CHI-266 their write blocked the whole scheduler, and the ticker
    // could not print until standard output was drained.
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stderr).lines();
        let _ = sender.send(lines.next().map(Result::unwrap));
        let rest: Vec<String> = lines.map(Result::unwrap).collect();
        let _ = sender.send(Some(rest.join("\n")));
    });
    let first = received.recv_timeout(LIMIT);
    if first.is_err() {
        let _ = child.kill();
    }
    assert_eq!(
        first.expect("the ticker printed while stdout was full"),
        Some("ticked".to_owned())
    );

    let mut output = String::new();
    BufReader::new(stdout).read_to_string(&mut output).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert_eq!(received.recv_timeout(LIMIT).unwrap(), Some(String::new()));

    // Every line is one writer's whole line.
    let a = "a".repeat(3000);
    let b = "b".repeat(3000);
    let lines: Vec<&str> = output.lines().collect();
    assert!(lines.iter().all(|line| *line == a || *line == b), "a line interleaved");
    assert_eq!(lines.iter().filter(|line| **line == a).count(), 100);
    assert_eq!(lines.iter().filter(|line| **line == b).count(), 100);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A line longer than a pipe holds arrives whole and in order, after the line
/// before it.
#[test]
fn a_line_longer_than_the_pipe_is_written_whole() {
    let scratch = scratch_directory("long-line");
    let executable = build_native(
        &scratch,
        "long",
        r#"import std/io.{Console}
import std/text as Text

pub fn main() -> Int needs { console: Console } {
  console.write_line("first")
  console.write_line(Text.repeat("x", 300000))
  console.write_line("last")
  0
}
"#,
    );
    let output = Command::new(&executable)
        .stdin(Stdio::null())
        .output()
        .expect("run native program");
    assert!(output.status.success(), "{}", output.status);
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "first");
    assert_eq!(lines[1], "x".repeat(300000));
    assert_eq!(lines[2], "last");
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A writer cancelled while its line waits on a full pipe: the line was
/// already accepted, so it still arrives whole, written at the program's end.
#[test]
fn a_line_whose_writer_timed_out_is_still_written_whole() {
    let scratch = scratch_directory("timed-out");
    let executable = build_native(
        &scratch,
        "timed",
        r#"import std/debug as Debug
import std/duration as Duration
import std/io.{Console}
import std/result as Result
import std/task as Task
import std/text as Text

pub fn main() -> Int needs { console: Console } {
  let line = Text.repeat("x", 300000)
  let outcome = Result.capture(fn() {
    Task.timeout(fn() { console.write_line(line) }, Duration.milliseconds(100))
  })
  console.write_error(Debug.render(outcome))
  0
}
"#,
    );
    let mut child = Command::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start native program");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stderr).lines();
        let _ = sender.send(lines.next().map(Result::unwrap));
    });
    let first = received.recv_timeout(LIMIT);
    if first.is_err() {
        let _ = child.kill();
    }
    assert_eq!(
        first.expect("the timeout ended the write while stdout was full"),
        Some("Result.Err(Timeout.TimedOut(100ms))".to_owned())
    );
    let mut output = String::new();
    BufReader::new(stdout).read_to_string(&mut output).unwrap();
    assert!(child.wait().unwrap().success());
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines, ["x".repeat(300000)]);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// With `2>&1` standard output and standard error are one pipe. A task
/// writing each stream still never splits the other's lines, since each
/// write after poll takes at most `PIPE_BUF` bytes of a shared queue.
#[test]
fn stdout_and_stderr_on_one_pipe_keep_their_lines_whole() {
    let scratch = scratch_directory("merged");
    let executable = build_native(
        &scratch,
        "merged",
        r#"import std/io.{Console}
import std/task as Task
import std/text as Text

fn flood(error: Bool) -> Void needs { console: Console } {
  let line = case error {
    True -> Text.repeat("e", 3000)
    False -> Text.repeat("o", 3000)
  }
  loop count = 0 {
    case count == 100 {
      True -> break
      False -> {
        case error {
          True -> console.write_error(line)
          False -> console.write_line(line)
        }
        count + 1
      }
    }
  }
}

pub fn main() -> Int needs { console: Console } {
  let out = Task.start(fn() { flood(False) })
  let err = Task.start(fn() { flood(True) })
  Task.result(out)
  Task.result(err)
  0
}
"#,
    );
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("exec \"$0\" 2>&1")
        .arg(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start native program");
    // Let both writers fill the pipe before reading, so they share it while
    // it is full. Nothing below depends on how long this takes.
    std::thread::sleep(Duration::from_millis(200));
    let mut output = String::new();
    child.stdout.take().unwrap().read_to_string(&mut output).unwrap();
    assert!(child.wait().unwrap().success());
    let out = "o".repeat(3000);
    let err = "e".repeat(3000);
    let lines: Vec<&str> = output.lines().collect();
    assert!(lines.iter().all(|line| *line == out || *line == err), "a line was split");
    assert_eq!(lines.len(), 200);
    std::fs::remove_dir_all(scratch).unwrap();
}
