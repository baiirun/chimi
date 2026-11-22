//! Conformance for the `std/http` client (CHI-257): the request it writes,
//! how it frames every kind of reply, its limits, the replies it refuses, the
//! URLs it accepts, and its timeout, over a fake `Net.pair` with a scripted
//! server; then `Http.send` against `Http.serve` over loopback.
//!
//! Each scripted test records the raw request the server read and shows the
//! reply the client returned, so the wire format is what is checked.

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-http-client-{name}-{}-{:?}",
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

/// `scripted` sends a request with `Http.send_over` to a server that reads
/// the request (its head, then `extra` more bytes), writes each of `reply` as
/// its own write and closes. It returns `request => outcome`, where the
/// outcome is `status [name=value;...] body` or the failure.
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
import std/task as Task
import std/test as Test
import std/text as Text
import std/tls.{Roots}

fn rendered(reply: Response) -> Text {
  let headers = Text.join(List.map(Http.response_headers(reply), fn(header) {
    let (name, value) = header
    "${name}=${value}"
  }), ";")
  "${Http.status(reply)} [${headers}] ${Bytes.to_text(Http.response_body(reply))}"
}

// Reads a request head and `extra` more bytes, or until the client closes.
fn read_request(connection: Connection, extra: Int) -> Text {
  let reader = Net.reader(connection)
  let read = loop seen = Bytes.empty() {
    let text = Bytes.to_text(seen)
    let complete = case Text.split_once(text, "\r\n\r\n") {
      Some((_, after)) -> Text.byte_length(after) >= extra
      None -> False
    }
    case complete {
      True -> break seen
      False -> {
        let chunk = Stream.read(reader)
        case chunk {
          Some(more) -> Bytes.append(seen, more)
          None -> break seen
        }
      }
    }
  }
  Bytes.to_text(read)
}

fn answer(connection: Connection, extra: Int, reply: List<Text>) -> Text {
  let request = read_request(connection, extra)
  let _ = Result.capture(fn() {
    List.each(reply, fn(piece) { Stream.write(Net.writer(connection), Bytes.from_text(piece)) })
  })
  Net.close(connection)
  request
}

fn scripted_with(settings: { roots: Roots, timeout: Duration, max_head_bytes: Int, max_headers: Int, max_body_bytes: Int }, method: Method, url: Text, headers: List<(Text, Text)>, body: Text, reply: List<Text>) -> Text needs { clock: Clock } {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() { answer(server, Text.byte_length(body), reply) })
  let outcome = Result.capture(fn() { Http.send_over(client, method, url, headers, Bytes.from_text(body), settings) })
  Net.close(client)
  let request = Task.result(serving)
  let shown = case outcome {
    Ok(response) -> rendered(response)
    Err(failure) -> Debug.render(failure)
  }
  "${request}=>${shown}"
}

fn scripted(method: Method, url: Text, reply: List<Text>) -> Text needs { clock: Clock } {
  scripted_with(Http.client_defaults, method, url, [], "", reply)
}

// The outcome alone, for tests about the reply.
fn replied(reply: List<Text>) -> Text needs { clock: Clock } {
  let exchanged = scripted(Method.Get, "http://x/", reply)
  case Text.split_once(exchanged, "=>") {
    Some((_, outcome)) -> outcome
    None -> "no outcome"
  }
}

// A URL's failure, before any byte is sent.
fn refused_url(url: Text) -> Text needs { clock: Clock } {
  let (client, server) = Net.pair(64)
  let outcome = Result.capture(fn() { Http.send_over(client, Method.Get, url, [], Bytes.empty(), Http.client_defaults) })
  Net.close(client)
  Net.close(server)
  case outcome {
    Ok(response) -> rendered(response)
    Err(failure) -> Debug.render(failure)
  }
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

// Rejects a client that writes a request other than HTTP/1.1 with `host`,
// `connection: close` and its own framing: a missing or wrong `host` (default
// and other ports, IPv6), the caller's framing or connection headers kept, no
// `content-length` on a POST without a body, a body sent unframed, a fragment
// sent, a query without a path, a method that is not upper case, or the
// caller's own `host` overridden.
#[test]
fn the_request_is_http_1_1_framed_by_the_client() {
    let run = run_tests(
        "request",
        r#"
fn request_of(method: Method, url: Text, headers: List<(Text, Text)>, body: Text) -> Text needs { clock: Clock } {
  let exchanged = scripted_with(Http.client_defaults, method, url, headers, body, ["HTTP/1.1 204 No Content\r\n\r\n"])
  case Text.split_once(exchanged, "=>") {
    Some((request, _)) -> request
    None -> "no request"
  }
}

test "a GET" {
  request_of(Method.Get, "http://example.com/users/7?q=1#top", [("Accept", "text/plain")], "")
    |> Test.assert_equal("GET /users/7?q=1 HTTP/1.1\r\nhost: example.com\r\naccept: text/plain\r\nconnection: close\r\n\r\n")
}

test "ports and hosts" {
  let other = request_of(Method.Get, "http://example.com:8080", [], "")
  let secure_default = request_of(Method.Get, "https://example.com:443/", [], "")
  let plain_on_443 = request_of(Method.Get, "http://example.com:443/", [], "")
  let six = request_of(Method.Get, "http://[::1]:9000/a", [], "")
  let query = request_of(Method.Get, "HTTP://Example.com?x=1", [], "")
  "${other}|${secure_default}|${plain_on_443}|${six}|${query}"
    |> Test.assert_equal("GET / HTTP/1.1\r\nhost: example.com:8080\r\nconnection: close\r\n\r\n|GET / HTTP/1.1\r\nhost: example.com\r\nconnection: close\r\n\r\n|GET / HTTP/1.1\r\nhost: example.com:443\r\nconnection: close\r\n\r\n|GET /a HTTP/1.1\r\nhost: [::1]:9000\r\nconnection: close\r\n\r\n|GET /?x=1 HTTP/1.1\r\nhost: Example.com\r\nconnection: close\r\n\r\n")
}

test "a body is framed by its length" {
  let posted = request_of(Method.Post, "http://x/echo", [("Content-Type", "text/plain"), ("Content-Length", "99"), ("Transfer-Encoding", "chunked"), ("Connection", "keep-alive")], "hello")
  let empty = request_of(Method.Post, "http://x/", [], "")
  let put = request_of(Method.Put, "http://x/", [], "")
  let patch = request_of(Method.Patch, "http://x/", [], "")
  let deleted = request_of(Method.Delete, "http://x/", [], "gone")
  let other = request_of(Method.Other("PURGE"), "http://x/", [], "")
  "${posted}|${empty}|${put}|${patch}|${deleted}|${other}"
    |> Test.assert_equal("POST /echo HTTP/1.1\r\nhost: x\r\ncontent-type: text/plain\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello|POST / HTTP/1.1\r\nhost: x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n|PUT / HTTP/1.1\r\nhost: x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n|PATCH / HTTP/1.1\r\nhost: x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n|DELETE / HTTP/1.1\r\nhost: x\r\ncontent-length: 4\r\nconnection: close\r\n\r\ngone|PURGE / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
}

test "a HEAD spelled as another method reads no body" {
  let exchanged = scripted_with(Http.client_defaults, Method.Other("HEAD"), "http://x/", [], "", ["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n"])
  exchanged |> Test.assert_equal("HEAD / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n=>200 [content-length=5] ")
}

test "two host headers defect" {
  Test.capture_defect(fn() { request_of(Method.Get, "http://x/", [("Host", "a"), ("host", "b")], "") })
    |> Test.assert_equal("Http: a request has one host header")
}

test "the caller's host stands" {
  request_of(Method.Get, "http://10.0.0.1/", [("Host", "api.example.com")], "")
    |> Test.assert_equal("GET / HTTP/1.1\r\nhost: api.example.com\r\nconnection: close\r\n\r\n")
}

test "a header that is not a token defects" {
  let name = Test.capture_defect(fn() { request_of(Method.Get, "http://x/", [("bad name", "v")], "") })
  let value = Test.capture_defect(fn() { request_of(Method.Get, "http://x/", [("name", "a\r\nb: c")], "") })
  let method = Test.capture_defect(fn() { request_of(Method.Other("GET X"), "http://x/", [], "") })
  let tunnel = Test.capture_defect(fn() { request_of(Method.Other("CONNECT"), "http://x/", [], "") })
  "${name}|${value}|${method}|${tunnel}"
    |> Test.assert_equal("Http: the header name \"bad name\" is not a token|Http: the value of the header name holds a control character|Http: the method \"GET X\" is not a token|Http: CONNECT opens a tunnel, which Http.send does not")
}
"#,
        "virtual",
    );
    assert_passed(&run, 7);
}

// Rejects a URL parser that sends what it should refuse, or refuses what it
// should send: other schemes, userinfo, a missing host, bad or out-of-range
// ports, an unclosed IPv6 bracket, text after one, spaces or controls.
#[test]
fn a_url_the_client_cannot_send_to_fails_before_any_io() {
    let run = run_tests(
        "urls",
        r#"
test "refused URLs" {
  let urls = ["ftp://x/", "x/", "http://user:pw@x/", "http:///a", "http://:80/", "http://x:0/", "http://x:65536/", "http://x:8a/", "http://x:123456/", "http://[::1/", "http://[::1]x/", "http://x y/", "http://x/a b", "http://x/\t", "http://x.com\\y/", "http://[example.com]/", "http://[abc]/", "http://ex%41mple.com/", "http://é.com/", "http://x/a{b}", "http://x/a\"b", "http://x/é", "http://x/a\\b"]
  Text.join(List.map(urls, refused_url), "\n")
    |> Test.assert_equal("HttpError.BadUrl(\"not an http or https URL\")\nHttpError.BadUrl(\"not an http or https URL\")\nHttpError.BadUrl(\"a user name or password in the URL\")\nHttpError.BadUrl(\"no host\")\nHttpError.BadUrl(\"no host\")\nHttpError.BadUrl(\"the port 0 is outside 1 to 65535\")\nHttpError.BadUrl(\"the port 65536 is outside 1 to 65535\")\nHttpError.BadUrl(\"a bad port\")\nHttpError.BadUrl(\"a bad port\")\nHttpError.BadUrl(\"an IPv6 address without its closing bracket\")\nHttpError.BadUrl(\"text after an IPv6 address\")\nHttpError.BadUrl(\"a space or control character\")\nHttpError.BadUrl(\"a space or control character\")\nHttpError.BadUrl(\"a space or control character\")\nHttpError.BadUrl(\"a character a host cannot hold\")\nHttpError.BadUrl(\"a character a host cannot hold\")\nHttpError.BadUrl(\"a character a host cannot hold\")\nHttpError.BadUrl(\"a character a host cannot hold\")\nHttpError.BadUrl(\"a character a host cannot hold\")\nHttpError.BadUrl(\"a character the path or query must percent-encode\")\nHttpError.BadUrl(\"a character the path or query must percent-encode\")\nHttpError.BadUrl(\"a character the path or query must percent-encode\")\nHttpError.BadUrl(\"a character the path or query must percent-encode\")")
}

test "edge ports are sent" {
  let empty = scripted(Method.Get, "http://x:/", ["HTTP/1.1 204 No Content\r\n\r\n"])
  let six = scripted(Method.Get, "http://[::FFFF:1.2.3.4]:/a%20b!$&'()*+,;=:@/?q=%7B~", ["HTTP/1.1 204 No Content\r\n\r\n"])
  let low = scripted(Method.Get, "http://x:1/", ["HTTP/1.1 204 No Content\r\n\r\n"])
  let high = scripted(Method.Get, "http://x:65535/", ["HTTP/1.1 204 No Content\r\n\r\n"])
  "${empty}|${six}|${low}|${high}"
    |> Test.assert_equal("GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n=>204 [] |GET /a%20b!$&'()*+,;=:@/?q=%7B~ HTTP/1.1\r\nhost: [::FFFF:1.2.3.4]\r\nconnection: close\r\n\r\n=>204 [] |GET / HTTP/1.1\r\nhost: x:1\r\nconnection: close\r\n\r\n=>204 [] |GET / HTTP/1.1\r\nhost: x:65535\r\nconnection: close\r\n\r\n=>204 [] ")
}
"#,
        "virtual",
    );
    assert_passed(&run, 2);
}

// Rejects a client that frames a reply's body any other way than RFC 9112
// section 6.3: a body read for HEAD, 204 or 304, chunked not decoded or not
// chosen over content-length, a body cut at or read past its length, a
// conflicting length accepted, a body without a length not read to the end,
// an interim 1xx answer returned, header names not lowercased, or a reply
// split across reads misread.
#[test]
fn a_reply_body_is_framed_as_http_1_1_says() {
    let run = run_tests(
        "framing",
        r#"
test "content-length" {
  let exact = replied(["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello extra"])
  let split = replied(["HTTP/1.1 2", "00 OK\r\nContent-", "Length: 5\r\n", "\r\nhe", "llo"])
  let repeated = replied(["HTTP/1.1 200 OK\r\nContent-Length: 2, 2\r\nContent-Length: 2\r\n\r\nhi"])
  let empty = replied(["HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"])
  "${exact}|${split}|${repeated}|${empty}"
    |> Test.assert_equal("200 [content-length=5] hello|200 [content-length=5] hello|200 [content-length=2, 2;content-length=2] hi|200 [content-length=0] ")
}

test "chunked" {
  let chunked = replied(["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 1\r\n\r\n5\r\nhello\r\n6;x=y\r\n world\r\n0\r\nTrailer: t\r\n\r\n"])
  let coded = replied(["HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"])
  let other = replied(["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\nraw to the end"])
  "${chunked}|${coded}|${other}"
    |> Test.assert_equal("200 [transfer-encoding=chunked;content-length=1] hello world|200 [transfer-encoding=gzip, chunked] abc|200 [transfer-encoding=chunked, gzip] raw to the end")
}

test "codings the client refuses or reads to the end" {
  let twice = replied(["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"])
  let empty = replied(["HTTP/1.1 200 OK\r\nTransfer-Encoding:\r\nContent-Length: 2\r\n\r\nhi"])
  let old = replied(["HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"])
  let coded = replied(["HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nContent-Length: 2\r\n\r\nhi and the rest"])
  "${twice}|${empty}|${old}|${coded}"
    |> Test.assert_equal("HttpError.BadReply(\"chunked is applied twice\")|HttpError.BadReply(\"an empty transfer-encoding\")|HttpError.BadReply(\"transfer-encoding in HTTP/1.0\")|200 [transfer-encoding=gzip;content-length=2] hi and the rest")
}

test "interim replies are bounded" {
  let sixteen = replied([Text.concat(List.repeat("HTTP/1.1 100 Continue\r\n\r\n", 16)), "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"])
  let seventeen = replied([Text.concat(List.repeat("HTTP/1.1 100 Continue\r\n\r\n", 17)), "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"])
  "${sixteen}|${seventeen}"
    |> Test.assert_equal("200 [content-length=0] |HttpError.BadReply(\"more than 16 interim replies\")")
}

test "to the end of the connection" {
  replied(["HTTP/1.1 200 OK\r\n\r\nall ", "of ", "it"])
    |> Test.assert_equal("200 [] all of it")
}

test "no body" {
  let head = scripted(Method.Head, "http://x/", ["HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n"])
  let none = replied(["HTTP/1.1 204 No Content\r\nContent-Length: 5\r\n\r\nhello"])
  let cached = replied(["HTTP/1.1 304 Not Modified\r\nContent-Length: 5\r\n\r\nhello"])
  "${head}|${none}|${cached}"
    |> Test.assert_equal("HEAD / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n=>200 [content-length=5] |204 [content-length=5] |304 [content-length=5] ")
}

test "interim answers are skipped" {
  let continued = replied(["HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </a>\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"])
  let old = replied(["HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok"])
  let blank = replied(["HTTP/1.1 404 \r\nContent-Length: 0\r\n\r\n"])
  let bare = replied(["HTTP/1.1 500\r\nContent-Length: 0\r\n\r\n"])
  "${continued}|${old}|${blank}|${bare}"
    |> Test.assert_equal("200 [content-length=2] ok|200 [content-length=2] ok|404 [content-length=0] |500 [content-length=0] ")
}
"#,
        "virtual",
    );
    assert_passed(&run, 7);
}

// Rejects a client that returns a reply it should refuse, or refuses one it
// should return: a bad status line or version, a status outside 100 to 599,
// 101, a conflicting or non-numeric content-length, a body cut short, bare LF
// or CR, a header without a colon, a reply over each limit, and a limit
// exactly met.
#[test]
fn a_reply_that_breaks_http_1_1_or_a_limit_fails() {
    let run = run_tests(
        "refused",
        r#"
test "malformed replies" {
  let replies = ["", "HTTP/1.1 200 OK\r\n", "HTTP/2 200 OK\r\n\r\n", "HTTP/2.0 200 OK\r\n\r\n", "HTTP/1.1 20 OK\r\n\r\n", "HTTP/1.1 2000 OK\r\n\r\n", "HTTP/1.1 0200 OK\r\n\r\n", "HTTP/1.1 099 OK\r\n\r\n", "HTTP/1.1 600 OK\r\n\r\n", "HTTP/1.1 abc OK\r\n\r\n", "HTTP/1.1 101 Switching Protocols\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab", "HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: \r\n\r\nab", "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab", "HTTP/1.1 200 OK\nContent-Length: 0\n\n", "HTTP/1.1 200 OK\r\nNoColon\r\n\r\n", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"]
  Text.join(List.map(replies, fn(reply) { replied([reply]) }), "\n")
    |> Test.assert_equal("HttpError.BadReply(\"the connection ended before a reply\")\nHttpError.BadReply(\"the connection ended inside a head\")\nHttpError.BadReply(\"not HTTP/1.x\")\nHttpError.BadReply(\"not HTTP/1.x\")\nHttpError.BadReply(\"bad status line\")\nHttpError.BadReply(\"bad status line\")\nHttpError.BadReply(\"bad status line\")\nHttpError.BadReply(\"bad status line\")\nHttpError.BadReply(\"bad status line\")\nHttpError.BadReply(\"bad status line\")\nHttpError.BadReply(\"101 Switching Protocols\")\nHttpError.BadReply(\"conflicting content-length\")\nHttpError.BadReply(\"content-length is not a number\")\nHttpError.BadReply(\"content-length is not a number\")\nHttpError.BadReply(\"the connection ended inside a body\")\nHttpError.BadReply(\"the connection ended inside a head\")\nHttpError.BadReply(\"a header line has no colon\")\nHttpError.BadReply(\"bad chunk size\")")
}

test "limits" {
  let small = { ..Http.client_defaults, max_head_bytes: 40, max_headers: 2, max_body_bytes: 4 }
  let shown = fn(reply) {
    let exchanged = scripted_with(small, Method.Get, "http://x/", [], "", [reply])
    case Text.split_once(exchanged, "=>") {
      Some((_, outcome)) -> outcome
      None -> "no outcome"
    }
  }
  let replies = ["HTTP/1.1 200 OK\r\nA: 1\r\nB: 2\r\n\r\n1234", "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n1234", "HTTP/1.1 200 OK\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n", "HTTP/1.1 200 OK\r\nA: 12345678901234567890\r\n\r\n", "HTTP/1.1 200 OK\r\nA: 123456789012345678901\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12345", "HTTP/1.1 200 OK\r\n\r\n12345", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n12345\r\n0\r\n\r\n"]
  // A chunked body's framing counts toward the limit, as for the server:
  // `1\r\na\r\n0\r\n\r\n` is 11 bytes.
  let framed = { ..Http.client_defaults, max_body_bytes: 11 }
  let tight = { ..Http.client_defaults, max_body_bytes: 10 }
  let chunked = ["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\n"]
  let at_limit = scripted_with(framed, Method.Get, "http://x/", [], "", chunked)
  let past = scripted_with(tight, Method.Get, "http://x/", [], "", chunked)
  "${at_limit}|${past}" |> Test.assert_equal("GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n=>200 [transfer-encoding=chunked] a|GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n=>HttpError.TooLarge")
  Text.join(List.map(replies, shown), "\n")
    |> Test.assert_equal("200 [a=1;b=2] 1234\n200 [content-length=4] 1234\nHttpError.TooLarge\n200 [a=12345678901234567890] \nHttpError.TooLarge\nHttpError.TooLarge\nHttpError.TooLarge\nHttpError.TooLarge")
}

test "a setting that is not positive defects" {
  let zero = Test.capture_defect(fn() { scripted_with({ ..Http.client_defaults, max_headers: 0 }, Method.Get, "http://x/", [], "", []) })
  let instant = Test.capture_defect(fn() { scripted_with({ ..Http.client_defaults, timeout: Duration.seconds(0) }, Method.Get, "http://x/", [], "", []) })
  "${zero}|${instant}"
    |> Test.assert_equal("Http: max_head_bytes, max_headers and timeout must be positive, and max_body_bytes not negative|Http: max_head_bytes, max_headers and timeout must be positive, and max_body_bytes not negative")
}
"#,
        "virtual",
    );
    assert_passed(&run, 3);
}

// Rejects a client whose failures name a private type: a caller handles
// exactly `HttpError`, `NetError` and `Timeout`.
#[test]
fn a_send_fails_only_with_public_types() {
    let run = run_tests(
        "failure-set",
        r#"
import std/http.{HttpError}
import std/task.{Timeout}

fn exact(operation: fn() -> Response errors HttpError | NetError | Timeout needs { clock: Clock }) -> Text needs { clock: Clock } {
  case Result.capture(operation) {
    Ok(reply) -> rendered(reply)
    Err(failure) -> Debug.render(failure)
  }
}

test "the failure set" {
  let (client, server) = Net.pair(64)
  let outcome = exact(fn() { Http.send_over(client, Method.Get, "http://[x/", [], Bytes.empty(), Http.client_defaults) })
  Net.close(client)
  Net.close(server)
  outcome |> Test.assert_equal("HttpError.BadUrl(\"an IPv6 address without its closing bracket\")")
}
"#,
        "virtual",
    );
    assert_passed(&run, 1);
}

// Rejects a client that waits forever for a server that never answers, or
// that gives up before its timeout.
#[test]
fn the_whole_exchange_runs_under_the_timeout() {
    let run = run_tests(
        "timeout",
        r#"
fn silent() -> Text needs { clock: Clock } {
  let (client, server) = Net.pair(64)
  let serving = Task.start(fn() {
    let request = read_request(server, 1048576)
    Net.close(server)
    request
  })
  let started = Clock.now()
  let outcome = Result.capture(fn() { Http.send_over(client, Method.Get, "http://x/", [], Bytes.empty(), { ..Http.client_defaults, timeout: Duration.seconds(5) }) })
  let waited = Clock.since(started)
  Net.close(client)
  let _ = Task.result(serving)
  let shown = case outcome {
    Ok(response) -> rendered(response)
    Err(failure) -> Debug.render(failure)
  }
  "${shown} ${Debug.render(waited == Duration.seconds(5))}"
}

test "a silent server" {
  silent() |> Test.assert_equal("Timeout.TimedOut(5s) True")
}
"#,
        "virtual",
    );
    assert_passed(&run, 1);
}

// Rejects a client that only talks to its own scripted server: `Http.send`
// reaches `Http.serve` over loopback for GET, POST with a body, HEAD and a
// 404, and a refused connection fails with `NetError`.
#[test]
fn http_send_talks_to_http_serve_over_loopback() {
    let run = run_tests(
        "loopback",
        r#"
fn handle(request: Request) -> Response {
  case (Http.method(request), Http.path(request)) {
    (Method.Get, ["users", id]) -> Http.text(200, "user ${id} ${Http.query(request)} ${Debug.render(Http.header(request, "host"))}")
    (Method.Post, ["echo"]) -> Http.response(201, [], Http.body(request))
    (Method.Head, ["health"]) -> Http.text(200, "ok")
    _ -> Http.text(404, "not found")
  }
}

fn short(reply: Response) -> Text {
  "${Http.status(reply)} ${Bytes.to_text(Http.response_body(reply))}"
}

fn round_trip() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, Http.defaults, handle) })
  let base = "http://127.0.0.1:${port}"
  let got = Http.send(Method.Get, "${base}/users/7?full=1", [], Bytes.empty(), Http.client_defaults)
  let posted = Http.send(Method.Post, "${base}/echo", [("content-type", "text/plain")], Bytes.from_text("ping"), Http.client_defaults)
  let headed = Http.send(Method.Head, "${base}/health", [], Bytes.empty(), Http.client_defaults)
  let missing = Http.send(Method.Get, "${base}/nothing", [], Bytes.empty(), Http.client_defaults)
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  let closed = case Result.capture(fn() { Http.send(Method.Get, "${base}/", [], Bytes.empty(), Http.client_defaults) }) {
    Ok(reply) -> short(reply)
    Err(NetError.Refused) -> "refused"
    Err(other) -> Debug.render(other)
  }
  let host = Text.join(Text.split(short(got), "127.0.0.1:${port}"), "HOST")
  "${host}|${short(posted)}|${short(headed)} ${Debug.render(Http.response_header(headed, "content-length"))}|${short(missing)}|${closed}"
}

// A server that keeps the connection open after its reply: the client must
// close it, so the server's next read ends.
fn closes() -> Text needs { network: Network, clock: Clock } {
  let listener = Net.listen("127.0.0.1", 0)
  let port = Net.port(Net.listener_address(listener))
  let serving = Task.start(fn() {
    let connection = Net.accept(listener)
    let _ = read_request(connection, 0)
    Stream.write(Net.writer(connection), Bytes.from_text("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"))
    let after = Task.timeout(fn() { Stream.read(Net.reader(connection)) }, Duration.seconds(5))
    Net.close(connection)
    Debug.render(after)
  })
  let reply = Http.send(Method.Get, "http://127.0.0.1:${port}/", [], Bytes.empty(), Http.client_defaults)
  let after = Task.result(serving)
  Net.close_listener(listener)
  "${short(reply)} ${after}"
}

test "the client closes its connection" {
  let outcome = closes() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("200 ok None")
}

test "round trips" {
  let outcome = round_trip() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("200 user 7 full=1 Some(\"HOST\")|201 ping|200  Some(\"2\")|404 not found|refused")
}
"#,
        "real",
    );
    assert_passed(&run, 2);
}
