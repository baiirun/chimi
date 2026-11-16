//! Conformance for `std/net` (CHI-255): TCP connections and listeners over the
//! real network on loopback, the failures the operating system reports, name
//! lookup on its own thread, the fake connection pair, and who may reach the
//! network (`main` by declaring the need, a test only through
//! `Net.host_for_tests`).
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI there.
//! Socket tests run with `--clock=real`: a task waiting on a socket holds
//! virtual time, so a test that waits on a socket and a timer needs the real
//! clock.

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-net-{name}-{}-{:?}",
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

const IMPORTS: &str = "import std/bytes as Bytes
import std/text as Text
import std/clock as Clock
import std/debug as Debug
import std/duration as Duration
import std/net as Net
import std/net.{Network, NetError}
import std/resource as Resource
import std/result as Result
import std/result.{Err, Ok}
import std/stream as Stream
import std/task as Task
import std/test as Test
";

fn suite(tests: &str) -> String {
    format!("{IMPORTS}{OUTCOME}\n{tests}")
}

const OUTCOME: &str = r#"
// What `operation` gives, rendered, with a success shown as `Ok` alone (a
// connection or listener has no stable rendering).
fn attempt<T, E, R>(operation: fn() -> T errors E needs R) -> Text needs R {
  case Result.capture(operation) {
    Ok(_) -> "Ok"
    Err(error) -> "Err(${Debug.render(error)})"
  }
}
"#;

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

/// A server on a free loopback port that answers each connection with what
/// it read, upper-cased by nothing: the bytes come back as they went.
const ECHO: &str = r#"
fn echo_once(host: Text, request: Text) -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen(host, 0) }, Net.close_listener, fn(listener) {
    let port = Net.port_of(Net.listener_address(listener))
    let server = Task.start(fn() {
      let connection = Net.accept(listener)
      let got = Stream.read_all(Net.reader(connection), 1048576)
      Stream.write(Net.writer(connection), got)
      Stream.close(Net.writer(connection))
      Net.close(connection)
    })
    let answer = Resource.scoped(fn() { Net.connect(host, port) }, Net.close, fn(connection) {
      Stream.write(Net.writer(connection), Bytes.from_text(request))
      // Half-close: the server reads the end of input, and this side still
      // reads the answer.
      Stream.close(Net.writer(connection))
      Stream.read_all(Net.reader(connection), 1048576)
    })
    Task.result(server)
    Bytes.to_text(answer)
  })
}
"#;

// Rejects a socket path that loses or reorders bytes, a half-close that closes
// the whole connection, a listener on port 0 that reports port 0, and wrong
// addresses: the request comes back whole (including a body larger than one
// 64 KiB read), and each end's addresses name loopback with the right ports.
#[test]
fn a_loopback_connection_carries_bytes_both_ways_after_a_half_close() {
    let run = run_tests(
        "echo",
        &format!(
            "{ECHO}{}",
            r#"
test "echo" {
  let answer = echo_once("127.0.0.1", "ping") using { network: Net.host_for_tests() }
  answer |> Test.assert_equal("ping")
}

test "a body larger than one read arrives whole" {
  let big = Text.repeat("abcdefgh", 40000)
  let answer = echo_once("127.0.0.1", big) using { network: Net.host_for_tests() }
  answer |> Test.assert_equal(big)
}

test "addresses" {
  let report = run_addresses() using { network: Net.host_for_tests() }
  report |> Test.assert_equal("127.0.0.1 127.0.0.1 (True, True, True)")
}

fn run_addresses() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let listening = Net.listener_address(listener)
    let port = Net.port_of(listening)
    let accepted = Task.start(fn() { Net.accept(listener) })
    let client = Net.connect("127.0.0.1", port)
    let server = Task.result(accepted)
    let local = Net.local_address(client)
    let peer = Net.peer_address(client)
    let checks = (port > 0, Net.port_of(peer) == port, Net.port_of(Net.peer_address(server)) == Net.port_of(local))
    let report = "${Net.host_of(local)} ${Net.host_of(peer)} ${Debug.render(checks)}"
    Net.close(client)
    Net.close(server)
    report
  })
}
"#
        ),
    );
    assert_passed(&run, 3);
}

// Rejects a name lookup that blocks the scheduler or loses its answer: a name
// (not a numeric address) goes through the lookup thread and connects.
#[test]
fn a_name_resolves_on_the_lookup_thread() {
    let run = run_tests(
        "lookup",
        &format!(
            "{ECHO}{}",
            r#"
test "localhost" {
  let ticker = Task.start(fn() {
    Clock.sleep(Duration.milliseconds(1))
    "ticked"
  })
  let answer = echo_once("localhost", "by name") using { network: Net.host_for_tests() }
  answer |> Test.assert_equal("by name")
  Task.result(ticker) |> Test.assert_equal("ticked")
}
"#
        ),
    );
    assert_passed(&run, 1);
}

// Rejects wrong failure codes: a closed port is `Refused`, a taken port is
// `AddressInUse`, a name under `.invalid` (which never resolves) is
// `HostNotFound` with the name, and a port outside 0 to 65535 is a defect.
#[test]
fn the_operating_systems_failures_become_net_errors() {
    let run = run_tests(
        "failures",
        r#"
fn closed_port() -> Int needs { network: Network } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port_of(Net.listener_address(listener))
  Net.close_listener(listener)
  port
}

fn refused() -> Text needs { network: Network } {
  let port = closed_port()
  attempt(fn() { Net.connect("127.0.0.1", port) })
}

fn in_use() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port_of(Net.listener_address(listener))
    attempt(fn() { Net.listen("127.0.0.1", port) })
  })
}

fn not_found() -> Text needs { network: Network } {
  "${attempt(fn() { Net.connect("nothing.invalid", 80) })} ${attempt(fn() { Net.connect("a\u{0}b", 80) })}"
}

test "refused" {
  let outcome = refused() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Err(NetError.Refused)")
}

test "address in use" {
  let outcome = in_use() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Err(NetError.AddressInUse)")
}

test "host not found" {
  let outcome = not_found() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Err(NetError.HostNotFound(\"nothing.invalid\")) Err(NetError.HostNotFound(\"a\\u{0}b\"))")
}

test "a port out of range is a defect" {
  Test.capture_defect(fn() { Net.connect("127.0.0.1", 65536) using { network: Net.host_for_tests() } })
    |> Test.assert_equal("Net: the port 65536 is outside 0 to 65535")
}
"#,
    );
    assert_passed(&run, 4);
}

// Rejects a close that leaves another task parked forever: closing a listener
// wakes a task waiting in `accept`, and closing a connection wakes a task
// waiting to read, each with `NetError.Closed`; later operations fail the same
// way. A connection scoped with `Resource.scoped` and `Net.close` is released
// when `use` returns. Stopping a reader ends only the reading side, so a
// server can still answer a request it refused to read whole, and a write to
// a peer that has gone fails instead of raising SIGPIPE.
#[test]
fn closing_wakes_a_waiting_task_with_closed() {
    let run = run_tests(
        "closing",
        r#"
fn accept_then_close() -> Text needs { network: Network } {
  let listener = Net.listen("127.0.0.1", 0)
  let waiting = Task.start(fn() { attempt(fn() { Net.accept(listener) }) })
  Clock.sleep(Duration.milliseconds(20))
  Net.close_listener(listener)
  Net.close_listener(listener)
  let woke = Task.timeout(fn() { Task.result(waiting) }, Duration.seconds(5))
  "${woke} ${attempt(fn() { Net.accept(listener) })}"
}

fn read_then_close() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port_of(Net.listener_address(listener))
    let client = Net.connect("127.0.0.1", port)
    let server = Net.accept(listener)
    let waiting = Task.start(fn() { Result.capture(fn() { Stream.read(Net.reader(client)) }) })
    Clock.sleep(Duration.milliseconds(20))
    Net.close(client)
    let woke = Task.timeout(fn() { Task.result(waiting) }, Duration.seconds(5)) |> Debug.render
    let wrote = Result.capture(fn() { Stream.write(Net.writer(client), Bytes.from_text("x")) }) |> Debug.render
    let read = Result.capture(fn() { Stream.read(Net.reader(client)) }) |> Debug.render
    Net.close(server)
    "${woke} ${wrote} ${read}"
  })
}

// The server stops reading a request that is too large and still answers.
fn stop_then_answer() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port_of(Net.listener_address(listener))
    let client = Net.connect("127.0.0.1", port)
    let server = Net.accept(listener)
    Stream.write(Net.writer(client), Bytes.from_text("far too long"))
    let request = Result.capture(fn() { Stream.read_all(Net.reader(server), 4) }) |> Debug.render
    Stream.write(Net.writer(server), Bytes.from_text("too large"))
    Stream.close(Net.writer(server))
    let answer = Bytes.to_text(Stream.read_all(Net.reader(client), 1024))
    let again = Result.capture(fn() { Stream.read(Net.reader(server)) }) |> Debug.render
    Net.close(client)
    Net.close(server)
    "${request} ${answer} ${again}"
  })
}

// Writing to a peer that has gone fails, and never raises SIGPIPE, which
// would kill the test process.
fn write_to_gone_peer() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port_of(Net.listener_address(listener))
    let client = Net.connect("127.0.0.1", port)
    Net.close(Net.accept(listener))
    let chunk = Bytes.from_text(Text.repeat("x", 65536))
    let written = Result.capture(fn() {
      loop count = 0 {
        case count > 1000 {
          True -> break "still writing"
          False -> {
            Stream.write(Net.writer(client), chunk)
            count + 1
          }
        }
      }
    })
    Net.close(client)
    case written {
      Err(NetError.Reset) -> "failed"
      Err(NetError.Closed) -> "failed"
      other -> Debug.render(other)
    }
  })
}

test "a stopped reader leaves the writer working" {
  let outcome = stop_then_answer() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Result.Err(StreamError.TooLarge(4)) too large Result.Err(NetError.Closed)")
}

test "a write to a gone peer fails" {
  let outcome = write_to_gone_peer() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("failed")
}

fn scoped_then_read() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port_of(Net.listener_address(listener))
    let kept = Resource.scoped(fn() { Net.connect("127.0.0.1", port) }, Net.close, fn(connection) { connection })
    let server = Net.accept(listener)
    let after = Result.capture(fn() { Stream.read(Net.reader(kept)) }) |> Debug.render
    // The peer reads the end of input: the scope released the socket.
    let peer = Result.capture(fn() { Stream.read(Net.reader(server)) }) |> Debug.render
    Net.close(server)
    "${after} ${peer}"
  })
}

test "a scoped connection is released" {
  let outcome = scoped_then_read() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Result.Err(NetError.Closed) Result.Ok(None)")
}

test "accept" {
  let outcome = accept_then_close() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Err(NetError.Closed) Err(NetError.Closed)")
}

test "read" {
  let outcome = read_then_close() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("Result.Err(NetError.Closed) Result.Err(NetError.Closed) Result.Err(NetError.Closed)")
}
"#,
    );
    assert_passed(&run, 5);
}

// Rejects a fake pair that crosses its wires or does not end: each end reads
// what the other writes, closing a writer ends the other reader, and a closed
// end reads `Closed`. A fake network made of the pair satisfies `Network`.
#[test]
fn a_fake_pair_and_a_fake_network_stand_in_for_sockets() {
    let run = Scratch::new(
        "fakes",
        &suite(
            r#"
fn greet() -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.connect("example.com", 80) }, Net.close, fn(connection) {
    Stream.write(Net.writer(connection), Bytes.from_text("hello"))
    Stream.close(Net.writer(connection))
    Bytes.to_text(Stream.read_all(Net.reader(connection), 1024))
  })
}

test "a pair" {
  let (left, right) = Net.pair(4)
  Stream.write(Net.writer(left), Bytes.from_text("to right"))
  Stream.close(Net.writer(left))
  Stream.read_all(Net.reader(right), 1024) |> Test.assert_equal(Bytes.from_text("to right"))
  Stream.write(Net.writer(right), Bytes.from_text("to left"))
  Stream.close(Net.writer(right))
  Stream.read_all(Net.reader(left), 1024) |> Test.assert_equal(Bytes.from_text("to left"))
  Net.port_of(Net.peer_address(left)) |> Test.assert_equal(Net.port_of(Net.local_address(right)))
}

test "a closed end" {
  let (left, _right) = Net.pair(4)
  Net.close(left)
  Result.capture(fn() { Stream.write(Net.writer(left), Bytes.from_text("x")) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(NetError.Closed)")
  Result.capture(fn() { Stream.read(Net.reader(left)) })
    |> Debug.render
    |> Test.assert_equal("Result.Err(NetError.Closed)")
}

test "a fake network" {
  let (client, server) = Net.pair(4)
  let fake = {
    connect: fn(_host, _port) { client },
    listen: fn(_host, _port) { Debug.crash("no listening here") },
  }
  let answering = Task.start(fn() {
    let got = Stream.read_all(Net.reader(server), 1024)
    Stream.write(Net.writer(server), Bytes.concat([Bytes.from_text("got "), got]))
    Stream.close(Net.writer(server))
  })
  let answer = greet() using { network: fake }
  Task.result(answering)
  answer |> Test.assert_equal("got hello")
}
"#,
        ),
    )
    .chimi(&["test", "cases.chimi"]);
    assert_passed(&run, 3);
}

// Rejects a runner that does not close `main`'s `network` need: a program that
// declares it opens a real socket (the built program exits 0 only when its
// echo comes back).
#[test]
fn main_gets_the_real_network_by_declaring_the_need() {
    let scratch = Scratch::new(
        "main",
        &format!(
            "{IMPORTS}{ECHO}\npub fn main() -> Void needs {{ network: Network }} {{\n  let answer = echo_once(\"127.0.0.1\", \"from main\")\n  case answer == \"from main\" {{\n    True -> Void\n    False -> Debug.crash(\"the echo differs\")\n  }}\n}}\n"
        ),
    );
    let run = scratch.chimi(&["build", "cases.chimi", "-o", "served"]);
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    let output = Command::new(scratch.0.join("served")).output().expect("run the program");
    let run = Run {
        status: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    };
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
}

// Rejects a back door to the network: `Net.host_for_tests` is test-only, and a
// test that does not supply `network` is refused before it runs.
#[test]
fn only_a_test_that_asks_gets_the_real_network() {
    let from_main = Scratch::new(
        "back-door",
        &format!(
            "{IMPORTS}{ECHO}\npub fn main() -> Text {{\n  echo_once(\"127.0.0.1\", \"x\") using {{ network: Net.host_for_tests() }}\n}}\n"
        ),
    )
    .chimi(&["build", "cases.chimi"]);
    assert_ne!(from_main.status, Some(0));
    assert!(from_main.stderr.contains("T4401"), "{}", from_main.stderr);

    let unsupplied = run_tests(
        "unsupplied",
        &format!("{ECHO}\ntest \"reaches out\" {{\n  echo_once(\"127.0.0.1\", \"x\") |> Test.assert_equal(\"x\")\n}}\n"),
    );
    assert_ne!(unsupplied.status, Some(0));
    assert!(unsupplied.stderr.contains("T4400"), "{}", unsupplied.stderr);
}
