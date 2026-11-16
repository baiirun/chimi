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
import std/clock as Clock
import std/clock.{Clock}
import std/debug as Debug
import std/duration as Duration
import std/http as Http
import std/http.{Method, Request, Response}
import std/net as Net
import std/net.{Network}
import std/resource as Resource
import std/result as Result
import std/stream as Stream
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
    (Method.Other(name), _) -> Http.text(405, "no ${name}")
    _ -> Http.text(404, "not found")
  }
}

fn exchange(raw: Text) -> Text {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, handle) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  Stream.close(Net.writer(client))
  let answer = Stream.read_all(Net.reader(client), 33554432)
  Task.result(serving)
  Bytes.to_text(answer)
}

// Sends `raw` without closing the client's writing side: the answer ends only
// if the server closes the connection.
fn exchange_open(raw: Text) -> Text {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { Http.serve_connection(server, handle) })
  Stream.write(Net.writer(client), Bytes.from_text(raw))
  let answer = Stream.read_all(Net.reader(client), 1048576)
  Task.result(serving)
  Bytes.to_text(answer)
}

fn ok(body: Text) -> Text {
  "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: ${Text.byte_length(body)}\r\n\r\n${body}"
}

fn refused(status: Int, reason: Text, body: Text) -> Text {
  "HTTP/1.1 ${status} ${reason}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: ${Text.byte_length(body)}\r\nconnection: close\r\n\r\n${body}"
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
    |> Test.assert_equal(Text.concat([ok("ok"), "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"]))
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
  exchange("GET /health HTTP/1.1\r\nHost:") |> Test.assert_equal(refused(400, "Bad Request", "bad request: the connection ended inside a head"))
  exchange("") |> Test.assert_equal("")
}

test "501 and 505" {
  after("POST /echo HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip\r\n\r\n")
    |> Test.assert_equal(refused(501, "Not Implemented", "not implemented: transfer-encoding gzip"))
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
  let many = Text.repeat("X: y\r\n", 101)
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

test "a status outside 100 to 599 is a defect" {
  Test.capture_defect(fn() { Http.text(600, "x") }) |> Test.assert_equal("Http: the status 600 is outside 100 to 599")
}

test "a header that would split the response is a defect" {
  Test.capture_defect(fn() { Http.response(200, [("Location", "/a\r\nSet-Cookie: x=1")], Bytes.empty()) })
    |> Test.assert_equal("Http: the value of the header Location holds a line break")
  Test.capture_defect(fn() { Http.response(200, [("Bad Name", "x")], Bytes.empty()) })
    |> Test.assert_equal("Http: the header name \"Bad Name\" is not a token")
}

test "a handler can be tested without a connection" {
  let request = Http.request(Method.Get, ["users", "9"], [("X-Trace", "abc")], Bytes.empty())
  Http.header(request, "x-trace") |> Test.assert_equal(Some("abc"))
  Http.header(request, "X-TRACE") |> Test.assert_equal(Some("abc"))
  Http.header(request, "missing") |> Test.assert_equal(None)
  let response = handle(request)
  Http.status(response) |> Test.assert_equal(200)
  Http.response_body(response) |> Test.assert_equal(Bytes.from_text("user 9 "))
}
"#,
        "virtual",
    );
    assert_passed(&run, 5);
}

// Rejects a server that serves one client at a time or never stops: two
// clients are answered while both hold a connection open, and closing the
// listener ends `serve`.
#[test]
fn a_server_on_loopback_serves_clients_at_once_and_stops_with_its_listener() {
    let run = run_tests(
        "loopback",
        r#"
fn get(port: Int, path: Text) -> Text needs { network: Network } {
  Resource.scoped(fn() { Net.connect("127.0.0.1", port) }, Net.close, fn(connection) {
    Stream.write(Net.writer(connection), Bytes.from_text("GET ${path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"))
    Bytes.to_text(Stream.read_all(Net.reader(connection), 1048576))
  })
}

fn run_server() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, handle) })
  // The first client holds its connection open without a request while the
  // second is answered.
  let idle = Net.connect("127.0.0.1", port)
  let answer = get(port, "/users/1")
  let other = get(port, "/health")
  Net.close(idle)
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  "${answer}|${other}"
}

test "two clients and a stop" {
  let outcome = run_server() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 7\r\nconnection: close\r\n\r\nuser 1 |HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
}
"#,
        "real",
    );
    assert_passed(&run, 1);
}
