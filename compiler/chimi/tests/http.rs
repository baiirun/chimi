//! Conformance for the `std/http` server (CHI-257): HTTP/1.1 framing over a
//! connection (keep-alive, HTTP/1.0, `HEAD`, chunked bodies, `100-continue`),
//! the error status of each kind of malformed request, the limits, the idle
//! timeout, the handler's failures, and a real server on loopback.
//!
//! Most tests drive `Http.serve_connection` over a fake `Net.pair` with raw
//! request bytes and compare the raw response bytes, so the wire format is
//! what is checked. Each test writes `cases.chimi` to a scratch directory and
//! runs the CLI there.

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-http-{name}-{}-{:?}",
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

/// A handler with a few routes, and `exchange`, which sends raw bytes to a
/// connection served by it, closes the client's writing side, and returns
/// every byte the server sent before it closed the connection.
const PRELUDE: &str = r#"import std/bytes as Bytes
import std/bytes.{Bytes}
import std/clock as Clock
import std/clock.{Clock}
import std/debug as Debug
import std/duration as Duration
import std/duration.{Duration}
import std/http as Http
import std/http.{Method, Request, Response}
import std/list as List
import std/net as Net
import std/net.{Connection, Listener, NetError, Network}
import std/resource as Resource
import std/result as Result
import std/result.{Err, Ok}
import std/stream as Stream
import std/stream.{Reader, StreamError}
import std/task as Task
import std/test as Test
import std/text as Text

enum Broken {
  Broken
}

fn handle(request: Request) -> Response {
  case (Http.method(request), Http.path(request)) {
    (Method.Get, ["health"]) -> Http.text(200, "ok")
    (Method.Head, ["health"]) -> Http.text(200, "ok")
    (Method.Post, ["echo"]) -> Http.response(200, [], Http.body(request))
    (Method.Get, ["users", id]) -> Http.text(200, "user ${id} ${Http.query(request)}")
    (Method.Get, ["bye"]) -> Http.response(200, [("Connection", "close")], Bytes.from_text("bye"))
    (Method.Get, ["broken"]) -> Failure(Broken.Broken)
    (Method.Get, ["crash"]) -> Debug.crash("boom")
    (Method.Get, ["host"]) -> case Http.header(request, "host") {
      Some(host) -> Http.text(200, host)
      None -> Http.text(200, "none")
    }
    (Method.Get, ["empty"]) -> Http.response(204, [("Content-Length", "5")], Bytes.from_text("ignored"))
    (Method.Get, ["cached"]) -> Http.response(304, [("ETag", "\"a\""), ("Content-Length", "10")], Bytes.empty())
    (Method.Get, ["framed"]) -> Http.response(200, [("Content-Length", "99"), ("Transfer-Encoding", "chunked"), ("Upgrade", "x"), ("Keep-Alive", "timeout=5")], Bytes.from_text("abc"))
    (Method.Options, ["*"]) -> Http.text(200, "star")
    (Method.Other(name), _) -> Http.text(405, "no ${name}")
    _ -> Http.text(404, "not found")
  }
}

fn exchange(raw: Text) -> Text {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, handle) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  Stream.close(Net.writer(client))
  let answer = Stream.read_all(Net.reader(client), 33554432)
  Task.result(serving)
  undated(answer)
}

// `exchange` with settings other than the defaults.
fn exchange_with(settings: { max_head_bytes: Int, max_headers: Int, max_body_bytes: Int, request_timeout: Duration, write_timeout: Duration, max_connections: Int }, raw: Text) -> Text {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, settings, handle) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  Stream.close(Net.writer(client))
  let answer = Stream.read_all(Net.reader(client), 33554432)
  Task.result(serving)
  undated(answer)
}

// Sends `raw` without closing the client's writing side: the answer ends only
// if the server closes the connection.
fn exchange_open(raw: Text) -> Text {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, handle) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  let answer = Stream.read_all(Net.reader(client), 1048576)
  Task.result(serving)
  undated(answer)
}

// Sends each of `pieces` as its own write, so a head arrives split across
// reads, then closes the client's writing side. Writes after the server
// closed the connection are dropped, as a refused client's would be.
fn trickle(pieces: List<Text>) -> Text {
  let (client, server) = Net.pair(4096)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, handle) })
  let _ = Result.capture(fn() {
    List.each(pieces, fn(piece) { Stream.write(Net.writer(client), Bytes.from_text(piece)) })
    Stream.close(Net.writer(client))
  })
  let answer = Stream.read_all(Net.reader(client), 1048576)
  Task.result(serving)
  undated(answer)
}

fn ok(body: Text) -> Text {
  "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: ${Text.byte_length(body)}\r\n\r\n${body}"
}

fn refused(status: Int, reason: Text, body: Text) -> Text {
  "HTTP/1.1 ${status} ${reason}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: ${Text.byte_length(body)}\r\nconnection: close\r\n\r\n${body}"
}

// A response without its `date` header, which depends on the clock; one
// test checks the header itself.
fn undated(answer: Bytes) -> Text {
  Text.join(List.filter(Text.split(Bytes.to_text(answer), "\r\n"), fn(line) { not Text.starts_with(line, "date: ") }), "\r\n")
}
"#;

fn run_tests(name: &str, tests: &str, clock: &str) -> Run {
    Scratch::new(name, &format!("{PRELUDE}\n{tests}")).chimi(&[
        "test",
        "cases.chimi",
        &format!("--clock={clock}"),
    ])
}

fn assert_passed(run: &Run, count: usize) {
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(&format!("{count} passed, 0 failed")),
        "{}",
        run.stdout
    );
}

// Rejects a server that sends no `Date`, formats it wrongly (weekday, a leap
// day, a negative timestamp, a century that is not a leap year, rounding a
// fraction up, the first and last years a clock can start in), or overrides
// a handler's own `Date`.
#[test]
fn every_response_carries_the_date() {
    let tests = r#"
fn dated(request: Request) -> Response {
  case Http.path(request) {
    ["own"] -> Http.response(200, [("Date", "earlier")], Bytes.empty())
    _ -> Http.text(200, "ok")
  }
}

test "the date" {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, dated) })
  Stream.write(Net.writer(client), Bytes.from_text("GET / HTTP/1.1\r\nHost: x\r\n\r\nGET /own HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
  let answer = Bytes.to_text(Stream.read_all(Net.reader(client), 1024))
  Task.result(serving)
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ndate: DATE\r\ncontent-length: 2\r\n\r\nokHTTP/1.1 200 OK\r\ndate: earlier\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
}
"#;
    for (start, date) in [
        ("1994-11-06T08:49:37Z", "Sun, 06 Nov 1994 08:49:37 GMT"),
        ("2000-02-29T23:59:59.9Z", "Tue, 29 Feb 2000 23:59:59 GMT"),
        ("1969-12-31T23:59:59.5Z", "Wed, 31 Dec 1969 23:59:59 GMT"),
        ("2100-03-01T00:00:00Z", "Mon, 01 Mar 2100 00:00:00 GMT"),
        ("1678-01-01T00:00:00.5Z", "Sat, 01 Jan 1678 00:00:00 GMT"),
        ("2262-01-01T23:59:59Z", "Wed, 01 Jan 2262 23:59:59 GMT"),
    ] {
        let run = Scratch::new(
            &format!("date-{}", &start[..10]),
            &format!("{PRELUDE}\n{}", tests.replace("DATE", date)),
        )
        .chimi(&["test", "cases.chimi", "--clock=virtual", &format!("--clock-start={start}")]);
        assert_passed(&run, 1);
    }
}

// Rejects a server that closes after one request, mixes up pipelined
// requests, ignores `connection: close` from either side, sends a body for
// `HEAD`, or keeps an HTTP/1.0 connection it was not asked to keep.
#[test]
fn a_connection_answers_requests_in_order_and_closes_when_asked() {
    let run = run_tests(
        "keep-alive",
        r#"
test "pipelined requests on one connection" {
  exchange("GET /health HTTP/1.1\r\nHost: x\r\n\r\nPOST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhelloGET /users/7?full=1 HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal(Text.concat([ok("ok"), "HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello", ok("user 7 full=1")]))
}

test "a client's connection: close is the last request" {
  exchange_open("GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
}

test "a handler's connection: close is the last response" {
  exchange_open("GET /bye HTTP/1.1\r\nHost: x\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 3\r\nconnection: close\r\n\r\nbye")
}

test "HTTP/1.0 closes unless asked to keep alive" {
  exchange_open("GET /health HTTP/1.0\r\n\r\nGET /health HTTP/1.0\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
  exchange("GET /health HTTP/1.0\r\nConnection: keep-alive\r\n\r\nGET /health HTTP/1.0\r\n\r\n")
    |> Test.assert_equal(Text.concat(["HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: keep-alive\r\n\r\nok", "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"]))
}

test "HEAD sends the length but not the body" {
  exchange("HEAD /health HTTP/1.1\r\nHost: x\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal(Text.concat(["HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\n\r\n", ok("ok")]))
}

test "an unknown method reaches the handler by name" {
  exchange("BREW /pot HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 405 Method Not Allowed\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 7\r\n\r\nno BREW")
}
"#,
        "virtual",
    );
    assert_passed(&run, 6);
}

// Rejects a body reader that mis-frames: a chunked body (with an extension and
// a trailer) arrives decoded and the next request still parses, and a client
// that expects `100 Continue` gets it before its body is read.
#[test]
fn chunked_bodies_and_100_continue() {
    let run = run_tests(
        "bodies",
        r#"
test "a chunked body" {
  exchange("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5;name=value\r\nhello\r\nA\r\n, world!!!\r\n0\r\nTrailer: yes\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal(Text.concat(["HTTP/1.1 200 OK\r\ncontent-length: 15\r\n\r\nhello, world!!!", ok("ok")]))
}

test "100-continue comes before the answer" {
  exchange("POST /echo HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\nhi")
    |> Test.assert_equal("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi")
}
"#,
        "virtual",
    );
    assert_passed(&run, 2);
}

// Rejects a server that answers a malformed request as if it were fine, or
// keeps reading after one: each gets its status and the connection closes,
// so the request after it is never answered.
#[test]
fn a_malformed_request_gets_its_status_and_closes_the_connection() {
    let run = run_tests(
        "refusals",
        r#"
fn after(raw: Text) -> Text {
  exchange(Text.concat([raw, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n"]))
}

test "400" {
  after("GET\r\n\r\n") |> Test.assert_equal(refused(400, "Bad Request", "bad request: bad request line"))
  after("GET /health HTTP/1.1\r\n\r\n") |> Test.assert_equal(refused(400, "Bad Request", "bad request: no host header"))
  after("GET /health HTTP/1.1\r\nHost: x\r\nBad Header\r\n\r\n")
    |> Test.assert_equal(refused(400, "Bad Request", "bad request: a header line has no colon"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n")
    |> Test.assert_equal(refused(400, "Bad Request", "bad request: both content-length and transfer-encoding"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n")
    |> Test.assert_equal(refused(400, "Bad Request", "bad request: conflicting content-length"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: -1\r\n\r\n")
    |> Test.assert_equal(refused(400, "Bad Request", "bad request: content-length is not a number"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")
    |> Test.assert_equal(refused(400, "Bad Request", "bad request: bad chunk size"))
}

test "a head cut short is 400, a request never started is no answer" {
  exchange("GET /health HTTP/1.1\r\nHost:") |> Test.assert_equal(refused(400, "Bad Request", "bad request: the connection ended inside a request"))
  exchange("") |> Test.assert_equal("")
}

test "501 and 505" {
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n")
    |> Test.assert_equal(refused(501, "Not Implemented", "not implemented: transfer codings other than chunked"))
  after("GET /health HTTP/2.0\r\nHost: x\r\n\r\n")
    |> Test.assert_equal(refused(505, "HTTP Version Not Supported", "http version not supported"))
}

test "413 and 431" {
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 8388609\r\n\r\n")
    |> Test.assert_equal(refused(413, "Content Too Large", "content too large"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 99999999999999999999999\r\n\r\n")
    |> Test.assert_equal(refused(413, "Content Too Large", "content too large"))
  let big_chunk = "POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n800001\r\n"
  after(big_chunk) |> Test.assert_equal(refused(413, "Content Too Large", "content too large"))
  let long_head = "GET /health HTTP/1.1\r\nHost: x\r\nX: ${Text.repeat("a", 70000)}\r\n\r\n"
  after(long_head) |> Test.assert_equal(refused(431, "Request Header Fields Too Large", "request header fields too large"))
  let many = Text.repeat("X: y\r\n", 100)
  after("GET /health HTTP/1.1\r\nHost: x\r\n${many}\r\n")
    |> Test.assert_equal(refused(431, "Request Header Fields Too Large", "request header fields too large"))
}

test "a body at the limit is fine" {
  let body = Text.repeat("a", 8388608)
  let answer = exchange("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 8388608\r\n\r\n${body}")
  Text.byte_length(answer) |> Test.assert_equal(8388608 + 44)
}
"#,
        "virtual",
    );
    assert_passed(&run, 5);
}

// Rejects a handler failure that drops the connection or leaks into the
// answer, and an idle connection held forever: a failing handler answers 500
// and the connection carries on, and a client that sends nothing for the
// idle limit is closed (virtual time, so the minute passes at once).
#[test]
fn a_handler_failure_answers_500_and_an_idle_client_is_closed() {
    let run = run_tests(
        "failures",
        r#"
test "a handler failure is 500" {
  exchange("GET /broken HTTP/1.1\r\nHost: x\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal(Text.concat(["HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 21\r\n\r\ninternal server error", ok("ok")]))
}

test "an idle client is closed after a minute" {
  let start = Clock.now()
  exchange_open("") |> Test.assert_equal("")
  Clock.since(start) |> Test.assert_equal(Duration.seconds(60))
}

test "a server applies the settings it is given" {
  let (client, server) = Net.pair(64)
  let settings = { ..Http.defaults, max_headers: 1, request_timeout: Duration.seconds(5) }
  let serving = Task.start(fn() { Http.serve_connection(server, settings, handle) })
  let start = Clock.now()
  Stream.write(Net.writer(client), Bytes.from_text("GET /health HTTP/1.1\r\nHost: x\r\n\r\n"))
  let first = Stream.read_all(Net.reader(client), 1048576)
  Task.result(serving)
  undated(first) |> Test.assert_equal(ok("ok"))
  Clock.since(start) |> Test.assert_equal(Duration.seconds(5))
  exchange_with(settings, "GET /health HTTP/1.1\r\nHost: x\r\nX-A: 1\r\n\r\n")
    |> Test.assert_equal(refused(431, "Request Header Fields Too Large", "request header fields too large"))
  exchange_with({ ..Http.defaults, max_body_bytes: 3 }, "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nabcd")
    |> Test.assert_equal(refused(413, "Content Too Large", "content too large"))
  exchange_with({ ..Http.defaults, max_head_bytes: 20 }, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal(refused(431, "Request Header Fields Too Large", "request header fields too large"))
}

test "settings that cannot work are a defect" {
  let (_, server) = Net.pair(64)
  Test.capture_defect(fn() { Http.serve_connection(server, { ..Http.defaults, max_headers: 0 }, handle) })
    |> Test.assert_equal("Http: max_head_bytes, max_headers and max_connections must be positive, and max_body_bytes not negative")
  let (_, capped) = Net.pair(64)
  Test.capture_defect(fn() { Http.serve_connection(capped, { ..Http.defaults, max_connections: 0 }, handle) })
    |> Test.assert_equal("Http: max_head_bytes, max_headers and max_connections must be positive, and max_body_bytes not negative")
  let (_, other) = Net.pair(64)
  Test.capture_defect(fn() { Http.serve_connection(other, { ..Http.defaults, write_timeout: Duration.zero }, handle) })
    |> Test.assert_equal("Http: request_timeout and write_timeout must be positive")
}

test "a status outside 200 to 599 is a defect" {
  Test.capture_defect(fn() { Http.text(600, "x") }) |> Test.assert_equal("Http: the status 600 is outside 200 to 599")
  Test.capture_defect(fn() { Http.text(101, "x") }) |> Test.assert_equal("Http: the status 101 is outside 200 to 599")
}

test "a header that would split the response is a defect" {
  Test.capture_defect(fn() { Http.response(200, [("Location", "/a\r\nSet-Cookie: x=1")], Bytes.empty()) })
    |> Test.assert_equal("Http: the value of the header Location holds a control character")
  Test.capture_defect(fn() { Http.response(200, [("X", Bytes.to_text(Bytes.from([97, 0])))], Bytes.empty()) })
    |> Test.assert_equal("Http: the value of the header X holds a control character")
  Test.capture_defect(fn() { Http.response(200, [("Bad Name", "x")], Bytes.empty()) })
    |> Test.assert_equal("Http: the header name \"Bad Name\" is not a token")
}

test "a handler can be tested without a connection" {
  let request = Http.request(Method.Get, ["users", "9"], "page=2", [("X-Trace", "abc")], Bytes.empty())
  Http.header(request, "x-trace") |> Test.assert_equal(Some("abc"))
  Http.header(request, "X-TRACE") |> Test.assert_equal(Some("abc"))
  Http.header(request, "missing") |> Test.assert_equal(None)
  let response = handle(request)
  Http.status(response) |> Test.assert_equal(200)
  Http.response_body(response) |> Test.assert_equal(Bytes.from_text("user 9 page=2"))
}
"#,
        "virtual",
    );
    assert_passed(&run, 7);
}

// Rejects a server that serves one client at a time or never stops (two
// clients are answered while both hold a connection open, and closing the
// listener ends `serve` and closes the connections still open), one whose
// handler defect ends it, and one that ignores its connection cap.
#[test]
fn a_server_on_loopback_serves_clients_at_once_and_stops_with_its_listener() {
    let run = run_tests(
        "loopback",
        r#"
fn get(port: Int, path: Text) -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.connect("127.0.0.1", port) }, Net.close, fn(connection) {
    Stream.write(Net.writer(connection), Bytes.from_text("GET ${path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
    undated(Stream.read_all(Net.reader(connection), 1048576))
  })
}

fn run_server() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, Http.defaults, handle) })
  // The first client holds its connection open without a request while the
  // second is answered.
  let idle = Net.connect("127.0.0.1", port)
  // A server that answers one connection at a time would never answer
  // these while the first client waits.
  let (answer, other) = Task.timeout(fn() { (get(port, "/users/1"), get(port, "/health")) }, Duration.seconds(5))
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  // Serving has ended, so the connection still open was closed.
  let left = Task.timeout(fn() { Bytes.to_text(Stream.read_all(Net.reader(idle), 1024)) }, Duration.seconds(5))
  Net.close(idle)
  "${answer}|${other}|${left}"
}

test "two clients and a stop" {
  let outcome = run_server() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 7\r\nconnection: close\r\n\r\nuser 1 |HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok|")
}

fn crash_then_health() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, Http.defaults, handle) })
  // A connection open while another's handler defects is still served.
  let held = Net.connect("127.0.0.1", port)
  let crashed = Task.timeout(fn() { get(port, "/crash") }, Duration.seconds(5))
  Stream.write(Net.writer(held), Bytes.from_text("GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
  let kept = Task.timeout(fn() { undated(Stream.read_all(Net.reader(held), 1048576)) }, Duration.seconds(5))
  Net.close(held)
  let after = Task.timeout(fn() { get(port, "/health") }, Duration.seconds(5))
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  "${crashed}|${kept}|${after}"
}

test "a handler's defect closes only its own connection" {
  let ok = "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
  let outcome = crash_then_health() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("|${ok}|${ok}")
}

// With a cap of one, a second client is closed at once while the first holds
// its connection open, and is answered once the first has closed it.
fn capped() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, { ..Http.defaults, max_connections: 1 }, handle) })
  let idle = Net.connect("127.0.0.1", port)
  Clock.sleep(Duration.milliseconds(100))
  let asked = Clock.now()
  let refused = case Result.capture(fn() { Task.timeout(fn() { get(port, "/health") }, Duration.seconds(5)) }) {
    Ok(answer) -> answer
    Err(_) -> case Clock.since(asked) < Duration.seconds(1) {
      True -> ""
      False -> "the connection past the cap was left open"
    }
  }
  Net.close(idle)
  Clock.sleep(Duration.milliseconds(100))
  let answered = Task.timeout(fn() { get(port, "/health") }, Duration.seconds(5))
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  "${refused}|${answered}"
}

test "past the cap a new connection is closed" {
  let outcome = capped() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("|HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
}
"#,
        "real",
    );
    assert_passed(&run, 3);
}

// Rejects a parser that lets through what a proxy in front could read
// differently (request smuggling) or what a handler could echo into a
// response header: bare CR, LF or NUL, a second host, a transfer coding
// hidden in a second header, or a transfer-encoding on HTTP/1.0.
#[test]
fn ambiguous_requests_are_refused() {
    let run = run_tests(
        "smuggling",
        r#"
fn after(raw: Text) -> Text {
  exchange(Text.concat([raw, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n"]))
}

fn bad(reason: Text) -> Text {
  refused(400, "Bad Request", "bad request: ${reason}")
}

test "control characters" {
  let nul = Bytes.to_text(Bytes.from([0]))
  after("GET /health HTTP/1.1\r\nHost: x\r\nX: a${nul}b\r\n\r\n") |> Test.assert_equal(bad("a control character in the head"))
  after("GET /health HTTP/1.1\r\nHost: x\r\nX: a\rb\r\n\r\n") |> Test.assert_equal(bad("a line break that is not CRLF"))
  after("GET /health HTTP/1.1\r\nHost: x\r\nX: a\nY: b\r\n\r\n") |> Test.assert_equal(bad("a line break that is not CRLF"))
  after("GET /he\talth HTTP/1.1\r\nHost: x\r\n\r\n") |> Test.assert_equal(bad("bad request line"))
}

test "hosts and transfer codings" {
  after("GET /health HTTP/1.1\r\nHost: x\r\nHost: y\r\n\r\n") |> Test.assert_equal(bad("more than one host header"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: gzip\r\n\r\n")
    |> Test.assert_equal(bad("chunked is not the final transfer coding"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n")
    |> Test.assert_equal(refused(501, "Not Implemented", "not implemented: transfer codings other than chunked"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip\r\n\r\n") |> Test.assert_equal(bad("chunked is not the final transfer coding"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: \r\n\r\n") |> Test.assert_equal(bad("content-length is not a number"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: ,\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nZ\r\n0\r\n\r\n")
    |> Test.assert_equal(bad("content-length is not a number"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n1;a\nb\r\nZ\r\n0\r\n\r\n")
    |> Test.assert_equal(bad("a control character in a chunked body"))
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nZ\r\n0\r\nX: a\rb\r\n\r\n")
    |> Test.assert_equal(bad("a control character in a chunked body"))
  after("POST /echo HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n") |> Test.assert_equal(bad("transfer-encoding in HTTP/1.0"))
  // The Kelvin sign lowercases to k in Unicode, not in HTTP.
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chun\u{212A}ed\r\n\r\n")
    |> Test.assert_equal(bad("chunked is not the final transfer coding"))
}

test "versions" {
  after("GET /health HTTP/x\r\nHost: x\r\n\r\n") |> Test.assert_equal(bad("bad request line"))
  after("GET /health HTTP/a.b\r\nHost: x\r\n\r\n") |> Test.assert_equal(bad("bad request line"))
  after("GET /health HTTP/3.0\r\nHost: x\r\n\r\n") |> Test.assert_equal(refused(505, "HTTP Version Not Supported", "http version not supported"))
  exchange("GET /health HTTP/1.2\r\nHost: x\r\n\r\n") |> Test.assert_equal(ok("ok"))
}
"#,
        "virtual",
    );
    assert_passed(&run, 3);
}

// Rejects request reading that depends on how the client splits its writes
// or that loses the bytes after a request, and target forms read wrongly.
#[test]
fn requests_arrive_however_they_are_split_and_in_every_target_form() {
    let run = run_tests(
        "reading",
        r#"
test "a head split across reads" {
  trickle(["GET /health HTTP/1.1\r\nHost: x\r", "\n\r", "\nGET /users/2 HTTP/1.1\r\nHo", "st: x\r\n\r\n"])
    |> Test.assert_equal(Text.concat([ok("ok"), ok("user 2 ")]))
  trickle(["POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 6\r\n\r\nab", "cd", "efGET /health HTTP/1.1\r\nHost: x\r\n\r\n"])
    |> Test.assert_equal(Text.concat(["HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nabcdef", ok("ok")]))
  trickle(["POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n3\r", "\nabc\r\n0\r", "\n\r\n"])
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nabc")
}

test "a head that never ends is 431 however it arrives" {
  let long = refused(431, "Request Header Fields Too Large", "request header fields too large")
  exchange("GET /health HTTP/1.1\r\nHost: x\r\nX: ${Text.repeat("a", 70000)}") |> Test.assert_equal(long)
  trickle(List.repeat(Text.repeat("a", 1000), 70)) |> Test.assert_equal(long)
  // The limit counts the head without its blank line, inclusively.
  let prefix = "GET /health HTTP/1.1\r\nHost: x\r\nX: "
  let fill = fn(size) { Text.concat([prefix, Text.repeat("a", size - Text.byte_length(prefix)), "\r\n\r\n"]) }
  exchange(fill(65536)) |> Test.assert_equal(ok("ok"))
  exchange(fill(65537)) |> Test.assert_equal(long)
}

test "empty lines before a request are skipped" {
  trickle(["\r\n\r", "\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\n\r\n"])
    |> Test.assert_equal(Text.concat([ok("ok"), ok("ok")]))
}

test "the header count limit" {
  let ninety_nine = Text.repeat("X: y\r\n", 99)
  exchange("GET /health HTTP/1.1\r\nHost: x\r\n${ninety_nine}\r\n") |> Test.assert_equal(ok("ok"))
}

test "repeated equal lengths are one length" {
  exchange("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nhi")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi")
  exchange("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 2, 2\r\n\r\nhi")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi")
}

test "absolute and asterisk targets" {
  exchange("GET http://example.com/users/3?a=b HTTP/1.1\r\nHost: x\r\n\r\n") |> Test.assert_equal(ok("user 3 a=b"))
  exchange("GET HTTP://example.com:8080/host HTTP/1.1\r\nHost: other\r\n\r\n") |> Test.assert_equal(ok("example.com:8080"))
  exchange("OPTIONS * HTTP/1.1\r\nHost: x\r\n\r\n") |> Test.assert_equal(ok("star"))
}

test "100-continue waits for the client and is HTTP/1.1 only" {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, handle) })
  Stream.write(Net.writer(client), Bytes.from_text("POST /echo HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\n"))
  Stream.read(Net.reader(client)) |> Test.assert_equal(Some(Bytes.from_text("HTTP/1.1 100 Continue\r\n\r\n")))
  Stream.write(Net.writer(client), Bytes.from_text("hi"))
  Stream.close(Net.writer(client))
  undated(Stream.read_all(Net.reader(client), 1024)) |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi")
  Task.result(serving)
  exchange("POST /echo HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\nhi")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi")
}
"#,
        "virtual",
    );
    assert_passed(&run, 7);
}

// Rejects a response writer that trusts the handler's framing headers, sends
// a length a 204 must not have, or measures a 304's empty body.
#[test]
fn the_server_frames_every_response_itself() {
    let run = run_tests(
        "framing",
        r#"
test "framing headers are the server's" {
  exchange("GET /framed HTTP/1.1\r\nHost: x\r\n\r\n") |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nabc")
}

test "204 and 304" {
  exchange("GET /empty HTTP/1.1\r\nHost: x\r\n\r\nGET /cached HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 204 No Content\r\n\r\nHTTP/1.1 304 Not Modified\r\netag: \"a\"\r\ncontent-length: 10\r\n\r\n")
}
"#,
        "virtual",
    );
    assert_passed(&run, 2);
}

// Rejects a server that a slow or failing peer can stop, or that leaves a
// connection open after a defect: a client that never
// reads its answers is dropped after the write limit, a handler's defect
// still closes the connection, and failed accepts are retried with a
// growing pause until the listener closes.
#[test]
fn slow_clients_and_failed_accepts_do_not_stop_the_server_and_a_defect_closes_its_connection() {
    let run = run_tests(
        "resilience",
        r#"
test "a client that never reads is dropped after a minute" {
  let (client, server) = Net.pair(8)
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, handle) })
  Stream.write(Net.writer(client), Bytes.from_text(Text.repeat("GET /health HTTP/1.1\r\nHost: x\r\n\r\n", 50)))
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(60))
}

test "a closing answer stops writing, then drops what the client still sends for up to 2 s" {
  let (client, server) = Net.pair(64)
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, handle) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /bye HTTP/1.1\r\nHost: x\r\n\r\n"))
  let answer = undated(Stream.read_all(Net.reader(client), 1024))
  Stream.write(Net.writer(client), Bytes.from_text("more than the server reads"))
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(2))
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-length: 3\r\nconnection: close\r\n\r\nbye")
  let (other, end) = Net.pair(64)
  let again = Clock.now()
  let closing = Task.start(fn() { Http.serve_connection(end, Http.defaults, handle) })
  Stream.write(Net.writer(other), Bytes.from_text("GET /bye HTTP/1.1\r\nHost: x\r\n\r\n"))
  let _ = Stream.read_all(Net.reader(other), 1024)
  Stream.close(Net.writer(other))
  Task.result(closing)
  Clock.since(again) |> Test.assert_equal(Duration.zero)
  let (sender, receiving) = Net.pair(64)
  let refused_at = Clock.now()
  let refusing = Task.start(fn() { Http.serve_connection(receiving, { ..Http.defaults, max_body_bytes: 3 }, handle) })
  Stream.write(Net.writer(sender), Bytes.from_text("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 9\r\n\r\n"))
  let _ = Stream.read_all(Net.reader(sender), 1024)
  Stream.write(Net.writer(sender), Bytes.from_text("123456789"))
  Task.result(refusing)
  Clock.since(refused_at) |> Test.assert_equal(Duration.seconds(2))
}

test "a handler's defect closes the connection" {
  let (client, server) = Net.pair(8)
  Stream.write(Net.writer(client), Bytes.from_text("GET /crash HTTP/1.1\r\nHost: x\r\n\r\n"))
  Stream.close(Net.writer(client))
  Test.capture_defect(fn() { Http.serve_connection(server, Http.defaults, handle) }) |> Test.assert_equal("boom")
  Stream.read_all(Net.reader(client), 1024) |> Test.assert_equal(Bytes.empty())
}

// A listener that follows `plan`: each 1 fails as if out of file
// descriptors, a 2 hands over `connection`, and the plan's end closes it.
fn flaky(connection: Connection, plan: Reader<StreamError>) -> Listener {
  Net.listener(fn() {
    case Result.capture(fn() { Stream.read(plan) }) {
      Ok(Some(step)) -> case step == Bytes.from([1]) {
        True -> Failure(NetError.TooManyOpen)
        False -> connection
      }
      _ -> Failure(NetError.Closed)
    }
  }, Net.address("fake", 3), fn() { Void })
}

test "failed accepts are retried" {
  let (client, server) = Net.pair(8)
  Stream.write(Net.writer(client), Bytes.from_text("GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  Stream.write(script, Bytes.from([1]))
  Stream.write(script, Bytes.from([2]))
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve(flaky(server, plan), Http.defaults, handle) })
  let answer = undated(Stream.read_all(Net.reader(client), 1024))
  Stream.close(script)
  Task.result(serving)
  // Three paused accepts, then the drain waits for the answered connection's
  // 2 s linger, since this client never closes its side.
  Clock.since(start) |> Test.assert_equal(Duration.milliseconds(2030))
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
}
"#,
        "virtual",
    );
    assert_passed(&run, 4);
}

// Rejects a drain that cuts off a request in progress, leaves an idle
// connection open (including one waiting for its next request), keeps a
// draining connection alive for another request, returns before the
// connections have closed, or cannot be bounded by a timeout around `serve`.
#[test]
fn closing_the_listener_drains_the_connections() {
    let run = run_tests(
        "drain",
        r#"
fn slow(request: Request) -> Response needs { clock: Clock } {
  case Http.path(request) {
    ["slow"] -> {
      Clock.sleep(Duration.seconds(5))
      Http.text(200, "slow")
    }
    ["one"] -> {
      Clock.sleep(Duration.seconds(1))
      Http.text(200, "one")
    }
    _ -> Http.text(200, "fast")
  }
}

// A listener that hands over `first` and then `second`, and then closes
// once `plan` ends.
fn two(first: Connection, second: Connection, plan: Reader<StreamError>) -> Listener {
  Net.listener(fn() {
    case Result.capture(fn() { Stream.read(plan) }) {
      Ok(Some(step)) -> case step == Bytes.from([1]) {
        True -> first
        False -> second
      }
      _ -> Failure(NetError.Closed)
    }
  }, Net.address("fake", 3), fn() { Void })
}

test "an idle connection closes at once and a request in progress is answered" {
  let (idle, idle_end) = Net.pair(64)
  let (busy, busy_end) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  Stream.write(script, Bytes.from([2]))
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve(two(idle_end, busy_end, plan), Http.defaults, slow) })
  Stream.write(Net.writer(busy), Bytes.from_text("GET /slow HTTP/1.1\r\nHost: x\r\n\r\nGET /fast HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  let idle_answer = Task.start(fn() {
    let read = Stream.read_all(Net.reader(idle), 1024)
    (read, Clock.since(start))
  })
  let busy_answer = undated(Stream.read_all(Net.reader(busy), 1024))
  Clock.since(start) |> Test.assert_equal(Duration.seconds(5))
  Stream.close(Net.writer(busy))
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(5))
  Task.result(idle_answer) |> Test.assert_equal((Bytes.empty(), Duration.seconds(1)))
  // The pipelined second request is not answered: the drain made the first
  // answer close the connection.
  busy_answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 4\r\nconnection: close\r\n\r\nslow")
}

test "a timeout around serve cuts the drain short" {
  let (busy, busy_end) = Net.pair(64)
  let (other, other_end) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let start = Clock.now()
  let serving = Task.start(fn() {
    Result.capture(fn() { Task.timeout(fn() { Http.serve(two(busy_end, other_end, plan), Http.defaults, slow) }, Duration.seconds(3)) })
  })
  Stream.write(Net.writer(busy), Bytes.from_text("GET /slow HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  let cut = Stream.read_all(Net.reader(busy), 1024)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(3))
  cut |> Test.assert_equal(Bytes.empty())
  let _ = Task.result(serving)
  Net.close(other)
}

// A listener that hands over `first`, then closes once `plan` ends.
fn once(first: Connection, plan: Reader<StreamError>) -> Listener {
  two(first, first, plan)
}

test "a connection answered keep-alive before the drain closes once it is idle" {
  let (client, server_end) = Net.pair(1)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let start = Clock.now()
  let serving = Task.start(fn() {
    Http.serve(once(server_end, plan), Http.defaults, slow)
    Clock.since(start)
  })
  // The client does not read, so the answers, both keep-alive, are still
  // being written when the drain starts; the connection must not then wait
  // for a third request until the 60 s request timeout.
  Stream.write(Net.writer(client), Bytes.from_text("GET /fast HTTP/1.1\r\nHost: x\r\n\r\nGET /fast HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  Clock.sleep(Duration.seconds(1))
  let answer = undated(Stream.read_all(Net.reader(client), 1048576))
  Stream.close(Net.writer(client))
  Task.result(serving) |> Test.assert_equal(Duration.seconds(2))
  let fast = "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 4\r\n"
  answer |> Test.assert_equal("${fast}\r\nfast${fast}\r\nfast")
}

test "a connection waiting for its next request closes at once" {
  let (client, server_end) = Net.pair(1024)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let start = Clock.now()
  let serving = Task.start(fn() {
    Http.serve(once(server_end, plan), Http.defaults, slow)
    Clock.since(start)
  })
  // The answer is written in full, so the connection is waiting for a
  // second request when the drain starts.
  Stream.write(Net.writer(client), Bytes.from_text("GET /fast HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  let answer = undated(Stream.read_all(Net.reader(client), 1024))
  Stream.close(Net.writer(client))
  Task.result(serving) |> Test.assert_equal(Duration.seconds(1))
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 4\r\n\r\nfast")
}

test "a connection admitted as the listener closes is closed at once" {
  let (client, server_end) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  Stream.close(script)
  let start = Clock.now()
  Http.serve(once(server_end, plan), Http.defaults, slow)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(0))
  Stream.read_all(Net.reader(client), 1024) |> Test.assert_equal(Bytes.empty())
}

test "a request whose first bytes came before the drain is answered" {
  let (client, server_end) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let serving = Task.start(fn() { Http.serve(once(server_end, plan), Http.defaults, slow) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /fast HTTP/1.1\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  Clock.sleep(Duration.seconds(1))
  Stream.write(Net.writer(client), Bytes.from_text("Host: x\r\n\r\n"))
  let answer = undated(Stream.read_all(Net.reader(client), 1024))
  Stream.close(Net.writer(client))
  Task.result(serving)
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 4\r\nconnection: close\r\n\r\nfast")
}

test "a pipelined request read before the drain is answered" {
  let (client, server_end) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let serving = Task.start(fn() { Http.serve(once(server_end, plan), Http.defaults, slow) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /one HTTP/1.1\r\nHost: x\r\n\r\nGET /fast HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  let answer = undated(Stream.read_all(Net.reader(client), 1024))
  Stream.close(Net.writer(client))
  Task.result(serving)
  let head = "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\n"
  answer |> Test.assert_equal("${head}content-length: 3\r\n\r\none${head}content-length: 4\r\nconnection: close\r\n\r\nfast")
}

test "busy connections drain together" {
  let (first, first_end) = Net.pair(64)
  let (second, second_end) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  Stream.write(script, Bytes.from([2]))
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve(two(first_end, second_end, plan), Http.defaults, slow) })
  Stream.write(Net.writer(first), Bytes.from_text("GET /slow HTTP/1.1\r\nHost: x\r\n\r\n"))
  Stream.write(Net.writer(second), Bytes.from_text("GET /slow HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(1))
  Stream.close(script)
  let other = Task.start(fn() {
    let _ = Stream.read_all(Net.reader(second), 1024)
    Stream.close(Net.writer(second))
  })
  let _ = Stream.read_all(Net.reader(first), 1024)
  Stream.close(Net.writer(first))
  Task.result(other)
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(5))
}
"#,
        "virtual",
    );
    assert_passed(&run, 8);
}

// Rejects a streamed response that is framed wrongly (chunk sizes, the last
// chunk, HTTP/1.0 without a close, a body for HEAD), that keeps a
// connection open after a failed producer, that bounds the whole response by
// the write limit instead of each write, or that lets a client which stops
// reading, or that leaves, hold the connection forever. Also rejects a body
// ended as whole after a write failed that the producer caught, and a drain
// under `serve` that cuts a stream in progress short, writes from the
// producer's tasks that interleave inside a chunk, a write after the writer
// was closed, and a producer's defect that leaves the connection open.
#[test]
fn a_streamed_response_is_sent_chunked_as_it_is_written() {
    let run = run_tests(
        "streamed",
        r#"
fn streaming(request: Request) -> Response needs { clock: Clock } {
  let given = clock
  case Http.path(request) {
    ["count"] -> Http.streamed(200, [("content-type", "text/plain")], fn(out) {
      List.each(["one", "two", "three"], fn(word) {
        Stream.write(out, Bytes.from_text(word))
        Clock.sleep(Duration.milliseconds(5)) using { clock: given }
      })
    })
    ["fail"] -> Http.streamed(200, [], fn(out) {
      Stream.write(out, Bytes.from_text("partial"))
      Failure(Broken.Broken)
    })
    ["big"] -> Http.streamed(200, [], fn(out) {
      Stream.write(out, Bytes.from_text(Text.repeat("x", 300)))
    })
    _ -> Http.text(200, "plain")
  }
}

fn stream_exchange(raw: Text) -> Text needs { clock: Clock } {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, streaming) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  Stream.close(Net.writer(client))
  let answer = Stream.read_all(Net.reader(client), 33554432)
  Task.result(serving)
  undated(answer)
}

test "chunked" {
  stream_exchange("GET /count HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n3\r\none\r\n3\r\ntwo\r\n5\r\nthree\r\n0\r\n\r\nHTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\n\r\nplain")
}

test "big" {
  stream_exchange("GET /big HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n12c\r\n${Text.repeat("x", 300)}\r\n0\r\n\r\n")
}

test "http 1.0" {
  stream_exchange("GET /count HTTP/1.0\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\nonetwothree")
}

test "head" {
  stream_exchange("HEAD /count HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\nHTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\n\r\nplain")
}

test "failure cuts it short" {
  stream_exchange("GET /fail HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n7\r\npartial\r\n")
}

test "http 1.0 keep-alive still closes" {
  stream_exchange("GET /count HTTP/1.0\r\nConnection: keep-alive\r\n\r\nGET /other HTTP/1.0\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\nonetwothree")
}

fn slow_stream(request: Request) -> Response needs { clock: Clock } {
  let given = clock
  case Http.path(request) {
    ["slow"] -> Http.streamed(200, [], fn(out) {
      List.each([1, 2, 3, 4, 5], fn(_n) {
        Clock.sleep(Duration.seconds(30)) using { clock: given }
        Stream.write(out, Bytes.from_text("."))
      })
    })
    _ -> Http.streamed(200, [], fn(out) {
      loop _more = True {
        Stream.write(out, Bytes.from_text(Text.repeat("y", 1000)))
        True
      }
    })
  }
}

test "a slow stream may outlast the write limit" {
  let (client, server) = Net.pair(64)
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, slow_stream) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /slow HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
  let answer = undated(Stream.read_all(Net.reader(client), 1024))
  Stream.close(Net.writer(client))
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(150))
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n1\r\n.\r\n1\r\n.\r\n1\r\n.\r\n1\r\n.\r\n1\r\n.\r\n0\r\n\r\n")
}

test "an endless stream to a client that never reads is dropped after the write limit" {
  let (client, server) = Net.pair(8)
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, slow_stream) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /endless HTTP/1.1\r\nHost: x\r\n\r\n"))
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.seconds(62))
}

test "a stream cannot be 204 or 304, and its body reads as empty" {
  Test.capture_defect(fn() { Http.streamed(204, [], fn(_out) { Void }) }) |> Test.assert_equal("Http: a 204 response has no body to stream")
  Test.capture_defect(fn() { Http.streamed(304, [], fn(_out) { Void }) }) |> Test.assert_equal("Http: a 304 response has no body to stream")
  Http.response_body(Http.streamed(200, [], fn(_out) { Void })) |> Test.assert_equal(Bytes.empty())
  Http.status(Http.streamed(201, [("X-A", "b")], fn(_out) { Void })) |> Test.assert_equal(201)
  Test.capture_defect(fn() { Http.streamed(700, [], fn(_out) { Void }) }) |> Test.assert_equal("Http: the status 700 is outside 200 to 599")
}

test "a client that leaves ends the stream at once" {
  let (client, server) = Net.pair(8)
  let start = Clock.now()
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, slow_stream) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /endless HTTP/1.1\r\nHost: x\r\n\r\n"))
  let _ = Stream.read(Net.reader(client))
  Net.close(client)
  Task.result(serving)
  Clock.since(start) |> Test.assert_equal(Duration.zero)
}

fn careless(request: Request) -> Response needs { clock: Clock } {
  let given = clock
  case Http.path(request) {
    ["caught"] -> Http.streamed(200, [], fn(out) {
      List.each([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], fn(_n) {
        let _ = Result.capture(fn() { Stream.write(out, Bytes.from_text(Text.repeat("c", 1000))) })
        Void
      })
      Stream.write(out, Bytes.from_text("z"))
    })
    _ -> Http.text(200, "plain")
  }
}

test "a write failure the producer caught still cuts the body short" {
  let (client, server) = Net.pair(8)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, careless) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /caught HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.seconds(70))
  let answer = undated(Stream.read_all(Net.reader(client), 1048576))
  Stream.close(Net.writer(client))
  Task.result(serving)
  (Text.contains(answer, "1\r\nz"), Text.contains(answer, "0\r\n\r\n"), Text.contains(answer, "plain")) |> Test.assert_equal((False, False, False))
  Text.starts_with(answer, "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n3e8\r\nccc") |> Test.assert_equal(True)
}

// A listener that hands over `connection`, then closes once `plan` ends.
fn handing(connection: Connection, plan: Reader<StreamError>) -> Listener {
  Net.listener(fn() {
    case Result.capture(fn() { Stream.read(plan) }) {
      Ok(Some(_)) -> connection
      _ -> Failure(NetError.Closed)
    }
  }, Net.address("fake", 3), fn() { Void })
}

test "under serve, a drain lets a stream in progress finish, then closes" {
  let (client, server) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let start = Clock.now()
  let serving = Task.start(fn() {
    Http.serve(handing(server, plan), Http.defaults, streaming)
    Clock.since(start)
  })
  Stream.write(Net.writer(client), Bytes.from_text("GET /count HTTP/1.1\r\nHost: x\r\n\r\n"))
  Clock.sleep(Duration.milliseconds(7))
  Stream.close(script)
  let answer = undated(Stream.read_all(Net.reader(client), 1048576))
  Stream.close(Net.writer(client))
  Task.result(serving) |> Test.assert_equal(Duration.milliseconds(15))
  answer |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n3\r\none\r\n3\r\ntwo\r\n5\r\nthree\r\n0\r\n\r\n")
}
fn racing(request: Request) -> Response needs { clock: Clock } {
  let given = clock
  case Http.path(request) {
    ["race"] -> Http.streamed(200, [], fn(out) {
      Stream.write(out, Bytes.from_text("a"))
      let _other = Task.start(fn() {
        Clock.sleep(Duration.milliseconds(10)) using { clock: given }
        Stream.write(out, Bytes.from_text(Text.repeat("L", 100000)))
      })
      Clock.sleep(Duration.milliseconds(100)) using { clock: given }
    })
    ["closed"] -> Http.streamed(200, [], fn(out) {
      Stream.write(out, Bytes.from_text("a"))
      Stream.close(out)
      let late = Result.capture(fn() { Stream.write(out, Bytes.from_text("b")) })
      case late {
        Ok(_) -> Debug.crash("a write after close went through")
        Err(_) -> Void
      }
    })
    ["crash"] -> Http.streamed(200, [], fn(out) {
      Stream.write(out, Bytes.from_text("a"))
      Debug.crash("producer")
    })
    ["done"] -> Http.streamed(200, [], fn(out) {
      Stream.close(out)
      Clock.sleep(Duration.milliseconds(10)) using { clock: given }
      Debug.crash("a producer ran on after its body ended")
    })
    ["later"] -> {
      Clock.sleep(Duration.milliseconds(50))
      Http.text(200, "plain")
    }
    _ -> Http.text(200, "plain")
  }
}
fn race_exchange(raw: Text) -> Text needs { clock: Clock } {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, Http.defaults, racing) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  Stream.close(Net.writer(client))
  let answer = Stream.read_all(Net.reader(client), 33554432)
  let _ = Result.capture(fn() { Task.result(serving) })
  undated(answer)
}
test "writes from the producer's tasks stay whole chunks of the body" {
  race_exchange("GET /race HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n186a0\r\n${Text.repeat("L", 100000)}\r\n0\r\n\r\nHTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\n\r\nplain")
}
test "closing the writer ends the body, and later writes fail" {
  race_exchange("GET /closed HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\nHTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\n\r\nplain")
}
test "a producer that closed its writer is stopped before the next request" {
  race_exchange("GET /done HTTP/1.1\r\nHost: x\r\n\r\nGET /later HTTP/1.1\r\nHost: x\r\n\r\n")
    |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\nHTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\n\r\nplain")
}

test "under serve, a producer's defect closes only its connection" {
  let (client, server) = Net.pair(64)
  let (script, plan) = Stream.pipe(8)
  Stream.write(script, Bytes.from([1]))
  let serving = Task.start(fn() { Http.serve(handing(server, plan), Http.defaults, racing) })
  Stream.write(Net.writer(client), Bytes.from_text("GET /crash HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\n\r\n"))
  let answer = undated(Stream.read_all(Net.reader(client), 1048576))
  Stream.close(Net.writer(client))
  Stream.close(script)
  Task.result(serving)
  // The defect cancels the connection's task, so a chunk not yet sent is
  // lost with it; what matters is that the body never ends as whole.
  Text.starts_with(answer, "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n") |> Test.assert_equal(True)
  (Text.contains(answer, "0\r\n\r\n"), Text.contains(answer, "plain")) |> Test.assert_equal((False, False))
}
"#,
        "virtual",
    );
    assert_passed(&run, 16);
}

// Rejects a streamed body whose writer still works after the producer
// returns: a task the producer handed it to would write after the last
// chunk, into what the client reads as the next response. Over loopback,
// because a fake pair's channel ends belong to the task that last used them.
#[test]
fn a_task_cannot_write_a_streamed_body_after_its_producer_returns() {
    let run = run_tests(
        "streamed-loopback",
        r#"
fn leaky(request: Request) -> Response needs { clock: Clock } {
  let given = clock
  case Http.path(request) {
    ["leak"] -> Http.streamed(200, [], fn(out) {
      Stream.write(out, Bytes.from_text("a"))
      let _late = Task.start(fn() {
        Clock.sleep(Duration.milliseconds(20)) using { clock: given }
        Stream.write(out, Bytes.from_text("LATE"))
      })
      Void
    })
    _ -> {
      Clock.sleep(Duration.milliseconds(50)) using { clock: given }
      Http.text(200, "plain")
    }
  }
}

fn leaked() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, Http.defaults, leaky) })
  let answer = Resource.scoped(fn() { Net.connect("127.0.0.1", port) }, Net.close, fn(connection) {
    Stream.write(Net.writer(connection), Bytes.from_text("GET /leak HTTP/1.1\r\nHost: x\r\n\r\nGET /other HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
    Task.timeout(fn() { undated(Stream.read_all(Net.reader(connection), 1048576)) }, Duration.seconds(5))
  })
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  answer
}

test "a task the producer started cannot write after it returns" {
  let outcome = leaked() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\nHTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 5\r\nconnection: close\r\n\r\nplain")
}
"#,
        "real",
    );
    assert_passed(&run, 1);
}
