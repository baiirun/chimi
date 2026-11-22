//! Conformance for byte streams (`std/stream.chimi`, G18) under the native
//! scheduler. Pipes and produced streams are built on channels and tasks,
//! which only the native backend runs, so these programs are compiled and run
//! under the virtual clock, and each renders everything it checks into one
//! value.

use std::path::PathBuf;
use std::process::Command;

const PRELUDE: &str = r#"import std/bytes as Bytes
import std/bytes.{Bytes}
import std/channel as Channel
import std/channel.{Receive}
import std/clock as Clock
import std/debug as Debug
import std/duration as Duration
import std/resource as Resource
import std/result as Result
import std/stream as Stream
import std/stream.{StreamError}
import std/task as Task
import std/list as List

fn text(bytes: Bytes) -> Text {
  Bytes.to_text_lossy(bytes)
}

"#;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-stream-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Builds `body` after the prelude, runs it under the virtual clock, and
/// returns its outcome line.
fn run_virtual(name: &str, body: &str) -> String {
    let scratch = scratch_directory(name);
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, format!("{PRELUDE}{body}")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native stream program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stdout)
    );
    let outcome_path = scratch.join("outcome.jsonl");
    let process = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--")
        .output()
        .expect("execute native stream program");
    let outcome = std::fs::read_to_string(outcome_path).unwrap_or_default();
    std::fs::remove_dir_all(scratch).unwrap();
    assert!(
        process.status.success(),
        "{outcome}{}",
        String::from_utf8_lossy(&process.stderr)
    );
    outcome
}

fn rendered(value: &str) -> String {
    format!(
        "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":\"{}\"}}\n",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

// Rejects a pipe that reorders or drops chunks, hands out an empty chunk, or keeps going after close: two chunks come back in order, then `None` for good.
#[test]
fn a_pipe_gives_chunks_in_order_and_skips_empty_writes() {
    let outcome = run_virtual(
        "a_pipe_gives_chunks_in_order_and_skips_empty_writes",
        r#"pub fn main() -> Text {
  let (writer, reader) = Stream.pipe(4)
  let read = Result.capture(fn() {
    Stream.write(writer, Bytes.from_text("ab"))
    Stream.write(writer, Bytes.empty())
    Stream.write(writer, Bytes.from_text("cd"))
    Stream.close(writer)
    [Stream.read(reader), Stream.read(reader), Stream.read(reader), Stream.read(reader)]
  })
  Debug.render(read)
}
"#,
    );
    assert_eq!(
        outcome,
        rendered(r#"Result.Ok([Some(Bytes(61 62)), Some(Bytes(63 64)), None, None])"#)
    );
}

// Rejects a `read_all` without a limit: a produced stream joins to its text, six bytes against a limit of five fail with `TooLarge(5)`, and exactly five bytes pass.
#[test]
fn read_all_joins_the_chunks_and_stops_past_the_limit() {
    let outcome = run_virtual(
        "read_all_joins_the_chunks_and_stops_past_the_limit",
        r#"pub fn main() -> Text {
  let body = Stream.produce(2, fn(out) {
    Stream.write(out, Bytes.from_text("hello, "))
    Stream.write(out, Bytes.from_text("world"))
  })
  let small = Stream.produce(2, fn(out) {
    Stream.write(out, Bytes.from_text("123"))
    Stream.write(out, Bytes.from_text("456"))
  })
  Debug.render([
    Result.capture(fn() { text(Stream.read_all(body, 64)) }),
    Result.capture(fn() { text(Stream.read_all(small, 5)) }),
    Result.capture(fn() { text(Stream.read_all(Stream.from_bytes(Bytes.from_text("12345")), 5)) }),
  ])
}
"#,
    );
    assert_eq!(
        outcome,
        rendered(r#"[Result.Ok("hello, world"), Result.Err(StreamError.TooLarge(5)), Result.Ok("12345")]"#)
    );
}

// Rejects a producer failure that the reader never sees, and a stop that leaves the producer writing: the reader fails with the body's own `Broke`, and after `stop` the endless producer's write fails with `Closed` (logged 1) and a later read fails with `Aborted`.
#[test]
fn a_producer_failure_reaches_the_reader_and_stop_closes_the_producer() {
    let outcome = run_virtual(
        "a_producer_failure_reaches_the_reader_and_stop_closes_the_producer",
        r#"pub enum Job {
  Broke
}

fn log_closed(log: Channel.Sender<Int>, outcome: Result.Result<Void, StreamError>) -> Void {
  case outcome {
    Result.Result.Err(StreamError.Closed) -> {
      let _ = Result.capture(fn() { Channel.send(log, 1) })
      Void
    }
    _ -> Void
  }
}

pub fn main() -> Text needs { clock: Clock.Clock } {
  let failing = Stream.produce(2, fn(out) {
    Stream.write(out, Bytes.from_text("a"))
    Failure(Job.Broke)
  })
  let made = Channel.bounded(1)
  let log = made.sender
  let endless = Stream.produce(1, fn(out) {
    let outcome = Result.capture(fn() {
      loop count = 0 {
        Stream.write(out, Bytes.from_text("x"))
        count + 1
      }
    })
    log_closed(log, outcome)
  })
  let first = Result.capture(fn() { Stream.read(endless) })
  Stream.stop(endless)
  let after = Result.capture(fn() { Stream.read(endless) })
  let got = Channel.receive(made.receiver)
  let closed = case got {
    Receive.Received(value) -> value
    Receive.Drained -> 0
  }
  Debug.render((Result.capture(fn() { Stream.read_all(failing, 64) }), first, after, closed))
}
"#,
    );
    assert_eq!(
        outcome,
        rendered(
            r#"(Result.Err(Job.Broke), Result.Ok(Some(Bytes(78))), Result.Err(StreamError.Aborted), 1)"#
        )
    );
}

// Rejects a stream without backpressure: with capacity 1 the producer's first write lands at once, and the rest wait until the reader starts reading after 5 s.
#[test]
fn a_produced_stream_waits_while_the_reader_is_behind() {
    let outcome = run_virtual(
        "a_produced_stream_waits_while_the_reader_is_behind",
        r#"fn record(log: Channel.Sender<Int>, start: Clock.Instant) -> Void needs { clock: Clock.Clock } {
  let _ = Result.capture(fn() { Channel.send(log, Duration.to_seconds(Clock.since(start))) })
  Void
}

pub fn main() -> Text needs { clock: Clock.Clock } {
  let start = Clock.now()
  let made = Channel.bounded(8)
  let log = made.sender
  let chunks = Stream.produce(1, fn(out) {
    Stream.write(out, Bytes.from_text("a"))
    record(log, start)
    Stream.write(out, Bytes.from_text("b"))
    record(log, start)
    Stream.write(out, Bytes.from_text("c"))
    record(log, start)
  })
  Clock.sleep(Duration.seconds(5))
  let all = Result.capture(fn() { text(Stream.read_all(chunks, 10)) })
  let times = loop times = [] {
    case List.length(times) == 3 {
      True -> break times
      False -> {
        let got = Channel.receive(made.receiver)
        case got {
          Receive.Received(seconds) -> List.push(times, seconds)
          Receive.Drained -> break times
        }
      }
    }
  }
  Debug.render((all, times))
}
"#,
    );
    assert_eq!(outcome, rendered(r#"(Result.Ok("abc"), [0, 5, 5])"#));
}

// Rejects a `copy` that loses bytes or swallows the writer's failure: five bytes copy through a pipe, empty `from_bytes` gives no chunk, a refusing writer's `Full` comes back, and an empty reader copies 0.
#[test]
fn copy_from_bytes_and_host_fakes() {
    let outcome = run_virtual(
        "copy_from_bytes_and_host_fakes",
        r#"pub enum Sink {
  Full
}

pub fn main() -> Text {
  let (writer, reader) = Stream.pipe(4)
  let copied = Result.capture(fn() {
    let count = Stream.copy(Stream.from_bytes(Bytes.from_text("hello")), writer)
    Stream.close(writer)
    (count, text(Stream.read_all(reader, 64)))
  })
  let empty = Result.capture(fn() { Stream.read(Stream.from_bytes(Bytes.empty())) })
  let refusing = Stream.writer(fn(_bytes) { Failure(Sink.Full) }, fn() { Void })
  let nothing = Stream.reader(fn() { None }, fn() { Void })
  Debug.render((
    copied,
    empty,
    Result.capture(fn() { Stream.copy(Stream.from_bytes(Bytes.from_text("z")), refusing) }),
    Result.capture(fn() { Stream.copy(nothing, refusing) }),
  ))
}
"#,
    );
    assert_eq!(
        outcome,
        rendered(
            r#"(Result.Ok((5, "hello")), Result.Ok(None), Result.Err(Sink.Full), Result.Ok(0))"#
        )
    );
}

// Rejects a scoped stream left running after its use: `Stream.stop` as the release makes the producer's next write fail with `Closed`.
#[test]
fn resource_scoped_with_stop_as_the_release_closes_the_producer() {
    let outcome = run_virtual(
        "resource_scoped_with_stop_as_the_release_closes_the_producer",
        r#"fn log_closed(log: Channel.Sender<Int>, outcome: Result.Result<Void, StreamError>) -> Void {
  case outcome {
    Result.Result.Err(StreamError.Closed) -> {
      let _ = Result.capture(fn() { Channel.send(log, 1) })
      Void
    }
    _ -> Void
  }
}

pub fn main() -> Text {
  let made = Channel.bounded(1)
  let log = made.sender
  let first = Result.capture(fn() {
    Resource.scoped(
      fn() {
        Stream.produce(1, fn(out) {
          let outcome = Result.capture(fn() {
            loop count = 0 {
              Stream.write(out, Bytes.from_text("x"))
              count + 1
            }
          })
          log_closed(log, outcome)
        })
      },
      Stream.stop,
      fn(body) { Stream.read(body) },
    )
  })
  let got = Channel.receive(made.receiver)
  let closed = case got {
    Receive.Received(value) -> value
    Receive.Drained -> 0
  }
  Debug.render((first, closed))
}
"#,
    );
    assert_eq!(outcome, rendered(r#"(Result.Ok(Some(Bytes(78))), 1)"#));
}

// Rejects a reader that treats a vanished writer as a clean end: the writer's task writes a chunk and ends without closing, and the read fails with `Aborted` (a bare pipe does not keep the unread chunk).
#[test]
fn a_pipe_whose_writer_task_ends_without_closing_aborts() {
    let outcome = run_virtual(
        "a_pipe_whose_writer_task_ends_without_closing_aborts",
        r#"pub fn main() -> Text {
  let (writer, reader) = Stream.pipe(4)
  let child = Task.start(fn() {
    let _ = Result.capture(fn() { Stream.write(writer, Bytes.from_text("a")) })
    Void
  })
  Task.result(child)
  Debug.render(Result.capture(fn() { Stream.read(reader) }))
}
"#,
    );
    assert_eq!(outcome, rendered(r#"Result.Err(StreamError.Aborted)"#));
}

// Rejects a producer failure that overtakes the chunks written before it, and streams that reopen: the reader sees `a`, `b`, `Broke`, then the end; a finished stream reads `None` again; a write after `close` fails with `Closed`; and stopping twice is harmless.
#[test]
fn a_producer_failure_comes_after_its_chunks_and_closed_streams_stay_closed() {
    let outcome = run_virtual("a_producer_failure_comes_after_its_chunks_and_closed_streams_stay_closed", r#"pub enum Job {
  Broke
}

pub fn main() -> Text needs { clock: Clock.Clock } {
  let failing = Stream.produce(4, fn(out) {
    Stream.write(out, Bytes.from_text("a"))
    Stream.write(out, Bytes.from_text("b"))
    Failure(Job.Broke)
  })
  Clock.sleep(Duration.seconds(1))
  let reads = [
    Result.capture(fn() { Stream.read(failing) }),
    Result.capture(fn() { Stream.read(failing) }),
    Result.capture(fn() { Stream.read(failing) }),
    Result.capture(fn() { Stream.read(failing) }),
  ]
  let done = Stream.produce(1, fn(out) { Stream.write(out, Bytes.from_text("z")) })
  let whole = Result.capture(fn() { text(Stream.read_all(done, 8)) })
  let after = Result.capture(fn() { Stream.read(done) })
  let (writer, reader) = Stream.pipe(2)
  let closed = Result.capture(fn() {
    Stream.close(writer)
    Stream.write(writer, Bytes.from_text("late"))
  })
  Stream.stop(reader)
  Stream.stop(reader)
  Debug.render((reads, whole, after, closed, Result.capture(fn() { Stream.read(reader) })))
}
"#);
    assert_eq!(outcome, rendered(r#"([Result.Ok(Some(Bytes(61))), Result.Ok(Some(Bytes(62))), Result.Err(Job.Broke), Result.Ok(None)], Result.Ok("z"), Result.Ok(None), Result.Err(StreamError.Closed), Result.Err(StreamError.Aborted))"#));
}

// Rejects a host reader that hands out an empty chunk: of empty, `x`, empty, only `x` comes back before the end.
#[test]
fn a_host_reader_skips_empty_chunks() {
    let outcome = run_virtual("a_host_reader_skips_empty_chunks", r#"pub fn main() -> Text {
  let made = Channel.bounded(4)
  let feed = made.sender
  let chunks = made.receiver
  let filled = Result.capture(fn() {
    Channel.send(feed, Bytes.empty())
    Channel.send(feed, Bytes.from_text("x"))
    Channel.send(feed, Bytes.empty())
  })
  Channel.close(made.completion)
  let host = Stream.reader(fn() {
    let got = Channel.receive(chunks)
    case got {
      Receive.Received(bytes) -> Some(bytes)
      Receive.Drained -> None
    }
  }, fn() { Void })
  Debug.render((filled, Result.capture(fn() { [Stream.read(host), Stream.read(host)] })))
}
"#);
    assert_eq!(outcome, rendered(r#"(Result.Ok(Void), Result.Ok([Some(Bytes(78)), None]))"#));
}

// Rejects a `read_all` that leaves its reader running: past the limit of 3 bytes it fails with `TooLarge(3)`, and the endless producer's next write fails with `Closed` (logged 1) instead of parking forever.
#[test]
fn read_all_stops_an_endless_producer_past_the_limit() {
    let outcome = run_virtual("read_all_stops_an_endless_producer_past_the_limit", r#"fn log_closed(log: Channel.Sender<Int>, outcome: Result.Result<Void, StreamError>) -> Void {
  case outcome {
    Result.Result.Err(StreamError.Closed) -> {
      let _ = Result.capture(fn() { Channel.send(log, 1) })
      Void
    }
    _ -> Void
  }
}

pub fn main() -> Text {
  let made = Channel.bounded(1)
  let log = made.sender
  let endless = Stream.produce(1, fn(out) {
    let outcome = Result.capture(fn() {
      loop count = 0 {
        Stream.write(out, Bytes.from_text("xy"))
        count + 1
      }
    })
    log_closed(log, outcome)
  })
  let all = Result.capture(fn() { Stream.read_all(endless, 3) })
  let got = Channel.receive(made.receiver)
  let closed = case got {
    Receive.Received(value) -> value
    Receive.Drained -> 0
  }
  Debug.render((all, closed))
}
"#);
    assert_eq!(outcome, rendered(r#"(Result.Err(StreamError.TooLarge(3)), 1)"#));
}
