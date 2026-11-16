//! `chimi trace show`: one run's telemetry read after the fact.

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
        std::env::temp_dir().join(format!("chimi-trace-show-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

const DEFECT: &str = "fn divide(n: Int, d: Int) -> Int {\n  n / d\n}\n\n\
                      pub fn main() -> Int {\n  let half = divide(4, 2)\n  divide(half, 0)\n}\n";

/// Builds `source` natively, runs it with `--trace=full`, and returns the
/// program path and a file holding its telemetry followed by its outcome.
fn traced_run(directory: &Path, source: &str) -> (PathBuf, PathBuf) {
    let program = directory.join("main.chimi");
    std::fs::write(&program, source).unwrap();
    let executable = directory.join("program");
    let build = chimi(&[
        "build",
        program.to_str().unwrap(),
        "-o",
        executable.to_str().unwrap(),
    ]);
    assert!(build.status.success(), "{build:?}");
    let telemetry = directory.join("telemetry.jsonl");
    let outcome = directory.join("outcome.jsonl");
    let run = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=full")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--execution-id=run")
        .arg("--")
        .output()
        .unwrap();
    assert!(!run.status.success(), "the program defects: {run:?}");
    let trace = directory.join("trace.jsonl");
    let mut text = std::fs::read_to_string(telemetry).unwrap();
    text.push_str(&std::fs::read_to_string(outcome).unwrap());
    std::fs::write(&trace, text).unwrap();
    (program, trace)
}

#[test]
fn shows_the_path_to_a_defect_with_source_locations() {
    let directory = scratch("defect");
    let (program, trace) = traced_run(&directory, DEFECT);
    let output = chimi(&[
        "trace",
        "show",
        trace.to_str().unwrap(),
        program.to_str().unwrap(),
    ]);

    assert!(output.status.success(), "{output:?}");
    let report = stdout(&output);
    let path = program.display().to_string();
    assert!(
        report.starts_with("execution run\noutcome: defect: "),
        "{report}"
    );
    assert!(
        report.contains(&format!("computation main  {path}:5:22  defect\n")),
        "{report}"
    );
    assert!(
        report.contains(&format!("  {path}:2:3  defect  <- started here\n")),
        "{report}"
    );
    assert!(report.contains("succeeded operation"), "{report}");
    assert!(
        !report.contains(&format!("{path}:6:")),
        "the succeeded let is folded: {report}"
    );

    let everything = chimi(&[
        "trace",
        "show",
        "--all",
        trace.to_str().unwrap(),
        program.to_str().unwrap(),
    ]);
    let everything = stdout(&everything);
    assert!(everything.contains(&format!("{path}:6:")), "{everything}");
    assert!(!everything.contains("succeeded operation"), "{everything}");
}

#[test]
fn without_a_source_locations_are_file_offsets() {
    let directory = scratch("no-source");
    let (_, trace) = traced_run(&directory, DEFECT);

    let output = chimi(&["trace", "show", trace.to_str().unwrap()]);

    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout(&output).contains("computation main  file 0 at "),
        "{output:?}"
    );
}

const SCHEDULER: &str = concat!(
    r#"{"schema":"chimi.runtime.v1","event":"computation_started","sequence":0,"computation":"main","execution_id":"x","span_id":1,"parent_span_id":null,"file_id":0,"source_start":0,"source_end":5,"operation":"computation","attributes":{}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"dependency_called","sequence":1,"computation":"main","execution_id":"x","span_id":2,"parent_span_id":1,"file_id":0,"source_start":2,"source_end":4,"operation":"dependency_call","attributes":{"requirement":"payment","method":"charge"}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"task_started","sequence":2,"computation":"main","execution_id":"x","task_id":1,"parent_task_id":null,"attributes":{}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"task_started","sequence":3,"computation":"main","execution_id":"x","task_id":2,"parent_task_id":1,"attributes":{}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"task_parked","sequence":4,"computation":"main","execution_id":"x","task_id":2,"parent_task_id":1,"channel_id":null,"attributes":{}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"channel_receive_parked","sequence":5,"computation":"main","execution_id":"x","task_id":2,"parent_task_id":1,"channel_id":7,"attributes":{}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"task_terminal","sequence":5,"computation":"main","execution_id":"x","task_id":1,"parent_task_id":null,"attributes":{}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"process_terminated","sequence":6,"computation":"main","execution_id":"x","incarnation_id":3,"attributes":{"cause":"defected","name":"worker","defect":"index out of range"}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"process_terminated","sequence":7,"computation":"main","execution_id":"x","incarnation_id":4,"attributes":{"cause":"returned","name":"quiet"}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","event":"computation_completed","sequence":8,"computation":"main","execution_id":"x","span_id":1,"parent_span_id":null,"file_id":0,"source_start":0,"source_end":5,"operation":"computation","attributes":{"outcome":"success"}}"#,
    "\n",
    r#"{"schema":"chimi.runtime.v1","outcome":"success","value":1}"#,
    "\n",
);

#[test]
fn lists_processes_that_did_not_return_and_tasks_that_never_ended() {
    let directory = scratch("scheduler");
    let trace = directory.join("trace.jsonl");
    std::fs::write(&trace, SCHEDULER).unwrap();

    let output = chimi(&["trace", "show", trace.to_str().unwrap()]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "execution x\n\
         outcome: success\n\
         events: 10\n\
         \n\
         computation main  file 0 at 0\n  \
           (1 succeeded operation)\n\
         \n\
         processes that did not return:\n  \
           worker incarnation 3: defected: index out of range\n\
         \n\
         tasks that never ended:\n  \
           task 2 (started by task 1): parked on channel 7\n"
    );
    let all = chimi(&["trace", "show", "--all", trace.to_str().unwrap()]);
    assert!(
        stdout(&all).contains("\n  dependency payment.charge  file 0 at 2\n"),
        "{all:?}"
    );
}

#[test]
fn reads_standard_input() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["trace", "show", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(SCHEDULER.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).starts_with("execution x\noutcome: success\n"));
}

#[test]
fn a_record_that_is_not_runtime_telemetry_is_an_error() {
    let directory = scratch("invalid");
    let trace = directory.join("trace.jsonl");
    std::fs::write(
        &trace,
        format!("{SCHEDULER}{{\"schema\":\"chimi.build.v1\"}}\n"),
    )
    .unwrap();

    let output = chimi(&["trace", "show", trace.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "chimi: {}: line 12: not a chimi.runtime.v1 record\n",
            trace.display()
        )
    );
}

#[test]
fn an_operation_that_never_completed_says_so() {
    let directory = scratch("unfinished");
    let trace = directory.join("trace.jsonl");
    let started = SCHEDULER.lines().next().unwrap();
    std::fs::write(&trace, format!("{started}\n")).unwrap();

    let output = chimi(&["trace", "show", trace.to_str().unwrap()]);

    assert_eq!(
        stdout(&output),
        "execution x\noutcome: none recorded\nevents: 1\n\n\
         computation main  file 0 at 0  (did not finish)\n"
    );
}

#[test]
fn a_deep_chain_is_listed_without_deep_recursion_or_indentation() {
    let directory = scratch("deep");
    let depth = 100_000;
    let mut text = String::new();
    for span in 1..=depth {
        let parent = if span == 1 {
            "null".to_owned()
        } else {
            (span - 1).to_string()
        };
        text.push_str(&format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"event\":\"operation_started\",\"execution_id\":\"x\",\"span_id\":{span},\"parent_span_id\":{parent},\"operation\":\"call\",\"attributes\":{{}}}}\n"
        ));
    }
    let trace = directory.join("trace.jsonl");
    std::fs::write(&trace, text).unwrap();

    let output = chimi(&["trace", "show", trace.to_str().unwrap()]);

    assert!(output.status.success(), "{:?}", output.status);
    let report = stdout(&output);
    let last = report.lines().last().unwrap();
    assert_eq!(
        last,
        format!(
            "{}[depth {}] call  (did not finish)",
            "  ".repeat(24),
            depth - 1
        )
    );
    assert!(report.len() < 100 * depth, "{} bytes", report.len());
}

#[test]
fn an_outcome_belongs_to_the_execution_just_before_it() {
    let directory = scratch("two-executions");
    let event = |id: &str| {
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"event\":\"task_started\",\"execution_id\":\"{id}\",\"task_id\":1,\"attributes\":{{}}}}\n"
        )
    };
    let outcome = "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1}\n";
    let trace = directory.join("trace.jsonl");
    std::fs::write(
        &trace,
        format!("{}{}{}{outcome}", event("a"), event("b"), event("a")),
    )
    .unwrap();

    let report = stdout(&chimi(&["trace", "show", trace.to_str().unwrap()]));

    assert!(
        report.starts_with("execution a\noutcome: success\n"),
        "{report}"
    );
    assert!(
        report.contains("execution b\noutcome: none recorded\n"),
        "{report}"
    );
}

#[test]
fn a_directory_names_the_places_of_its_test_traces() {
    let directory = scratch("directory");
    let tests = directory.join("tests");
    std::fs::create_dir_all(&tests).unwrap();
    std::fs::write(
        tests.join("a_test.chimi"),
        "test \"a\" {\n  let _ = 1 / 0\n  Void\n}\n",
    )
    .unwrap();
    std::fs::write(
        tests.join("b_test.chimi"),
        "test \"b\" {\n  let _ = 2 / 0\n  Void\n}\n",
    )
    .unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&directory)
        .args(["test", "--trace=full", "tests"])
        .output()
        .unwrap();
    assert!(!run.status.success(), "the tests defect: {run:?}");
    let traces = directory.join("target/chimi/tests/test/traces");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&traces)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");

    for (file, name) in files.iter().zip(["a_test.chimi", "b_test.chimi"]) {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&directory)
            .args(["trace", "show", file.to_str().unwrap(), "tests"])
            .output()
            .unwrap();
        let report = stdout(&output);
        assert!(
            report.contains(&format!("{name}:2:11  defect  <- started here")),
            "{report}"
        );
        assert!(output.stderr.is_empty(), "{output:?}");
    }

    // A source that is not the one the run was built from is flagged.
    let other = directory.join("other.chimi");
    std::fs::write(&other, "pub fn main() -> Int {\n  1\n}\n").unwrap();
    let output = chimi(&[
        "trace",
        "show",
        files[1].to_str().unwrap(),
        other.to_str().unwrap(),
    ]);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not in the given source"),
        "{output:?}"
    );
}
