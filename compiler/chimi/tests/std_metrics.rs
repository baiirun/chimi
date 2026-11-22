//! Conformance for `std/metrics` (Phase 7c, CHI-289): the measurement each
//! recording function builds, fakes through `using`, and the runner's
//! provider, which adds measurements into one aggregate per name and label
//! set and writes them to `--metrics-file` when the program ends.
//!
//! Each test writes `cases.chimi` to a scratch directory and runs the CLI there.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chimi-metrics-{name}-{}-{:?}",
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
        run(Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&self.0)
            .args(arguments))
    }

    /// Builds `cases.chimi` with the native backend and returns the binary.
    fn build(&self) -> PathBuf {
        let built = self.chimi(&["build", "cases.chimi", "-o", "program"]);
        assert_eq!(built.status, Some(0), "{}{}", built.stdout, built.stderr);
        self.0.join("program")
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

fn run(command: &mut Command) -> Run {
    let output = command.output().expect("run the command");
    Run {
        status: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn runner(program: &Path, options: &[&str]) -> Run {
    run(Command::new(program)
        .arg("--chimi-runner")
        .args(options)
        .arg("--"))
}

const PROGRAM: &str = r#"import std/metrics as Metrics.{Metrics}

pub fn main() -> Void needs { metrics: Metrics } {
  Metrics.count("orders", 1, [("route", "/pay"), ("method", "POST")])
  Metrics.count("orders", 2, [("method", "POST"), ("route", "/pay")])
  Metrics.count("orders", 4, [])
  Metrics.gauge("queue \"depth\"", 12.0, [("route", "a"), ("route", "b")])
  Metrics.gauge("queue \"depth\"", -3.5, [("route", "b")])
  Metrics.observe("charge_seconds", 0.0, [])
  Metrics.observe("charge_seconds", 0.25, [])
  Metrics.observe("charge_seconds", 0.5, [])
  Metrics.observe("charge_seconds", 11.0, [])
  Metrics.gauge("orders", 1.0, [])
}
"#;

const DOCUMENT: &str = concat!(
    r#"{"schema":"chimi.metrics.v1","metrics":["#,
    r#"{"name":"charge_seconds","labels":{},"kind":"observe","count":4,"sum":11.75,"#,
    r#""bounds":[0.0,0.005,0.01,0.025,0.05,0.075,0.1,0.25,0.5,0.75,1.0,2.5,5.0,7.5,10.0],"#,
    r#""bucket_counts":[1,0,0,0,0,0,0,1,1,0,0,0,0,0,0,1]},"#,
    r#"{"name":"orders","labels":{"method":"POST","route":"/pay"},"kind":"count","value":3},"#,
    r#"{"name":"orders","labels":{},"kind":"count","value":4},"#,
    r#"{"name":"queue \"depth\"","labels":{"route":"b"},"kind":"gauge","value":-3.5}"#,
    r#"],"dropped":{"label_sets":0,"kind_conflicts":1},"start_unix_ns":0,"time_unix_ns":0}"#,
    "\n"
);

// Rejects a provider that keys a label set by the order it was written in,
// keeps a repeated label's first value, replaces a count instead of adding
// to it, adds gauges, puts a value on a bound into the next bucket, sorts the
// series some other way, escapes a name badly, or lets a name change kind.
#[test]
fn the_runner_writes_one_aggregate_per_name_and_label_set() {
    let scratch = Scratch::new("document", PROGRAM);
    let program = scratch.build();
    let file = scratch.0.join("metrics.json");
    let option = format!("--metrics-file={}", file.display());
    let output = runner(&program, &["--clock=virtual", &option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    assert_eq!((output.stdout.as_str(), output.stderr.as_str()), ("", ""));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), DOCUMENT);
    // A second run replaces the file: it holds one run's aggregates.
    let output = runner(&program, &["--clock=virtual", &option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), DOCUMENT);
}

// Rejects a provider that writes somewhere without the option, or a runner
// that lets the metrics file share a report or log file, even under another
// spelling, or that runs `main` when the file cannot be opened.
#[test]
fn the_metrics_file_option_is_checked_before_main_runs() {
    let scratch = Scratch::new("options", PROGRAM);
    let program = scratch.build();
    let output = runner(&program, &[]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let mut written: Vec<String> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(written, ["cases.chimi", "program", "target"], "nothing beside the build");
    let file = scratch.0.join("same.json");
    let metrics = format!("--metrics-file={}", file.display());
    let expected = "the metrics file must be distinct from the runner's report files and the log file\n";
    for other in ["--outcome-file", "--telemetry-file", "--log-file", "--memory-report-file", "--profile-file"] {
        let clash = runner(&program, &[&metrics, &format!("{other}={}", file.display())]);
        assert_eq!((clash.status, clash.stderr.as_str()), (Some(64), expected), "{other}");
    }
    let log = format!("--log-file={}", file.display());
    let respelled = format!("--metrics-file={}/./same.json", scratch.0.display());
    let clash = runner(&program, &[&log, &respelled]);
    assert_eq!((clash.status, clash.stderr.as_str()), (Some(64), expected), "another spelling");
    // The memory report and profile are opened only at exit.
    for later in ["--memory-report-file", "--profile-file"] {
        let file = scratch.0.join(format!("{}.json", &later[2..]));
        let metrics = format!("--metrics-file={}", file.display());
        let respelled = format!("{later}={}/./{}.json", scratch.0.display(), &later[2..]);
        let clash = runner(&program, &[&metrics, &respelled]);
        assert_eq!((clash.status, clash.stderr.as_str()), (Some(64), expected), "{later} under another spelling");
    }
    let missing = runner(&program, &["--metrics-file=/nonexistent/dir/metrics.json"]);
    assert_eq!(missing.status, Some(74));
    assert!(missing.stderr.starts_with("cannot open metrics file /nonexistent/dir/metrics.json"), "{}", missing.stderr);
}

const LIMITS: &str = r#"import std/int as Int
import std/list as List
import std/metrics as Metrics.{Metrics}

pub fn main() -> Void needs { metrics: Metrics } {
  List.each(List.enumerate(List.repeat(0, 1001)), fn(pair) {
    let (index, _) = pair
    Metrics.count("ids", 1, [("id", Int.to_text(index))])
  })
  Metrics.count("ids", 5, [("id", "7")])
  Metrics.count("ids", 1, [("id", "1000")])
}
"#;

// Rejects a provider with no bound on the label sets one name can make, or
// one that drops a measurement into a label set it already has.
#[test]
fn one_name_keeps_at_most_a_thousand_label_sets() {
    let scratch = Scratch::new("limits", LIMITS);
    let program = scratch.build();
    let file = scratch.0.join("metrics.json");
    let output = runner(&program, &["--clock=virtual", &format!("--metrics-file={}", file.display())]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written.matches(r#""name":"ids""#).count(), 1000);
    assert!(written.contains(r#"{"name":"ids","labels":{"id":"7"},"kind":"count","value":6}"#), "{written}");
    assert!(!written.contains(r#""id":"1000""#), "{written}");
    assert!(written.ends_with("\"dropped\":{\"label_sets\":2,\"kind_conflicts\":0},\"start_unix_ns\":0,\"time_unix_ns\":0}\n"));
}

const CARRIERS: &str = r#"import std/int as Int
import std/list as List
import std/metrics as Metrics.{Metrics}
import std/task as Task

fn work(task: Int) -> Int needs { metrics: Metrics } {
  List.each(List.enumerate(List.repeat(0, 500)), fn(pair) {
    let (index, _) = pair
    Metrics.count("done", 1, [])
    Metrics.observe("size", 1.0, [])
    // New label sets while every task records, so the tables grow under
    // contention: 8 tasks times 50 label sets.
    Metrics.count("by_task", 1, [("task", Int.to_text(task)), ("slot", Int.to_text(index % 50))])
  })
  0
}

pub fn main() -> Void needs { metrics: Metrics } {
  let tasks = List.map([1, 2, 3, 4, 5, 6, 7, 8], fn(task) { Task.start(fn() { work(task) }) })
  List.each(tasks, fn(task) {
    let _ = Task.result(task)
    Void
  })
}
"#;

// Rejects an aggregate that loses a measurement when tasks on several
// carriers record at once.
#[test]
fn tasks_on_several_carriers_record_into_one_aggregate() {
    let scratch = Scratch::new("carriers", CARRIERS);
    let program = scratch.build();
    let file = scratch.0.join("metrics.json");
    let option = format!("--metrics-file={}", file.display());
    let output = runner(&program, &["--clock=real", "--carriers=4", &option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains(r#"{"name":"done","labels":{},"kind":"count","value":4000}"#), "{written}");
    assert!(written.contains(r#""kind":"observe","count":4000,"sum":4000.0,"#), "{written}");
    assert_eq!(written.matches(r#""name":"by_task""#).count(), 400, "{written}");
    assert_eq!(written.matches(r#""kind":"count","value":10}"#).count(), 400, "{written}");
}

const NEGATIVE: &str = r#"import std/metrics as Metrics.{Metrics}

pub fn main() -> Void needs { metrics: Metrics } {
  Metrics.count("refunds", 1, [])
  Metrics.count("refunds", -1, [])
}
"#;

const AFTER_ONE: &str = r#"{"schema":"chimi.metrics.v1","metrics":[{"name":"refunds","labels":{},"kind":"count","value":1}],"dropped":{"label_sets":0,"kind_conflicts":0},"start_unix_ns":0,"time_unix_ns":0}"#;

// Rejects a counter that can go down, and a runner that writes the metrics
// only when `main` succeeds.
#[test]
fn a_negative_count_is_a_defect() {
    let scratch = Scratch::new("negative", NEGATIVE);
    let program = scratch.build();
    let outcome = scratch.0.join("outcome.json");
    let metrics = scratch.0.join("metrics.json");
    let output = runner(&program, &[
        "--clock=virtual",
        &format!("--outcome-file={}", outcome.display()),
        &format!("--metrics-file={}", metrics.display()),
    ]);
    assert_eq!(output.status, Some(70), "{}", output.stderr);
    let outcome = std::fs::read_to_string(&outcome).unwrap();
    assert!(outcome.contains("Metrics: count must not be negative, got -1"), "{outcome}");
    assert_eq!(std::fs::read_to_string(&metrics).unwrap(), format!("{AFTER_ONE}\n"));
}

// Rejects a runner that reports success when the document cannot be written.
#[cfg(target_os = "linux")]
#[test]
fn a_failed_metrics_write_is_a_sink_failure() {
    let scratch = Scratch::new("full", PROGRAM);
    let program = scratch.build();
    let output = runner(&program, &["--metrics-file=/dev/full"]);
    assert_eq!(output.status, Some(74), "{}", output.stderr);
}

const EDGES: &str = r#"import std/metrics as Metrics.{Metrics}

pub fn main() -> Void needs { metrics: Metrics } {
  Metrics.count("big", 9223372036854775807, [])
  Metrics.count("big", 9223372036854775807, [])
  Metrics.observe("huge", 1.0e308, [])
  Metrics.observe("huge", 1.0e308, [])
  Metrics.observe("huge", -1.0e308, [])
  Metrics.gauge("text", 1.0, [("ключ", "é\u{1}\"\\"), ("ключ", "last"), ("a", "é\u{1}\"\\")])
}
"#;

// Rejects a count that wraps past the largest Int, a sum that becomes
// infinite, and labels escaped differently by the two runtimes.
#[test]
fn counts_and_sums_saturate_and_both_runtimes_agree() {
    use chimi::execution::{ExecutionConfig, TraceMode};
    use chimi::source::SourceSnapshot;
    use chimi::{Compiler, ProjectSource};
    let scratch = Scratch::new("edges", EDGES);
    let program = scratch.build();
    let native = scratch.0.join("native.json");
    let output = runner(&program, &["--clock=virtual", &format!("--metrics-file={}", native.display())]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let native = std::fs::read_to_string(&native).unwrap();
    assert!(native.contains(r#"{"name":"big","labels":{},"kind":"count","value":9223372036854775807}"#), "{native}");
    // The largest Float, then less 1.0e308.
    assert!(native.contains(r#""count":3,"sum":7.976931348623157e307,"#), "{native}");
    assert!(native.contains(r#""labels":{"a":"é\u0001\"\\","ключ":"last"}"#), "{native}");
    let interpreted = scratch.0.join("interpreted.json");
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", EDGES)).expect("valid module path");
    Compiler::run_project(
        vec![module],
        ExecutionConfig::new("metrics")
            .unwrap()
            .with_metrics_file(interpreted.to_string_lossy())
            .with_trace_mode(TraceMode::Off),
    )
    .expect("the program compiles");
    assert_eq!(std::fs::read_to_string(&interpreted).unwrap(), native);
}

const TESTS: &str = r#"import std/duration as Duration
import std/float as Float
import std/list as List
import std/metrics as Metrics.{Kind, Measurement, Metrics}
import std/test as Test

fn kept(recorder: Test.Recorder<Measurement>) {
  { record: fn(measurement) { Test.record(recorder, measurement) } }
}

fn charge(cents: Int) -> Int needs { metrics: Metrics } {
  Metrics.count("charges", 1, [("currency", "usd")])
  Metrics.observe("cents", Float.from_int(cents), [])
  cents
}

test "a fake keeps each measurement with its kind, name, value and labels" {
  let recorder = Test.recorder()
  let charged = charge(250) using { metrics: kept(recorder) }
  charged |> Test.assert_equal(250)
  Metrics.gauge("depth", 2.5, [("b", "1"), ("a", "2")]) using { metrics: kept(recorder) }
  let measurements = Test.recorded(recorder)
  List.map(measurements, Metrics.kind) |> Test.assert_equal([Kind.Count, Kind.Observe, Kind.Gauge])
  List.map(measurements, Metrics.name) |> Test.assert_equal(["charges", "cents", "depth"])
  List.map(measurements, Metrics.value) |> Test.assert_equal([1.0, 250.0, 2.5])
  List.map(measurements, Metrics.labels)
    |> Test.assert_equal([[("currency", "usd")], [], [("b", "1"), ("a", "2")]])
}

test "a duration is observed in seconds" {
  Metrics.seconds(Duration.milliseconds(1500)) |> Test.assert_equal(1.5)
  Metrics.seconds(Duration.nanoseconds(1)) |> Test.assert_equal(0.000000001)
}
"#;

// Rejects recording functions that build the wrong kind, change the value or
// the labels, or bypass the provider `using` supplies; on both backends.
#[test]
fn a_test_records_measurements_through_a_fake() {
    let scratch = Scratch::new("fake", TESTS);
    for backend in [&[][..], &["--backend=native"][..]] {
        let mut arguments = vec!["test", "cases.chimi"];
        arguments.extend_from_slice(backend);
        let output = scratch.chimi(&arguments);
        assert_eq!(output.status, Some(0), "{}{}", output.stdout, output.stderr);
        assert!(output.stdout.contains("2 passed, 0 failed"), "{}", output.stdout);
    }
}

// Rejects a test that reaches the runner's provider: a test never records to
// the machine by accident.
#[test]
fn a_test_without_a_metrics_provider_is_rejected() {
    let scratch = Scratch::new(
        "no-provider",
        "import std/metrics as Metrics\n\ntest \"records\" {\n  Metrics.gauge(\"x\", 1.0, [])\n}\n",
    );
    let output = scratch.chimi(&["test", "cases.chimi"]);
    assert_ne!(output.status, Some(0));
    assert!(output.stderr.contains("T4400") || output.stdout.contains("T4400"), "{}{}", output.stdout, output.stderr);
}

// The interpreter's provider aggregates the same way and writes the same
// document, and records nothing without a file.
#[test]
fn the_interpreter_writes_the_same_document() {
    use chimi::execution::{ExecutionConfig, TraceMode};
    use chimi::source::SourceSnapshot;
    use chimi::{Compiler, ProjectSource};
    let run = |source: &str, config: ExecutionConfig| {
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        Compiler::run_project(vec![module], config.with_trace_mode(TraceMode::Off))
            .expect("the program compiles")
    };
    let scratch = Scratch::new("interpreter", "");
    let file = scratch.0.join("metrics.json");
    let config = || ExecutionConfig::new("metrics").unwrap().with_metrics_file(file.to_string_lossy());
    run(PROGRAM, config());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), DOCUMENT);
    run(LIMITS, config());
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written.matches(r#""name":"ids""#).count(), 1000);
    assert!(written.ends_with("\"dropped\":{\"label_sets\":2,\"kind_conflicts\":0},\"start_unix_ns\":0,\"time_unix_ns\":0}\n"));
    run(NEGATIVE, config());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{AFTER_ONE}\n"));
    std::fs::remove_file(&file).unwrap();
    run(PROGRAM, ExecutionConfig::new("metrics").unwrap());
    assert!(!file.exists());
}

const TIMED: &str = r#"import std/clock as Clock
import std/duration as Duration
import std/metrics as Metrics.{Metrics}

pub fn main() -> Void needs { clock: Clock.Clock, metrics: Metrics } {
  Clock.sleep(Duration.seconds(2))
  Metrics.count("naps", 1, [])
}
"#;

// The document says when the run started and when `main` ended, as Unix
// nanoseconds: under the virtual clock from `--clock-start` and the time
// slept, under the real clock from the host. Rejects a document without
// times, or one whose end ignores the run's time.
#[test]
fn the_document_carries_the_runs_start_and_end() {
    let scratch = Scratch::new("timed", TIMED);
    let program = scratch.build();
    let file = scratch.0.join("metrics.json");
    let option = format!("--metrics-file={}", file.display());
    let output = runner(
        &program,
        &[
            "--clock=virtual",
            "--clock-start=2026-11-10T09:30:00Z",
            &option,
        ],
    );
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(
        written.ends_with(
            r#","start_unix_ns":1794303000000000000,"time_unix_ns":1794303002000000000}
"#
        ),
        "{written}"
    );
    let export = scratch.chimi(&["metrics", "export", "--otlp", "metrics.json"]);
    assert_eq!(export.status, Some(0), "{}", export.stderr);
    assert!(
        export
            .stdout
            .contains(r#""startTimeUnixNano":"1794303000000000000","timeUnixNano":"1794303002000000000""#),
        "{}",
        export.stdout
    );
    let output = runner(&program, &["--clock=real", &option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let written = std::fs::read_to_string(&file).unwrap();
    let number = |key: &str| -> i64 {
        let start = written.find(key).unwrap() + key.len();
        written[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap()
    };
    let (start, end) = (number("\"start_unix_ns\":"), number("\"time_unix_ns\":"));
    assert!(end - start >= 2_000_000_000, "{written}");
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    assert!(end <= wall && wall - end < 120_000_000_000, "{written}");
    assert!(wall - start < 120_000_000_000, "{written}");
}

// `chimi metrics export --otlp` turns the document into OTLP/JSON metrics:
// one metric per name and one cumulative data point per label set, from the
// run's start to the end of `main`. Rejects a count that is not a monotonic
// sum, a gauge or histogram spelled some other way, label sets split into
// separate metrics, times that are not the run's, and dropped measurements
// that go unreported.
#[test]
fn the_document_exports_as_opentelemetry_metrics() {
    let scratch = Scratch::new("export", PROGRAM);
    let program = scratch.build();
    let option = format!("--metrics-file={}", scratch.0.join("metrics.json").display());
    let output = runner(&program, &["--clock=virtual", "--clock-start=2026-11-10T09:30:00Z", &option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let export = scratch.chimi(&["metrics", "export", "--otlp", "--service-name", "shop", "metrics.json"]);
    assert_eq!(export.status, Some(0), "{}", export.stderr);
    let times = r#""startTimeUnixNano":"1794303000000000000","timeUnixNano":"1794303000000000000""#;
    let expected = [
        r#"{"resourceMetrics":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"shop"}}]},"#,
        r#""scopeMetrics":[{"scope":{"name":"chimi","version":""#,
    ];
    assert!(export.stdout.starts_with(expected[0]), "{}", export.stdout);
    assert!(export.stdout.contains(expected[1]), "{}", export.stdout);
    for metric in [
        format!(
            concat!(
                r#"{{"name":"charge_seconds","histogram":{{"dataPoints":[{{"attributes":[],{times},"#,
                r#""count":"4","sum":11.75,"bucketCounts":["1","0","0","0","0","0","0","1","1","0","0","0","0","0","0","1"],"#,
                r#""explicitBounds":[0.0,0.005,0.01,0.025,0.05,0.075,0.1,0.25,0.5,0.75,1.0,2.5,5.0,7.5,10.0]}}],"#,
                r#""aggregationTemporality":2}}}}"#
            ),
            times = times
        ),
        format!(
            concat!(
                r#"{{"name":"orders","sum":{{"dataPoints":["#,
                r#"{{"attributes":[{{"key":"method","value":{{"stringValue":"POST"}}}},{{"key":"route","value":{{"stringValue":"/pay"}}}}],{times},"asInt":"3"}},"#,
                r#"{{"attributes":[],{times},"asInt":"4"}}],"#,
                r#""aggregationTemporality":2,"isMonotonic":true}}}}"#
            ),
            times = times
        ),
        format!(
            r#"{{"name":"queue \"depth\"","gauge":{{"dataPoints":[{{"attributes":[{{"key":"route","value":{{"stringValue":"b"}}}}],{times},"asDouble":-3.5}}]}}}}"#,
            times = times
        ),
        format!(
            r#"{{"name":"chimi.metrics.dropped_kind_conflicts","sum":{{"dataPoints":[{{"attributes":[],{times},"asInt":"1"}}],"aggregationTemporality":2,"isMonotonic":true}}}}"#,
            times = times
        ),
    ] {
        assert!(export.stdout.contains(&metric), "{metric}\nin {}", export.stdout);
    }
    assert!(!export.stdout.contains("dropped_label_sets"), "none were dropped");
    assert!(export.stdout.ends_with("]}]}]}\n"));
}

// A document from before run times, or anything else, cannot be exported and
// prints nothing. Rejects metrics silently placed at 1970.
#[test]
fn a_metrics_document_without_times_is_refused() {
    let scratch = Scratch::new("export-old", "");
    std::fs::write(scratch.0.join("old.json"), format!("{AFTER_ONE}\n").replace(r#","start_unix_ns":0,"time_unix_ns":0"#, "")).unwrap();
    let old = scratch.chimi(&["metrics", "export", "--otlp", "old.json"]);
    assert_eq!((old.status, old.stdout.as_str()), (Some(1), ""));
    assert!(old.stderr.contains("the document has no start_unix_ns"), "{}", old.stderr);
    let before_1970 = format!("{AFTER_ONE}\n").replace(r#""start_unix_ns":0"#, r#""start_unix_ns":-5"#);
    std::fs::write(scratch.0.join("early.json"), before_1970).unwrap();
    let early = scratch.chimi(&["metrics", "export", "--otlp", "early.json"]);
    assert_eq!((early.status, early.stdout.as_str()), (Some(1), ""));
    assert!(early.stderr.contains("start_unix_ns is before 1970"), "{}", early.stderr);
    std::fs::write(scratch.0.join("trace.json"), "{\"schema\":\"chimi.runtime.v1\"}\n").unwrap();
    let other = scratch.chimi(&["metrics", "export", "--otlp", "trace.json"]);
    assert_eq!((other.status, other.stdout.as_str()), (Some(1), ""));
    assert!(other.stderr.contains("not a chimi.metrics.v1 document"), "{}", other.stderr);
}

const READER: &str = r#"import std/clock as Clock
import std/duration as Duration
import std/metrics as Metrics.{Metrics}

pub fn main() -> Text needs { clock: Clock.Clock, metrics: Metrics } {
  Metrics.count("before", 1, [])
  let first = Metrics.runner_document()
  Metrics.count("orders", 2, [("route", "/pay")])
  Clock.sleep(Duration.seconds(1))
  Metrics.gauge("depth", 1.5, [])
  "${first}|${Metrics.runner_document()}"
}
"#;

const READ_FIRST: &str = concat!(
    r#"{"schema":"chimi.metrics.v1","metrics":[],"dropped":{"label_sets":0,"kind_conflicts":0},"#,
    r#""start_unix_ns":1794303000000000000,"time_unix_ns":1794303000000000000}"#,
);

const READ_SECOND: &str = concat!(
    r#"{"schema":"chimi.metrics.v1","metrics":["#,
    r#"{"name":"depth","labels":{},"kind":"gauge","value":1.5},"#,
    r#"{"name":"orders","labels":{"route":"/pay"},"kind":"count","value":2}],"#,
    r#""dropped":{"label_sets":0,"kind_conflicts":0},"#,
    r#""start_unix_ns":1794303000000000000,"time_unix_ns":1794303001000000000}"#,
);

/// The text value of the outcome record among `lines`.
fn text_outcome(lines: &str) -> String {
    let line = lines
        .lines()
        .find(|line| line.starts_with(r#"{"schema":"chimi.runtime.v1","outcome":"success""#))
        .unwrap_or_else(|| panic!("no success outcome in {lines}"));
    let value = line.split_once("\"value\":").expect("a value").1;
    let value = value.strip_suffix('}').unwrap_or(value);
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

// Reading the aggregate mid-run gives the document `--metrics-file` holds,
// with the time of the read, and turns recording on without the file:
// what was recorded before the first read is not in it. Rejects a read that
// is empty, that records nothing afterwards, that stamps the run's end or
// start as the read's time, or that differs between the backends.
#[test]
fn the_aggregate_can_be_read_while_main_runs() {
    let scratch = Scratch::new("reader", READER);
    let program = scratch.build();
    let outcome = scratch.0.join("outcome.jsonl");
    let outcome_option = format!("--outcome-file={}", outcome.display());
    let clock = ["--clock=virtual", "--clock-start=2026-11-10T09:30:00Z"];
    let output = runner(&program, &[clock[0], clock[1], &outcome_option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let expected = format!("{READ_FIRST}\n|{READ_SECOND}\n");
    assert_eq!(text_outcome(&std::fs::read_to_string(&outcome).unwrap()), expected);

    // The interpreter has no clock, so its times are Unix 0 and it cannot
    // sleep.
    let untimed_source = READER.replace("  Clock.sleep(Duration.seconds(1))\n", "");
    std::fs::write(scratch.0.join("untimed.chimi"), untimed_source).unwrap();
    let interpreted = scratch.chimi(&["run", "untimed.chimi"]);
    assert_eq!(interpreted.status, Some(0), "{}", interpreted.stderr);
    let untimed = expected
        .replace("1794303000000000000", "0")
        .replace("1794303001000000000", "0");
    assert_eq!(text_outcome(&interpreted.stdout), untimed);

    // With the file, recording was on from the start, and the file at exit
    // is the last read.
    std::fs::remove_file(&outcome).unwrap();
    let file = scratch.0.join("metrics.json");
    let file_option = format!("--metrics-file={}", file.display());
    let output = runner(&program, &[clock[0], clock[1], &outcome_option, &file_option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let before = r#"{"name":"before","labels":{},"kind":"count","value":1}"#;
    let with_before = |document: &str| {
        document.replacen(r#""metrics":["#, &format!(r#""metrics":[{before},"#), 1)
    };
    let first = READ_FIRST.replace(r#""metrics":[]"#, &format!(r#""metrics":[{before}]"#));
    assert_eq!(
        text_outcome(&std::fs::read_to_string(&outcome).unwrap()),
        format!("{first}\n|{}\n", with_before(READ_SECOND))
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{}\n", with_before(READ_SECOND)));
}

const CARRIER_READER: &str = r#"import std/list as List
import std/metrics as Metrics.{Metrics}
import std/task as Task

pub fn main() -> Text needs { metrics: Metrics } {
  let _ = Metrics.runner_document()
  let tasks = List.map([1, 2, 3, 4], fn(task) {
    Task.start(fn() {
      List.each(List.enumerate(List.repeat(0, 500)), fn(pair) {
        let (index, _) = pair
        Metrics.count("done", 1, [])
        // Reads on every carrier while the others record.
        case index % 100 == 0 {
          True -> {
            let _ = Metrics.runner_document()
            Void
          }
          False -> Void
        }
      })
      0
    })
  })
  List.each(tasks, fn(task) {
    let _ = Task.result(task)
    Void
  })
  Metrics.runner_document()
}
"#;

// Once the aggregate has been read, tasks on several carriers record into
// it without `--metrics-file`, and read it while others record. Rejects
// recording that only the first carrier sees turned on, or a read that
// loses or corrupts a measurement.
#[test]
fn a_read_turns_recording_on_for_every_carrier() {
    let scratch = Scratch::new("carrier-reader", CARRIER_READER);
    let program = scratch.build();
    let outcome = scratch.0.join("outcome.jsonl");
    let option = format!("--outcome-file={}", outcome.display());
    let output = runner(&program, &["--clock=real", "--carriers=4", &option]);
    assert_eq!(output.status, Some(0), "{}", output.stderr);
    let document = text_outcome(&std::fs::read_to_string(&outcome).unwrap());
    assert!(document.contains(r#"{"name":"done","labels":{},"kind":"count","value":2000}"#), "{document}");
}
