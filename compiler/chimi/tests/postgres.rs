//! Conformance for `std/postgres`, the PostgreSQL driver for `std/sql`.
//!
//! The scripted tests run a fake server on loopback that speaks the wire
//! protocol: it records what the driver sent (the startup parameters, the
//! authentication messages, each statement and its parameters) and answers
//! as a test asks, so the bytes on the wire are what is checked. They run on
//! the real clock, because the server is a real socket.
//!
//! `a_real_postgresql_server` then runs std/sql end to end against
//! PostgreSQL's own server, started with `initdb` in a scratch directory on
//! a free port, with SCRAM, TLS and the fixture certificate
//! (`tests/fixtures/tls/`). It needs the PostgreSQL server binaries, and
//! fails, saying what to install, when they are missing.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CA: &str = include_str!("fixtures/tls/ca.pem");
const SERVER_CERT: &str = include_str!("fixtures/tls/server.pem");
const SERVER_KEY: &str = include_str!("fixtures/tls/server.key");

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-postgres-{name}-{}-{:?}",
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

    fn test(&self) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(["test", "cases.chimi", "--clock=real"])
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

fn pem(text: &str) -> String {
    text.replace('\n', "\\n")
}

fn assert_passed(name: &str, tests: &str, count: usize) {
    let source = format!(
        "{}\n{tests}",
        PRELUDE
            .replace("AUTHORITY", &pem(CA))
            .replace("SERVER_CERTIFICATE", &pem(SERVER_CERT))
            .replace("SERVER_KEY", &pem(SERVER_KEY))
    );
    let run = Scratch::new(name, &source).test();
    assert_eq!(
        run.status,
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stdout.contains(&format!("{count} passed, 0 failed")),
        "stdout:\n{}",
        run.stdout
    );
}

/// The fake server. `served(target, greet, answer, body)` runs `body` on a
/// pool of one connection to a server that greets the driver with `greet`
/// (`trust`, `cleartext`, `scram(...)`, `secured(...)`) and answers each
/// statement with `answer(query, params)`. It returns what the server saw,
/// then `=>` and what `body` returned or failed with.
const PRELUDE: &str = r#"import std/bytes as Bytes
import std/bytes.{Bytes, IntLayout}
import std/clock as Clock
import std/clock.{Clock}
import std/debug as Debug
import std/decimal as Decimal
import std/duration as Duration
import std/list as List
import std/net as Net
import std/net.{Connection, NetError}
import std/postgres as Postgres
import std/result as Result
import std/result.{Err, Ok}
import std/sql as Sql
import std/sql.{Database, SqlError, Value}
import std/stream as Stream
import std/task as Task
import std/test as Test
import std/test.{Recorder}
import std/text as Text
import std/tls as Tls
import std/tls.{Roots}
import std/duration.{Duration}

// The fake server's side of the protocol.

fn i32(value: Int) -> Bytes {
  Bytes.from_int(value, IntLayout.I32Be)
}

fn i16(value: Int) -> Bytes {
  Bytes.from_int(value, IntLayout.I16Be)
}

fn cstr(text: Text) -> Bytes {
  Bytes.append(Bytes.from_text(text), Bytes.from([0]))
}

fn msg(kind: Int, body: Bytes) -> Bytes {
  Bytes.concat([Bytes.from([kind]), i32(Bytes.length(body) + 4), body])
}

fn ready(status: Text) -> Bytes {
  msg(90, Bytes.from_text(status))
}

fn auth(code: Int, rest: Bytes) -> Bytes {
  msg(82, Bytes.append(i32(code), rest))
}

fn described(columns: List<(Text, Int)>) -> Bytes {
  msg(84, Bytes.concat(List.append([i16(List.length(columns))], List.map(columns, fn(column) {
    let (name, oid) = column
    Bytes.concat([cstr(name), i32(0), i16(0), i32(oid), i16(-1), i32(-1), i16(0)])
  }))))
}

fn data(values: List<Option<Bytes>>) -> Bytes {
  msg(68, Bytes.concat(List.append([i16(List.length(values))], List.map(values, fn(value) {
    case value {
      Some(bytes) -> Bytes.append(i32(Bytes.length(bytes)), bytes)
      None -> i32(-1)
    }
  }))))
}

fn complete(tag: Text) -> Bytes {
  msg(67, cstr(tag))
}

fn error(severity: Text, code: Text, message: Text, constraint: Text) -> Bytes {
  msg(69, Bytes.concat([Bytes.from([83]), cstr(severity), Bytes.from([86]), cstr(severity), Bytes.from([67]), cstr(code), Bytes.from([77]), cstr(message), Bytes.from([68]), cstr("Key (name)=(secret row value) already exists."), Bytes.from([110]), cstr(constraint), Bytes.from([0])]))
}

// The usual reply to a statement: ParseComplete, BindComplete, then `rest`.
fn replied(rest: List<Bytes>) -> List<Bytes> {
  List.append([msg(49, Bytes.empty()), msg(50, Bytes.empty())], rest)
}

// Exactly `count` bytes from `buffer` and the connection, and what is left.
fn exact(connection: Connection, buffer: Bytes, count: Int) -> (Bytes, Bytes) {
  let filled = loop have = buffer {
    case Bytes.length(have) >= count {
      True -> break have
      False -> {
        let chunk = Stream.read(Net.reader(connection))
        case chunk {
          Some(more) -> Bytes.append(have, more)
          None -> Failure(NetError.Closed)
        }
      }
    }
  }
  (Bytes.slice(filled, 0..count), Bytes.slice(filled, count..Bytes.length(filled)))
}

fn int_at(bytes: Bytes, at: Int) -> Int {
  case Bytes.read_int(bytes, at, IntLayout.I32Be) {
    Some(value) -> value
    None -> Failure(NetError.Failed("short message"))
  }
}

// A message without a type byte (the startup message and the SSL request).
fn untyped(connection: Connection, buffer: Bytes) -> (Bytes, Bytes) {
  let (header, rest) = exact(connection, buffer, 4)
  exact(connection, rest, int_at(header, 0) - 4)
}

fn typed(connection: Connection, buffer: Bytes) -> (Int, Bytes, Bytes) {
  let (header, rest) = exact(connection, buffer, 5)
  let (body, left) = exact(connection, rest, int_at(header, 1) - 4)
  let kind = case Bytes.get(header, 0) {
    Some(byte) -> byte
    None -> 0
  }
  (kind, body, left)
}

fn send(connection: Connection, messages: List<Bytes>) -> Void {
  Stream.write(Net.writer(connection), Bytes.concat(messages))
}

// The NUL-separated parts of a body.
fn parts(body: Bytes) -> List<Text> {
  Text.split(Bytes.to_text_lossy(body), "\u{0}")
}

// Reads the startup message (answering an SSL request with `N`) and
// records its parameters.
fn startup(connection: Connection, log: Recorder<Text>) -> Bytes {
  let (body, rest) = untyped(connection, Bytes.empty())
  let code = int_at(body, 0)
  case code == 80877103 {
    True -> {
      Test.record(log, "ssl request")
      send(connection, [Bytes.from_text("N")])
      startup(connection, log)
    }
    False -> {
      let shown = Text.join(List.filter(parts(Bytes.slice(body, 4..Bytes.length(body))), fn(part) { not Text.is_empty(part) }), " ")
      Test.record(log, "startup ${int_at(body, 0)} ${shown}")
      rest
    }
  }
}

fn welcome() -> List<Bytes> {
  [auth(0, Bytes.empty()), msg(83, Bytes.append(cstr("server_version"), cstr("16.0"))), msg(75, Bytes.append(i32(1), i32(2))), msg(78, Bytes.concat([Bytes.from([83]), cstr("NOTICE"), Bytes.from([0])])), ready("I")]
}

fn trust(connection: Connection, log: Recorder<Text>) -> (Connection, Bytes) {
  let rest = startup(connection, log)
  send(connection, welcome())
  (connection, rest)
}

fn cleartext(connection: Connection, log: Recorder<Text>) -> (Connection, Bytes) {
  let rest = startup(connection, log)
  send(connection, [auth(3, Bytes.empty())])
  let (kind, body, left) = typed(connection, rest)
  Test.record(log, "password ${Int_text(kind)} ${Debug.render(parts(body))}")
  send(connection, welcome())
  (connection, left)
}

fn Int_text(value: Int) -> Text {
  "${value}"
}

// SCRAM-SHA-256 as PostgreSQL runs it, checking the client's proof with the
// driver's own computation; `final` is the server-final message to send.
fn scram(password: Text, extended: fn(Text) -> Text, final: fn(Text) -> List<Bytes>) -> fn(Connection, Recorder<Text>) -> (Connection, Bytes) errors NetError {
  fn(connection, log) {
    let rest = startup(connection, log)
    send(connection, [auth(10, Bytes.concat([cstr("SCRAM-SHA-256-PLUS"), cstr("SCRAM-SHA-256"), Bytes.from([0])]))])
    let (_, first, after_first) = typed(connection, rest)
    let first_parts = parts(first)
    let mechanism = case List.first(first_parts) {
      Some(name) -> name
      None -> ""
    }
    let client_first = Bytes.to_text_lossy(Bytes.slice(first, (Text.byte_length(mechanism) + 5)..Bytes.length(first)))
    Test.record(log, "${mechanism} ${Text.byte_slice(client_first, 0..8)} and a nonce of ${Int_text(Text.byte_length(client_first) - 8)} bytes")
    let client_bare = Text.byte_slice(client_first, 3..Text.byte_length(client_first))
    let nonce = Text.byte_slice(client_bare, 5..Text.byte_length(client_bare))
    let server_first = "r=${extended(nonce)},s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"
    send(connection, [auth(11, Bytes.from_text(server_first))])
    let (_, final_body, after_final) = typed(connection, after_first)
    let client_final = Bytes.to_text_lossy(final_body)
    let (without_proof, proof) = case Text.split_once(client_final, ",p=") {
      Some(split) -> split
      None -> (client_final, "")
    }
    let (expected, signature) = Postgres.scram_for_tests(password, "W22ZaJ0SNY7soEsUEjb6gQ==", 4096, "${client_bare},${server_first},${without_proof}")
    Test.record(log, "${Debug.render(without_proof == "c=biws,r=${extended(nonce)}")} proof ${Debug.render(proof == expected)}")
    send(connection, final(signature))
    (connection, after_final)
  }
}

// Answers each statement (Parse to Sync) with `answer(query, params)`,
// recording `query [params]`; NULL is shown as `NULL`.
fn statements(connection: Connection, buffer: Bytes, log: Recorder<Text>, answer: fn(Text, List<Text>) -> List<Bytes>) -> Void {
  let _ = loop state = (buffer, "", []) {
    let (left, query, params) = state
    let read = Result.capture(fn() { typed(connection, left) })
    case read {
      Err(_) -> break Void
      Ok((80, body, rest)) -> {
        let query = case parts(body) {
          [_, text, ..] -> text
          _ -> ""
        }
        (rest, query, [])
      }
      Ok((66, body, rest)) -> (rest, query, bound(body))
      Ok((83, _, rest)) -> {
        Test.record(log, "${query} ${Debug.render(params)}")
        let reply = case query == "copy failed" {
          True -> [error("ERROR", "57014", "COPY from stdin failed", ""), ready("I")]
          False -> answer(query, params)
        }
        send(connection, reply)
        case query == "close" {
          True -> break Void
          False -> (rest, "", [])
        }
      }
      Ok((88, _, _)) -> {
        Test.record(log, "terminate")
        break Void
      }
      // As PostgreSQL does, the Sync sent before a CopyFail was skipped, so
      // the error waits for the next one.
      Ok((102, body, rest)) -> {
        Test.record(log, "copy fail ${Debug.render(parts(body))}")
        (rest, "copy failed", params)
      }
      Ok((_, _, rest)) -> (rest, query, params)
    }
  }
  Void
}

// A Bind message's parameters, as text.
fn bound(body: Bytes) -> List<Text> {
  // Portal "", statement "", no format codes.
  let count = case Bytes.read_int(body, 4, IntLayout.I16Be) {
    Some(count) -> count
    None -> 0
  }
  let (_, values) = loop state = (6, []) {
    let (at, values) = state
    case List.length(values) == count {
      True -> break (at, values)
      False -> {
        let length = int_at(body, at)
        case length == -1 {
          True -> (at + 4, List.push(values, "NULL"))
          False -> (at + 4 + length, List.push(values, Bytes.to_text_lossy(Bytes.slice(body, (at + 4)..(at + 4 + length)))))
        }
      }
    }
  }
  values
}

// Runs `body` on a pool of one connection to a fake server on loopback
// that greets with `greet` and answers statements with `answer`. Returns
// what the server read, then `=>` and the outcome. The driver's network
// records the host and port it was asked for, and connects to the server.
fn served<T>(target: Text, greet: fn(Connection, Recorder<Text>) -> (Connection, Bytes) errors NetError, answer: fn(Text, List<Text>) -> List<Bytes>, body: fn(Database) -> T errors SqlError needs { clock: Clock }) -> Text needs { clock: Clock } {
  served_with({ ..Postgres.defaults, connect_timeout: Duration.seconds(2) }, target, greet, answer, body)
}

fn served_with<T>(settings: { roots: Roots, connect_timeout: Duration }, target: Text, greet: fn(Connection, Recorder<Text>) -> (Connection, Bytes) errors NetError, answer: fn(Text, List<Text>) -> List<Bytes>, body: fn(Database) -> T errors SqlError needs { clock: Clock }) -> Text needs { clock: Clock } {
  let host = Net.host_for_tests()
  let listener = Net.listen("127.0.0.1", 0) using { network: host }
  let port = Net.port(Net.listener_address(listener))
  let log = Test.recorder()
  let serving = Task.start(fn() {
    let accepted = Result.capture(fn() { Net.accept(listener) })
    Net.close_listener(listener)
    case accepted {
      Ok(server) -> {
        let _ = Result.capture(fn() {
          let (greeted, rest) = greet(server, log)
          statements(greeted, rest, log, answer)
        })
        Net.close(server)
      }
      Err(_) -> Void
    }
  })
  let network = { connect: fn(asked, asked_port) {
    Test.record(log, "connect ${asked} ${Int_text(asked_port)}")
    Net.connect("127.0.0.1", port) using { network: host }
  }, listen: fn(_, _) { Failure(NetError.Refused) } }
  let driver = Postgres.driver(settings) using { network: network }
  let outcome = Result.capture(fn() { Sql.open(driver, target, { ..Sql.defaults, max_connections: 1 }, body) })
  let _ = Result.capture(fn() { Net.close_listener(listener) })
  Task.result(serving)
  let shown = case outcome {
    Ok(value) -> Debug.render(value)
    Err(failure) -> Debug.render(failure)
  }
  "${Text.join(Test.recorded(log), " | ")} => ${shown}"
}

const url: Text = "postgres://app:secret@db:5432/app?sslmode=disable"

fn nothing(_: Text, _: List<Text>) -> List<Bytes> {
  replied([msg(110, Bytes.empty()), complete("SELECT 0"), ready("I")])
}

const authority: Text = "AUTHORITY"
const server_certificate: Text = "SERVER_CERTIFICATE"
const server_key: Text = "SERVER_KEY"
fn ok_row(columns: List<(Text, Int)>, values: List<Option<Bytes>>) -> List<Bytes> {
  replied([described(columns), data(values), complete("SELECT 1"), ready("I")])
}

fn text(value: Text) -> Option<Bytes> {
  Some(Bytes.from_text(value))
}

fn failing(failure: Bytes) -> fn(Text, List<Text>) -> List<Bytes> {
  fn(_, _) { [failure, ready("I")] }
}

fn tagged(tag: Text, rows: Bool) -> fn(Text, List<Text>) -> List<Bytes> {
  fn(_, _) {
    case rows {
      True -> replied([described([("a", 23)]), data([text("1")]), complete(tag), ready("I")])
      False -> replied([msg(110, Bytes.empty()), complete(tag), ready("I")])
    }
  }
}

fn outcome(shown: Text) -> Text {
  case Text.split_once(shown, " => ") {
    Some((_, after)) -> after
    None -> shown
  }
}

fn longer(nonce: Text) -> Text {
  "${nonce}server"
}

fn verified(signature: Text) -> List<Bytes> {
  List.append([auth(12, Bytes.from_text("v=${signature}"))], welcome())
}

// Replies to each statement by its text: `T` inside a transaction, `I`
// after COMMIT, ROLLBACK or a statement whose text starts with `idle`;
// `bad` and a statement in `failures` fail.
fn transactional(failures: List<(Text, Bytes)>) -> fn(Text, List<Text>) -> List<Bytes> {
  fn(query, _) {
    let status = case query == "COMMIT" or query == "ROLLBACK" or Text.starts_with(query, "idle") {
      True -> "I"
      False -> "T"
    }
    let failure = List.find(failures, fn(pair) {
      let (name, _) = pair
      name == query
    })
    case (failure, query) {
      (Some((_, response)), _) -> [response, ready(status)]
      (None, "COMMIT ROLLBACK") -> replied([msg(110, Bytes.empty()), complete("ROLLBACK"), ready("I")])
      (None, "hang") -> []
      (None, "close") -> [msg(49, Bytes.empty()), Bytes.from([50, 0])]
      _ -> replied([described([("q", 25)]), data([text(query)]), complete("SELECT 1"), ready(status)])
    }
  }
}

fn only(rows: List<Sql.Row>) -> Text {
  case rows {
    [row] -> Sql.text(row, "q")
    _ -> "?"
  }
}

fn log_of(shown: Text) -> Text {
  case Text.split_once(shown, "DateStyle ISO | ") {
    Some((_, after)) -> after
    None -> shown
  }
}

// A server that takes the TLS request, answers `S` (and `extra`), and runs
// `greet` over TLS with the fixture certificate for `localhost`.
fn secured(identity: Tls.Identity, extra: Text, greet: fn(Connection, Recorder<Text>) -> (Connection, Bytes) errors NetError) -> fn(Connection, Recorder<Text>) -> (Connection, Bytes) errors NetError {
  fn(connection, log) {
    let (body, _) = untyped(connection, Bytes.empty())
    Test.record(log, "ssl request ${Int_text(int_at(body, 0))}")
    send(connection, [Bytes.from_text("S${extra}")])
    let inner = case Result.capture(fn() { Tls.server(connection, identity) }) {
      Ok(inner) -> inner
      Err(_) -> Failure(NetError.Failed("no TLS session"))
    }
    greet(inner, log)
  }
}

fn fixture() -> Tls.Identity {
  Tls.identity(Bytes.from_text(server_certificate), Bytes.from_text(server_key))
}

fn trusting() -> { roots: Roots, connect_timeout: Duration } {
  { roots: Tls.roots(Bytes.from_text(authority)), connect_timeout: Duration.seconds(2) }
}
"#;

// Rejects a driver that sends the wrong startup, trusts a server that cannot
// prove it knows the password, sends a password in the clear, falls back
// from TLS, or reads bytes a server sent before the TLS handshake.
#[test]
fn startup_and_authentication() {
    assert_passed(
        "startup-and-authentication",
        r#"test "trust startup" {
  served(url, trust, nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO | select 1 [] | terminate => 0")
}

test "scram matches RFC 7677's example" {
  let nonce = "rOprNGfwEbeRWgbNEkqO"
  let server_nonce = "rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0"
  Postgres.scram_for_tests("pencil", "W22ZaJ0SNY7soEsUEjb6gQ==", 4096, "n=user,r=${nonce},r=${server_nonce},s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096,c=biws,r=${server_nonce}")
    |> Test.assert_equal(("dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=", "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="))
  Test.capture_defect(fn() { Postgres.scram_for_tests("pencil", "W22Z", 0, "") })
    |> Test.assert_equal("Postgres: the server asked for an iteration count out of range")
  Test.capture_defect(fn() { Postgres.scram_for_tests("pencil", "W22Z=", 4096, "") })
    |> Test.assert_equal("Postgres: the salt is not base64")
}

test "scram" {
  served(url, scram("secret", longer, verified), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO | SCRAM-SHA-256 n,,n=,r= and a nonce of 24 bytes | True proof True | select 1 [] | terminate => 0")
}

test "scram wrong signature" {
  served(url, scram("secret", longer, fn(_) { verified("6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=") }), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO | SCRAM-SHA-256 n,,n=,r= and a nonce of 24 bytes | True proof True => SqlError.ConnectFailed(\"Postgres: the server's SCRAM signature is wrong, so it does not know the password\")")
}

test "scram error" {
  served(url, scram("secret", longer, fn(_) { List.append([auth(12, Bytes.from_text("e=invalid-proof"))], welcome()) }), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO | SCRAM-SHA-256 n,,n=,r= and a nonce of 24 bytes | True proof True => SqlError.ConnectFailed(\"Postgres: the server refused the SCRAM proof: invalid-proof\")")
}

test "a scram server that skips its signature" {
  served(url, scram("secret", longer, fn(_) { welcome() }), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> outcome
    |> Test.assert_equal("SqlError.ConnectFailed(\"Postgres: the server broke off the SCRAM exchange\")")
}

test "a scram nonce the server did not extend" {
  let shown = List.map([fn(nonce) { nonce }, fn(_) { "someone else's" }], fn(extended) {
    outcome(served(url, scram("secret", extended, verified), nothing, fn(db) { Sql.execute(db, "select 1", []) }))
  })
  shown |> Test.assert_equal([
    "SqlError.ConnectFailed(\"Postgres: the server's SCRAM nonce does not extend the client's\")",
    "SqlError.ConnectFailed(\"Postgres: the server's SCRAM nonce does not extend the client's\")",
  ])
}

test "cleartext refused without tls" {
  served(url, cleartext, nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO => SqlError.ConnectFailed(\"Postgres: the server asks for a cleartext password, which is only sent over TLS; use scram-sha-256 or sslmode=require\")")
}

test "md5 refused" {
  served(url, fn(connection, log) {
    let rest = startup(connection, log)
    send(connection, [auth(5, Bytes.from([1, 2, 3, 4]))])
    (connection, rest)
  }, nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO => SqlError.ConnectFailed(\"Postgres: MD5 password authentication is not supported; use scram-sha-256\")")
}

test "startup error" {
  served(url, fn(connection, log) {
    let rest = startup(connection, log)
    send(connection, [error("FATAL", "28P01", "password authentication failed for user \"app\"", "")])
    (connection, rest)
  }, nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO => SqlError.ConnectFailed(\"Postgres: password authentication failed for user \\\"app\\\"\")")
}

test "require against a server without tls" {
  served("postgres://app:secret@db/app", trust, nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect db 5432 | ssl request => SqlError.ConnectFailed(\"Postgres: the server does not accept TLS; connect with sslmode=disable only to a server you reach over a trusted network\")")
}

test "cleartext over tls" {
  served_with(trusting(), "postgres://app:secret@localhost/app", secured(fixture(), "", cleartext), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect localhost 5432 | ssl request 80877103 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO | password 112 [\"secret\", \"\"] | select 1 [] | terminate => 0")
}

test "tls with a certificate for another host" {
  served_with(trusting(), "postgres://app:secret@127.0.0.1/app", secured(fixture(), "", cleartext), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect 127.0.0.1 5432 | ssl request 80877103 => SqlError.ConnectFailed(\"Postgres: the server's certificate is not for 127.0.0.1\")")
}

test "tls with an untrusted certificate" {
  served_with(Postgres.defaults, "postgres://app:secret@localhost/app", secured(fixture(), "", cleartext), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect localhost 5432 | ssl request 80877103 => SqlError.ConnectFailed(\"Postgres: the server's certificate is not trusted\")")
}

test "bytes sent with the answer to the tls request" {
  served_with(trusting(), "postgres://app:secret@localhost/app", secured(fixture(), "R", cleartext), nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> Test.assert_equal("connect localhost 5432 | ssl request 80877103 => SqlError.ConnectFailed(\"Postgres: the server's answer to the TLS request is not PostgreSQL's\")")
}

test "connect timeout" {
  served(url, fn(connection, log) {
    let rest = startup(connection, log)
    let _ = typed(connection, rest)
    (connection, rest)
  }, nothing, fn(db) { Sql.execute(db, "select 1", []) })
    |> outcome
    |> Test.assert_equal("SqlError.ConnectFailed(\"Postgres: no answer within 2s\")")
}
"#,
        16,
    );
}

// Rejects parameters or columns in the wrong text form, an affected-row count
// read from the wrong tag, a SQLSTATE mapped to the wrong failure, and COPY
// left hanging.
#[test]
fn values_tags_and_failures() {
    assert_passed(
        "values-tags-and-failures",
        r#"test "parameters" {
  let price = Decimal.parse("12.50")
  served(url, trust, nothing, fn(db) { Sql.execute(db, "insert $1", [Value.Null, Value.Bool(True), Value.Bool(False), Value.Int(-42), Value.Float(1.5), Value.Float(1.0e20), Value.Decimal(price), Value.Text("héllo"), Value.Bytes(Bytes.from([0, 15, 255]))]) })
    |> Test.assert_equal("connect db 5432 | startup 196608 user app database app client_encoding UTF8 application_name chimi DateStyle ISO | insert $1 [\"NULL\", \"t\", \"f\", \"-42\", \"1.5\", \"1e20\", \"12.50\", \"héllo\", \"\\\\x000fff\"] | terminate => 0")
}

test "columns" {
  let answer = ok_row([("b", 16), ("i8", 20), ("i2", 21), ("i4", 23), ("f4", 700), ("f8", 701), ("nan", 701), ("n", 1700), ("nn", 1700), ("hex", 17), ("esc", 17), ("j", 114), ("z", 25)], [text("t"), text("-9223372036854775808"), text("7"), text("8"), text("1.5"), text("1e+20"), text("NaN"), text("12.50"), text("NaN"), text("\\x00ff"), text("a\\\\b\\001"), text("{}"), None])
  served(url, trust, fn(_, _) { answer }, fn(db) { Sql.query(db, "select", []) })
    |> outcome
    |> Test.assert_equal("[Row { columns: [\"b\", \"i8\", \"i2\", \"i4\", \"f4\", \"f8\", \"nan\", \"n\", \"nn\", \"hex\", \"esc\", \"j\", \"z\"], values: [Value.Bool(True), Value.Int(-9223372036854775808), Value.Int(7), Value.Int(8), Value.Float(1.5), Value.Float(1e20), Value.Text(\"NaN\"), Value.Decimal(12.5), Value.Text(\"NaN\"), Value.Bytes(Bytes(00 ff)), Value.Bytes(Bytes(61 5c 62 01)), Value.Text(\"{}\"), Value.Null] }]")
}

test "tags" {
  let shown = List.map([("INSERT 0 3", False), ("UPDATE 2", False), ("DELETE 0", False), ("SELECT 5", True), ("SELECT 5", False), ("CREATE TABLE", False), ("MERGE 4", False), ("COPY 9", False)], fn(pair) {
    let (tag, rows) = pair
    outcome(served(url, trust, tagged(tag, rows), fn(db) { Sql.execute(db, "x", []) }))
  })
  shown |> Test.assert_equal([
    "3",
    "2",
    "0",
    "0",
    "5",
    "0",
    "4",
    "9",
  ])
}

test "sqlstates" {
  let shown = List.map([("ERROR", "23505", "uq"), ("ERROR", "23502", ""), ("ERROR", "40001", ""), ("ERROR", "40P01", ""), ("ERROR", "55P03", ""), ("ERROR", "42601", ""), ("ERROR", "42501", ""), ("ERROR", "0A000", ""), ("ERROR", "22P02", ""), ("ERROR", "25P02", ""), ("ERROR", "08P01", ""), ("FATAL", "57P01", ""), ("ERROR", "57014", ""), ("ERROR", "XX000", "")], fn(case_) {
    let (severity, code, constraint) = case_
    outcome(served(url, trust, failing(error(severity, code, "the message", constraint)), fn(db) { Sql.execute(db, "x", []) }))
  })
  shown |> Test.assert_equal([
    "SqlError.ConstraintFailed(constraint: \"uq\", message: \"the message\")",
    "SqlError.ConstraintFailed(constraint: \"\", message: \"the message\")",
    "SqlError.Conflict(\"the message\")",
    "SqlError.Conflict(\"the message\")",
    "SqlError.Conflict(\"the message\")",
    "SqlError.Rejected(code: \"42601\", message: \"the message\")",
    "SqlError.Rejected(code: \"42501\", message: \"the message\")",
    "SqlError.Rejected(code: \"0A000\", message: \"the message\")",
    "SqlError.Rejected(code: \"22P02\", message: \"the message\")",
    "SqlError.Rejected(code: \"25P02\", message: \"the message\")",
    "SqlError.Rejected(code: \"08P01\", message: \"the message\")",
    "SqlError.ConnectionLost",
    "SqlError.Failed(\"Postgres: 57014: the message\")",
    "SqlError.Failed(\"Postgres: XX000: the message\")",
  ])
}

test "copy, empty statements and NUL bytes" {
  let copy_in = served(url, trust, fn(_, _) { [msg(49, Bytes.empty()), msg(50, Bytes.empty()), msg(71, Bytes.concat([Bytes.from([0]), i16(0)]))] }, fn(db) { Sql.execute(db, "copy t from stdin", []) })
  let copy_out = served(url, trust, fn(_, _) { replied([msg(72, Bytes.concat([Bytes.from([0]), i16(0)])), msg(100, Bytes.from_text("1\n")), msg(99, Bytes.empty()), complete("COPY 1"), ready("I")]) }, fn(db) { Sql.execute(db, "copy t to stdout", []) })
  let empty = served(url, trust, fn(_, _) { replied([msg(110, Bytes.empty()), msg(73, Bytes.empty()), ready("I")]) }, fn(db) { Sql.execute(db, "", []) })
  let nul = served(url, trust, nothing, fn(db) {
    let query = Result.capture(fn() { Sql.execute(db, "select \u{0}", []) })
    let param = Result.capture(fn() { Sql.execute(db, "select $1", [Value.Text("a\u{0}b")]) })
    Debug.render((query, param))
  })
  [log_of(copy_in), log_of(copy_out), log_of(empty), log_of(nul)] |> Test.assert_equal([
    "copy t from stdin [] | copy fail [\"COPY is not supported\", \"\"] | copy failed [] | terminate => SqlError.Rejected(code: \"0A000\", message: \"Postgres: COPY is not supported\")",
    "copy t to stdout [] | terminate => SqlError.Rejected(code: \"0A000\", message: \"Postgres: COPY is not supported\")",
    " [] | terminate => 0",
    "terminate => \"(Result.Err(SqlError.Rejected(code: \\\"22021\\\", message: \\\"Postgres: a statement or parameter holds a NUL byte, which PostgreSQL text cannot\\\")), Result.Err(SqlError.Rejected(code: \\\"22021\\\", message: \\\"Postgres: a statement or parameter holds a NUL byte, which PostgreSQL text cannot\\\")))\"",
  ])
}
"#,
        5,
    );
}

// Rejects transaction statements in the wrong order, a commit that the server
// rolled back passing for a success, a connection reused after a call was
// cut off, bytes read past a reply lost, and two tasks' statements
// interleaving on one connection.
#[test]
fn transactions_and_sharing() {
    assert_passed(
        "transactions-and-sharing",
        r#"test "a transaction with a savepoint" {
  served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "a", [])
      Sql.transaction(tx, fn(nested) { Sql.execute(nested, "b", []) })
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | a [] | SAVEPOINT chimi_1 [] | b [] | RELEASE SAVEPOINT chimi_1 [] | COMMIT [] | terminate => 0")
}

test "a savepoint rolled back" {
  served(url, trust, transactional([("bad", error("ERROR", "23505", "dup", "uq"))]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let inner = Result.capture(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "bad", []) }) })
      Debug.render(inner)
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | SAVEPOINT chimi_1 [] | bad [] | ROLLBACK TO SAVEPOINT chimi_1 [] | RELEASE SAVEPOINT chimi_1 [] | COMMIT [] | terminate => \"Result.Err(SqlError.ConstraintFailed(constraint: \\\"uq\\\", message: \\\"dup\\\"))\"")
}

test "a release that fails, then a savepoint already gone" {
  served(url, trust, transactional([("RELEASE SAVEPOINT chimi_1", error("ERROR", "25P02", "aborted", "")), ("ROLLBACK TO SAVEPOINT chimi_1", error("ERROR", "3B001", "no such savepoint", ""))]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let inner = Result.capture(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "b", []) }) })
      Debug.render(inner)
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | SAVEPOINT chimi_1 [] | b [] | RELEASE SAVEPOINT chimi_1 [] | ROLLBACK TO SAVEPOINT chimi_1 [] | COMMIT [] | terminate => \"Result.Err(SqlError.Rejected(code: \\\"25P02\\\", message: \\\"aborted\\\"))\"")
}

test "a commit answered with rollback" {
  served(url, trust, fn(query, params) {
    case query == "COMMIT" {
      True -> replied([msg(110, Bytes.empty()), complete("ROLLBACK"), ready("I")])
      False -> transactional([])(query, params)
    }
  }, fn(db) {
    Sql.transaction(db, fn(tx) { Sql.execute(tx, "a", []) })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | a [] | COMMIT [] | terminate => SqlError.Failed(\"Postgres: the transaction had failed and was rolled back\")")
}

test "reset rolls back what a use left open" {
  served(url, trust, transactional([]), fn(db) {
    let _ = Sql.execute(db, "begin by hand", [])
    Sql.execute(db, "idle after", [])
  })
    |> log_of
    |> Test.assert_equal("begin by hand [] | ROLLBACK [] | idle after [] | terminate => 0")
}

test "a statement after the transaction ended" {
  served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "COMMIT", [])
      Sql.execute(tx, "a", [])
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | COMMIT [] | terminate => SqlError.TransactionEnded")
}

test "a call cut off breaks the connection" {
  let seen = Test.recorder()
  served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let cut = Result.capture(fn() { Task.timeout(fn() { Sql.execute(tx, "hang", []) }, Duration.milliseconds(100)) })
      Test.record(seen, Debug.render(cut))
      let after = Result.capture(fn() { Sql.execute(tx, "a", []) })
      Test.record(seen, Debug.render(after))
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | hang [] => SqlError.ConnectionLost")
  Test.recorded(seen) |> Test.assert_equal([
    "Result.Err(Timeout.TimedOut(100ms))",
    "Result.Err(SqlError.ConnectionLost)",
  ])
}

test "the server ends the connection in a reply" {
  let seen = Test.recorder()
  served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let lost = Result.capture(fn() { Sql.execute(tx, "close", []) })
      Test.record(seen, Debug.render(lost))
      let after = Result.capture(fn() { Sql.execute(tx, "a", []) })
      Test.record(seen, Debug.render(after))
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | close [] => SqlError.ConnectionLost")
  Test.recorded(seen) |> Test.assert_equal([
    "Result.Err(SqlError.ConnectionLost)",
    "Result.Err(SqlError.ConnectionLost)",
  ])
}

test "a FATAL error breaks the connection" {
  let seen = Test.recorder()
  served(url, trust, transactional([("fatal", error("FATAL", "57P01", "terminating connection due to administrator command", ""))]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let fatal = Result.capture(fn() { Sql.execute(tx, "fatal", []) })
      Test.record(seen, Debug.render(fatal))
      let after = Result.capture(fn() { Sql.execute(tx, "a", []) })
      Test.record(seen, Debug.render(after))
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | fatal [] => SqlError.ConnectionLost")
  Test.recorded(seen) |> Test.assert_equal([
    "Result.Err(SqlError.ConnectionLost)",
    "Result.Err(SqlError.ConnectionLost)",
  ])
}

test "bytes read past a reply are kept for the next call" {
  let notice = msg(78, Bytes.concat([Bytes.from([83]), cstr("NOTICE"), Bytes.from([0])]))
  let head = Bytes.slice(notice, 0..3)
  let tail = Bytes.slice(notice, 3..Bytes.length(notice))
  served(url, trust, fn(query, params) {
    case query {
      "first" -> List.push(transactional([])(query, params), head)
      "second" -> List.append([tail], transactional([])(query, params))
      _ -> transactional([])(query, params)
    }
  }, fn(db) {
    Sql.transaction(db, fn(tx) {
      let first = only(Sql.query(tx, "first", []))
      let second = only(Sql.query(tx, "second", []))
      "${first} ${second}"
    })
  })
    |> outcome
    |> Test.assert_equal("\"first second\"")
}

test "two tasks share a transaction's connection" {
  served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let tasks = List.map(["one", "two", "three", "four"], fn(name) { Task.start(fn() { Debug.render(Result.capture(fn() { only(Sql.query(tx, name, [Value.Text(name)])) })) }) })
      List.map(tasks, fn(task) { Debug.render(Result.capture(fn() { Task.result(task) })) })
    })
  })
    |> outcome
    |> Test.assert_equal("[\"Result.Ok(\\\"Result.Ok(\\\\\\\"one\\\\\\\")\\\")\", \"Result.Ok(\\\"Result.Ok(\\\\\\\"two\\\\\\\")\\\")\", \"Result.Ok(\\\"Result.Ok(\\\\\\\"three\\\\\\\")\\\")\", \"Result.Ok(\\\"Result.Ok(\\\\\\\"four\\\\\\\")\\\")\"]")
}

test "nested transactions in two tasks run one after another" {
  served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let first = Task.start(fn() {
        Sql.transaction(tx, fn(nested) {
          Clock.sleep(Duration.milliseconds(300))
          Sql.execute(nested, "first", [])
        })
      })
      Clock.sleep(Duration.milliseconds(100))
      let second = Result.capture(fn() { Sql.transaction(tx, fn(nested) { Sql.execute(nested, "second", []) }) })
      Debug.render((Task.result(first), second))
    })
  })
    |> log_of
    |> Test.assert_equal("BEGIN [] | SAVEPOINT chimi_1 [] | first [] | RELEASE SAVEPOINT chimi_1 [] | COMMIT [] | terminate => \"(0, Result.Err(SqlError.Failed(\\\"Postgres: a nested transaction is already open on this transaction; nested transactions on one transaction run one after another\\\")))\"")
}

test "a transaction a statement rolled back does not commit" {
  let ended = served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "a", [])
      Sql.execute(tx, "ROLLBACK", [])
    })
  })
  let nested = served(url, trust, transactional([]), fn(db) {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "COMMIT", [])
      Sql.transaction(tx, fn(inner) { Sql.execute(inner, "b", []) })
    })
  })
  [log_of(ended), log_of(nested)] |> Test.assert_equal([
    "BEGIN [] | a [] | ROLLBACK [] | terminate => SqlError.TransactionEnded",
    "BEGIN [] | COMMIT [] | terminate => SqlError.TransactionEnded",
  ])
}

test "a connection opened by a task that has ended" {
  served(url, trust, transactional([]), fn(db) {
    let first = Task.start(fn() { Debug.render(Result.capture(fn() { Sql.execute(db, "idle first", []) })) })
    let _ = Task.result(first)
    Sql.transaction(db, fn(tx) {
      let tasks = List.map(["one", "two"], fn(name) { Task.start(fn() { Debug.render(Result.capture(fn() { only(Sql.query(tx, name, [])) })) }) })
      let shared = List.map(tasks, fn(task) { Debug.render(Result.capture(fn() { Task.result(task) })) })
      let nested = Sql.transaction(tx, fn(inner) { Sql.execute(inner, "b", []) })
      Debug.render((shared, nested))
    })
  })
    |> log_of
    |> Test.assert_equal("idle first [] | BEGIN [] | one [] | two [] | SAVEPOINT chimi_1 [] | b [] | RELEASE SAVEPOINT chimi_1 [] | COMMIT [] | terminate => \"([\\\"Result.Ok(\\\\\\\"Result.Ok(\\\\\\\\\\\\\\\"one\\\\\\\\\\\\\\\")\\\\\\\")\\\", \\\"Result.Ok(\\\\\\\"Result.Ok(\\\\\\\\\\\\\\\"two\\\\\\\\\\\\\\\")\\\\\\\")\\\"], 0)\"")
}
"#,
        14,
    );
}

// Rejects a target read differently from libpq, or a parameter quietly
// ignored.
#[test]
fn targets() {
    assert_passed(
        "targets",
        r#"test "targets" {
  let shown = List.map([
    "postgresql://a%40b:p%3Aw@[::1]:6000/my%20db?sslmode=disable",
    "postgres://u@h?sslmode=disable",
    "postgres://u@h:5433?sslmode=disable&sslmode=disable",
    "mysql://u@h/d",
    "postgres://h/d",
    "postgres://:pw@h/d",
    "postgres://u@/d",
    "postgres://u@h:0/d",
    "postgres://u@h:65536/d",
    "postgres://u@h:+80/d",
    "postgres://u@h:/d?sslmode=disable",
    "postgres://u@[::1/d",
    "postgres://u@[::1]x/d",
    "postgres://u@h/d?sslmode=prefer",
    "postgres://u@h/d?sslmode=allow",
    "postgres://u@h/d?sslmode=verify-ca",
    "postgres://u@h/d?sslmode=verify-full",
    "postgres://u@h/d?connect_timeout=5",
    "postgres://u@h/d?sslmode",
    "postgres://u%zz@h/d",
    "postgres://u%2@h/d",
    "postgres://u%ff@h/d",
    "postgres://app%00options%00-c%20x:secret@db/app",
    "postgres://app\u{0}options\u{0}-c x:secret@db/app?sslmode=disable",
    "postgres://u:p@ss@h/d?sslmode=disable",
  ], fn(target) {
    let shown = served(target, trust, nothing, fn(db) { Sql.execute(db, "x", []) })
    Text.replace(shown, " client_encoding UTF8 application_name chimi DateStyle ISO | x [] | terminate", "")
  })
  shown |> Test.assert_equal([
    "connect ::1 6000 | startup 196608 user a@b database my db => 0",
    "connect h 5432 | startup 196608 user u database u => 0",
    "connect h 5433 | startup 196608 user u database u => 0",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: not a postgres:// URL\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: no user name\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: no user name\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: no host\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: the port 0\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: the port 65536\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: the port +80\")",
    "connect h 5432 | startup 196608 user u database d => 0",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: an IPv6 address without its closing bracket\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: text after an IPv6 address\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: sslmode=prefer is not supported; use require or disable\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: sslmode=allow is not supported; use require or disable\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: sslmode=verify-ca is not supported; use require or disable\")",
    "connect h 5432 | ssl request => SqlError.ConnectFailed(\"Postgres: the server does not accept TLS; connect with sslmode=disable only to a server you reach over a trusted network\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: the parameter connect_timeout is not supported\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: the parameter sslmode has no value\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: a bad percent escape\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: a bad percent escape\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: a percent escape that is not UTF-8\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: %00, which would end a startup field early\")",
    " => SqlError.ConnectFailed(\"Postgres: the target is not usable: a NUL byte, which would end a startup field early\")",
    "connect h 5432 | startup 196608 user u database d => 0",
  ])
}
"#,
        1,
    );
}

// Rejects a driver that only talks to its own fake: PostgreSQL's server
// checks the SCRAM proof, the TLS handshake, the extended protocol, the
// type of every column, deadlock detection and a pool of connections.
#[test]
fn a_real_postgresql_server() {
    let server = Server::start();
    let source = REAL
        .replace("AUTHORITY", &pem(CA))
        .replace("PORT", &server.port.to_string());
    let run = Scratch::new("real", &source).test();
    assert_eq!(
        run.status,
        Some(0),
        "stdout:\n{}\nstderr:\n{}\nserver log:\n{}",
        run.stdout,
        run.stderr,
        server.log()
    );
    assert!(
        run.stdout.contains("6 passed, 0 failed"),
        "stdout:\n{}",
        run.stdout
    );
}

/// A PostgreSQL server in a scratch directory: user `app` (password
/// `secret`) by SCRAM, and user `plain` (password `plain secret`) by a
/// cleartext password over TLS only. It listens on `localhost` with the
/// fixture certificate, and stops when dropped.
struct Server {
    port: u16,
    directory: PathBuf,
    process: Child,
}

impl Server {
    fn start() -> Self {
        let bin = postgres_binaries();
        let directory =
            std::env::temp_dir().join(format!("chimi-postgres-server-{}", std::process::id()));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let data = directory.join("data");
        std::fs::write(directory.join("password"), "secret\n").unwrap();
        std::fs::write(directory.join("server.pem"), SERVER_CERT).unwrap();
        std::fs::write(directory.join("server.key"), SERVER_KEY).unwrap();
        set_mode(&directory.join("server.key"), 0o600);
        let owner = as_owner(&directory);
        let initdb = owner(&bin.join("initdb"))
            .arg("-D")
            .arg(&data)
            .args([
                "-U",
                "app",
                "-E",
                "UTF8",
                "--auth=scram-sha-256",
                "--no-sync",
            ])
            .arg(format!("--pwfile={}", directory.join("password").display()))
            .output()
            .expect("initdb runs");
        assert!(
            initdb.status.success(),
            "initdb failed:\n{}{}",
            String::from_utf8_lossy(&initdb.stdout),
            String::from_utf8_lossy(&initdb.stderr)
        );
        std::fs::write(
            data.join("pg_hba.conf"),
            "hostssl all plain 127.0.0.1/32 password\n\
             hostssl all plain ::1/128 password\n\
             host all plain all reject\n\
             host all all 127.0.0.1/32 scram-sha-256\n\
             host all all ::1/128 scram-sha-256\n",
        )
        .unwrap();
        let port = free_port();
        let log = std::fs::File::create(directory.join("log")).unwrap();
        let process = owner(&bin.join("postgres"))
            .arg("-D")
            .arg(&data)
            .arg("-p")
            .arg(port.to_string())
            .arg("-k")
            .arg(&directory)
            .args([
                "-c",
                "listen_addresses=localhost",
                "-c",
                "ssl=on",
                "-c",
                "fsync=off",
            ])
            .args(["-c", "deadlock_timeout=200ms"])
            .arg("-c")
            .arg(format!(
                "ssl_cert_file={}",
                directory.join("server.pem").display()
            ))
            .arg("-c")
            .arg(format!(
                "ssl_key_file={}",
                directory.join("server.key").display()
            ))
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("postgres starts");
        let server = Self {
            port,
            directory,
            process,
        };
        server.wait_until_ready();
        server.create_plain_user(&bin);
        server
    }

    fn wait_until_ready(&self) {
        let started = Instant::now();
        while !self.log().contains("ready to accept connections") {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "postgres did not start:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn create_plain_user(&self, bin: &Path) {
        let psql = Command::new(bin.join("psql"))
            .env("PGPASSWORD", "secret")
            .args([
                "-h",
                "127.0.0.1",
                "-U",
                "app",
                "-d",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
            ])
            .arg("-p")
            .arg(self.port.to_string())
            .args(["-c", "create role plain login password 'plain secret'"])
            .output()
            .expect("psql runs");
        assert!(
            psql.status.success(),
            "psql failed:\n{}",
            String::from_utf8_lossy(&psql.stderr)
        );
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.directory.join("log")).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

// The server binaries: CHIMI_POSTGRES_BIN, else the newest under Debian's
// and Homebrew's usual places, else `initdb` on PATH.
fn postgres_binaries() -> PathBuf {
    if let Ok(bin) = std::env::var("CHIMI_POSTGRES_BIN") {
        return PathBuf::from(bin);
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for parent in ["/usr/lib/postgresql", "/opt/homebrew/opt", "/usr/local/opt"] {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let bin = entry.path().join("bin");
                let versioned = parent == "/usr/lib/postgresql" || name.starts_with("postgresql");
                if versioned && bin.join("initdb").exists() {
                    found.push(bin);
                }
            }
        }
    }
    found.sort_by_key(|bin| version_of(bin));
    if let Some(newest) = found.pop() {
        return newest;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if directory.join("initdb").exists() && directory.join("postgres").exists() {
                return directory;
            }
        }
    }
    panic!(
        "the PostgreSQL server binaries (initdb, postgres, psql) are not installed; \
         install them (apt install postgresql, or brew install postgresql@16) \
         or set CHIMI_POSTGRES_BIN to their directory"
    );
}

fn version_of(bin: &Path) -> u32 {
    let name = bin
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    name.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// PostgreSQL refuses to run as root, so as root the server runs as the
// `postgres` user, which then owns the scratch directory.
#[cfg(unix)]
fn as_owner(directory: &Path) -> impl Fn(&Path) -> Command + use<> {
    let root = Command::new("id")
        .arg("-u")
        .output()
        .map(|out| out.stdout == b"0\n")
        .unwrap_or(false);
    if root {
        let chown = Command::new("chown")
            .args(["-R", "postgres"])
            .arg(directory)
            .status()
            .expect("chown runs");
        assert!(
            chown.success(),
            "running as root needs a `postgres` user to own the server"
        );
    }
    move |program: &Path| {
        if root {
            let mut command = Command::new("runuser");
            command.args(["-u", "postgres", "--"]).arg(program);
            command
        } else {
            Command::new(program)
        }
    }
}

const REAL: &str = r#"import std/bytes as Bytes
import std/clock as Clock
import std/clock.{Clock}
import std/debug as Debug
import std/decimal as Decimal
import std/duration as Duration
import std/list as List
import std/int as Int
import std/net as Net
import std/net.{Network}
import std/postgres as Postgres
import std/result as Result
import std/result.{Err, Ok}
import std/sql as Sql
import std/sql.{Database, SqlError, Value}
import std/task as Task
import std/test as Test
import std/text as Text
import std/tls as Tls

const port: Text = "PORT"
const authority: Text = "AUTHORITY"

fn target(user: Text, password: Text, host: Text, mode: Text) -> Text {
  "postgres://${user}:${password}@${host}:${port}/postgres?sslmode=${mode}"
}

fn settings() -> { roots: Tls.Roots, connect_timeout: Duration.Duration } {
  { roots: Tls.roots(Bytes.from_text(authority)), connect_timeout: Duration.seconds(10) }
}

fn with_db<T>(url: Text, body: fn(Database) -> T errors SqlError needs { clock: Clock }) -> Text needs { network: Network, clock: Clock } {
  let driver = Postgres.driver(settings())
  let outcome = Result.capture(fn() { Sql.open(driver, url, { ..Sql.defaults, max_connections: 4 }, body) })
  case outcome {
    Ok(value) -> Debug.render(value)
    Err(failure) -> summary(failure)
  }
}

fn app<T>(body: fn(Database) -> T errors SqlError needs { clock: Clock }) -> Text needs { network: Network, clock: Clock } {
  with_db(target("app", "secret", "127.0.0.1", "disable"), body)
}

// The outcome without the server's wording, which changes between
// PostgreSQL versions: a constraint's name, a SQLSTATE, or the driver's own
// message.
fn rendered<T>(body: fn() -> T errors SqlError needs { clock: Clock }) -> Text needs { clock: Clock } {
  case Result.capture(body) {
    Ok(value) -> "Ok ${Debug.render(value)}"
    Err(failure) -> summary(failure)
  }
}

fn summary(failure: SqlError) -> Text {
  case failure {
    SqlError.ConstraintFailed(constraint, _) -> "ConstraintFailed ${constraint}"
    SqlError.Rejected(code, _) -> "Rejected ${code}"
    SqlError.Conflict(_) -> "Conflict"
    SqlError.ConnectFailed(message) -> case Text.starts_with(message, "Postgres: the server's") {
      True -> message
      False -> "ConnectFailed"
    }
    other -> Debug.render(other)
  }
}

fn round_trip() -> Text needs { network: Network, clock: Clock } {
  let price = Decimal.parse("12345678901234.5678")
  app(fn(db) {
    let _ = Sql.execute(db, "drop table if exists kinds", [])
    let _ = Sql.execute(db, "create table kinds (b boolean, i2 smallint, i4 integer, i8 bigint, f4 real, f8 double precision, n numeric, by bytea, t text, j jsonb, u uuid, ts timestamptz, z text)", [])
    let inserted = Sql.execute(db, "insert into kinds values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)", [Value.Bool(False), Value.Int(-32768), Value.Int(2147483647), Value.Int(-9223372036854775808), Value.Float(0.5), Value.Float(1.0e-300), Value.Decimal(price), Value.Bytes(Bytes.from([0, 92, 255, 39])), Value.Text("héllo 'quoted' ✓"), Value.Text("{\"a\": [1, 2]}"), Value.Text("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"), Value.Text("2026-11-21 10:00:00+00"), Value.Null])
    let rows = Sql.query(db, "select * from kinds", [])
    let special = Sql.query(db, "select 'NaN'::float8 as a, '-Infinity'::float8 as b, 'NaN'::numeric as c, 'Infinity'::numeric as d, 1e20::float8 as e, -0.0::float8 as f", [])
    let empty = Sql.execute(db, "", [])
    let counted = Sql.execute(db, "update kinds set t = 'x'", [])
    let made = Sql.execute(db, "create table kinds_copy as select * from kinds", [])
    let _ = Sql.execute(db, "drop table kinds_copy", [])
    "${Int_text(inserted)} ${Debug.render(rows)} ${Debug.render(special)} ${Int_text(empty)} ${Int_text(counted)} ${Int_text(made)}"
  })
}

fn finished<T, E>(task: Task.Task<T, E>, otherwise: T) -> T {
  case Result.capture(fn() { Task.result(task) }) {
    Ok(value) -> value
    Err(_) -> otherwise
  }
}

fn Int_text(value: Int) -> Text {
  "${value}"
}

fn failures() -> Text needs { network: Network, clock: Clock } {
  app(fn(db) {
    let _ = Sql.execute(db, "drop table if exists people", [])
    let _ = Sql.execute(db, "create table people (id integer primary key, name text not null)", [])
    let _ = Sql.execute(db, "insert into people values (1, 'Ada')", [])
    [
      rendered(fn() { Sql.execute(db, "insert into people values (1, 'Bob')", []) }),
      rendered(fn() { Sql.execute(db, "insert into people values (2, $1)", [Value.Null]) }),
      rendered(fn() { Sql.execute(db, "selec 1", []) }),
      rendered(fn() { Sql.execute(db, "select 1; select 2", []) }),
      rendered(fn() { Sql.execute(db, "select $1::integer", [Value.Text("seven")]) }),
      rendered(fn() { Sql.execute(db, "select $1, $2", [Value.Int(1)]) }),
      rendered(fn() { Sql.execute(db, "select * from nowhere", []) }),
      rendered(fn() { Sql.execute(db, "copy people to stdout", []) }),
      rendered(fn() { Sql.execute(db, "copy people from stdin", []) }),
      rendered(fn() { Sql.execute(db, "select 1/0", []) }),
      rendered(fn() { Sql.query(db, "select count(*) as n from people", []) }),
    ]
  })
}

fn transactions() -> Text needs { network: Network, clock: Clock } {
  app(fn(db) {
    let _ = Sql.execute(db, "drop table if exists log", [])
    let _ = Sql.execute(db, "create table log (entry text unique)", [])
    let committed = rendered(fn() { Sql.transaction(db, fn(tx) { Sql.execute(tx, "insert into log values ('kept')", []) }) })
    let rolled = rendered(fn() {
      Sql.transaction(db, fn(tx) {
        let _ = Sql.execute(tx, "insert into log values ('dropped')", [])
        Sql.execute(tx, "insert into log values ('kept')", [])
      })
    })
    let partial = rendered(fn() {
      Sql.transaction(db, fn(tx) {
        let _ = Sql.execute(tx, "insert into log values ('outer')", [])
        let inner = Result.capture(fn() {
          Sql.transaction(tx, fn(nested) {
            let _ = Sql.execute(nested, "insert into log values ('inner')", [])
            Sql.execute(nested, "insert into log values ('kept')", [])
          })
        })
        let after = Sql.execute(tx, "insert into log values ('after')", [])
        let shown = case inner {
          Ok(_) -> "kept"
          Err(failure) -> summary(failure)
        }
        "${shown} ${Int_text(after)}"
      })
    })
    let failed_commit = rendered(fn() {
      Sql.transaction(db, fn(tx) {
        let _ = Result.capture(fn() { Sql.execute(tx, "insert into log values ('kept')", []) })
        Void
      })
    })
    let ended = rendered(fn() {
      Sql.transaction(db, fn(tx) {
        let _ = Sql.execute(tx, "commit", [])
        Sql.execute(tx, "insert into log values ('late')", [])
      })
    })
    let rolled_by_hand = rendered(fn() {
      Sql.transaction(db, fn(tx) {
        let _ = Sql.execute(tx, "insert into log values ('undone')", [])
        Sql.execute(tx, "rollback", [])
      })
    })
    let shared = rendered(fn() {
      Sql.transaction(db, fn(tx) {
        let tasks = List.map(["a", "b", "c", "d", "e", "f"], fn(name) {
          Task.start(fn() { rendered(fn() { Sql.execute(tx, "insert into log values ($1)", [Value.Text(name)]) }) })
        })
        List.map(tasks, fn(task) { finished(task, "lost") })
      })
    })
    let entries = Sql.query(db, "select string_agg(entry, ',' order by entry) as entries from log", [])
    [committed, rolled, partial, failed_commit, ended, rolled_by_hand, shared, Debug.render(entries)]
  })
}

fn transfer(db: Database, first: Int, second: Int) -> Text needs { clock: Clock } {
  let outcome = Result.capture(fn() {
    Sql.transaction(db, fn(tx) {
      let _ = Sql.execute(tx, "update accounts set balance = balance + 1 where id = $1", [Value.Int(first)])
      Clock.sleep(Duration.milliseconds(300))
      Sql.execute(tx, "update accounts set balance = balance + 1 where id = $1", [Value.Int(second)])
    })
  })
  case outcome {
    Ok(_) -> "committed"
    Err(SqlError.Conflict(_)) -> "conflict"
    Err(failure) -> Debug.render(failure)
  }
}

fn deadlock() -> Text needs { network: Network, clock: Clock } {
  app(fn(db) {
    let _ = Sql.execute(db, "drop table if exists accounts", [])
    let _ = Sql.execute(db, "create table accounts (id integer primary key, balance integer)", [])
    let _ = Sql.execute(db, "insert into accounts values (1, 0), (2, 0)", [])
    let one = Task.start(fn() { transfer(db, 1, 2) })
    let two = Task.start(fn() { transfer(db, 2, 1) })
    List.sort([finished(one, "lost"), finished(two, "lost")])
  })
}

fn pooled() -> Text needs { network: Network, clock: Clock } {
  app(fn(db) {
    let tasks = List.map(List.repeat(Void, 20), fn(_) {
      Task.start(fn() {
        case Result.capture(fn() { Sql.query(db, "select pg_sleep(0.05), 1 as one", []) }) {
          Ok(rows) -> List.length(rows)
          Err(_) -> 0
        }
      })
    })
    Int.sum(List.map(tasks, fn(task) { finished(task, 0) }))
  })
}

fn secure() -> List<Text> needs { network: Network, clock: Clock } {
  let ssl = fn(db) { Sql.query(db, "select ssl from pg_stat_ssl where pid = pg_backend_pid()", []) }
  [
    with_db(target("app", "secret", "localhost", "require"), ssl),
    with_db(target("app", "secret", "localhost", "verify-full"), ssl),
    with_db(target("app", "secret", "127.0.0.1", "require"), ssl),
    with_db(target("app", "wrong", "127.0.0.1", "disable"), ssl),
    with_db(target("plain", "plain secret", "localhost", "require"), ssl),
    with_db(target("plain", "plain secret", "127.0.0.1", "disable"), ssl),
  ]
}

test "values round-trip" {
  let outcome = round_trip() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("\"1 [Row { columns: [\\\"b\\\", \\\"i2\\\", \\\"i4\\\", \\\"i8\\\", \\\"f4\\\", \\\"f8\\\", \\\"n\\\", \\\"by\\\", \\\"t\\\", \\\"j\\\", \\\"u\\\", \\\"ts\\\", \\\"z\\\"], values: [Value.Bool(False), Value.Int(-32768), Value.Int(2147483647), Value.Int(-9223372036854775808), Value.Float(0.5), Value.Float(1e-300), Value.Decimal(12345678901234.5678), Value.Bytes(Bytes(00 5c ff 27)), Value.Text(\\\"héllo 'quoted' ✓\\\"), Value.Text(\\\"{\\\\\\\"a\\\\\\\": [1, 2]}\\\"), Value.Text(\\\"a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11\\\"), Value.Text(\\\"2026-11-21 10:00:00+00\\\"), Value.Null] }] [Row { columns: [\\\"a\\\", \\\"b\\\", \\\"c\\\", \\\"d\\\", \\\"e\\\", \\\"f\\\"], values: [Value.Text(\\\"NaN\\\"), Value.Text(\\\"-Infinity\\\"), Value.Text(\\\"NaN\\\"), Value.Text(\\\"Infinity\\\"), Value.Float(1e20), Value.Float(0.0)] }] 0 1 1\"")
}

test "failures" {
  let outcome = failures() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("[\"ConstraintFailed people_pkey\", \"ConstraintFailed \", \"Rejected 42601\", \"Rejected 42601\", \"Rejected 22P02\", \"Rejected 08P01\", \"Rejected 42P01\", \"Rejected 0A000\", \"Rejected 0A000\", \"Rejected 22012\", \"Ok [Row { columns: [\\\"n\\\"], values: [Value.Int(1)] }]\"]")
}

test "transactions" {
  let outcome = transactions() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("[\"Ok 1\", \"ConstraintFailed log_entry_key\", \"Ok \\\"ConstraintFailed log_entry_key 1\\\"\", \"SqlError.Failed(\\\"Postgres: the transaction had failed and was rolled back\\\")\", \"SqlError.TransactionEnded\", \"SqlError.TransactionEnded\", \"Ok [\\\"Ok 1\\\", \\\"Ok 1\\\", \\\"Ok 1\\\", \\\"Ok 1\\\", \\\"Ok 1\\\", \\\"Ok 1\\\"]\", \"[Row { columns: [\\\"entries\\\"], values: [Value.Text(\\\"a,after,b,c,d,e,f,kept,outer\\\")] }]\"]")
}

test "a deadlock" {
  let outcome = deadlock() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("[\"committed\", \"conflict\"]")
}

test "a pool of connections" {
  let outcome = pooled() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("20")
}

test "tls and authentication" {
  let outcome = secure() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal([
    "[Row { columns: [\"ssl\"], values: [Value.Bool(True)] }]",
    "[Row { columns: [\"ssl\"], values: [Value.Bool(True)] }]",
    "Postgres: the server's certificate is not for 127.0.0.1",
    "ConnectFailed",
    "[Row { columns: [\"ssl\"], values: [Value.Bool(True)] }]",
    "ConnectFailed",
  ])
}
"#;
