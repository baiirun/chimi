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
    assert!(report.contains("\"dynamic_dependencies\":["));
    assert!(executable.is_file());

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
fn native_chimi_code_classifies_and_aggregates_trace_records() {
    let scratch = scratch_directory("trace-summary-core");
    let executable = scratch.join("trace-summary-core");
    let root = repository_path("compiler/chimi/tests/fixtures/project-trace-summary");
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .args(["build", "--root"])
        .arg(&root)
        .args(["app/main", "-o"])
        .arg(&executable)
        .output()
        .expect("build the pure Chimi trace-summary core");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let execution = execute_native(&executable, &scratch, "trace-summary", "off");
    assert!(execution.process.status.success());
    assert!(execution.process.stderr.is_empty());
    assert_eq!(
        execution.outcome,
        concat!(
            "{\"schema\":\"chimi.runtime.v1\",",
            "\"outcome\":\"success\",\"value\":\"",
            "{\\\"dependency_calls\\\":{\\\"payment.charge\\\":1},",
            "\\\"events\\\":2,\\\"operations\\\":{\\\"call\\\":1},",
            "\\\"records\\\":3,\\\"schema\\\":\\\"chimi.trace-summary.v1\\\",",
            "\\\"terminal_outcomes\\\":{\\\"success\\\":1}}",
            "\"}\n",
        )
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
