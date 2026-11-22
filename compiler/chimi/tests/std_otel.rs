//! Conformance for `std/otel` (CHI-296): `Otel.metrics_request` turns the
//! runner's metrics document into the OTLP/JSON body that
//! `chimi metrics export --otlp` writes, on both backends.
//!
//! Each test writes its program to a scratch directory and runs the CLI there.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-otel-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }

    fn chimi(&self, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("run chimi")
    }

    /// `main`'s text outcome on both backends, which must agree.
    fn outcome(&self, program: &str) -> String {
        self.write("main.chimi", program);
        let interpreted = self.chimi(&["run", "main.chimi"]);
        let interpreted = text_outcome(&String::from_utf8(interpreted.stdout).unwrap());
        let built = self.chimi(&["build", "main.chimi", "-o", "program"]);
        assert!(built.status.success(), "{built:?}");
        let outcome = self.0.join("outcome.jsonl");
        let _ = std::fs::remove_file(&outcome);
        let native = Command::new(self.0.join("program"))
            .current_dir(&self.0)
            .arg("--chimi-runner")
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg("--")
            .output()
            .unwrap();
        assert!(native.status.success(), "{native:?}");
        let native = text_outcome(&std::fs::read_to_string(&outcome).unwrap());
        assert_eq!(interpreted, native, "the backends disagree");
        native
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The text value of the outcome record among `lines`.
fn text_outcome(lines: &str) -> String {
    let line = lines
        .lines()
        .find(|line| line.starts_with(r#"{"schema":"chimi.runtime.v1","outcome":"success""#))
        .unwrap_or_else(|| panic!("no success outcome in {lines}"));
    let value = line
        .split_once("\"value\":")
        .expect("a value")
        .1
        .trim_end_matches('}');
    let mut text = String::new();
    let mut characters = value.trim_matches('"').chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some('n') => text.push('\n'),
                Some(other) => text.push(other),
                None => {}
            },
            other => text.push(other),
        }
    }
    text
}

const DOCUMENT: &str = concat!(
    r#"{"schema":"chimi.metrics.v1","metrics":["#,
    r#"{"name":"charge_seconds","labels":{},"kind":"observe","count":4,"sum":11.75,"#,
    r#""bounds":[0.0,0.005,0.01,0.025,0.05,0.075,0.1,0.25,0.5,0.75,1.0,2.5,5.0,7.5,10.0],"#,
    r#""bucket_counts":[1,0,0,0,0,0,0,1,1,0,0,0,0,0,0,1]},"#,
    r#"{"name":"mode","labels":{},"kind":"count","value":1},"#,
    r#"{"name":"mode","labels":{},"kind":"gauge","value":2.0},"#,
    r#"{"name":"orders","labels":{"route":"/pay","method":"POST"},"kind":"count","value":3},"#,
    r#"{"name":"orders","labels":{},"kind":"count","value":9223372036854775807},"#,
    r#"{"name":"queue \"depth\"","labels":{"route":"b"},"kind":"gauge","value":-3.5e-7},"#,
    r#"{"name":"\u00e9t\u00e9 \ud83d\ude00","labels":{"z\u0000":"\u2603"},"kind":"gauge","value":1e3}"#,
    r#"],"dropped":{"label_sets":2,"kind_conflicts":18446744073709551615},"#,
    r#""start_unix_ns":1794303000000000000,"time_unix_ns":18446744073709551615}"#,
    "\n"
);

const CONVERT: &str = r#"import std/fs as Fs.{Files}
import std/json as Json
import std/otel as Otel
import std/path as Path

pub fn main() -> Text needs { files: Files } {
  let ours = Otel.metrics_request(Fs.read_text(Path.from("document.json")), "shop")
  let theirs = case Json.decode(Fs.read_text(Path.from("export.json"))) {
    Success(value) -> Json.encode(value)
    Failure(_) -> "the export is not JSON"
  }
  case ours == theirs {
    True -> "same"
    False -> "${ours}\n${theirs}"
  }
}
"#;

// The body is the CLI's export, compared as JSON values, apart from the
// scope's version, which only the CLI knows. Rejects a count, histogram
// count or bucket count written as a number instead of decimal text, a gauge
// or bound rounded, labels or groups lost, labels out of key order, a name
// that changes kind merged into one metric, a time or dropped total above
// the largest `Int` refused or left out, or an escape gone wrong.
#[test]
fn a_metrics_request_is_what_the_cli_exports() {
    let scratch = Scratch::new("matches-cli");
    scratch.write("document.json", DOCUMENT);
    let export = scratch.chimi(&[
        "metrics",
        "export",
        "--otlp",
        "--service-name",
        "shop",
        "document.json",
    ]);
    assert!(export.status.success(), "{export:?}");
    let export = String::from_utf8(export.stdout).unwrap();
    let version = format!(",\"version\":\"{}\"", env!("CARGO_PKG_VERSION"));
    assert!(export.contains(&version), "{export}");
    scratch.write("export.json", &export.replace(&version, ""));
    assert!(
        export.contains("\"chimi.metrics.dropped_kind_conflicts\""),
        "{export}"
    );
    assert!(export.contains(r#"[{"key":"method""#), "{export}");

    assert_eq!(scratch.outcome(CONVERT), "same");
}

// Documents a runtime does not write but a person might: no metrics, no
// dropped totals, and dropped totals that are not whole numbers. Rejects a
// dropped total of zero, a fraction or a negative number written as a count.
#[test]
fn an_unusual_document_is_what_the_cli_exports() {
    let scratch = Scratch::new("unusual");
    for document in [
        r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":0,"time_unix_ns":0}"#,
        r#"{"schema":"chimi.metrics.v1","metrics":[],"dropped":{"label_sets":0,"kind_conflicts":-2},"start_unix_ns":5,"time_unix_ns":9}"#,
        r#"{"schema":"chimi.metrics.v1","metrics":[],"dropped":{"label_sets":2.0,"kind_conflicts":1e1},"start_unix_ns":5,"time_unix_ns":9}"#,
    ] {
        scratch.write("document.json", document);
        let export = scratch.chimi(&[
            "metrics",
            "export",
            "--otlp",
            "--service-name",
            "shop",
            "document.json",
        ]);
        assert!(export.status.success(), "{export:?}");
        let export = String::from_utf8(export.stdout).unwrap();
        assert!(!export.contains("dropped"), "{export}");
        let version = format!(",\"version\":\"{}\"", env!("CARGO_PKG_VERSION"));
        scratch.write("export.json", &export.replace(&version, ""));
        assert_eq!(scratch.outcome(CONVERT), "same", "{document}");
    }
}

// A document the encoder cannot read fails with the reason the CLI gives.
#[test]
fn a_document_that_is_not_metrics_fails_with_the_reason() {
    let scratch = Scratch::new("invalid");
    let cases = [
        ("[]", "not a JSON object"),
        ("{", "not a JSON document"),
        (
            r#"{"schema":"chimi.runtime.v1"}"#,
            "not a chimi.metrics.v1 document",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[]}"#,
            "the document has no start_unix_ns; it was written by a runtime without run times",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":-1,"time_unix_ns":0}"#,
            "the document's start_unix_ns is before 1970, which OpenTelemetry cannot carry",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":-0,"time_unix_ns":0}"#,
            "the document's start_unix_ns is before 1970, which OpenTelemetry cannot carry",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":0,"time_unix_ns":1.0}"#,
            "the document has no time_unix_ns; it was written by a runtime without run times",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":1e3,"time_unix_ns":0}"#,
            "the document has no start_unix_ns; it was written by a runtime without run times",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":18446744073709551616,"time_unix_ns":0}"#,
            "the document has no start_unix_ns; it was written by a runtime without run times",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[3],"start_unix_ns":0,"time_unix_ns":0}"#,
            "metric 0 is not an object",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","start_unix_ns":0,"time_unix_ns":0}"#,
            "the document has no metrics list",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[{"name":"a","labels":{},"kind":"count"}],"start_unix_ns":0,"time_unix_ns":0}"#,
            "metric 0 is not a count, gauge or observation",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[{"name":"a","labels":{},"kind":"observe","count":1,"sum":1,"bounds":[1],"bucket_counts":[1]}],"start_unix_ns":0,"time_unix_ns":0}"#,
            "metric 0 is not a count, gauge or observation",
        ),
    ];
    for (document, reason) in cases {
        scratch.write("document.json", document);
        let program = r#"import std/fs as Fs.{Files}
import std/otel as Otel
import std/path as Path
import std/result as Result.{Err, Ok}

pub fn main() -> Text needs { files: Files } {
  let document = Fs.read_text(Path.from("document.json"))
  case Result.capture(fn() { Otel.metrics_request(document, "shop") }) {
    Ok(_) -> "converted"
    Err(Otel.DocumentError.Invalid(message)) -> message
  }
}
"#;
        assert_eq!(scratch.outcome(program), reason, "{document}");
        let cli = scratch.chimi(&["metrics", "export", "--otlp", "document.json"]);
        assert!(
            String::from_utf8_lossy(&cli.stderr).contains(reason),
            "the CLI gives the same reason: {cli:?}"
        );
    }
}

/// What a test collector does with each push.
#[derive(Clone, Copy)]
enum Answer {
    /// Replies with this status.
    Status(u16),
    /// Reads the push and never replies.
    Hang,
}

/// One push a test collector received.
struct Push {
    path: String,
    headers: String,
    body: String,
}

/// A collector on a local port that records each push.
struct Collector {
    port: u16,
    pushes: std::sync::Arc<std::sync::Mutex<Vec<Push>>>,
}

impl Collector {
    fn start(answer: Answer) -> Self {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let pushes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = pushes.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let recorded = recorded.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                    let mut headers = String::new();
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                            break;
                        }
                        let lower = header.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                        headers.push_str(&lower);
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    recorded.lock().unwrap().push(Push {
                        path,
                        headers,
                        body: String::from_utf8(body).unwrap(),
                    });
                    match answer {
                        Answer::Status(status) => {
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {status} X\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                            );
                        }
                        Answer::Hang => std::thread::sleep(std::time::Duration::from_secs(30)),
                    }
                });
            }
        });
        Self { port, pushes }
    }

    fn pushes(&self) -> std::sync::MutexGuard<'_, Vec<Push>> {
        self.pushes.lock().unwrap()
    }
}

/// A program that exports to `port` with `settings` (extra fields of the
/// settings record) while `body` runs.
fn exporting(port: u16, settings: &str, body: &str) -> String {
    format!(
        r#"import std/clock as Clock.{{Clock}}
import std/duration as Duration
import std/metrics as Metrics.{{Metrics}}
import std/net as Net.{{Network}}
import std/otel as Otel

pub fn main() -> Text needs {{ network: Network, clock: Clock, metrics: Metrics }} {{
  let settings = {{ ..Otel.defaults, endpoint: "http://127.0.0.1:{port}", service_name: "shop"{settings} }}
  Otel.export(settings, fn(_) {{
{body}
  }})
}}
"#
    )
}

impl Scratch {
    /// Builds `program` natively and runs it on the real clock, writing
    /// its metrics to `metrics.json`; returns the run and the file.
    fn run_native(&self, program: &str) -> (std::process::Output, String) {
        self.write("main.chimi", program);
        let built = self.chimi(&["build", "main.chimi", "-o", "program"]);
        assert!(built.status.success(), "{built:?}");
        let metrics = self.0.join("metrics.json");
        let output = Command::new(self.0.join("program"))
            .current_dir(&self.0)
            .args(["--chimi-runner", "--clock=real"])
            .arg(format!("--metrics-file={}", metrics.display()))
            .arg("--")
            .output()
            .unwrap();
        (output, std::fs::read_to_string(&metrics).unwrap_or_default())
    }
}

// The aggregate is pushed every interval and once more when the body ends,
// as OTLP/JSON to `/v1/metrics` with the settings' headers, and the last
// push holds what the body recorded last. Rejects no periodic push, no
// final push, a final push before the body's last measurement, the wrong
// path, content type or service, or lost headers.
#[test]
fn metrics_are_pushed_every_interval_and_when_the_body_ends() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("export");
    let program = exporting(
        collector.port,
        r#", interval: Duration.milliseconds(100), headers: [("x-api-key", "secret")]"#,
        r#"    Metrics.count("orders", 3, [])
    Clock.sleep(Duration.milliseconds(350))
    Metrics.count("orders", 1, [])
    "done""#,
    );
    let (output, metrics) = scratch.run_native(&program);
    assert!(output.status.success(), "{output:?}");
    assert!(!metrics.contains("chimi.otel.failed_exports"), "{metrics}");
    let pushes = collector.pushes();
    assert!(pushes.len() >= 3, "{} pushes", pushes.len());
    for push in pushes.iter() {
        assert_eq!(push.path, "/v1/metrics");
        assert!(push.headers.contains("content-type: application/json"), "{}", push.headers);
        assert!(push.headers.contains("x-api-key: secret"), "{}", push.headers);
        assert!(push.body.contains(r#"{"stringValue":"shop"}"#), "{}", push.body);
    }
    assert!(pushes[0].body.contains(r#""asInt":"3""#), "{}", pushes[0].body);
    let last = &pushes.last().unwrap().body;
    assert!(last.contains(r#""asInt":"4""#), "{last}");
}

// A collector that refuses, answers 5xx or never answers does not fail the
// program or hold up its end past `flush_within`; each failed push is
// counted. Rejects a push failure raised into the body, an unbounded final
// push, or a failure left uncounted.
#[test]
fn a_collector_that_is_down_slow_or_failing_is_counted_not_raised() {
    let refused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_port = refused.local_addr().unwrap().port();
    drop(refused);
    let failing = Collector::start(Answer::Status(503));
    let slow = Collector::start(Answer::Hang);
    let scratch = Scratch::new("down");
    let body = r#"    Metrics.count("orders", 1, [])
    Clock.sleep(Duration.milliseconds(250))
    "done""#;
    for (port, name) in [(refused_port, "refused"), (failing.port, "failing"), (slow.port, "slow")] {
        let settings = ", interval: Duration.milliseconds(100), timeout: Duration.milliseconds(200), flush_within: Duration.milliseconds(300)";
        let started = std::time::Instant::now();
        let (output, metrics) = scratch.run_native(&exporting(port, settings, body));
        let took = started.elapsed();
        assert!(output.status.success(), "{name}: {output:?}");
        assert!(
            metrics.contains(r#"{"name":"chimi.otel.failed_exports","labels":{},"kind":"count","value":"#),
            "{name}: {metrics}"
        );
        // The build is not timed; the run is the sleep plus at most one
        // timed-out push and the final one.
        assert!(took < std::time::Duration::from_secs(10), "{name} took {took:?}");
    }
    assert!(!failing.pushes().is_empty());
    assert!(!slow.pushes().is_empty());
}

// Stopping the program pushes once more before it exits. Rejects a final
// push only on return.
#[test]
fn a_stopped_program_pushes_before_it_exits() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("stopped");
    let program = exporting(
        collector.port,
        ", interval: Duration.seconds(60)",
        r#"    Metrics.count("orders", 7, [])
    Clock.sleep(Duration.seconds(60))
    "done""#,
    );
    scratch.write("main.chimi", &program);
    let built = scratch.chimi(&["build", "main.chimi", "-o", "program"]);
    assert!(built.status.success(), "{built:?}");
    let mut runner = Command::new(scratch.0.join("program"))
        .current_dir(&scratch.0)
        .args(["--chimi-runner", "--clock=real", "--"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let sent = Command::new("kill")
        .args(["-TERM", &runner.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let _ = runner.wait().unwrap();
    let pushes = collector.pushes();
    assert_eq!(pushes.len(), 1, "one push, on stop");
    assert!(pushes[0].body.contains(r#""asInt":"7""#), "{}", pushes[0].body);
}
