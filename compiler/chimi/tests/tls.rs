//! Conformance for `std/tls`: a TLS client and server over a `Net.Connection`
//! (a `Net.pair` fake, and once over loopback sockets), the certificate checks
//! that cannot be turned off, the failures each side reports, and the
//! system's roots.
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI
//! there. The certificates are throwaway fixtures (`tests/fixtures/tls/`).

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-tls-{name}-{}-{:?}",
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

    fn test(&self, environment: &[(&str, &str)], arguments: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .env_remove("SSL_CERT_FILE")
            .envs(environment.iter().copied())
            .arg("test")
            .arg("cases.chimi")
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

fn assert_passed(run: &Run, count: usize) {
    assert_eq!(run.status, Some(0), "{}{}", run.stdout, run.stderr);
    assert!(
        run.stdout.contains(&format!("{count} passed, 0 failed")),
        "{}",
        run.stdout
    );
}

const CA: &str = include_str!("fixtures/tls/ca.pem");
const OTHER_CA: &str = include_str!("fixtures/tls/other-ca.pem");
const OTHER_KEY: &str = include_str!("fixtures/tls/other.key");
const SERVER_CERT: &str = include_str!("fixtures/tls/server.pem");
const SERVER_KEY: &str = include_str!("fixtures/tls/server.key");
const EXPIRED_CERT: &str = include_str!("fixtures/tls/expired.pem");
const FUTURE_CERT: &str = include_str!("fixtures/tls/future.pem");
const STRANGER_EXPIRED_CERT: &str = include_str!("fixtures/tls/stranger-expired.pem");

fn literal(pem: &str) -> String {
    format!("\"{}\"", pem.replace('\n', "\\n"))
}

/// The imports, the fixtures as constants, and the helpers every suite shares.
fn suite(tests: &str) -> String {
    format!(
        r#"import std/bytes as Bytes
import std/debug as Debug
import std/duration as Duration
import std/net as Net
import std/net.{{Connection, NetError, Network}}
import std/resource as Resource
import std/result as Result
import std/result.{{Err, Ok}}
import std/stream as Stream
import std/task as Task
import std/test as Test
import std/text as Text
import std/tls as Tls
import std/tls.{{TlsError}}

const CA: Text = {}
const OTHER_CA: Text = {}
const OTHER_KEY: Text = {}
const SERVER_CERT: Text = {}
const SERVER_KEY: Text = {}
const EXPIRED_CERT: Text = {}
const FUTURE_CERT: Text = {}
const STRANGER_EXPIRED_CERT: Text = {}

fn attempt<T, E, R>(operation: fn() -> T errors E needs R) -> Text needs R {{
  case Result.capture(operation) {{
    Ok(_) -> "Ok"
    Err(error) -> "Err(${{Debug.render(error)}})"
  }}
}}

fn trusting(pem: Text) -> {{ roots: Tls.Roots }} {{
  {{ ..Tls.defaults, roots: Tls.roots(Bytes.from_text(pem)) }}
}}

fn server_identity(certificate: Text) -> Tls.Identity {{
  Tls.identity(Bytes.from_text(certificate), Bytes.from_text(SERVER_KEY))
}}

// A client over `client_end` (wrapped by `secure`) and a server over
// `server_end`: the server answers what it read, to the end, with "got " and
// those bytes. Gives what each side saw. The client end is closed once the
// client is done, so a client that fails before its first byte still ends
// the server's wait.
fn exchange(client_end: Connection, server_end: Connection, secure: fn(Connection) -> Connection errors TlsError | NetError, certificate: Text, request: Text) -> (Text, Text) {{
  let server = Task.start(fn() {{
    attempt(fn() {{
      let connection = Tls.server(server_end, server_identity(certificate))
      let got = Stream.read_all(Net.reader(connection), 33554432)
      Stream.write(Net.writer(connection), Bytes.append(Bytes.from_text("got "), got))
      Stream.close(Net.writer(connection))
    }})
  }})
  let answer = case Result.capture(fn() {{
    let connection = secure(client_end)
    Stream.write(Net.writer(connection), Bytes.from_text(request))
    Stream.close(Net.writer(connection))
    Bytes.to_text(Stream.read_all(Net.reader(connection), 33554432))
  }}) {{
    Ok(text) -> text
    Err(error) -> "Err(${{Debug.render(error)}})"
  }}
  Net.close(client_end)
  (answer, Task.result(server))
}}

fn over_pair(roots: Text, certificate: Text, host: Text, request: Text) -> (Text, Text) {{
  let (client_end, server_end) = Net.pair(4)
  exchange(client_end, server_end, fn(inner) {{ Tls.client(inner, host, trusting(roots)) }}, certificate, request)
}}
{tests}"#,
        literal(CA),
        literal(OTHER_CA),
        literal(OTHER_KEY),
        literal(SERVER_CERT),
        literal(SERVER_KEY),
        literal(EXPIRED_CERT),
        literal(FUTURE_CERT),
        literal(STRANGER_EXPIRED_CERT),
    )
}

fn run(name: &str, tests: &str) -> Run {
    Scratch::new(name, &suite(tests)).test(&[], &[])
}

// Rejects plaintext on the wire's place (no encryption layer), a reader that
// stops before close_notify or after the first record, and a writer that
// drops bytes past one record: the answer comes back whole, both for a short
// message and for 1 MiB, which spans many 16 KiB records.
#[test]
fn a_client_and_a_server_exchange_bytes_over_a_fake_pair() {
    let result = run(
        "exchange",
        r#"
test "a short exchange" {
  over_pair(CA, SERVER_CERT, "localhost", "hello") |> Test.assert_equal(("got hello", "Ok"))
}

test "a body spanning many records" {
  let big = Text.repeat("abcdefgh", 131072)
  let (answer, server) = over_pair(CA, SERVER_CERT, "localhost", big)
  server |> Test.assert_equal("Ok")
  answer |> Test.assert_equal("got ${big}")
}

test "the bytes on the wire are not the plaintext" {
  let (client_end, server_end) = Net.pair(4)
  // The server reads through a tap that copies what arrives on the wire.
  let server = Task.start(fn() {
    let (copy_out, copy_in) = Stream.pipe(1024)
    let tapped = Net.connection(
      Stream.reader(fn() {
        let next = Stream.read(Net.reader(server_end))
        case next {
          Some(bytes) -> {
            let _ = Result.capture(fn() { Stream.write(copy_out, bytes) })
            Some(bytes)
          }
          None -> None
        }
      }, fn() { Stream.stop(Net.reader(server_end)) }),
      Net.writer(server_end),
      Net.local_address(server_end),
      Net.peer_address(server_end),
      fn() { Net.close(server_end) },
    )
    let connection = Tls.server(tapped, server_identity(SERVER_CERT))
    let got = Stream.read_all(Net.reader(connection), 1024)
    Stream.close(Net.writer(connection))
    Stream.close(copy_out)
    (got, Stream.read_all(copy_in, 1048576))
  })
  let connection = Tls.client(client_end, "localhost", trusting(CA))
  Stream.write(Net.writer(connection), Bytes.from_text("a secret worth hiding"))
  Stream.close(Net.writer(connection))
  let _ = Stream.read_all(Net.reader(connection), 1024)
  let (got, wire) = Task.result(server)
  got |> Test.assert_equal(Bytes.from_text("a secret worth hiding"))
  Bytes.contains(wire, Bytes.from_text("secret worth")) |> Test.assert_equal(False)
  (Bytes.length(wire) > 100) |> Test.assert_equal(True)
}
"#,
    );
    assert_passed(&result, 3);
}

// Rejects a client that skips any of the checks: a certificate from an
// authority it does not trust, one for another host, and one that has
// expired each fail the handshake on the client with their own variant, and
// the server sees the handshake fail.
#[test]
fn the_client_refuses_an_untrusted_wrong_or_expired_certificate() {
    let result = run(
        "checks",
        r#"
test "an untrusted authority" {
  let (client, server) = over_pair(OTHER_CA, SERVER_CERT, "localhost", "hello")
  client |> Test.assert_equal("Err(TlsError.Untrusted)")
  Text.starts_with(server, "Err(TlsError.Handshake(") |> Test.assert_equal(True)
}

test "another host" {
  let (client, server) = over_pair(CA, SERVER_CERT, "example.com", "hello")
  client |> Test.assert_equal("Err(TlsError.WrongHost(\"example.com\"))")
  Text.starts_with(server, "Err(TlsError.Handshake(") |> Test.assert_equal(True)
}

test "an expired certificate" {
  let (client, _server) = over_pair(CA, EXPIRED_CERT, "localhost", "hello")
  client |> Test.assert_equal("Err(TlsError.Expired)")
}

test "a certificate not yet valid" {
  let (client, _server) = over_pair(CA, FUTURE_CERT, "localhost", "hello")
  client |> Test.assert_equal("Err(TlsError.Expired)")
}

test "a stranger's expired certificate is untrusted first" {
  let (client, _server) = over_pair(CA, STRANGER_EXPIRED_CERT, "localhost", "hello")
  client |> Test.assert_equal("Err(TlsError.Untrusted)")
}

test "an empty host" {
  let (client, _server) = over_pair(CA, SERVER_CERT, "", "hello")
  Text.starts_with(client, "Err(TlsError.Invalid(") |> Test.assert_equal(True)
  let (client_end, _server_end) = Net.pair(4)
  let system = attempt(fn() { Tls.client(client_end, "", Tls.defaults) })
  Text.starts_with(system, "Err(TlsError.Invalid(") |> Test.assert_equal(True)
}
"#,
    );
    assert_passed(&result, 6);
}

// Rejects roots or an identity that are accepted unchecked and fail only at
// the first handshake, and a key that is not the certificate's.
#[test]
fn bad_roots_and_identities_fail_where_they_are_made() {
    let result = run(
        "invalid",
        r#"
test "roots that are not PEM" {
  let got = attempt(fn() { Tls.roots(Bytes.from_text("not a certificate")) })
  Text.starts_with(got, "Err(TlsError.Invalid(") |> Test.assert_equal(True)
}

test "an empty bundle" {
  let got = attempt(fn() { Tls.roots(Bytes.empty()) })
  Text.starts_with(got, "Err(TlsError.Invalid(") |> Test.assert_equal(True)
}

test "a key that is not PEM" {
  let got = attempt(fn() { Tls.identity(Bytes.from_text(SERVER_CERT), Bytes.from_text("nope")) })
  Text.starts_with(got, "Err(TlsError.Invalid(") |> Test.assert_equal(True)
}

test "a key that is not the certificate's" {
  let got = attempt(fn() { Tls.identity(Bytes.from_text(SERVER_CERT), Bytes.from_text(OTHER_KEY)) })
  Text.starts_with(got, "Err(TlsError.Invalid(") |> Test.assert_equal(True)
}

test "a good bundle and identity" {
  attempt(fn() { Tls.roots(Bytes.from_text(CA)) }) |> Test.assert_equal("Ok")
  attempt(fn() { server_identity(SERVER_CERT) }) |> Test.assert_equal("Ok")
}
"#,
    );
    assert_passed(&result, 5);
}

// Rejects a reader that treats a cut connection as a clean end (the data may
// be truncated), a handshake that hangs or reports success when the peer
// leaves or speaks something else, and a closed TLS connection that still
// reads.
#[test]
fn a_peer_that_leaves_or_speaks_something_else_fails_the_connection() {
    let result = run(
        "peers",
        r#"
test "the peer ends during the handshake" {
  let (client_end, server_end) = Net.pair(4)
  let leaving = Task.start(fn() {
    let _ = Stream.read(Net.reader(server_end))
    Stream.close(Net.writer(server_end))
  })
  attempt(fn() { Tls.client(client_end, "localhost", trusting(CA)) })
    |> Test.assert_equal("Err(TlsError.Handshake(\"the peer ended the connection during the handshake\"))")
  Task.result(leaving)
}

test "the peer is not speaking TLS" {
  let (client_end, server_end) = Net.pair(4)
  let talking = Task.start(fn() {
    let _ = Stream.read(Net.reader(server_end))
    Stream.write(Net.writer(server_end), Bytes.from_text("HTTP/1.1 400 Bad Request\r\n\r\n"))
    Stream.close(Net.writer(server_end))
  })
  let got = attempt(fn() { Tls.client(client_end, "localhost", trusting(CA)) })
  Text.starts_with(got, "Err(TlsError.Handshake(") |> Test.assert_equal(True)
  Task.result(talking)
}

test "the peer ends without close_notify" {
  let (client_end, server_end) = Net.pair(4)
  let server = Task.start(fn() {
    let connection = Tls.server(server_end, server_identity(SERVER_CERT))
    Stream.write(Net.writer(connection), Bytes.from_text("partial"))
    // Ends the inner stream, skipping the TLS close.
    Stream.close(Net.writer(server_end))
  })
  let connection = Tls.client(client_end, "localhost", trusting(CA))
  Stream.read(Net.reader(connection)) |> Test.assert_equal(Some(Bytes.from_text("partial")))
  attempt(fn() { Stream.read(Net.reader(connection)) })
    |> Test.assert_equal("Err(NetError.Failed(\"TLS: the peer ended the connection without close_notify\"))")
  Task.result(server)
}

test "closing the writer sends close_notify, then ends the inner stream" {
  let (client_end, server_end) = Net.pair(4)
  let server = Task.start(fn() {
    let connection = Tls.server(server_end, server_identity(SERVER_CERT))
    let first = Stream.read(Net.reader(connection))
    // The TLS stream ends at close_notify, and the inner one right after.
    (first, Stream.read(Net.reader(server_end)))
  })
  let connection = Tls.client(client_end, "localhost", trusting(CA))
  Stream.close(Net.writer(connection))
  Task.result(server) |> Test.assert_equal((None, None))
}

test "a closed connection" {
  let (client_end, server_end) = Net.pair(4)
  let server = Task.start(fn() { Tls.server(server_end, server_identity(SERVER_CERT)) })
  let connection = Tls.client(client_end, "localhost", trusting(CA))
  let _ = Task.result(server)
  Net.close(connection)
  attempt(fn() { Stream.read(Net.reader(connection)) }) |> Test.assert_equal("Err(NetError.Closed)")
  attempt(fn() { Stream.write(Net.writer(connection), Bytes.from_text("late")) })
    |> Test.assert_equal("Err(NetError.Closed)")
}
"#,
    );
    assert_passed(&result, 5);
}

// Rejects a default that trusts nothing or everything, and one that ignores
// `SSL_CERT_FILE`: the system's roots do not include the fixture authority,
// `SSL_CERT_FILE` naming it makes the defaults trust it, and naming a missing
// file fails closed with `NoRoots`.
#[test]
fn the_defaults_trust_the_systems_roots() {
    let tests = r#"
test "the defaults" {
  let (client_end, server_end) = Net.pair(4)
  let (client, _server) = exchange(client_end, server_end, fn(inner) { Tls.client(inner, "localhost", Tls.defaults) }, SERVER_CERT, "hello")
  client |> Debug.render |> Test.assert_equal(Debug.render(expected()))
}
"#;
    // The expectation differs per run, so each run gets its own suite.
    for (environment, expected) in [
        (None, "Err(TlsError.Untrusted)"),
        (Some("ca.pem"), "got hello"),
        (Some("missing.pem"), "Err(TlsError.NoRoots)"),
    ] {
        let source = suite(&format!(
            "{tests}\nfn expected() -> Text {{ {} }}\n",
            literal(expected)
        ));
        let scratch = Scratch::new("system", &source);
        std::fs::write(scratch.0.join("ca.pem"), CA).unwrap();
        let file = environment.map(|name| scratch.0.join(name).to_string_lossy().into_owned());
        let variables = file
            .as_deref()
            .map(|path| vec![("SSL_CERT_FILE", path)])
            .unwrap_or_default();
        let result = scratch.test(&variables, &[]);
        assert_passed(&result, 1);
    }
}

// Rejects a wrapper that only works over the fake pair: a TLS exchange over
// real loopback sockets.
#[test]
fn a_tls_connection_runs_over_loopback_sockets() {
    let result = Scratch::new(
        "loopback",
        &suite(
            r#"
fn over_loopback() -> (Text, Text) needs { network: Network } {
  Resource.scoped(fn() { Net.listen("127.0.0.1", 0) }, Net.close_listener, fn(listener) {
    let port = Net.port(Net.listener_address(listener))
    let accepted = Task.start(fn() { Net.accept(listener) })
    Resource.scoped(fn() { Net.connect("127.0.0.1", port) }, Net.close, fn(client_end) {
      let server_end = Task.result(accepted)
      let result = exchange(client_end, server_end, fn(inner) { Tls.client(inner, "localhost", trusting(CA)) }, SERVER_CERT, "over a socket")
      Net.close(server_end)
      result
    })
  })
}

test "loopback" {
  let result = over_loopback() using { network: Net.host_for_tests() }
  result |> Test.assert_equal(("got over a socket", "Ok"))
}
"#,
        ),
    )
    .test(&[], &["--clock=real"]);
    assert_passed(&result, 1);
}

// Rejects a session that outlives its connection: one freed only when the
// handshake fails and not when a timeout cuts it short, or one that `Net.close`
// leaves open. The count of live sessions comes back to zero after each.
#[test]
fn no_session_outlives_its_connection_or_its_handshake() {
    let result = run(
        "leaks",
        r#"
test "sessions are freed" {
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
  let _ = over_pair(OTHER_CA, SERVER_CERT, "localhost", "hello")
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
  // A server that never answers, and a client that gives up waiting.
  let (client_end, _server_end) = Net.pair(4)
  let waited = attempt(fn() {
    Task.timeout(fn() { Tls.client(client_end, "localhost", trusting(CA)) }, Duration.seconds(1))
  })
  Text.starts_with(waited, "Err(") |> Test.assert_equal(True)
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
  // An open connection holds one session at each end until it is closed.
  let (left, right) = Net.pair(4)
  let server = Task.start(fn() { Tls.server(right, server_identity(SERVER_CERT)) })
  let client = Tls.client(left, "localhost", trusting(CA))
  let served = Task.result(server)
  Tls.live_sessions_for_tests() |> Test.assert_equal(2)
  Net.close(client)
  Net.close(served)
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
}
"#,
    );
    assert_passed(&result, 1);
}

/// Starts `openssl s_server` on a free loopback port and returns it once it
/// is listening. A port read from a released listener can be taken again
/// before the server binds it, so the server's own `ACCEPT` line, which it
/// prints only after binding, is the readiness signal, and a server that
/// exits first (its port was taken) is started again on another port.
fn start_openssl_server(scratch: &Scratch, version: &str) -> (std::process::Child, u16) {
    let free = std::iter::repeat_with(|| {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    });
    start_openssl_server_on(scratch, version, free)
}

/// Starts `openssl s_server` on the first of `ports` it can bind, trying at
/// most five.
fn start_openssl_server_on(
    scratch: &Scratch,
    version: &str,
    ports: impl Iterator<Item = u16>,
) -> (std::process::Child, u16) {
    use std::io::BufRead;
    for port in ports.take(5) {
        let mut server = Command::new("openssl")
            .current_dir(&scratch.0)
            .args(["s_server", "-www", "-num_tickets", "2", version])
            .args(["-cert", "server.pem", "-key", "server.key"])
            .args(["-accept", &format!("127.0.0.1:{port}")])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("openssl s_server runs (the TLS interop test needs OpenSSL)");
        let (ready, listening) = std::sync::mpsc::channel();
        let output = std::io::BufReader::new(server.stdout.take().unwrap());
        // Reads the server's output to its end, so it never blocks on a full pipe.
        std::thread::spawn(move || {
            for line in output.lines() {
                let Ok(line) = line else { break };
                if line.starts_with("ACCEPT") {
                    let _ = ready.send(());
                }
            }
        });
        match listening.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(()) => return (server, port),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The server exited before it listened.
                let _ = server.wait();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = server.kill();
                let _ = server.wait();
                panic!("openssl s_server did not start");
            }
        }
    }
    panic!("openssl s_server could not bind a loopback port in five tries");
}

// Rejects a server start that trusts a port it never bound: the first port
// offered is already taken, so the server must come up on the second.
#[test]
fn the_openssl_server_moves_off_a_taken_port() {
    let scratch = Scratch::new("openssl-port", "");
    std::fs::write(scratch.0.join("server.pem"), SERVER_CERT).unwrap();
    std::fs::write(scratch.0.join("server.key"), SERVER_KEY).unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let ports = [taken_port, free].into_iter();
    let (mut server, port) = start_openssl_server_on(&scratch, "-tls1_3", ports);
    assert_eq!(port, free);
    assert!(server.try_wait().unwrap().is_none(), "the server exited");
    let _ = server.kill();
    let _ = server.wait();
    drop(taken);
}

// Rejects an engine that tests only against itself: OpenSSL's server sends
// TLS 1.3 session tickets after the handshake (a record with no data, ahead of
// the answer), and speaks TLS 1.2 when told to. Twenty exchanges each, since
// a client that waits for input it already holds hangs only when the ticket
// and the answer arrive in one read.
#[test]
fn a_client_talks_to_an_openssl_server() {
    for version in ["-tls1_3", "-tls1_2"] {
        let scratch = Scratch::new("openssl", "");
        std::fs::write(scratch.0.join("server.pem"), SERVER_CERT).unwrap();
        std::fs::write(scratch.0.join("server.key"), SERVER_KEY).unwrap();
        let (mut server, port) = start_openssl_server(&scratch, version);
        let tests = format!(
            r#"
import std/clock.{{Clock}}
import std/http as Http
import std/http.{{Method}}

fn fetch() -> Text needs {{ network: Network }} {{
  Resource.scoped(fn() {{ Net.connect("127.0.0.1", {port}) }}, Net.close, fn(inner) {{
    let connection = Tls.client(inner, "localhost", trusting(CA))
    Stream.write(Net.writer(connection), Bytes.from_text("GET / HTTP/1.0\r\n\r\n"))
    let page = Bytes.to_text(Stream.read_all(Net.reader(connection), 1048576))
    Net.close(connection)
    page
  }})
}}

// `-www` answers HTTP/1.0 without a length, so the body runs to the close.
fn page_by_http() -> Text needs {{ network: Network, clock: Clock }} {{
  let reply = Http.send(Method.Get, "https://localhost:{port}/", [], Bytes.empty(), {{ ..Http.client_defaults, roots: Tls.roots(Bytes.from_text(CA)) }})
  "${{Http.status(reply)}} ${{Debug.render(Text.contains(Bytes.to_text(Http.response_body(reply)), "</HTML>"))}}"
}}

test "a page through Http.send" {{
  let page = page_by_http() using {{ network: Net.host_for_tests() }}
  page |> Test.assert_equal("200 True")
}}

test "twenty pages" {{
  let pages = Task.timeout(fn() {{
    loop count = 0 {{
      case count == 20 {{
        True -> break count
        False -> {{
          let page = fetch() using {{ network: Net.host_for_tests() }}
          Text.starts_with(page, "HTTP/1.0 200 ok") |> Test.assert_equal(True)
          count + 1
        }}
      }}
    }}
  }}, Duration.seconds(60))
  pages |> Test.assert_equal(20)
}}
"#
        );
        std::fs::write(scratch.0.join("cases.chimi"), suite(&tests)).unwrap();
        let result = scratch.test(&[], &["--clock=real"]);
        let _ = server.kill();
        let _ = server.wait();
        assert_passed(&result, 2);
    }
}

// Rejects an engine that ends the stream at two empty data records, or asks
// for input it already holds after a record of several session tickets: the
// engine runs, in C, against records a `Tls.server` never sends
// (`fixtures/tls-engine.c`), linked with the package archive the build
// script prebuilt.
#[test]
fn the_engine_reads_past_records_without_data() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let package = manifest.join("runtime/native/bindings/tls");
    let archive = std::path::Path::new(env!("OUT_DIR")).join("libchimi_bind_tls.a");
    assert!(
        archive.exists(),
        "the build script did not prebuild the tls package: {}",
        archive.display()
    );
    let executable = std::env::temp_dir().join(format!("chimi-tls-engine-{}", std::process::id()));
    let mut compile = Command::new("cc");
    compile.args(["-std=c11", "-O1", "-w"]);
    for include in [
        "",
        "mbedtls/include",
        "mbedtls/library",
        "mbedtls/tf-psa-crypto/include",
        "mbedtls/tf-psa-crypto/drivers/builtin/include",
        "mbedtls/tf-psa-crypto/drivers/builtin/src",
        "mbedtls/tf-psa-crypto/core",
        "mbedtls/tf-psa-crypto/dispatch",
        "mbedtls/tf-psa-crypto/utilities",
        "mbedtls/tf-psa-crypto/platform",
        "mbedtls/tf-psa-crypto/extras",
    ] {
        compile.arg("-I").arg(package.join(include));
    }
    let built = compile
        .arg(manifest.join("tests/fixtures/tls-engine.c"))
        .arg(&archive)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("run cc");
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = Command::new(&executable)
        .arg(manifest.join("tests/fixtures/tls"))
        .output()
        .expect("run the engine fixture");
    let _ = std::fs::remove_file(&executable);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "one empty record: data hello closed\n\
         two empty records: data hello closed\n\
         three empty records: data hello closed\n\
         two tickets in a record: data hello closed\n\
         eight tickets in a record: data hello closed\n\
         forty tickets in a record: data hello closed\n",
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

// Rejects a vendored mbedTLS edited in place, or a file added or dropped
// without rerunning `scripts/vendor-mbedtls.sh`: every file under the vendored
// tree matches its line in `MANIFEST.sha256`, and the manifest lists every
// file there.
#[test]
fn the_vendored_mbedtls_matches_its_manifest() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("runtime/native/bindings/tls/mbedtls");
    let manifest = std::fs::read_to_string(root.join("MANIFEST.sha256")).unwrap();
    let listed: std::collections::BTreeSet<&str> = manifest
        .lines()
        .map(|line| line.split_once("  ").expect("a sha256sum line").1)
        .collect();
    let mut present = std::collections::BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if relative != "MANIFEST.sha256" && relative != "PROVENANCE.md" {
                    present.insert(relative);
                }
            }
        }
    }
    let listed: std::collections::BTreeSet<String> =
        listed.into_iter().map(str::to_owned).collect();
    assert_eq!(present, listed, "files differ from MANIFEST.sha256");
    // sha256sum on Linux, shasum on macOS.
    let check = Command::new("sha256sum")
        .args(["--check", "--quiet", "MANIFEST.sha256"])
        .current_dir(&root)
        .output()
        .or_else(|_| {
            Command::new("shasum")
                .args(["-a", "256", "--check", "--quiet", "MANIFEST.sha256"])
                .current_dir(&root)
                .output()
        })
        .expect("run sha256sum or shasum");
    assert!(
        check.status.success(),
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}

// Rejects a TLS listener that handshakes in `accept` (a silent client would
// then hold every other client), one whose connections skip the handshake
// or report its failure as anything but a connection failure, and one that
// leaks the sessions of connections it accepted.
#[test]
fn a_tls_listener_hands_over_connections_that_handshake_when_first_used() {
    let result = run(
        "listener",
        r#"
// A listener that hands over `first`, then `second`, then fails as closed.
fn handing(first: Connection, second: Connection) -> Net.Listener {
  let (script, plan) = Stream.pipe(4)
  Stream.write(script, Bytes.from([1]))
  Stream.write(script, Bytes.from([2]))
  Stream.close(script)
  Net.listener(fn() {
    case Result.capture(fn() { Stream.read(plan) }) {
      Ok(Some(step)) -> case step == Bytes.from([1]) {
        True -> first
        False -> second
      }
      _ -> Failure(NetError.Closed)
    }
  }, Net.address("fake", 443), fn() { Void })
}

test "accepting waits for no client, and the first read handshakes" {
  let (silent, silent_end) = Net.pair(4)
  let (client_end, server_end) = Net.pair(4)
  let listener = Tls.listener(handing(silent_end, server_end), server_identity(SERVER_CERT))
  // The first client never says anything; accepting it does not wait.
  let quiet = Net.accept(listener)
  let served = Net.accept(listener)
  Tls.live_sessions_for_tests() |> Test.assert_equal(2)
  let server = Task.start(fn() {
    let got = Stream.read_all(Net.reader(served), 1024)
    Stream.write(Net.writer(served), Bytes.append(Bytes.from_text("got "), got))
    Stream.close(Net.writer(served))
    Net.close(served)
  })
  let client = Tls.client(client_end, "localhost", trusting(CA))
  Stream.write(Net.writer(client), Bytes.from_text("hello"))
  Stream.close(Net.writer(client))
  Bytes.to_text(Stream.read_all(Net.reader(client), 1024)) |> Test.assert_equal("got hello")
  Task.result(server)
  Net.close(client)
  Net.close(quiet)
  Net.close(silent)
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
  Net.listener_address(listener) |> Test.assert_equal(Net.address("fake", 443))
  attempt(fn() { Net.accept(listener) }) |> Test.assert_equal("Err(NetError.Closed)")
}

test "a client that does not speak TLS fails the first read" {
  let (client_end, server_end) = Net.pair(4)
  let (_unused, other) = Net.pair(4)
  let listener = Tls.listener(handing(server_end, other), server_identity(SERVER_CERT))
  let served = Net.accept(listener)
  Stream.write(Net.writer(client_end), Bytes.from_text("GET / HTTP/1.1\r\nHost: x\r\n\r\n"))
  let read = attempt(fn() { Stream.read(Net.reader(served)) })
  Text.starts_with(read, "Err(NetError.Failed(\"TLS: the handshake failed: ") |> Test.assert_equal(True)
  Net.close(served)
  Net.close(client_end)
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
}

test "a server that writes first handshakes on its write" {
  let (client_end, server_end) = Net.pair(4)
  let (_unused, other) = Net.pair(4)
  let listener = Tls.listener(handing(server_end, other), server_identity(SERVER_CERT))
  let served = Net.accept(listener)
  let server = Task.start(fn() {
    Stream.write(Net.writer(served), Bytes.from_text("220 ready"))
    Stream.close(Net.writer(served))
    Net.close(served)
  })
  let client = Tls.client(client_end, "localhost", trusting(CA))
  Bytes.to_text(Stream.read_all(Net.reader(client), 1024)) |> Test.assert_equal("220 ready")
  Task.result(server)
  Net.close(client)
  Tls.live_sessions_for_tests() |> Test.assert_equal(0)
}
"#,
    );
    assert_passed(&result, 3);
}

// Rejects HTTPS that a silent client can stall: `Http.serve` over a TLS
// listener answers a client over loopback while another connection that
// never sends a byte is open, and frees every session once serving ends.
#[test]
fn http_serve_over_a_tls_listener_is_https() {
    let result = Scratch::new(
        "https",
        &suite(
            r#"
import std/clock.{Clock}
import std/http as Http
import std/http.{Request, Response}

fn hello(request: Request) -> Response {
  Http.text(200, "hello ${Text.join(Http.path(request), "/")}")
}

fn https() -> Text needs { network: Network, clock: Clock } {
  let listener = Tls.listener(Net.listen("127.0.0.1", 0), server_identity(SERVER_CERT))
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, Http.defaults, hello) })
  let silent = Net.connect("127.0.0.1", port)
  let answer = Task.timeout(fn() {
    Resource.scoped(fn() { Tls.client(Net.connect("127.0.0.1", port), "localhost", trusting(CA)) }, Net.close, fn(connection) {
      Stream.write(Net.writer(connection), Bytes.from_text("GET /a/b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"))
      Bytes.to_text(Stream.read_all(Net.reader(connection), 65536))
    })
  }, Duration.seconds(5))
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  Net.close(silent)
  let body = case Text.split_once(answer, "\r\n\r\n") {
    Some((_, body)) -> body
    None -> answer
  }
  "${Debug.render(Text.starts_with(answer, "HTTP/1.1 200 OK\r\n"))} ${body} ${Tls.live_sessions_for_tests()}"
}

test "https" {
  let outcome = https() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("True hello a/b 0")
}
"#,
        ),
    )
    .test(&[], &["--clock=real"]);
    assert_passed(&result, 1);
}

// Rejects an HTTPS client that skips the certificate checks or the host
// name: `Http.send` gets an answer from `Http.serve` over `Tls.listener`
// when it trusts the fixture authority and names `localhost`, and fails with
// the `TlsError` when it trusts only the system's roots or names another
// host the certificate does not cover.
#[test]
fn http_send_over_https_checks_the_certificate() {
    let result = Scratch::new(
        "https-client",
        &suite(
            r#"
import std/clock.{Clock}
import std/http as Http
import std/http.{Method, Request, Response}

fn hello(request: Request) -> Response {
  Http.text(200, "hello ${Text.join(Http.path(request), "/")} ${Debug.render(Http.header(request, "host"))}")
}

fn get(url: Text, roots: Tls.Roots) -> Text needs { network: Network, clock: Clock } {
  case Result.capture(fn() { Http.send(Method.Get, url, [], Bytes.empty(), { ..Http.client_defaults, roots: roots, timeout: Duration.seconds(5) }) }) {
    Ok(reply) -> "${Http.status(reply)} ${Bytes.to_text(Http.response_body(reply))}"
    Err(error) -> Debug.render(error)
  }
}

fn https() -> Text needs { network: Network, clock: Clock } {
  let listener = Tls.listener(Net.listen("127.0.0.1", 0), server_identity(SERVER_CERT))
  let port = Net.port(Net.listener_address(listener))
  let server = Task.start(fn() { Http.serve(listener, Http.defaults, hello) })
  let fixture = Tls.roots(Bytes.from_text(CA))
  let trusted = get("https://localhost:${port}/a/b", fixture)
  let system = get("https://localhost:${port}/", Tls.defaults.roots)
  let other_host = get("https://127.0.0.1:${port}/", fixture)
  Net.close_listener(listener)
  Task.timeout(fn() { Task.result(server) }, Duration.seconds(5))
  let shown = Text.join(Text.split(trusted, "${port}"), "PORT")
  "${shown}|${system}|${other_host} ${Tls.live_sessions_for_tests()}"
}

test "https" {
  let outcome = https() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("200 hello a/b Some(\"localhost:PORT\")|TlsError.Untrusted|TlsError.WrongHost(\"127.0.0.1\") 0")
}
"#,
        ),
    )
    .test(&[], &["--clock=real"]);
    assert_passed(&result, 1);
}

// Rejects a TLS listener that only mbedTLS can talk to: OpenSSL's client,
// over TLS 1.3 and TLS 1.2, gets an HTTPS answer from `Http.serve` over
// `Tls.listener`, checking the fixture authority and the name `localhost`.
#[test]
fn an_openssl_client_talks_to_a_tls_listener() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let tests = format!(
        r#"
import std/clock.{{Clock}}
import std/http as Http
import std/http.{{Request, Response}}

fn hello(request: Request) -> Response {{
  Http.text(200, "hello from chimi")
}}

fn serve_for_a_while() -> Text needs {{ network: Network, clock: Clock }} {{
  let listener = Tls.listener(Net.listen("127.0.0.1", {port}), server_identity(SERVER_CERT))
  let _ = Result.capture(fn() {{ Task.timeout(fn() {{ Http.serve(listener, Http.defaults, hello) }}, Duration.seconds(8)) }})
  "served"
}}

test "serve" {{
  let outcome = serve_for_a_while() using {{ network: Net.host_for_tests() }}
  outcome |> Test.assert_equal("served")
}}
"#
    );
    let scratch = Scratch::new("openssl-client", &suite(&tests));
    std::fs::write(scratch.0.join("ca.pem"), CA).unwrap();
    let directory = scratch.0.clone();
    let serving = std::thread::spawn(move || {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&directory)
            .env_remove("SSL_CERT_FILE")
            .args(["test", "cases.chimi", "--clock=real"])
            .output()
            .expect("run chimi");
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    });
    let started = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(6),
            "the Chimi server did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for version in ["-tls1_3", "-tls1_2"] {
        let mut client = Command::new("openssl")
            .current_dir(&scratch.0)
            .args(["s_client", "-quiet", "-verify_return_error", version])
            .args(["-CAfile", "ca.pem", "-servername", "localhost"])
            .args(["-verify_hostname", "localhost"])
            .args(["-connect", &format!("127.0.0.1:{port}")])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("openssl s_client runs (the TLS interop test needs OpenSSL)");
        use std::io::Write;
        client
            .stdin
            .take()
            .unwrap()
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let output = client.wait_with_output().unwrap();
        let answer = String::from_utf8_lossy(&output.stdout);
        assert!(
            answer.starts_with("HTTP/1.1 200 OK\r\n") && answer.ends_with("hello from chimi"),
            "{version}: {answer}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let (status, stdout, stderr) = serving.join().unwrap();
    assert_eq!(status, Some(0), "{stdout}{stderr}");
}

// Rejects a deferred handshake that two tasks drive at once: a server that
// reads in one task and writes its banner in another, the reader first,
// leaves the writer waiting in an inner read that never comes unless the
// second task waits for the first's handshake. Over loopback, since a fake
// pair's channel ends belong to one task.
#[test]
fn a_deferred_handshake_is_driven_by_one_task_at_a_time() {
    let result = Scratch::new(
        "deferred-tasks",
        &suite(
            r#"
import std/clock.{Clock}

fn banner() -> Text needs { network: Network, clock: Clock } {
  let listener = Tls.listener(Net.listen("127.0.0.1", 0), server_identity(SERVER_CERT))
  let port = Net.port(Net.listener_address(listener))
  let client_end = Net.connect("127.0.0.1", port)
  let served = Net.accept(listener)
  let reading = Task.start(fn() { Bytes.to_text(Stream.read_all(Net.reader(served), 1024)) })
  let writing = Task.start(fn() {
    Stream.write(Net.writer(served), Bytes.from_text("220 ready"))
    Stream.close(Net.writer(served))
  })
  let client = Tls.client(client_end, "localhost", trusting(CA))
  let seen = Task.timeout(fn() { Bytes.to_text(Stream.read_all(Net.reader(client), 1024)) }, Duration.seconds(5))
  Task.timeout(fn() { Task.result(writing) }, Duration.seconds(5))
  Stream.write(Net.writer(client), Bytes.from_text("QUIT"))
  Stream.close(Net.writer(client))
  let heard = Task.timeout(fn() { Task.result(reading) }, Duration.seconds(5))
  Net.close(client)
  Net.close(served)
  Net.close_listener(listener)
  "${seen} ${heard} ${Tls.live_sessions_for_tests()}"
}

test "two tasks" {
  let outcome = banner() using { network: Net.host_for_tests() }
  outcome |> Test.assert_equal("220 ready QUIT 0")
}
"#,
        ),
    )
    .test(&[], &["--clock=real"]);
    assert_passed(&result, 1);
}
