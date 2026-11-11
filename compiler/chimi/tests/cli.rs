use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}.chimi", env!("CARGO_MANIFEST_DIR"))
}

fn repository_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-cli-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct NativeExecution {
    process: Output,
    telemetry: String,
    outcome: String,
}

fn execute_native(executable: &Path, scratch: &Path, name: &str, trace: &str) -> NativeExecution {
    let telemetry_path = scratch.join(format!("{name}-telemetry.jsonl"));
    let outcome_path = scratch.join(format!("{name}-outcome.jsonl"));
    let process = Command::new(executable)
        .arg("--chimi-runner")
        .arg(format!("--trace={trace}"))
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--execution-id={name}"))
        .arg("--")
        .output()
        .expect("execute native Chimi program");
    NativeExecution {
        process,
        telemetry: std::fs::read_to_string(telemetry_path).unwrap(),
        outcome: std::fs::read_to_string(outcome_path).unwrap(),
    }
}

#[test]
fn version_reports_the_bootstrap_compiler() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("--version")
        .output()
        .expect("run chimi --version");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "chimi 0.0.0-bootstrap\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn concurrency_lab_runs_each_evidence_kind_independently() {
    for kind in ["static", "runtime", "observability", "performance", "smoke"] {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi-concurrency-lab"))
            .arg(kind)
            .output()
            .expect("run concurrency conformance lab");
        assert!(
            output.status.success(),
            "{kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{kind}");
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(report.starts_with("{\"schema\":\"chimi.concurrency-conformance.v1\",\"kind\":\""));
        assert!(report.contains(&format!("\"kind\":\"{kind}\"")));
        assert!(report.contains("\"checks\":["));
        assert!(report.contains("\"metrics\":{"));
        assert_eq!(report.lines().count(), 1);
    }
}

#[test]
fn check_accepts_a_single_module_project() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", &fixture("success")])
        .output()
        .expect("run chimi check");

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn format_prints_canonical_source_without_modifying_the_file() {
    let path = fixture("format_messy");
    let before = std::fs::read_to_string(&path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["format", &path])
        .output()
        .expect("format valid Chimi source");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let formatted = String::from_utf8(output.stdout).unwrap();
    assert!(formatted.contains("// comments survive"));
    assert_ne!(formatted, before);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn format_returns_malformed_source_unchanged_with_syntax_diagnostics() {
    let path = fixture("missing_closing_brace");
    let source = std::fs::read(&path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["format", &path])
        .output()
        .expect("format malformed Chimi source");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, source);
    assert!(String::from_utf8_lossy(&output.stderr).contains("error[S1001] syntax"));
}

#[test]
fn run_executes_source_and_prints_structured_events_and_outcome() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["run", &fixture("success")])
        .output()
        .expect("run valid Chimi source");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert!(lines.len() > 3);
    assert!(lines[0].contains("\"event\":\"computation_started\""));
    assert!(lines[0].contains("\"computation\":\"main\""));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"operation\":\"construct\""))
    );
    assert!(lines[lines.len() - 2].contains("\"event\":\"computation_completed\""));
    assert_eq!(
        lines[lines.len() - 1],
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}"
        )
    );
}

#[test]
fn build_links_a_runnable_executable_and_reports_its_artifacts() {
    let scratch = scratch_directory("build");
    let executable = scratch.join("answer");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&executable)
        .output()
        .expect("build valid Chimi source");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.starts_with("{\"schema\":\"chimi.build.v1\""));
    assert!(report.contains("\"backend\":\"native-aot\""));
    assert!(report.contains("\"application_objects\":["));
    assert!(report.contains("\"metadata_object\":"));
    assert!(report.contains("\"runtime_archive\":"));
    assert!(report.contains("\"fiber_runtime_archive\":null"));
    assert!(report.contains("\"dynamic_dependencies\":["));
    assert!(executable.is_file());
    assert!(
        std::fs::metadata(&executable).unwrap().len() < 1024 * 1024,
        "an ordinary binary must not retain the unused fiber runtime"
    );

    let manifest = scratch.join("target/chimi/answer/build.json");
    assert_eq!(std::fs::read_to_string(manifest).unwrap(), report);
    let execution = execute_native(&executable, &scratch, "cli-build", "boundaries");
    assert!(execution.process.status.success());
    assert!(execution.process.stdout.is_empty());
    assert!(execution.process.stderr.is_empty());
    assert!(
        execution
            .telemetry
            .lines()
            .next()
            .unwrap()
            .contains("\"execution_id\":\"cli-build\"")
    );
    assert_eq!(
        execution.outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}\n"
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_executes_task_start_and_result() {
    let scratch = scratch_directory("native-task-start-result");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("task-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "enum WorkError { Stopped }\n",
            "fn work() -> Int {\n",
            "  case 1 {\n",
            "    0 -> Failure(Stopped())\n",
            "    _ -> 42\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(work)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Task program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-task", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_executes_bounded_channel_send_close_and_receive() {
    let scratch = scratch_directory("native-channel-round-trip");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("channel-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.send(channel.sender, 42)\n",
            "  Channel.close(channel.completion)\n",
            "  let first = Channel.receive(channel.receiver)\n",
            "  let second = Channel.receive(channel.receiver)\n",
            "  case first {\n",
            "    Receive.Received(value) -> case second {\n",
            "      Receive.Received(_) -> 0\n",
            "      Receive.Drained -> value\n",
            "    }\n",
            "    Receive.Drained -> 0\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Channel program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-channel", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_suspends_a_task_on_a_rendezvous_channel() {
    let scratch = scratch_directory("native-task-channel-rendezvous");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("rendezvous-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Channel.send(sender, 42)\n",
            "    7\n",
            "  })\n",
            "  let received = Channel.receive(receiver)\n",
            "  let child_value = Task.result(child)\n",
            "  Channel.close(completion)\n",
            "  case received {\n",
            "    Receive.Received(value) -> value + child_value\n",
            "    Receive.Drained -> 0\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native Task and Channel rendezvous program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-rendezvous", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":49}\n"
    );
    assert!(execution.telemetry.is_empty());

    let boundaries = execute_native(
        &executable,
        &scratch,
        "native-rendezvous-boundaries",
        "boundaries",
    );
    assert!(boundaries.process.status.success());
    assert!(boundaries.telemetry.contains("\"event\":\"task_started\""));
    assert!(
        boundaries
            .telemetry
            .contains("\"event\":\"channel_created\"")
    );
    assert!(!boundaries.telemetry.contains("\"event\":\"task_parked\""));

    let full = execute_native(&executable, &scratch, "native-rendezvous-full", "full");
    assert!(full.process.status.success());
    assert!(full.telemetry.contains("\"event\":\"task_parked\""));
    assert!(
        full.telemetry
            .contains("\"event\":\"channel_receive_parked\"")
    );
    assert!(
        full.telemetry
            .contains("\"event\":\"channel_receive_delivered\"")
    );
    let sequences = full
        .telemetry
        .lines()
        .map(|line| json_u64_field(line, "sequence"))
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_source_transfers_an_exclusive_receiver_to_a_child() {
    let scratch = scratch_directory("native-channel-receiver-transfer");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("receiver-transfer-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    let received = Channel.receive(receiver)\n",
            "    case received {\n",
            "      Receive.Received(value) -> value\n",
            "      Receive.Drained -> 0\n",
            "    }\n",
            "  })\n",
            "  Channel.send(sender, 42)\n",
            "  let value = Task.result(child)\n",
            "  Channel.close(completion)\n",
            "  value\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native receiver-transfer program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-transfer", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_channel_close_recovers_the_unaccepted_value() {
    let scratch = scratch_directory("native-channel-close-recovery");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("close-recovery-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{SendError}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.close(channel.completion)\n",
            "  case Channel.send(channel.sender, 42) {\n",
            "    Success(_) -> 0\n",
            "    Failure(error) -> case error {\n",
            "      SendError.Closed(value) -> value\n",
            "      SendError.Aborted(value) -> value\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native close-recovery program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-close-recovery", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_task_cancel_cleans_up_a_parked_child() {
    let scratch = scratch_directory("native-task-cancel");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("cancel-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive}\n",
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let receiver = channel.receiver\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    let received = Channel.receive(receiver)\n",
            "    case received {\n",
            "      Receive.Received(value) -> value\n",
            "      Receive.Drained -> 0\n",
            "    }\n",
            "  })\n",
            "  Task.cancel(child)\n",
            "  42\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native cancellation program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-cancel", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_channel_abort_discards_values_and_fails_receive_explicitly() {
    let scratch = scratch_directory("native-channel-abort");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("abort-program");
    std::fs::write(
        &source,
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{ChannelError}\n",
            "pub fn main() -> Int {\n",
            "  let channel = Channel.bounded(1)\n",
            "  Channel.send(channel.sender, 10)\n",
            "  Channel.abort(channel.receiver)\n",
            "  case Channel.receive(channel.receiver) {\n",
            "    Success(_) -> 0\n",
            "    Failure(ChannelError.Aborted) -> 42\n",
            "  }\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native abort program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-abort", "full");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );
    assert!(
        execution
            .telemetry
            .contains("\"event\":\"channel_aborted\"")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_child_defect_propagates_through_task_result() {
    let scratch = scratch_directory("native-task-defect");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("defect-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "enum WorkError { Stopped }\n",
            "fn broken() -> Int {\n",
            "  case 1 {\n",
            "    0 -> Failure(Stopped())\n",
            "    _ -> 1 / 0\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(broken)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native child-defect program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-defect", "full");
    assert_eq!(execution.process.status.code(), Some(70));
    assert!(execution.outcome.contains("\"outcome\":\"defect\""));
    assert!(execution.outcome.contains("integer division by zero"));
    assert!(execution.telemetry.contains("\"event\":\"task_terminal\""));
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_task_result_preserves_expected_failure_values() {
    let scratch = scratch_directory("native-task-expected-failure");
    let source = scratch.join("main.chimi");
    let executable = scratch.join("expected-failure-program");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "enum WorkError { Stopped }\n",
            "fn fail() -> Int { Failure(Stopped()) }\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(fail)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native expected-failure task program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let execution = execute_native(&executable, &scratch, "native-task-failure", "full");
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(
        execution
            .outcome
            .contains("\"outcome\":\"expected_failure\"")
    );
    assert!(execution.outcome.contains("\"type\":\"WorkError\""));
    assert!(execution.outcome.contains("\"variant\":\"Stopped\""));
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_runner_separates_versioned_telemetry_outcome_and_application_streams() {
    let scratch = scratch_directory("runner-channels");
    let executable = scratch.join("answer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&executable)
        .output()
        .expect("build runner-channel fixture");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let default_execution = Command::new(&executable)
        .output()
        .expect("run with the default trace-off policy");
    assert!(default_execution.status.success());
    assert!(default_execution.stdout.is_empty());
    assert!(default_execution.stderr.is_empty());

    let telemetry = scratch.join("telemetry.jsonl");
    let outcome = scratch.join("outcome.jsonl");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=full")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--execution-id=separated-run")
        .arg("--")
        .output()
        .expect("run with separate runner sinks");
    assert!(execution.status.success());
    assert!(execution.stdout.is_empty());
    assert!(execution.stderr.is_empty());

    let telemetry = std::fs::read_to_string(&telemetry).unwrap();
    assert!(telemetry.lines().count() > 3);
    assert!(
        telemetry
            .lines()
            .all(|line| line.starts_with("{\"schema\":\"chimi.runtime.v1\",\"event\":"))
    );
    assert!(telemetry.contains("\"execution_id\":\"separated-run\""));
    assert_eq!(
        std::fs::read_to_string(&outcome).unwrap(),
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}\n",
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_runner_options_require_the_explicit_configuration_boundary() {
    let scratch = scratch_directory("runner-boundary");
    let executable = scratch.join("answer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&executable)
        .output()
        .expect("build runner-boundary fixture");
    assert!(build.status.success());

    let execution = Command::new(&executable)
        .arg("--trace=full")
        .output()
        .expect("keep unbounded arguments application-owned");
    assert_eq!(execution.status.code(), Some(64));
    assert!(execution.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&execution.stderr)
            .contains("program entry does not accept application arguments")
    );

    let missing_telemetry = Command::new(&executable)
        .args(["--chimi-runner", "--trace=full", "--"])
        .output()
        .expect("reject tracing without a selected sink");
    assert_eq!(missing_telemetry.status.code(), Some(64));
    assert!(
        String::from_utf8_lossy(&missing_telemetry.stderr).contains("requires --telemetry-file")
    );

    let shared_path = scratch.join("shared.jsonl");
    let shared_sinks = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--telemetry-file={}", shared_path.display()))
        .arg(format!("--outcome-file={}", shared_path.display()))
        .arg("--")
        .output()
        .expect("reject one file for both runner protocols");
    assert_eq!(shared_sinks.status.code(), Some(64));
    assert!(!shared_path.exists());

    let shared_memory_sink = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", shared_path.display()))
        .arg(format!("--memory-report-file={}", shared_path.display()))
        .arg("--")
        .output()
        .expect("reject one file for distinct outcome and memory protocols");
    assert_eq!(shared_memory_sink.status.code(), Some(64));
    assert!(!shared_path.exists());

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_main_receives_arguments_and_uses_real_scoped_host_io() {
    let scratch = scratch_directory("host-io");
    let executable = scratch.join("host-io");
    let project = repository_path("compiler/chimi/tests/fixtures/project-host-io");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&project)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build host-I/O fixture");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let input = scratch.join("input.txt");
    std::fs::write(&input, "\"alpha\"\r\n\n\"β\"").unwrap();
    let telemetry = scratch.join("telemetry.jsonl");
    let outcome = scratch.join("outcome.jsonl");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=boundaries")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg("--execution-id=host-io")
        .arg("--")
        .arg(&input)
        .output()
        .expect("execute real host-I/O fixture");

    assert!(execution.status.success());
    assert_eq!(
        String::from_utf8(execution.stdout).unwrap(),
        "\"alpha\"\n\"β\"\n"
    );
    assert_eq!(
        String::from_utf8(execution.stderr).unwrap(),
        "processed input\n"
    );
    assert_eq!(
        std::fs::read_to_string(outcome).unwrap(),
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":{\"type\":\"Void\"}}\n"
    );
    let telemetry = std::fs::read_to_string(telemetry).unwrap();
    assert!(telemetry.contains("\"method\":\"process_utf8_lines\""));
    assert_eq!(telemetry.matches("\"method\":\"write_line\"").count(), 2);
    assert!(telemetry.contains("\"method\":\"write_error\""));

    let stdin_outcome = scratch.join("stdin-outcome.jsonl");
    let mut stdin_execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", stdin_outcome.display()))
        .arg("--")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stdin host-I/O fixture");
    stdin_execution
        .stdin
        .take()
        .unwrap()
        .write_all("\"from stdin\"\n".as_bytes())
        .unwrap();
    let stdin_execution = stdin_execution.wait_with_output().unwrap();
    assert!(stdin_execution.status.success());
    assert_eq!(stdin_execution.stdout, b"\"from stdin\"\n");

    let missing_outcome = scratch.join("missing-outcome.jsonl");
    let missing = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", missing_outcome.display()))
        .arg("--")
        .output()
        .expect("run without required application data");
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        std::fs::read_to_string(missing_outcome)
            .unwrap()
            .contains("\"type\":\"UsageError\",\"variant\":\"MissingPath\"")
    );

    let invalid = scratch.join("invalid.txt");
    std::fs::write(&invalid, [0xff]).unwrap();
    let invalid_outcome = scratch.join("invalid-outcome.jsonl");
    let invalid = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", invalid_outcome.display()))
        .arg("--")
        .arg(&invalid)
        .output()
        .expect("reject invalid UTF-8");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    assert!(
        std::fs::read_to_string(invalid_outcome)
            .unwrap()
            .contains("\"type\":\"Utf8Error\",\"variant\":\"InvalidUtf8\"")
    );

    let malformed = scratch.join("malformed.jsonl");
    std::fs::write(&malformed, "{").unwrap();
    let malformed_outcome = scratch.join("malformed-outcome.jsonl");
    let malformed = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--outcome-file={}", malformed_outcome.display()))
        .arg("--")
        .arg(&malformed)
        .output()
        .expect("retain callback failure across allocation cleanup");
    assert_eq!(malformed.status.code(), Some(1));
    assert!(malformed.stdout.is_empty());
    assert!(
        std::fs::read_to_string(malformed_outcome)
            .unwrap()
            .contains("\"type\":\"DecodeError\",\"variant\":\"InvalidJson\"")
    );

    let oversized = scratch.join("oversized.txt");
    std::fs::write(&oversized, "four").unwrap();
    let oversized_outcome = scratch.join("oversized-outcome.jsonl");
    let oversized = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg("--max-line-bytes=3")
        .arg(format!("--outcome-file={}", oversized_outcome.display()))
        .arg("--")
        .arg(&oversized)
        .output()
        .expect("enforce the configured line bound");
    assert_eq!(oversized.status.code(), Some(1));
    assert!(
        std::fs::read_to_string(oversized_outcome)
            .unwrap()
            .contains("\"type\":\"FileError\",\"variant\":\"LineTooLong\"")
    );

    let measure_stream = |name: &str, lines: usize| {
        let input = scratch.join(format!("{name}.jsonl"));
        std::fs::write(&input, "{\"value\":1}\n".repeat(lines)).unwrap();
        let report = scratch.join(format!("{name}-memory.json"));
        let execution = Command::new(&executable)
            .arg("--chimi-runner")
            .arg("--trace=off")
            .arg(format!("--memory-report-file={}", report.display()))
            .arg("--")
            .arg(input)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("measure bounded streaming allocation");
        assert!(execution.success());
        let report = std::fs::read_to_string(report).unwrap();
        (
            json_u64_field(&report, "scoped_total_allocated_bytes"),
            json_u64_field(&report, "scoped_peak_live_bytes"),
            json_u64_field(&report, "scoped_retained_bytes"),
            json_u64_field(&report, "scoped_collections"),
        )
    };
    let small = measure_stream("small", 64);
    let large = measure_stream("large", 16_384);
    assert!(large.0 > small.0 * 100, "{small:?} {large:?}");
    assert_eq!(large.1, small.1, "{small:?} {large:?}");
    assert_eq!(large.2, small.2, "{small:?} {large:?}");
    assert!(large.3 > small.3 * 100, "{small:?} {large:?}");

    std::fs::remove_dir_all(scratch).unwrap();
}

fn json_u64_field(document: &str, name: &str) -> u64 {
    let marker = format!("\"{name}\":");
    document
        .split_once(&marker)
        .unwrap()
        .1
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn build_stops_before_artifact_emission_when_compilation_fails() {
    let scratch = scratch_directory("invalid-build");
    let executable = scratch.join("missing-main");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("missing_main"), "-o"])
        .arg(&executable)
        .output()
        .expect("reject invalid Chimi build");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("error[E5001] execution-setup"));
    assert!(!executable.exists());
    assert!(!scratch.join("target/chimi/missing-main").exists());

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn build_loads_and_links_an_explicit_multi_module_project() {
    let scratch = scratch_directory("project-build");
    let executable = scratch.join("project");
    let root = repository_path("compiler/chimi/tests/fixtures/project");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build multi-module Chimi project");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("module-0.o"));
    assert!(report.contains("module-1.o"));
    assert!(report.contains("entry.o"));
    let execution = execute_native(&executable, &scratch, "project-build", "off");
    assert!(execution.process.status.success());
    assert!(execution.telemetry.is_empty());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn build_executes_virtual_json_and_map_standard_modules() {
    let scratch = scratch_directory("stdlib-project-build");
    let executable = scratch.join("stdlib-project");
    let root = repository_path("compiler/chimi/tests/fixtures/project-stdlib");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build project using virtual standard modules");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let execution = execute_native(&executable, &scratch, "stdlib", "off");
    assert!(execution.process.status.success());
    assert!(execution.process.stderr.is_empty());
    assert_eq!(
        execution.outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",",
            "\"value\":\"{\\\"alpha\\\":1,\\\"zeta\\\":2}\"}\n"
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_concurrency_pipeline_forces_the_connected_runtime() {
    let scratch = scratch_directory("native-concurrency-pipeline");
    let executable = scratch.join("concurrency-pipeline");
    let root = repository_path("programs/concurrency-pipeline");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build the source-level concurrency forcing program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let execution = execute_native(&executable, &scratch, "native-concurrency-pipeline", "full");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":25}\n"
    );
    for event in [
        "channel_send_parked",
        "channel_receiver_transferred",
        "channel_completion_transferred",
        "task_terminal",
        "dependency_called",
    ] {
        assert!(
            execution
                .telemetry
                .contains(&format!("\"event\":\"{event}\"")),
            "missing {event} in {}",
            execution.telemetry
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_json_decode_reports_malformed_input_as_an_expected_failure() {
    let scratch = scratch_directory("stdlib-malformed-json");
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        concat!(
            "import std/json as Json\n",
            "import std/json.{Value}\n\n",
            "pub fn main() -> Value { Json.decode(\"{\") }\n",
        ),
    )
    .unwrap();
    let executable = scratch.join("malformed-json");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&project)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build malformed JSON program");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let execution = execute_native(&executable, &scratch, "malformed-json", "off");
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(execution.outcome.starts_with(concat!(
        "{\"schema\":\"chimi.runtime.v1\",",
        "\"outcome\":\"expected_failure\",\"error\":",
        "{\"type\":\"DecodeError\",\"variant\":\"InvalidJson\""
    )));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_function_values_retain_provisioned_requirements() {
    let scratch = scratch_directory("native-latent-using");
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        concat!(
            "interface Source { read: fn() -> Int }\n",
            "let provider = { read: fn() -> Int { 41 } }\n",
            "fn read() -> Int needs { source: Source } { source.read() }\n",
            "fn configured() -> fn(Int) -> Int {\n",
            "  let operation = read using { source: provider }\n",
            "  let offset = 1\n",
            "  fn(value: Int) -> Int { operation() + offset + value }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let operation = configured()\n",
            "  operation(0)\n",
            "}\n",
        ),
    )
    .unwrap();
    let executable = scratch.join("latent-using");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&project)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build latent using program");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let execution = execute_native(&executable, &scratch, "latent-using", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_chimi_code_classifies_and_aggregates_trace_records() {
    let scratch = scratch_directory("trace-summary");
    let executable = scratch.join("chimi-trace-summary");
    let root = repository_path("programs/chimi-trace-summary");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build chimi-trace-summary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let producer = scratch.join("producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", &fixture("success"), "-o"])
        .arg(&producer)
        .output()
        .expect("build trace producer");
    assert!(build.status.success());

    let capture = |producer: &Path, name: &str, trace: &str, cancel: bool| {
        let telemetry = scratch.join(format!("{name}-source-telemetry.jsonl"));
        let outcome = scratch.join(format!("{name}-source-outcome.jsonl"));
        let mut command = Command::new(producer);
        command
            .arg("--chimi-runner")
            .arg(format!("--trace={trace}"))
            .arg(format!("--telemetry-file={}", telemetry.display()))
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg(format!("--execution-id=source-{name}"));
        if cancel {
            command.arg("--cancel");
        }
        let output = command.arg("--").output().expect("capture native trace");
        assert!(output.status.code().is_some());
        let input = scratch.join(format!("{name}-input.jsonl"));
        let records = format!(
            "{}{}",
            std::fs::read_to_string(telemetry).unwrap(),
            std::fs::read_to_string(outcome).unwrap()
        );
        std::fs::write(&input, records).unwrap();
        input
    };

    let summarize = |input: &Path, name: &str, trace: &str, max_line_bytes: Option<usize>| {
        let telemetry = scratch.join(format!("{name}-summary-telemetry.jsonl"));
        let outcome = scratch.join(format!("{name}-summary-outcome.jsonl"));
        let mut command = Command::new(&executable);
        command
            .arg("--chimi-runner")
            .arg(format!("--trace={trace}"))
            .arg(format!("--telemetry-file={}", telemetry.display()))
            .arg(format!("--outcome-file={}", outcome.display()))
            .arg(format!("--execution-id=summary-{name}"));
        if let Some(limit) = max_line_bytes {
            command.arg(format!("--max-line-bytes={limit}"));
        }
        let process = command
            .arg("--")
            .arg(input)
            .output()
            .expect("execute chimi-trace-summary");
        NativeExecution {
            process,
            telemetry: std::fs::read_to_string(telemetry).unwrap(),
            outcome: std::fs::read_to_string(outcome).unwrap(),
        }
    };

    let modes = [
        (
            "off",
            concat!(
                "{\"dependency_calls\":{},\"events\":0,\"operations\":{},",
                "\"records\":1,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            ),
        ),
        (
            "boundaries",
            concat!(
                "{\"dependency_calls\":{},\"events\":2,\"operations\":{},",
                "\"records\":3,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            ),
        ),
        (
            "full",
            concat!(
                "{\"dependency_calls\":{},\"events\":8,",
                "\"operations\":{\"constant\":1,\"construct\":1,\"sequence\":1},",
                "\"records\":9,\"schema\":\"chimi.trace-summary.v1\",",
                "\"terminal_outcomes\":{\"success\":1}}\n",
            ),
        ),
    ];
    for (mode, expected) in modes {
        let input = capture(&producer, mode, mode, false);
        let summary_modes: &[&str] = if mode == "boundaries" {
            &["off", "boundaries", "full"]
        } else {
            &[mode]
        };
        for summary_mode in summary_modes {
            let name = format!("{mode}-under-{summary_mode}");
            let execution = summarize(&input, &name, summary_mode, None);
            assert!(execution.process.status.success(), "{name}");
            assert_eq!(
                String::from_utf8(execution.process.stdout).unwrap(),
                expected
            );
            assert!(execution.process.stderr.is_empty());
            assert!(execution.outcome.contains("\"outcome\":\"success\""));
            if *summary_mode == "off" {
                assert!(execution.telemetry.is_empty());
            } else {
                assert!(!execution.telemetry.is_empty());
            }
        }
    }

    let dependency = scratch.join("dependency-producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(repository_path(
            "prototypes/golden/valid/dependencies_and_execution.chimi",
        ))
        .arg("-o")
        .arg(&dependency)
        .output()
        .expect("build dependency trace producer");
    assert!(build.status.success());
    let input = capture(&dependency, "dependency", "boundaries", false);
    let execution = summarize(&input, "dependency", "off", None);
    assert!(execution.process.status.success());
    assert_eq!(
        String::from_utf8(execution.process.stdout).unwrap(),
        concat!(
            "{\"dependency_calls\":{\"inventory.reserve\":1,\"payment.charge\":1},",
            "\"events\":4,\"operations\":{},\"records\":5,",
            "\"schema\":\"chimi.trace-summary.v1\",",
            "\"terminal_outcomes\":{\"success\":1}}\n",
        )
    );

    let defect = scratch.join("defect-producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build"])
        .arg(repository_path(
            "prototypes/backend-comparison/defect.chimi",
        ))
        .arg("-o")
        .arg(&defect)
        .output()
        .expect("build defect trace producer");
    assert!(build.status.success());

    let failure_project = scratch.join("failure-project");
    std::fs::create_dir_all(failure_project.join("app")).unwrap();
    std::fs::write(
        failure_project.join("app/main.chimi"),
        concat!(
            "import std/json as Json\n",
            "import std/json.{Value}\n\n",
            "pub fn main() -> Value { Json.decode(\"{\") }\n",
        ),
    )
    .unwrap();
    let failure = scratch.join("failure-producer");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&failure_project)
        .args(["app/main", "-o"])
        .arg(&failure)
        .output()
        .expect("build expected-failure trace producer");
    assert!(build.status.success());

    let terminal_cases = [
        (&failure, "expected-failure", false, "expected_failure"),
        (&defect, "defect", false, "defect"),
        (&producer, "cancellation", true, "cancellation"),
    ];
    for (producer, name, cancel, outcome) in terminal_cases {
        let input = capture(producer, name, "off", cancel);
        let execution = summarize(&input, name, "off", None);
        assert!(execution.process.status.success(), "{name}");
        assert_eq!(
            String::from_utf8(execution.process.stdout).unwrap(),
            format!(
                concat!(
                    "{{\"dependency_calls\":{{}},\"events\":0,\"operations\":{{}},",
                    "\"records\":1,\"schema\":\"chimi.trace-summary.v1\",",
                    "\"terminal_outcomes\":{{\"{}\":1}}}}\n",
                ),
                outcome
            )
        );
    }

    let allocation_records = 1_024_u64;
    let allocation_input = scratch.join("allocation-input.jsonl");
    let mut allocation_source = String::new();
    for sequence in 0..allocation_records {
        allocation_source.push_str(&format!(
            concat!(
                "{{\"schema\":\"chimi.runtime.v1\",",
                "\"event\":\"operation_completed\",\"sequence\":{},",
                "\"execution_id\":\"allocation\",\"computation\":\"main\",",
                "\"operation\":\"call\"}}\n"
            ),
            sequence
        ));
    }
    allocation_source
        .push_str("{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}\n");
    std::fs::write(&allocation_input, allocation_source).unwrap();
    let memory_report = scratch.join("allocation-memory.json");
    let allocation = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg("--")
        .arg(&allocation_input)
        .output()
        .expect("measure trace-summary callback allocation");
    assert!(allocation.status.success());
    assert_eq!(
        String::from_utf8(allocation.stdout).unwrap(),
        concat!(
            "{\"dependency_calls\":{},\"events\":1024,",
            "\"operations\":{\"call\":1024},\"records\":1025,",
            "\"schema\":\"chimi.trace-summary.v1\",",
            "\"terminal_outcomes\":{\"success\":1}}\n"
        )
    );
    let memory_report = std::fs::read_to_string(memory_report).unwrap();
    let total_allocated = json_u64_field(&memory_report, "scoped_total_allocated_bytes");
    let accounted_allocated: u64 = [
        "bytes",
        "values",
        "controls",
        "environments",
        "value_pointers",
        "fields",
        "callables",
        "map_nodes",
        "list_nodes",
    ]
    .into_iter()
    .map(|kind| json_u64_field(&memory_report, kind))
    .sum();
    assert_eq!(accounted_allocated, total_allocated);
    let bytes_per_record = total_allocated / (allocation_records + 1);
    assert!(
        bytes_per_record < 45_000,
        "trace summary allocated {bytes_per_record} bytes per record"
    );
    assert!(
        json_u64_field(&memory_report, "scoped_peak_live_bytes") < 50_000,
        "trace summary callback exceeded its live-memory budget"
    );
    assert!(
        json_u64_field(&memory_report, "scoped_retained_bytes") < 2_048,
        "trace summary retained too much callback state"
    );

    let invalid_cases = [
        (
            "malformed",
            "{\n",
            "\"type\":\"TraceError\",\"variant\":\"InvalidJson\",\"fields\":{\"line\":1}",
        ),
        (
            "blank",
            "\n",
            "\"type\":\"TraceError\",\"variant\":\"BlankLine\",\"fields\":{\"line\":1}",
        ),
        (
            "schema",
            "{\"schema\":\"other.v1\",\"outcome\":\"success\"}\n",
            "\"type\":\"TraceError\",\"variant\":\"InvalidSchema\",\"fields\":{\"line\":1}",
        ),
        (
            "event",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"unknown\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"InvalidEvent\",\"fields\":{\"line\":1}",
        ),
        (
            "missing-outcome",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"MissingOutcome\",\"fields\":{\"line\":2}",
        ),
        (
            "mixed-execution",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_completed\",",
                "\"sequence\":1,\"execution_id\":\"two\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"MixedExecution\",\"fields\":{\"line\":2}",
        ),
        (
            "sequence",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":1,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_completed\",",
                "\"sequence\":1,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"InvalidSequence\",\"fields\":{\"line\":2}",
        ),
        (
            "mixed-computation",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_started\",",
                "\"sequence\":0,\"execution_id\":\"one\",\"computation\":\"main\"}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"event\":\"computation_completed\",",
                "\"sequence\":1,\"execution_id\":\"one\",\"computation\":\"other\"}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"MixedComputation\",\"fields\":{\"line\":2}",
        ),
        (
            "after-outcome",
            concat!(
                "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1}\n",
                "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1}\n",
            ),
            "\"type\":\"TraceError\",\"variant\":\"RecordAfterOutcome\",\"fields\":{\"line\":2}",
        ),
    ];
    for (name, source, expected_error) in invalid_cases {
        let input = scratch.join(format!("invalid-{name}.jsonl"));
        std::fs::write(&input, source).unwrap();
        let execution = summarize(&input, &format!("invalid-{name}"), "off", None);
        assert_eq!(execution.process.status.code(), Some(1), "{name}");
        assert!(execution.process.stdout.is_empty(), "{name}");
        assert_eq!(
            execution.process.stderr, b"trace summary failed\n",
            "{name}"
        );
        assert!(execution.outcome.contains(expected_error), "{name}");
    }

    let oversized = scratch.join("oversized-summary-input.jsonl");
    std::fs::write(&oversized, "{\"schema\":\"chimi.runtime.v1\"}\n").unwrap();
    let execution = summarize(&oversized, "oversized", "off", Some(8));
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(execution.process.stdout.is_empty());
    assert_eq!(
        execution.process.stderr,
        b"trace summary input record is too large\n"
    );
    assert!(execution.outcome.contains("\"variant\":\"LineTooLong\""));

    let invalid_utf8 = scratch.join("invalid-utf8-summary-input.jsonl");
    std::fs::write(&invalid_utf8, [0xff]).unwrap();
    let execution = summarize(&invalid_utf8, "invalid-utf8", "off", None);
    assert_eq!(execution.process.status.code(), Some(1));
    assert!(execution.process.stdout.is_empty());
    assert_eq!(
        execution.process.stderr,
        b"trace summary input is not UTF-8\n"
    );
    assert!(execution.outcome.contains("\"variant\":\"InvalidUtf8\""));

    let missing = summarize(
        &scratch.join("does-not-exist.jsonl"),
        "missing-file",
        "off",
        None,
    );
    assert_eq!(missing.process.status.code(), Some(1));
    assert!(missing.process.stdout.is_empty());
    assert_eq!(missing.process.stderr, b"trace summary file IO failed\n");
    assert!(missing.outcome.contains("\"variant\":\"Open\""));

    let usage = Command::new(&executable)
        .output()
        .expect("reject missing application argument");
    assert_eq!(usage.status.code(), Some(1));
    assert!(usage.stdout.is_empty());
    assert_eq!(
        String::from_utf8(usage.stderr).unwrap(),
        "usage: chimi-trace-summary <path-or-dash>\n"
    );

    let extra = Command::new(&executable)
        .args(["one", "two"])
        .output()
        .expect("reject extra application arguments");
    assert_eq!(extra.status.code(), Some(1));
    assert!(extra.stdout.is_empty());
    assert_eq!(
        String::from_utf8(extra.stderr).unwrap(),
        "usage: chimi-trace-summary <path-or-dash>\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn build_reuses_unchanged_artifacts_and_invalidates_only_changed_core_inputs() {
    let scratch = scratch_directory("build-cache");
    let project = scratch.join("project");
    std::fs::create_dir_all(project.join("app")).unwrap();
    std::fs::create_dir_all(project.join("lib")).unwrap();
    std::fs::write(
        project.join("app/main.chimi"),
        "import lib/answer as Answer\n\npub fn main() -> Int {\n  Answer.value()\n}\n",
    )
    .unwrap();
    let answer = project.join("lib/answer.chimi");
    std::fs::write(&answer, "pub fn value() -> Int {\n  42\n}\n").unwrap();
    let executable = scratch.join("cached-app");

    let build = || {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .current_dir(&scratch)
            .args(["build", "--root"])
            .arg(&project)
            .args(["app/main", "-o"])
            .arg(&executable)
            .output()
            .expect("build cache test project");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let cold = build();
    let cold_cache = cold.split("\"cache\":").nth(1).unwrap();
    assert!(cold_cache.starts_with("{\"hits\":[],\"misses\":["));
    assert_eq!(cold_cache.matches("module-").count(), 2);
    assert!(cold_cache.contains("entry.o"));
    assert!(cold_cache.contains("libchimi_rt.a"));
    assert!(cold_cache.contains("program_data.o"));

    let unchanged = build();
    let unchanged_cache = unchanged.split("\"cache\":").nth(1).unwrap();
    assert!(unchanged_cache.ends_with("\"misses\":[]}}\n"));
    assert_eq!(unchanged_cache.matches("module-").count(), 2);
    assert!(unchanged_cache.contains("entry.o"));
    assert!(unchanged_cache.contains("libchimi_rt.a"));
    assert!(unchanged_cache.contains("program_data.o"));

    std::fs::write(&answer, "pub fn value() -> Int {\n  43\n}\n").unwrap();
    let changed = build();
    let changed_cache = changed.split("\"cache\":").nth(1).unwrap();
    let (hits, misses) = changed_cache.split_once("],\"misses\":[").unwrap();
    assert_eq!(hits.matches("module-").count(), 1);
    assert!(hits.contains("entry.o"));
    assert!(hits.contains("libchimi_rt.a"));
    assert_eq!(misses.matches("module-").count(), 1);
    assert!(misses.contains("program_data.o"));
    let execution = execute_native(&executable, &scratch, "cache", "off");
    assert!(execution.process.status.success());
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":43}\n"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn direct_integer_build_preserves_off_boundary_and_full_trace_behavior() {
    let scratch = scratch_directory("direct-integer");
    let executable = scratch.join("integer");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(repository_path(
            "prototypes/native-direct-lowering/integer-arithmetic.chimi",
        ))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build direct integer workload");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let off = execute_native(&executable, &scratch, "direct-off", "off");
    assert!(off.process.status.success());
    assert!(off.telemetry.is_empty());
    assert_eq!(
        off.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":-529}\n"
    );
    let boundaries = execute_native(&executable, &scratch, "direct-boundaries", "boundaries");
    assert_eq!(boundaries.telemetry.lines().count(), 2);
    assert!(
        boundaries
            .telemetry
            .contains("\"event\":\"computation_started\"")
    );
    let full = execute_native(&executable, &scratch, "direct-full", "full");
    assert!(full.telemetry.lines().count() > 200);
    assert!(full.telemetry.contains("\"operation\":\"constant\""));
    assert!(full.telemetry.contains("\"operation\":\"binary\""));
    assert!(full.telemetry.contains("\"operation\":\"unary\""));
    assert_eq!(full.outcome, off.outcome);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn direct_integer_overflow_returns_the_existing_runtime_defect() {
    let scratch = scratch_directory("direct-integer-overflow");
    let executable = scratch.join("overflow");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(repository_path(
            "prototypes/native-direct-lowering/integer-overflow.chimi",
        ))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build direct integer overflow workload");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let execution = execute_native(&executable, &scratch, "overflow", "off");
    assert_eq!(execution.process.status.code(), Some(70));
    assert!(execution.outcome.contains("\"outcome\":\"defect\""));
    assert!(
        execution
            .outcome
            .contains("\"message\":\"integer arithmetic overflow\"")
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn run_executes_the_checkout_forcing_program_with_dependency_traces() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args([
            "run",
            repository_path("prototypes/golden/valid/dependencies_and_execution.chimi")
                .to_str()
                .unwrap(),
        ])
        .output()
        .expect("run the checkout forcing program");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches("\"event\":\"dependency_called\"").count(), 2);
    assert_eq!(
        stdout.lines().last().unwrap(),
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",",
            "\"outcome\":\"success\",\"value\":{\"type\":\"Order\",",
            "\"fields\":{\"id\":{\"type\":\"OrderId\",",
            "\"fields\":{\"value\":\"order-1\"}}}}}"
        )
    );
}

#[test]
fn invalid_programs_stop_at_the_owning_stage() {
    let cases = [
        ("missing_closing_brace", "error[S1001] syntax"),
        ("missing_main", "error[E5001] execution-setup"),
        ("unresolved_constructor", "error[E_UNRESOLVED_VALUE] name"),
        ("invalid_field_type", "error[T4203] type"),
    ];
    for (name, expected) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .args(["check", &fixture(name)])
            .output()
            .expect("run invalid Chimi source");

        assert_eq!(output.status.code(), Some(1), "fixture {name}");
        assert!(output.stdout.is_empty(), "fixture {name}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "fixture {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn structured_check_keeps_compiler_stages_and_source_ownership_explicit() {
    let cases = [
        ("missing_closing_brace", "syntax", "S1001"),
        ("unresolved_constructor", "name", "E_UNRESOLVED_VALUE"),
        ("invalid_field_type", "type", "T4203"),
    ];
    for (fixture_name, stage, code) in cases {
        let path = fixture(fixture_name);
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .args(["check", "--json", &path])
            .output()
            .expect("run structured check");

        assert_eq!(output.status.code(), Some(1), "fixture {fixture_name}");
        assert!(output.stderr.is_empty(), "fixture {fixture_name}");
        let document = String::from_utf8(output.stdout).unwrap();
        assert!(
            document.starts_with("{\"schema\":\"chimi.diagnostics.v1\","),
            "{document}"
        );
        assert!(document.contains(&format!("\"stage\":\"{stage}\"")));
        assert!(document.contains(&format!("\"code\":\"{code}\"")));
        assert!(document.contains("\"severity\":\"error\""));
        assert!(document.contains("\"primary\":{\"module\":\"main\""));
        assert!(document.contains("\"start\":"));
        assert!(document.contains("\"end\":"));
        assert!(document.contains("\"labels\":[]"));
        assert!(document.contains("\"fixes\":[]"));
        assert!(document.ends_with("\n"));
        if fixture_name == "missing_closing_brace" {
            let end = std::fs::read_to_string(&path).unwrap().len();
            assert_eq!(
                document,
                format!(
                    concat!(
                        "{{\"schema\":\"chimi.diagnostics.v1\",\"diagnostics\":[{{",
                        "\"stage\":\"syntax\",\"code\":\"S1001\",",
                        "\"severity\":\"error\",\"message\":\"expected `}}`\",",
                        "\"primary\":{{\"module\":\"main\",\"path\":\"{}\",",
                        "\"start\":{},\"end\":{}}},\"labels\":[],\"fixes\":[]}}]}}\n"
                    ),
                    path, end, end
                )
            );
        }
    }

    let success = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "--json", &fixture("success")])
        .output()
        .expect("run successful structured check");
    assert!(success.status.success());
    assert!(success.stderr.is_empty());
    assert_eq!(
        String::from_utf8(success.stdout).unwrap(),
        "{\"schema\":\"chimi.diagnostics.v1\",\"diagnostics\":[]}\n"
    );
}

#[test]
fn structured_project_diagnostic_names_the_owning_module() {
    let root = repository_path("compiler/chimi/tests/fixtures/project-invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args([
            "check",
            "--json",
            "--root",
            root.to_str().unwrap(),
            "app/main",
        ])
        .output()
        .expect("check an invalid multi-module project as JSON");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let document = String::from_utf8(output.stdout).unwrap();
    assert!(document.contains("\"module\":\"lib/broken\""));
    assert!(document.contains("lib/broken.chimi"));
}

#[test]
fn inspect_explains_resolved_callable_channels_and_executable_origin() {
    let path = repository_path("prototypes/golden/valid/dependencies_and_execution.chimi");
    let source = std::fs::read_to_string(&path).unwrap();
    let offset = source.rfind("submit(").unwrap().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["inspect", path.to_str().unwrap(), &offset])
        .output()
        .expect("inspect the reachable submit call");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let document = String::from_utf8(output.stdout).unwrap();
    assert!(document.starts_with("{\"schema\":\"chimi.inspection.v1\""));
    assert!(document.contains("\"kind\":\"name_reference\""));
    assert!(document.contains("\"text\":\"submit\""));
    assert!(document.contains("\"status\":\"resolved\""));
    assert!(document.contains("\"kind\":\"function\""));
    assert!(document.contains("\"module\":\"main\""));
    assert!(document.contains("\"display\":\"fn(Cart) -> Order errors CheckoutError needs"));
    assert!(document.contains("\"failures\":[\"PaymentFailed\",\"InventoryFailed\"]"));
    assert!(document.contains("\"name\":\"inventory\",\"interface\":\"Inventory\""));
    assert!(!document.contains("\"executable_origin\":null"));
    assert!(document.ends_with("\"diagnostic_count\":0}\n"));
}

#[test]
fn invalid_cli_input_is_rejected_at_the_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "one.chimi", "two.chimi"])
        .output()
        .expect("run invalid chimi check");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        concat!(
            "chimi: check and run received unexpected arguments\n",
            "usage: chimi --version | chimi format <path> | ",
            "chimi inspect <path> <byte-offset> | ",
            "chimi inspect --root <project-dir> <entry-module> <module> <byte-offset> | ",
            "chimi check [--json] <path> | chimi run <path> | ",
            "chimi build <path> [-o <executable>] | ",
            "chimi check [--json] --root <project-dir> <entry-module> | ",
            "chimi run --root <project-dir> <entry-module> | ",
            "chimi build --root <project-dir> <entry-module> ",
            "[-o <executable>]\n"
        )
    );
}

#[test]
fn explicit_project_root_loads_and_executes_imported_modules() {
    let root = repository_path("compiler/chimi/tests/fixtures/project");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["run", "--root", root.to_str().unwrap(), "app/main"])
        .output()
        .expect("run a multi-module Chimi project");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .last()
            .unwrap(),
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":42}"
    );
}

#[test]
fn imported_module_diagnostics_render_against_the_owning_source() {
    let root = repository_path("compiler/chimi/tests/fixtures/project-invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .args(["check", "--root", root.to_str().unwrap(), "app/main"])
        .output()
        .expect("check an invalid multi-module Chimi project");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("lib/broken.chimi"), "{stderr}");
    assert!(stderr.contains("error[T"), "{stderr}");
}

#[test]
fn golden_and_recovery_corpus_reaches_the_real_check_path() {
    let cases = [
        ("prototypes/golden/valid/composition.chimi", false),
        (
            "prototypes/golden/valid/dependencies_and_execution.chimi",
            false,
        ),
        (
            "prototypes/golden/valid/expressions_and_matching.chimi",
            false,
        ),
        ("prototypes/golden/valid/functions_as_values.chimi", false),
        ("prototypes/golden/valid/modules/money.chimi", false),
        ("prototypes/golden/valid/modules/use_money.chimi", false),
        ("prototypes/golden/valid/values_and_variants.chimi", false),
        (
            "prototypes/golden/valid/worker_forcing_example.chimi",
            false,
        ),
        ("prototypes/golden/invalid/ignored_value.chimi", false),
        (
            "prototypes/golden/invalid/interface_shape_mismatch.chimi",
            false,
        ),
        ("prototypes/golden/invalid/missing_dependency.chimi", false),
        ("prototypes/golden/invalid/non_exhaustive_case.chimi", false),
        (
            "prototypes/golden/invalid/non_exhaustive_outcome_case.chimi",
            false,
        ),
        (
            "prototypes/golden/invalid/observable_case_guard.chimi",
            false,
        ),
        (
            "prototypes/golden/invalid/private_struct_construction.chimi",
            false,
        ),
        (
            "prototypes/golden/invalid/private_struct_field_access.chimi",
            false,
        ),
        ("prototypes/golden/invalid/empty_braces.chimi", true),
        (
            "prototypes/golden/invalid/implicit_module_alias.chimi",
            true,
        ),
        ("prototypes/golden/invalid/standalone_if.chimi", true),
        ("prototypes/golden/invalid/void_value.chimi", true),
        (
            "prototypes/lossless-parser/cases/incomplete_pipeline.chimi",
            true,
        ),
        (
            "prototypes/lossless-parser/cases/missing_closing_brace.chimi",
            true,
        ),
        (
            "prototypes/lossless-parser/cases/missing_record_colon.chimi",
            true,
        ),
        (
            "prototypes/lossless-parser/cases/missing_record_value.chimi",
            true,
        ),
    ];

    for (relative_path, expects_syntax_error) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
            .args(["check", repository_path(relative_path).to_str().unwrap()])
            .output()
            .expect("run corpus source through chimi check");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.contains("error[S"),
            expects_syntax_error,
            "fixture {relative_path}: {stderr}"
        );
    }
}

fn build_native(scratch: &Path, name: &str, source_text: &str) -> PathBuf {
    let source = scratch.join(format!("{name}.chimi"));
    let executable = scratch.join(name);
    std::fs::write(&source, source_text).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .args(["build"])
        .arg(&source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .expect("build native loop program");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    executable
}

// Guards: native loops run passes without growing the C stack (recursion overflowed here).
#[test]
fn native_loop_runs_a_million_passes() {
    let scratch = scratch_directory("native-loop-million");
    let executable = build_native(
        &scratch,
        "loop-million",
        concat!(
            "pub fn main() -> Int {\n",
            "  loop count = 0 {\n",
            "    case count {\n",
            "      1000000 -> break count\n",
            "      _ -> count + 1\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "loop-million", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":1000000}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: a loop drives a real channel consumer until Drained, across task parking.
#[test]
fn native_loop_consumes_a_channel_until_drained() {
    let scratch = scratch_directory("native-loop-channel");
    let executable = build_native(
        &scratch,
        "loop-channel",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receiver, Receive}\n",
            "import std/task as Task\n",
            "fn total(jobs: Receiver<Int>) -> Int {\n",
            "  loop sum = 0 {\n",
            "    let next = Channel.receive(jobs)\n",
            "    case next {\n",
            "      Receive.Received(value) -> sum + value\n",
            "      Receive.Drained -> break sum\n",
            "    }\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Int {\n",
            "  let jobs = Channel.bounded(1)\n",
            "  let receiver = jobs.receiver\n",
            "  let sender = jobs.sender\n",
            "  let completion = jobs.completion\n",
            "  let child = Task.start(fn() -> Int { total(receiver) })\n",
            "  Channel.send(sender, 1)\n",
            "  Channel.send(sender, 2)\n",
            "  Channel.send(sender, 3)\n",
            "  Channel.send(sender, 4)\n",
            "  Channel.send(sender, 5)\n",
            "  Channel.close(completion)\n",
            "  Task.result(child)\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "loop-channel", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":15}\n"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: without a scheduler, each pass's garbage is reclaimed, so live memory stays
// bounded by one pass instead of growing with the pass count.
#[test]
fn native_scheduler_free_loop_reclaims_each_pass() {
    let scratch = scratch_directory("native-loop-reclaim");
    let passes = 100_000_u64;
    let executable = build_native(
        &scratch,
        "loop-reclaim",
        &format!(
            concat!(
                "pub fn main() -> Int {{\n",
                "  loop count = 0 {{\n",
                "    case count {{\n",
                "      {passes} -> break count\n",
                "      _ -> count + 1\n",
                "    }}\n",
                "  }}\n",
                "}}\n",
            ),
            passes = passes
        ),
    );
    let memory_report = scratch.join("loop-memory.json");
    let execution = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!("--memory-report-file={}", memory_report.display()))
        .arg("--")
        .output()
        .expect("run native loop with a memory report");
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    let memory_report = std::fs::read_to_string(memory_report).unwrap();
    assert!(json_u64_field(&memory_report, "scoped_collections") >= passes);
    let peak_live = json_u64_field(&memory_report, "scoped_peak_live_bytes");
    assert!(
        peak_live < 16_384,
        "loop kept {peak_live} live bytes across {passes} passes"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn native_list_is_a_persistent_vector() {
    let scratch = scratch_directory("native-list-runtime");
    let executable = scratch.join("native-list-runtime");
    let compilation = Command::new("cc")
        .arg("-std=c11")
        .arg("-O2")
        .arg("-I")
        .arg(repository_path("compiler/chimi/runtime/native"))
        .arg("-I")
        .arg(repository_path("runtime/chimi-fiber/include"))
        .arg(format!(
            "{}/tests/fixtures/native-list-runtime.c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("compile native list runtime test");
    assert!(
        compilation.status.success(),
        "{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("run native list runtime test");
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(
        run.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.ends_with("ok\n"), "{stdout}");
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards: native list literals, decoded JSON arrays, and List.first/length agree
// past the 32-element boundary where the persistent vector leaves its tail.
#[test]
fn native_lists_longer_than_one_vector_leaf_round_trip_through_json() {
    let scratch = scratch_directory("native-long-list");
    let elements = (0..40)
        .map(|index| format!("Json.int({index})"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        concat!(
            "import std/json as Json\n",
            "import std/json.{{Value}}\n",
            "\n",
            "fn summarize(values: List<Value>) -> Text {{\n",
            "  case List.first(values) {{\n",
            "    Some(first) -> Json.encode(Json.array([Json.array(values), first, Json.int(List.length(values))]))\n",
            "    None -> \"empty\"\n",
            "  }}\n",
            "}}\n",
            "\n",
            "pub fn main() -> Text {{\n",
            "  let decoded = Json.decode(Json.encode(Json.array([{elements}])))\n",
            "  case decoded {{\n",
            "    Value.Array(values) -> summarize(values)\n",
            "    _ -> \"unexpected\"\n",
            "  }}\n",
            "}}\n",
        ),
        elements = elements
    );
    let executable = build_native(&scratch, "long-list", &source);
    let execution = execute_native(&executable, &scratch, "long-list", "off");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    let numbers = (0..40).map(|index| index.to_string()).collect::<Vec<_>>();
    assert_eq!(
        execution.outcome,
        format!(
            "{{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":\"[[{}],0,40]\"}}\n",
            numbers.join(",")
        )
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards B5: a skipped `and` operand emits no channel events. Native-only: the
// reference interpreter has no Channel runtime.
#[test]
fn native_skipped_boolean_operand_sends_nothing() {
    let scratch = scratch_directory("native-boolean-short-circuit");
    let executable = build_native(
        &scratch,
        "boolean-short-circuit",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Sender}\n",
            "fn consume(sender: Sender<Int>) -> Bool {\n",
            "  Channel.send(sender, 1)\n",
            "  True\n",
            "}\n",
            "pub fn main() -> Bool {\n",
            "  let channel = Channel.bounded(2)\n",
            "  let skipped = False and consume(channel.sender)\n",
            "  let taken = True and consume(channel.sender)\n",
            "  skipped or not taken\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "boolean-short-circuit", "full");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":false}\n"
    );
    let sends = execution
        .telemetry
        .matches("\"event\":\"channel_send_accepted\"")
        .count();
    assert_eq!(sends, 1, "{}", execution.telemetry);
    std::fs::remove_dir_all(scratch).unwrap();
}

// Guards B8: an operand that parks on Channel.receive waits like a `case` subject,
// then the expression completes. Native-only: the interpreter has no Channel runtime.
#[test]
fn native_boolean_operand_parks_on_receive_then_succeeds() {
    let scratch = scratch_directory("native-boolean-parks");
    let executable = build_native(
        &scratch,
        "boolean-parks",
        concat!(
            "import std/channel as Channel\n",
            "import std/channel.{Receive, Receiver}\n",
            "import std/task as Task\n",
            "fn got_one(receiver: Receiver<Int>) -> Bool {\n",
            "  let next = Channel.receive(receiver)\n",
            "  case next {\n",
            "    Receive.Received(value) -> value < 2 and value > 0\n",
            "    Receive.Drained -> False\n",
            "  }\n",
            "}\n",
            "pub fn main() -> Bool {\n",
            "  let channel = Channel.bounded(0)\n",
            "  let sender = channel.sender\n",
            "  let receiver = channel.receiver\n",
            "  let completion = channel.completion\n",
            "  let child = Task.start(fn() -> Int {\n",
            "    Channel.send(sender, 1)\n",
            "    7\n",
            "  })\n",
            "  let ready = True\n",
            "  let received = ready and got_one(receiver)\n",
            "  let child_value = Task.result(child)\n",
            "  Channel.close(completion)\n",
            "  received and child_value > 0\n",
            "}\n",
        ),
    );
    let execution = execute_native(&executable, &scratch, "boolean-parks", "full");
    assert!(
        execution.process.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.process.stderr)
    );
    assert_eq!(
        execution.outcome,
        "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":true}\n"
    );
    assert!(
        execution
            .telemetry
            .contains("\"event\":\"channel_receive_parked\"")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
