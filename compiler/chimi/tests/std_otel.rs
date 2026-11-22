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
    r#"{"name":"orders","labels":{"method":"POST","route":"/pay"},"kind":"count","value":3},"#,
    r#"{"name":"orders","labels":{},"kind":"count","value":9223372036854775807},"#,
    r#"{"name":"queue \"depth\"","labels":{"route":"b"},"kind":"gauge","value":-3.5e-7}"#,
    r#"],"dropped":{"label_sets":2,"kind_conflicts":1},"#,
    r#""start_unix_ns":1794303000000000000,"time_unix_ns":1794303002000000000}"#,
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
// or bound rounded, labels or groups lost, a dropped total left out, or an
// escape gone wrong.
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
    assert!(export.contains("\"chimi.metrics.dropped_label_sets\""), "{export}");

    assert_eq!(scratch.outcome(CONVERT), "same");
}

// A document the encoder cannot read fails with the reason the CLI gives.
#[test]
fn a_document_that_is_not_metrics_fails_with_the_reason() {
    let scratch = Scratch::new("invalid");
    let cases = [
        ("[]", "not a JSON object"),
        ("{", "not a JSON document"),
        (r#"{"schema":"chimi.runtime.v1"}"#, "not a chimi.metrics.v1 document"),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[]}"#,
            "the document has no start_unix_ns; it was written by a runtime without run times",
        ),
        (
            r#"{"schema":"chimi.metrics.v1","metrics":[],"start_unix_ns":-1,"time_unix_ns":0}"#,
            "the document's start_unix_ns is before 1970, which OpenTelemetry cannot carry",
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
