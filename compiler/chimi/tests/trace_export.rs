//! `chimi trace export --otlp`: one run's telemetry as an OTLP/JSON trace.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn chimi(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(arguments)
        .output()
        .expect("run chimi")
}

fn scratch(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("chimi-trace-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// Two 5 ms naps around a dependency call.
const NAPS: &str = concat!(
    "import std/clock as Clock\n",
    "import std/duration as Duration\n",
    "interface Store { load: fn() -> Int }\n",
    "fn nap() -> Int {\n  Clock.sleep(Duration.milliseconds(5))\n  1\n}\n",
    "fn loaded() -> Int needs { clock: Clock.Clock, store: Store } {\n",
    "  let first = nap() + store.load()\n  first + nap()\n}\n",
    "pub fn main() -> Int {\n",
    "  let run = loaded using { store: { load: fn() -> Int { 1 } } }\n",
    "  run()\n",
    "}\n",
);

const DEFECT: &str = "fn divide(n: Int, d: Int) -> Int {\n  n / d\n}\n\n\
                      pub fn main() -> Int {\n  let half = divide(4, 2)\n  divide(half, 0)\n}\n";

/// Builds `source` natively and runs it under the virtual clock with
/// `--trace=full` and `runner` options; returns the program and telemetry
/// paths.
fn traced_run(directory: &Path, source: &str, runner: &[&str]) -> (PathBuf, PathBuf) {
    let program = directory.join("main.chimi");
    std::fs::write(&program, source).unwrap();
    let executable = directory.join("program");
    // Each build in its own directory: builds keep their artifacts under the
    // working directory, and the tests run at once.
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(directory)
        .args([
            "build",
            program.to_str().unwrap(),
            "-o",
            executable.to_str().unwrap(),
        ])
        .output()
        .expect("run chimi build");
    assert!(build.status.success(), "{build:?}");
    let telemetry = directory.join("telemetry.jsonl");
    let _ = std::fs::remove_file(&telemetry);
    Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg("--trace=full")
        .args(runner)
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg("--execution-id=run")
        .arg("--")
        .output()
        .unwrap();
    (program, telemetry)
}

/// One exported span's fields, read from the document's text.
#[derive(Debug)]
struct Span {
    trace_id: String,
    id: String,
    parent: Option<String>,
    name: String,
    start: u64,
    end: u64,
    status: Option<String>,
    text: String,
}

fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let start = text.find(&format!("\"{key}\":\""))? + key.len() + 4;
    text[start..].split('"').next()
}

fn spans(document: &str) -> Vec<Span> {
    document
        .split("{\"traceId\":")
        .skip(1)
        .map(|chunk| {
            let text = format!("{{\"traceId\":{chunk}");
            Span {
                trace_id: field(&text, "traceId").unwrap().to_owned(),
                id: field(&text, "spanId").unwrap().to_owned(),
                parent: field(&text, "parentSpanId").map(str::to_owned),
                name: field(&text, "name").unwrap().to_owned(),
                start: field(&text, "startTimeUnixNano").unwrap().parse().unwrap(),
                end: field(&text, "endTimeUnixNano").unwrap().parse().unwrap(),
                status: text
                    .find("\"status\":{")
                    .map(|at| text[at..].split('}').next().unwrap().to_owned()),
                text,
            }
        })
        .collect()
}

fn export(arguments: &[&str]) -> String {
    let mut full = vec!["trace", "export", "--otlp"];
    full.extend_from_slice(arguments);
    let output = chimi(&full);
    assert!(output.status.success(), "{output:?}");
    stdout(&output)
}

// Each operation is a span inside its parent, timed by its events, under one
// trace id. Rejects spans with the wrong ids or parents, times that ignore
// the naps, a dependency call with no length, and a resource without the
// service name.
#[test]
fn exports_each_operation_as_a_span_with_its_times() {
    let directory = scratch("naps");
    let (_, telemetry) = traced_run(&directory, NAPS, &[]);
    let document = export(&[telemetry.to_str().unwrap()]);
    assert!(document.starts_with("{\"resourceSpans\":[{\"resource\":{\"attributes\":["));
    assert!(document.contains("{\"key\":\"service.name\",\"value\":{\"stringValue\":\"main\"}}"));
    assert!(document.ends_with("]}]}]}]}\n"), "{document}");
    let spans = spans(&document);
    let root = &spans[0];
    assert_eq!(
        (root.name.as_str(), root.id.as_str()),
        ("computation main", "0000000000000001")
    );
    assert_eq!(
        (root.start, root.end, root.parent.as_deref()),
        (0, 10_000_000, None)
    );
    assert_eq!(root.trace_id.len(), 32);
    for span in &spans {
        assert_eq!(span.trace_id, root.trace_id, "one trace");
        assert_eq!(span.id.len(), 16);
        assert!(span.start <= span.end, "{span:?}");
        if let Some(parent) = &span.parent {
            let parent = spans
                .iter()
                .find(|candidate| &candidate.id == parent)
                .expect("parent");
            assert!(
                parent.start <= span.start && span.end <= parent.end,
                "{span:?} in {parent:?}"
            );
        }
        assert_eq!(span.status, None, "nothing failed: {span:?}");
        assert!(span.text.contains("\"kind\":1,"));
    }
    assert!(
        spans.iter().skip(1).all(|span| span.parent.is_some()),
        "every span but the computation has a parent: {spans:?}"
    );
    let dependency = spans
        .iter()
        .find(|span| span.name == "dependency store.load")
        .expect("the store call");
    assert_eq!((dependency.start, dependency.end), (5_000_000, 5_000_000));
    assert!(
        dependency
            .text
            .contains("{\"key\":\"chimi.method\",\"value\":{\"stringValue\":\"load\"}}")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

// Times are Unix nanoseconds from the run's start. Rejects an export that
// ignores `--clock-start` or writes times as JSON numbers.
#[test]
fn times_start_at_the_runs_unix_start() {
    let directory = scratch("clock-start");
    let (_, telemetry) = traced_run(&directory, NAPS, &["--clock-start=2026-11-10T09:30:00Z"]);
    let document = export(&[telemetry.to_str().unwrap()]);
    assert!(document.contains(
        "\"startTimeUnixNano\":\"1794303000000000000\",\"endTimeUnixNano\":\"1794303000010000000\""
    ));
    std::fs::remove_dir_all(directory).unwrap();
}

// A defect marks every span on its path as an error, names the source with
// the program given, and uses the service name given. Rejects a failed
// operation exported as a success, and successes marked as errors.
#[test]
fn a_defect_marks_its_path_as_errors() {
    let directory = scratch("defect");
    let (program, telemetry) = traced_run(&directory, DEFECT, &[]);
    let document = export(&[
        "--service-name",
        "calculator",
        telemetry.to_str().unwrap(),
        program.to_str().unwrap(),
    ]);
    assert!(
        document.contains("{\"key\":\"service.name\",\"value\":{\"stringValue\":\"calculator\"}}"),
        "{document}"
    );
    let spans = spans(&document);
    let failed = |span: &Span| {
        span.status.as_deref() == Some("\"status\":{\"code\":2,\"message\":\"defect\"")
    };
    assert!(failed(&spans[0]), "{:?}", spans[0]);
    let path = program.display().to_string();
    assert!(spans[0].text.contains(&format!(
        "{{\"key\":\"chimi.source\",\"value\":{{\"stringValue\":\"{path}:5:22\"}}}}"
    )));
    assert!(
        spans
            .iter()
            .any(|span| !failed(span) && span.status.is_none()),
        "the first divide succeeded"
    );
    assert!(
        spans.iter().filter(|span| failed(span)).count() >= 2,
        "{spans:?}"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

// The interpreter's trace exports too, from standard input, with every time
// at 0. Rejects an export that cannot read `-`.
#[test]
fn exports_an_interpreter_trace_from_standard_input() {
    let directory = scratch("interpreter");
    let program = directory.join("main.chimi");
    std::fs::write(
        &program,
        "fn add(a: Int, b: Int) -> Int { a + b }\npub fn main() -> Int { add(1, 2) }\n",
    )
    .unwrap();
    let trace = directory.join("trace.jsonl");
    let run = chimi(&[
        "run",
        program.to_str().unwrap(),
        "--trace=full",
        &format!("--telemetry-file={}", trace.display()),
    ]);
    assert!(run.status.success(), "{run:?}");
    let mut export = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["trace", "export", "--otlp", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    export
        .stdin
        .take()
        .unwrap()
        .write_all(&std::fs::read(&trace).unwrap())
        .unwrap();
    let output = export.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let spans = spans(&stdout(&output));
    assert!(spans.len() >= 2, "{spans:?}");
    assert!(spans.iter().all(|span| span.start == 0 && span.end == 0));
    std::fs::remove_dir_all(directory).unwrap();
}

// A trace from before event times cannot be exported, and says where.
// Rejects spans silently placed at 1970.
#[test]
fn a_trace_without_event_times_is_refused() {
    let directory = scratch("untimed");
    let trace = directory.join("old.jsonl");
    std::fs::write(
        &trace,
        "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",\"sequence\":0,\
         \"computation\":\"main\",\"execution_id\":\"x\",\"span_id\":1,\"parent_span_id\":null,\
         \"core_node_id\":0,\"origin_id\":0,\"semantic_id\":0,\"file_id\":0,\"source_start\":0,\
         \"source_end\":0,\"operation\":\"computation\",\"attributes\":{}}\n",
    )
    .unwrap();
    let output = chimi(&["trace", "export", "--otlp", trace.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("line 1: computation_started has no time_ns"),
        "{stderr}"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

/// One hand-written semantic event.
fn event(
    event: &str,
    sequence: u32,
    span: u32,
    parent: &str,
    operation: &str,
    attributes: &str,
) -> String {
    format!(
        "{{\"schema\":\"chimi.runtime.v1\",\"event\":\"{event}\",\"sequence\":{sequence},\
         \"computation\":\"main\",\"execution_id\":\"x\",\"span_id\":{span},\
         \"parent_span_id\":{parent},\"core_node_id\":0,\"origin_id\":0,\"semantic_id\":0,\
         \"file_id\":0,\"source_start\":0,\"source_end\":0,\"operation\":\"{operation}\",\
         \"attributes\":{{{attributes}}}}}\n"
    )
}

// A dependency call is a point in time, and an operation the run never
// finished ends with its last child and is an error. As the runtimes write
// it, the provider's operation is the calling operation's child. Rejects
// unfinished work shown as fine, a call given its caller's length, and a
// second run under one execution id.
#[test]
fn open_spans_end_with_their_last_child() {
    let directory = scratch("open-spans");
    let trace = directory.join("trace.jsonl");
    let text = [
        event(
            "computation_started",
            0,
            1,
            "null",
            "computation",
            "\"time_ns\":0,\"start_unix_ns\":100",
        ),
        event("operation_started", 1, 2, "1", "call", "\"time_ns\":1"),
        event(
            "dependency_called",
            2,
            3,
            "2",
            "dependency_call",
            "\"requirement\":\"store\",\"method\":\"load\",\"time_ns\":2",
        ),
        event("operation_started", 3, 4, "2", "sleep", "\"time_ns\":2"),
        event(
            "operation_completed",
            4,
            4,
            "2",
            "sleep",
            "\"outcome\":\"success\",\"time_ns\":7",
        ),
    ]
    .concat();
    std::fs::write(&trace, text).unwrap();
    let spans = spans(&export(&[trace.to_str().unwrap()]));
    let by_name = |name: &str| spans.iter().find(|span| span.name == name).unwrap();
    let dependency = by_name("dependency store.load");
    assert_eq!(
        (
            dependency.start,
            dependency.end,
            dependency.status.as_deref()
        ),
        (102, 102, None)
    );
    let call = by_name("call");
    assert_eq!((call.start, call.end), (101, 107));
    let unfinished = "\"status\":{\"code\":2,\"message\":\"did not complete\"";
    assert_eq!(call.status.as_deref(), Some(unfinished));
    assert_eq!(by_name("computation main").end, 107);

    let mut again = std::fs::read_to_string(&trace).unwrap();
    again.push_str(&event(
        "computation_started",
        0,
        1,
        "null",
        "computation",
        "\"time_ns\":0,\"start_unix_ns\":100",
    ));
    std::fs::write(&trace, again).unwrap();
    let output = chimi(&["trace", "export", "--otlp", trace.to_str().unwrap()]);
    assert!(!output.status.success() && output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("line 6: execution `x` starts again"),
        "{stderr}"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

// An expected failure is an error span, a parent missing from the file is
// left out rather than pointing nowhere, and an end written before its start
// is moved to the start. Rejects a failed outcome shown as fine, a dangling
// parentSpanId, and a span that ends before it begins.
#[test]
fn hand_written_traces_export_as_valid_spans() {
    let directory = scratch("hand-written");
    let trace = directory.join("trace.jsonl");
    let text = [
        event(
            "computation_started",
            0,
            1,
            "null",
            "computation",
            "\"time_ns\":0,\"start_unix_ns\":100",
        ),
        event("operation_started", 1, 2, "1", "call", "\"time_ns\":50"),
        event(
            "operation_completed",
            2,
            2,
            "1",
            "call",
            "\"outcome\":\"expected_failure\",\"time_ns\":10",
        ),
        event("operation_started", 3, 3, "99", "sleep", "\"time_ns\":60"),
        event(
            "operation_completed",
            4,
            3,
            "99",
            "sleep",
            "\"outcome\":\"success\",\"time_ns\":70",
        ),
    ]
    .concat();
    std::fs::write(&trace, text).unwrap();
    let spans = spans(&export(&[trace.to_str().unwrap()]));
    let by_name = |name: &str| spans.iter().find(|span| span.name == name).unwrap();
    let call = by_name("call");
    assert_eq!((call.start, call.end), (150, 150));
    assert!(
        call.status.as_deref().is_some_and(|status| status.starts_with("\"status\":{\"code\":2")),
        "{}",
        call.text
    );
    let sleep = by_name("sleep");
    assert_eq!((sleep.parent.as_deref(), sleep.status.as_deref()), (None, None));
}

// CHI-312: a process body is a span named after its incarnation, tagged with
// the incarnation id, with the body's operations under it. Rejects a body
// left out (its operations would have no parent), a name without the
// process, and a span without `chimi.incarnation_id`.
#[test]
fn a_process_body_is_a_span_tagged_with_its_incarnation() {
    let directory = scratch("process-body");
    let trace = directory.join("trace.jsonl");
    let process = |event: &str, sequence: u32, extra: &str, attributes: &str| {
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"event\":\"{event}\",\"sequence\":{sequence},\
             \"computation\":\"main\",\"execution_id\":\"x\",{extra}\"incarnation_id\":2,\
             \"attributes\":{{{attributes}}}}}\n"
        )
    };
    let text = [
        event(
            "computation_started",
            0,
            1,
            "null",
            "computation",
            "\"time_ns\":0,\"start_unix_ns\":100",
        ),
        process("process_started", 1, "", "\"time_ns\":0,\"name\":\"main.worker\""),
        process(
            "process_body_started",
            2,
            "\"span_id\":5,\"parent_span_id\":null,\"operation\":\"process_body\",",
            "\"time_ns\":1",
        ),
        event("operation_started", 3, 6, "5", "call", "\"time_ns\":1"),
        event(
            "operation_completed",
            4,
            6,
            "5",
            "call",
            "\"outcome\":\"defect\",\"time_ns\":3",
        ),
        process(
            "process_body_completed",
            5,
            "\"span_id\":5,\"parent_span_id\":null,\"operation\":\"process_body\",",
            "\"outcome\":\"defect\",\"time_ns\":4",
        ),
    ]
    .concat();
    std::fs::write(&trace, text).unwrap();
    let spans = spans(&export(&[trace.to_str().unwrap()]));
    let body = spans
        .iter()
        .find(|span| span.name == "process main.worker incarnation 2")
        .unwrap_or_else(|| panic!("no body span: {:?}", spans.iter().map(|s| &s.name).collect::<Vec<_>>()));
    assert_eq!((body.start, body.end, body.parent.as_deref()), (101, 104, None));
    assert!(
        body.text
            .contains("{\"key\":\"chimi.incarnation_id\",\"value\":{\"intValue\":\"2\"}}"),
        "{}",
        body.text
    );
    assert!(!body.text.contains("chimi.file_id"), "{}", body.text);
    let call = spans.iter().find(|span| span.name == "call").unwrap();
    assert_eq!(call.parent.as_deref(), Some(body.id.as_str()));
    std::fs::remove_dir_all(directory).unwrap();
}
