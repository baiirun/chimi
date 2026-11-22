//! Conformance for `std/log` (Phase 7c, CHI-289): the entry a level function
//! builds, fakes through `using`, and the runner's provider, which writes one
//! JSON line per entry to stderr or `--log-file` and filters by `--log-level`.
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI there.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-log-{name}-{}-{:?}",
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
        run(Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments))
    }

    /// Builds `cases.chimi` with the native backend and returns the binary.
    fn build(&self) -> PathBuf {
        let built = self.chimi(&["build", "cases.chimi", "-o", "program"]);
        assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
        self.0.join("program")
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

fn run(command: &mut Command) -> Run {
    let output = command.output().expect("run the command");
    Run {
        status: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn runner(program: &Path, options: &[&str]) -> Run {
    run(Command::new(program)
        .arg("--chimi-runner")
        .args(options)
        .arg("--"))
}

/// The line with its `time` member checked for shape and removed, so the
/// rest compares exactly.
fn without_time(line: &str) -> String {
    let rest = line
        .strip_prefix("{\"time\":\"")
        .unwrap_or_else(|| panic!("a log line starts with its time: {line}"));
    let (time, rest) = rest.split_at(24);
    let shape = time.bytes().enumerate().all(|(index, byte)| match index {
        4 | 7 => byte == b'-',
        10 => byte == b'T',
        13 | 16 => byte == b':',
        19 => byte == b'.',
        23 => byte == b'Z',
        _ => byte.is_ascii_digit(),
    });
    assert!(shape, "an RFC 3339 UTC time with milliseconds: {time}");
    format!("{{{}", rest.strip_prefix("\",").expect("the time is a JSON string"))
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(without_time).collect()
}

const PROGRAM: &str = r#"import std/io.{Console}
import std/json as Json
import std/log as Log

pub fn main() -> Void needs { console: Console, log: Log.Log } {
  Log.debug("checking", [])
  Log.info("started", [])
  console.write_error("plain stderr")
  Log.warn("slow charge", [
    Log.int("order", 42),
    Log.float("seconds", 1.5),
    Log.text("card", "4\"2"),
    Log.bool("retried", True),
    Log.json("tags", Json.array([Json.text("a")])),
    Log.int("order", 43),
  ])
  Log.error("failed", [Log.text("why", "line\nbreak")])
}
"#;

const STARTED: &str = r#"{"level":"info","message":"started","fields":{}}"#;
const SLOW: &str = r#"{"level":"warn","message":"slow charge","fields":{"card":"4\"2","order":43,"retried":true,"seconds":1.5,"tags":["a"]}}"#;
const FAILED: &str = r#"{"level":"error","message":"failed","fields":{"why":"line\nbreak"}}"#;

// Rejects a provider that writes anywhere but stderr, writes debug entries by
// default, reorders log lines against the program's own stderr lines, drops a
// field, keeps the first of two values under one key, or escapes text badly.
#[test]
fn the_runner_writes_one_json_line_per_entry_to_stderr() {
    let scratch = Scratch::new("stderr", PROGRAM);
    let program = scratch.build();
    let output = runner(&program, &[]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "");
    let mut stderr = output.stderr.lines();
    assert_eq!(without_time(stderr.next().unwrap()), STARTED);
    assert_eq!(stderr.next(), Some("plain stderr"));
    let rest: Vec<String> = stderr.map(without_time).collect();
    assert_eq!(rest, [SLOW, FAILED]);
}

// Rejects a level filter that is ignored, inverted, or applied only to some
// levels, and a bad level that runs the program anyway.
#[test]
fn the_log_level_option_filters_entries() {
    let scratch = Scratch::new("level", PROGRAM);
    let program = scratch.build();
    let debug = runner(&program, &["--log-level=debug"]);
    assert_eq!(
        lines(&debug.stderr.replace("plain stderr\n", "")),
        [r#"{"level":"debug","message":"checking","fields":{}}"#, STARTED, SLOW, FAILED]
    );
    let error = runner(&program, &["--log-level=error"]);
    assert_eq!(lines(&error.stderr.replace("plain stderr\n", "")), [FAILED]);
    let warn = runner(&program, &["--log-level=warn"]);
    assert_eq!(lines(&warn.stderr.replace("plain stderr\n", "")), [SLOW, FAILED]);
    let bad = runner(&program, &["--log-level=loud"]);
    assert_eq!(bad.status, Some(64));
    assert_eq!(bad.stderr, "--log-level requires debug, info, warn or error\n");
}

// Rejects a log file that is truncated by a second run, that also gets the
// program's own stderr lines, or that may share a path with a runner report.
#[test]
fn the_log_file_option_appends_lines_there() {
    let scratch = Scratch::new("file", PROGRAM);
    let program = scratch.build();
    let file = scratch.0.join("app.log");
    let option = format!("--log-file={}", file.display());
    for _ in 0..2 {
        let output = runner(&program, &[&option]);
        assert_eq!(output.status, Some(0), "{}", output.stderr);
        assert_eq!(output.stderr, "plain stderr\n");
    }
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(lines(&written), [STARTED, SLOW, FAILED, STARTED, SLOW, FAILED]);
    let outcome = format!("--outcome-file={}", file.display());
    let clash = runner(&program, &[&option, &outcome]);
    assert_eq!(clash.status, Some(64));
    assert_eq!(clash.stderr, "the log file must be distinct from the runner's report files\n");
    let fresh = format!("--log-file={}/fresh.log", scratch.0.display());
    let respelled = format!("--outcome-file={}/./fresh.log", scratch.0.display());
    let clash = runner(&program, &[&fresh, &respelled]);
    assert_eq!(clash.status, Some(64), "the same file under another spelling");
    let missing = runner(&program, &["--log-file=/nonexistent/dir/app.log"]);
    assert_eq!(missing.status, Some(74));
}

const SLOW_READER: &str = r#"import std/clock.{Clock}
import std/duration as Duration
import std/io.{Console}
import std/list as List
import std/log as Log
import std/task as Task
import std/text as Text

pub fn main() -> Void needs { console: Console, clock: Clock, log: Log.Log } {
  let line = Text.repeat("y", 10000)
  let flood = fn() {
    List.each(List.repeat(0, 100000), fn(_) { Log.info(line, []) })
    "finished"
  }
  let ended = case Task.timeout(flood, Duration.milliseconds(300)) {
    Success(ended) -> ended
    Failure(_) -> "timed out"
  }
  console.write_line(ended)
}
"#;

// Rejects a log file written with blocking writes: a reader that falls behind
// must not stall the program or keep a timeout from firing.
#[test]
fn a_slow_log_file_reader_does_not_block_the_program() {
    let scratch = Scratch::new("slow-reader", SLOW_READER);
    let program = scratch.build();
    let fifo = scratch.0.join("log.fifo");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    let mut reader = Command::new("sh")
        .arg("-c")
        // Opens the pipe at once, then reads nothing for two seconds.
        .arg("exec 3<\"$0\"; sleep 2; cat <&3 > /dev/null")
        .arg(&fifo)
        .spawn()
        .unwrap();
    let option = format!("--log-file={}", fifo.display());
    let output = runner(&program, &["--clock=real", &option]);
    reader.wait().unwrap();
    assert_eq!(output.stdout, "timed out\n", "{}", output.stderr);
}

const TESTS: &str = r#"import std/json as Json
import std/list as List
import std/log as Log.{Entry, Level, Log}
import std/test as Test

fn kept(recorder: Test.Recorder<Entry>) {
  { write: fn(entry) { Test.record(recorder, entry) } }
}

fn charge(order: Int) -> Int needs { log: Log } {
  Log.info("charging", [Log.int("order", order)])
  order * 2
}

test "a fake keeps each entry with its level, message and fields" {
  let recorder = Test.recorder()
  let total = charge(21) using { log: kept(recorder) }
  Log.warn("late", [Log.text("by", "2s"), Log.bool("again", False)]) using { log: kept(recorder) }
  total |> Test.assert_equal(42)
  let entries = Test.recorded(recorder)
  List.map(entries, Log.level) |> Test.assert_equal([Level.Info, Level.Warn])
  List.map(entries, Log.message) |> Test.assert_equal(["charging", "late"])
  List.map(entries, Log.fields)
    |> Test.assert_equal([[("order", Json.int(21))], [("by", Json.text("2s")), ("again", Json.boolean(False))]])
}

test "the fields object keeps the last value of a key" {
  let recorder = Test.recorder()
  Log.debug("x", [Log.int("a", 1), Log.float("b", 0.25), Log.int("a", 2)]) using { log: kept(recorder) }
  let entries = Test.recorded(recorder)
  List.map(entries, fn(entry) { Json.encode(Log.fields_object(entry)) })
    |> Test.assert_equal(["{\"a\":2,\"b\":0.25}"])
}
"#;

// Rejects level functions that build the wrong level, reorder or drop fields,
// or bypass the provider `using` supplies; on both backends.
#[test]
fn a_test_records_entries_through_a_fake() {
    let scratch = Scratch::new("fake", TESTS);
    for backend in [&[][..], &["--backend=native"][..]] {
        let mut arguments = vec!["test", "cases.chimi"];
        arguments.extend_from_slice(backend);
        let output = scratch.chimi(&arguments);
        assert_eq!(output.status, Some(0), "{}{}", output.stdout, output.stderr);
        assert!(output.stdout.contains("2 passed, 0 failed"), "{}", output.stdout);
    }
}

// Rejects a test that reaches the runner's provider: a test never logs to the
// machine by accident.
#[test]
fn a_test_without_a_log_provider_is_rejected() {
    let scratch = Scratch::new(
        "no-provider",
        "import std/log as Log\n\ntest \"logs\" {\n  Log.info(\"x\", [])\n}\n",
    );
    let output = scratch.chimi(&["test", "cases.chimi"]);
    assert_ne!(output.status, Some(0));
    assert!(output.stderr.contains("T4400") || output.stdout.contains("T4400"), "{}{}", output.stdout, output.stderr);
}

// The interpreter's provider writes the same lines, into the report's stderr,
// and takes the same level and file settings.
#[test]
fn the_interpreter_writes_the_same_lines() {
    use chimi::execution::{ExecutionConfig, TraceMode};
    use chimi::source::SourceSnapshot;
    use chimi::{Compiler, ProjectSource};
    let run = |config: ExecutionConfig| {
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", PROGRAM))
            .expect("valid module path");
        Compiler::run_project(vec![module], config.with_trace_mode(TraceMode::Off))
            .expect("the program compiles")
    };
    let config = || ExecutionConfig::new("log").unwrap();
    let report = run(config());
    assert_eq!(
        lines(&report.application_stderr.replace("plain stderr\n", "")),
        [STARTED, SLOW, FAILED]
    );
    let report = run(config().with_log_level("info").unwrap());
    assert_eq!(
        lines(&report.application_stderr.replace("plain stderr\n", "")),
        [STARTED, SLOW, FAILED]
    );
    let report = run(config().with_log_level("error").unwrap());
    assert_eq!(lines(&report.application_stderr.replace("plain stderr\n", "")), [FAILED]);
    assert!(config().with_log_level("loud").is_err());
    let scratch = Scratch::new("interpreter-file", "");
    let file = scratch.0.join("app.log");
    let report = run(config().with_log_file(file.to_string_lossy()));
    assert_eq!(report.application_stderr, "plain stderr\n");
    assert_eq!(lines(&std::fs::read_to_string(&file).unwrap()), [STARTED, SLOW, FAILED]);
}
