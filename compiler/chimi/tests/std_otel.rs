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
        let _ = std::fs::remove_file(self.0.join("interpreted.jsonl"));
        let interpreted = self.chimi(&["run", "main.chimi", "--outcome-file=interpreted.jsonl"]);
        assert!(interpreted.status.success(), "{interpreted:?}");
        let interpreted =
            text_outcome(&std::fs::read_to_string(self.0.join("interpreted.jsonl")).unwrap());
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
    /// Replies 200, after this many milliseconds for the first push only.
    SlowFirst(u64),
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
                        Answer::SlowFirst(milliseconds) => {
                            if recorded.lock().unwrap().len() == 1 {
                                std::thread::sleep(std::time::Duration::from_millis(milliseconds));
                            }
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                            );
                        }
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
        self.run_native_on(program, "real")
    }

    /// As `run_native`, on the runner's `clock` source ("real" or "virtual").
    fn run_native_on(&self, program: &str, clock: &str) -> (std::process::Output, String) {
        let (output, metrics, _) = self.run_native_clocked(program, clock);
        (output, metrics)
    }

    /// As `run_native`, with how long the run took, the build left out.
    fn run_native_timed(
        &self,
        program: &str,
    ) -> (std::process::Output, String, std::time::Duration) {
        self.run_native_clocked(program, "real")
    }

    fn run_native_clocked(
        &self,
        program: &str,
        clock: &str,
    ) -> (std::process::Output, String, std::time::Duration) {
        self.write("main.chimi", program);
        let built = self.chimi(&["build", "main.chimi", "-o", "program"]);
        assert!(built.status.success(), "{built:?}");
        let metrics = self.0.join("metrics.json");
        let _ = std::fs::remove_file(&metrics);
        let started = std::time::Instant::now();
        let output = Command::new(self.0.join("program"))
            .current_dir(&self.0)
            .args(["--chimi-runner", &format!("--clock={clock}")])
            .arg(format!("--metrics-file={}", metrics.display()))
            .arg("--")
            .output()
            .unwrap();
        let took = started.elapsed();
        (
            output,
            std::fs::read_to_string(&metrics).unwrap_or_default(),
            took,
        )
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
        assert!(
            push.headers.contains("content-type: application/json"),
            "{}",
            push.headers
        );
        assert!(
            push.headers.contains("x-api-key: secret"),
            "{}",
            push.headers
        );
        assert!(
            push.body.contains(r#"{"stringValue":"shop"}"#),
            "{}",
            push.body
        );
    }
    assert!(
        pushes[0].body.contains(r#""asInt":"3""#),
        "{}",
        pushes[0].body
    );
    let last = &pushes.last().unwrap().body;
    assert!(last.contains(r#""asInt":"4""#), "{last}");
}

// A collector that refuses, answers 5xx or never answers does not fail the
// program, and each failed push is counted once. Rejects a push failure
// raised into the body, or a failure counted twice or not at all.
//
// The refused and failing collectors run on the virtual clock: time holds
// while a connect or send is in flight, so the 20 ms timeout cannot cut a
// push short on a slow machine, and the 503 collector receives exactly the
// two pushes. The collector that never answers needs the real clock, since
// only real time ends its wait. There a fixed sleep could end the body while
// a stalled periodic push is still waiting, and a cancelled push is not
// counted, so that body waits until the first failure is counted instead.
// Whether a push's bytes reach that collector before the 20 ms timeout also
// depends on the machine, so its pushes are not counted.
#[test]
fn a_collector_that_is_down_slow_or_failing_is_counted_not_raised() {
    let refused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_port = refused.local_addr().unwrap().port();
    drop(refused);
    let failing = Collector::start(Answer::Status(503));
    let slow = Collector::start(Answer::Hang);
    let scratch = Scratch::new("down");
    // One periodic push at 200 ms and the final one: two failures.
    let body = r#"    Metrics.count("orders", 1, [])
    Clock.sleep(Duration.milliseconds(300))
    "done""#;
    let settings = ", interval: Duration.milliseconds(200), timeout: Duration.milliseconds(20), flush_within: Duration.milliseconds(20)";
    let until_one_failure = r#"    Metrics.count("orders", 1, [])
    loop _waiting = Void {
      case Text.contains(Metrics.runner_document(), "\"chimi.otel.failed_exports\"") {
        True -> break "done"
        False -> Clock.sleep(Duration.milliseconds(5))
      }
    }"#;
    for (port, name, clock, body) in [
        (refused_port, "refused", "virtual", body),
        (failing.port, "failing", "virtual", body),
        (slow.port, "slow", "real", until_one_failure),
    ] {
        let program = exporting(port, settings, body).replacen(
            "import std/otel as Otel\n",
            "import std/otel as Otel\nimport std/text as Text\n",
            1,
        );
        let (output, metrics) = scratch.run_native_on(&program, clock);
        assert!(output.status.success(), "{name}: {output:?}");
        assert!(
            metrics.contains(
                r#"{"name":"chimi.otel.failed_exports","labels":{},"kind":"count","value":2}"#
            ),
            "{name}: {metrics}"
        );
    }
    assert_eq!(failing.pushes().len(), 2);
}

// The last push is given `flush_within`, not `timeout`, so a collector that
// never answers delays the end by that much and no more. Rejects a final
// push bounded by `timeout` or not bounded.
#[test]
fn the_last_push_is_bounded_by_flush_within() {
    let slow = Collector::start(Answer::Hang);
    let scratch = Scratch::new("flush");
    let settings = ", interval: Duration.seconds(60), timeout: Duration.seconds(5), flush_within: Duration.milliseconds(400)";
    let (output, _, took) =
        scratch.run_native_timed(&exporting(slow.port, settings, r#"    "done""#));
    assert!(output.status.success(), "{output:?}");
    assert_eq!(slow.pushes().len(), 1);
    assert!(took >= std::time::Duration::from_millis(400), "{took:?}");
    assert!(took < std::time::Duration::from_millis(2500), "{took:?}");
}

// The last push runs when the body defects too. Rejects a final push only
// on return.
#[test]
fn a_body_that_defects_still_pushes() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("defect");
    let body = r#"    Metrics.count("orders", 5, [])
    Debug.crash("broken")"#;
    let program = exporting(collector.port, ", interval: Duration.seconds(60)", body).replace(
        "import std/duration as Duration",
        "import std/debug as Debug\nimport std/duration as Duration",
    );
    let (output, _) = scratch.run_native(&program);
    assert_eq!(output.status.code(), Some(70), "{output:?}");
    let pushes = collector.pushes();
    assert_eq!(pushes.len(), 1);
    assert!(
        pushes[0].body.contains(r#""asInt":"5""#),
        "{}",
        pushes[0].body
    );
}

// An interval, timeout or limit for the last push that is not positive is a
// defect before anything is pushed. Rejects a zero interval that pushes in a
// tight loop.
#[test]
fn settings_that_are_not_positive_are_a_defect() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("settings");
    for field in ["interval", "timeout", "flush_within"] {
        let settings = format!(", {field}: Duration.milliseconds(0)");
        let (output, _) =
            scratch.run_native(&exporting(collector.port, &settings, r#"    "done""#));
        assert_eq!(output.status.code(), Some(70), "{field}: {output:?}");
    }
    assert!(collector.pushes().is_empty());
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
    assert!(
        pushes[0].body.contains(r#""asInt":"7""#),
        "{}",
        pushes[0].body
    );
}

/// A program that exports to `port` with `settings` while `body` runs with
/// `Otel.log` as its `log` provider.
fn logging(port: u16, settings: &str, body: &str) -> String {
    format!(
        r#"import std/clock as Clock.{{Clock}}
import std/duration as Duration
import std/json as Json
import std/log as Log.{{Log}}
import std/map as Map
import std/metrics as Metrics.{{Metrics}}
import std/net.{{Network}}
import std/otel as Otel

fn work() -> Text needs {{ log: Log, clock: Clock }} {{
{body}
}}

pub fn main() -> Text needs {{ network: Network, clock: Clock, metrics: Metrics, log: Log }} {{
  let settings = {{ ..Otel.defaults, endpoint: "http://127.0.0.1:{port}", service_name: "shop"{settings} }}
  Otel.export(settings, fn(exporter) {{
    work() using {{ log: Otel.log(exporter) }}
  }})
}}
"#
    )
}

// `Otel.log` writes each entry to the outer provider as before and pushes
// it to `/v1/logs` within an interval, or with the last push, when its level
// is at least `log_level`; fields become typed attributes. Rejects an entry
// lost from the outer log, a filtered entry sent, an entry written last
// that never leaves, the wrong severity, or an attribute of the wrong type.
#[test]
fn logs_are_pushed_and_still_written() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("logs");
    let body = r#"  Log.info("charged", [Log.int("order", 42), Log.float("seconds", 1.5), Log.text("who", "ann"), Log.bool("ok", True), Log.json("tags", Json.object(Map.from([("a", Json.text("b"))]))), Log.json("thousand", Json.decode("1e3"))])
  Log.debug("hidden", [])
  Clock.sleep(Duration.milliseconds(300))
  Log.warn("slow", [])
  "done""#;
    let (output, metrics) = scratch.run_native(&logging(
        collector.port,
        ", interval: Duration.milliseconds(100)",
        body,
    ));
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""message":"charged""#), "{stderr}");
    assert!(stderr.contains(r#""message":"slow""#), "{stderr}");
    assert!(!metrics.contains("chimi.otel"), "{metrics}");
    let pushes = collector.pushes();
    let logs: Vec<&Push> = pushes
        .iter()
        .filter(|push| push.path == "/v1/logs")
        .collect();
    assert_eq!(logs.len(), 2, "one within the interval, one at the end");
    let first = &logs[0].body;
    assert!(
        first.contains(r#""body":{"stringValue":"charged"}"#),
        "{first}"
    );
    assert!(
        first.contains(r#""severityNumber":9,"severityText":"INFO""#),
        "{first}"
    );
    for attribute in [
        r#"{"key":"order","value":{"intValue":"42"}}"#,
        r#"{"key":"seconds","value":{"doubleValue":1.5}}"#,
        r#"{"key":"who","value":{"stringValue":"ann"}}"#,
        r#"{"key":"ok","value":{"boolValue":true}}"#,
        r#"{"key":"thousand","value":{"intValue":"1000"}}"#,
        r#"{"key":"tags","value":{"kvlistValue":{"values":[{"key":"a","value":{"stringValue":"b"}}]}}}"#,
    ] {
        assert!(first.contains(attribute), "{attribute} in {first}");
    }
    assert!(first.contains(r#"{"stringValue":"shop"}"#), "{first}");
    assert!(!logs.iter().any(|push| push.body.contains("hidden")));
    let last = &logs[1].body;
    assert!(last.contains(r#""body":{"stringValue":"slow"}"#), "{last}");
    assert!(
        last.contains(r#""severityNumber":13,"severityText":"WARN""#),
        "{last}"
    );
    assert!(!last.contains("charged"), "an entry is pushed once: {last}");
}

// Writing a log never waits for the collector: with the logs process busy
// on a push that hangs and its buffer full, more entries are dropped and
// counted. Rejects a write that blocks, or drops left uncounted.
#[test]
fn a_full_log_buffer_drops_and_counts() {
    let slow = Collector::start(Answer::Hang);
    let scratch = Scratch::new("log-buffer");
    let body = r#"  Log.info("first", [])
  Clock.sleep(Duration.milliseconds(300))
  Log.info("a", [])
  Log.info("b", [])
  Log.info("c", [])
  Log.info("d", [])
  Log.info("e", [])
  "done""#;
    let settings = ", interval: Duration.milliseconds(100), timeout: Duration.seconds(5), flush_within: Duration.milliseconds(200), log_buffer: 2";
    let (output, metrics) = scratch.run_native(&logging(slow.port, settings, body));
    assert!(output.status.success(), "{output:?}");
    assert!(
        metrics
            .contains(r#"{"name":"chimi.otel.dropped_logs","labels":{},"kind":"count","value":3}"#),
        "{metrics}"
    );
}

// Stopping the program pushes the waiting logs too. Rejects logs left in
// the logs process when it is stopped.
#[test]
fn a_stopped_program_pushes_its_waiting_logs() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("stopped-logs");
    let body = r#"  Log.info("bye", [])
  Clock.sleep(Duration.seconds(60))
  "done""#;
    scratch.write(
        "main.chimi",
        &logging(collector.port, ", interval: Duration.seconds(60)", body),
    );
    let built = scratch.chimi(&["build", "main.chimi", "-o", "program"]);
    assert!(built.status.success(), "{built:?}");
    let mut runner = Command::new(scratch.0.join("program"))
        .current_dir(&scratch.0)
        .args(["--chimi-runner", "--clock=real", "--"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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
    let logs: Vec<&Push> = pushes
        .iter()
        .filter(|push| push.path == "/v1/logs")
        .collect();
    assert_eq!(logs.len(), 1, "one logs push, on stop");
    assert!(
        logs[0].body.contains(r#""body":{"stringValue":"bye"}"#),
        "{}",
        logs[0].body
    );
}

// Entries written while the logs process is busy on a slow push wait in
// its mailbox and still go in the last push; the flush request does not
// queue behind them. Rejects a flush lost to a full mailbox.
#[test]
fn entries_waiting_behind_a_slow_push_are_flushed() {
    let collector = Collector::start(Answer::SlowFirst(600));
    let scratch = Scratch::new("slow-first");
    let body = r#"  Log.info("first", [])
  Clock.sleep(Duration.milliseconds(300))
  Log.info("a", [])
  Log.info("b", [])
  Log.info("c", [])
  "done""#;
    let settings =
        ", interval: Duration.milliseconds(100), flush_within: Duration.seconds(3), log_buffer: 2";
    let (output, metrics) = scratch.run_native(&logging(collector.port, settings, body));
    assert!(output.status.success(), "{output:?}");
    assert!(
        metrics
            .contains(r#"{"name":"chimi.otel.dropped_logs","labels":{},"kind":"count","value":1}"#),
        "{metrics}"
    );
    let pushes = collector.pushes();
    let logs: Vec<&Push> = pushes
        .iter()
        .filter(|push| push.path == "/v1/logs")
        .collect();
    let last = &logs.last().unwrap().body;
    assert!(
        last.contains(r#""stringValue":"a""#) && last.contains(r#""stringValue":"b""#),
        "{last}"
    );
}

// At most `log_buffer` entries wait between pushes, even when the logs
// process is idle. Rejects a buffer that only bounds the mailbox.
#[test]
fn at_most_log_buffer_entries_wait() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("log-cap");
    let body = r#"  Log.info("a", [])
  Clock.sleep(Duration.milliseconds(20))
  Log.info("b", [])
  Clock.sleep(Duration.milliseconds(20))
  Log.info("c", [])
  Clock.sleep(Duration.milliseconds(20))
  Log.info("d", [])
  "done""#;
    let settings = ", interval: Duration.seconds(60), log_buffer: 2";
    let (output, metrics) = scratch.run_native(&logging(collector.port, settings, body));
    assert!(output.status.success(), "{output:?}");
    assert!(
        metrics
            .contains(r#"{"name":"chimi.otel.dropped_logs","labels":{},"kind":"count","value":2}"#),
        "{metrics}"
    );
    let pushes = collector.pushes();
    let logs: Vec<&Push> = pushes
        .iter()
        .filter(|push| push.path == "/v1/logs")
        .collect();
    assert_eq!(logs.len(), 1);
    assert!(
        logs[0].body.contains(r#""stringValue":"b""#)
            && !logs[0].body.contains(r#""stringValue":"c""#),
        "{}",
        logs[0].body
    );
}

// The last logs push and the last metrics push share one `flush_within`.
// Rejects a release that spends it twice.
#[test]
fn the_last_pushes_share_flush_within() {
    let slow = Collector::start(Answer::Hang);
    let scratch = Scratch::new("shared-flush");
    let body = r#"  Log.info("bye", [])
  "done""#;
    let settings = ", interval: Duration.seconds(60), flush_within: Duration.seconds(1)";
    let (output, _, took) = scratch.run_native_timed(&logging(slow.port, settings, body));
    assert!(output.status.success(), "{output:?}");
    assert_eq!(slow.pushes().len(), 2, "a logs push and a metrics push");
    assert!(took >= std::time::Duration::from_secs(1), "{took:?}");
    assert!(took < std::time::Duration::from_millis(1800), "{took:?}");
}

// The example program `programs/otel-worker` runs its jobs, writes each log
// line to standard error, and pushes its metrics and its `Info` and `Warn`
// logs to the collector `--collector` names. Rejects an example that no
// longer builds, loses a job's count or log, or pushes a `Debug` entry.
#[test]
fn the_otel_worker_example_pushes_its_jobs() {
    let collector = Collector::start(Answer::Status(200));
    let scratch = Scratch::new("example");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/otel-worker");
    let built = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch.0)
        .arg("build")
        .arg("--root")
        .arg(&root)
        .args(["app/main", "-o", "program"])
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let output = Command::new(scratch.0.join("program"))
        .current_dir(&scratch.0)
        .args(["--chimi-runner", "--clock=real", "--", "--collector"])
        .arg(format!("http://127.0.0.1:{}", collector.port))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr.matches(r#""message":"job done""#).count(),
        4,
        "{stderr}"
    );
    assert_eq!(
        stderr.matches(r#""message":"job failed""#).count(),
        2,
        "{stderr}"
    );
    let pushes = collector.pushes();
    let logs: String = pushes
        .iter()
        .filter(|push| push.path == "/v1/logs")
        .map(|push| push.body.as_str())
        .collect();
    assert_eq!(
        logs.matches(r#"{"stringValue":"job done"}"#).count(),
        4,
        "{logs}"
    );
    assert_eq!(
        logs.matches(r#"{"stringValue":"job failed"}"#).count(),
        2,
        "{logs}"
    );
    assert!(!logs.contains("starting"), "{logs}");
    // Each push is answered on its own thread, so the last to arrive may
    // be a periodic one; the final push is the one that holds every job.
    let series = [
        r#"{"asInt":"4","attributes":[{"key":"outcome","value":{"stringValue":"done"}}]"#,
        r#"{"asInt":"2","attributes":[{"key":"outcome","value":{"stringValue":"failed"}}]"#,
        r#"{"stringValue":"otel-worker"}"#,
    ];
    let metrics: Vec<&str> = pushes
        .iter()
        .filter(|push| push.path == "/v1/metrics")
        .map(|push| push.body.as_str())
        .collect();
    assert!(
        metrics
            .iter()
            .any(|body| series.iter().all(|one| body.contains(one))),
        "no metrics push holds every job: {metrics:?}"
    );
}
