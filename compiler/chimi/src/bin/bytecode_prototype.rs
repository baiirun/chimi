//! PROTOTYPE: build and validate the disposable embedded-bytecode backend candidate.

use chimi::backend::embedded_bytecode_prototype::{BytecodePrototypeArtifact, build_executable};
use chimi::execution::{ExecutionConfig, TraceMode};
use chimi::source::SourceSnapshot;
use chimi::{BackendProgram, Compiler, ProjectSource};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum InitialControl {
    Run,
    Cancel,
}

struct Workload {
    id: &'static str,
    sources: Vec<ProjectSource>,
    source_bytes: usize,
    control: InitialControl,
}

fn main() {
    let samples = arguments();
    println!(
        "{{\"schema\":\"chimi.embedded-bytecode-prototype.v1\",\"kind\":\"environment\",\"os\":\"{}\",\"arch\":\"{}\",\"samples\":{}}}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        samples,
    );
    for workload in workloads() {
        if let Err(error) = run_workload(workload, samples) {
            eprintln!("embedded bytecode prototype failed: {error}");
            std::process::exit(1);
        }
    }
}

fn arguments() -> usize {
    let mut samples = 20;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--samples" {
            samples = arguments
                .next()
                .expect("--samples requires a value")
                .parse()
                .expect("--samples requires an integer");
        } else {
            panic!("unknown argument: {argument}");
        }
    }
    assert!(samples >= 2, "--samples must be at least 2");
    samples
}

fn run_workload(workload: Workload, samples: usize) -> Result<(), String> {
    let file_count = workload.sources.len();
    let program = Compiler::prepare_backend(workload.sources)
        .map_err(|diagnostics| format!("{} did not prepare: {diagnostics:#?}", workload.id))?;
    let output_directory = PathBuf::from("target/embedded-bytecode-prototype").join(workload.id);
    let artifact = build_executable(&program, &output_directory)?;
    verify(
        &program,
        &artifact,
        workload.id,
        workload.control,
        TraceMode::Off,
    )?;
    verify(
        &program,
        &artifact,
        workload.id,
        workload.control,
        TraceMode::Boundaries,
    )?;
    verify(
        &program,
        &artifact,
        workload.id,
        workload.control,
        TraceMode::Full,
    )?;

    for _ in 0..3 {
        let _ = execute(
            &artifact,
            workload.id,
            workload.control,
            TraceMode::Boundaries,
        )?;
    }
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let output = execute(
            &artifact,
            workload.id,
            workload.control,
            TraceMode::Boundaries,
        )?;
        durations.push(started.elapsed());
        if !output.process.status.success() {
            return Err(format!(
                "{} exited with {}",
                workload.id, output.process.status
            ));
        }
    }
    durations.sort_unstable();
    let steady_off = benchmark_trace_mode(
        &artifact,
        workload.id,
        workload.control,
        TraceMode::Off,
        samples,
    )?;
    let steady_boundaries = benchmark_trace_mode(
        &artifact,
        workload.id,
        workload.control,
        TraceMode::Boundaries,
        samples,
    )?;
    let steady_full = benchmark_trace_mode(
        &artifact,
        workload.id,
        workload.control,
        TraceMode::Full,
        samples,
    )?;
    let peak_rss_bytes = peak_rss(&artifact, workload.id, workload.control)?;
    let dependencies = dynamic_dependencies(&artifact.executable_path)?;
    println!(
        concat!(
            "{{\"schema\":\"chimi.embedded-bytecode-prototype.v1\",\"kind\":\"workload\",",
            "\"id\":\"{}\",\"files\":{},\"source_bytes\":{},\"core_nodes\":{},",
            "\"image_encoding_ns\":{},\"runtime_compilation_ns\":{},\"image_compilation_ns\":{},",
            "\"link_ns\":{},\"cold_start_p50_ns\":{},\"cold_start_p95_ns\":{},",
            "\"steady_state_p50_ns\":{},\"steady_state_p95_ns\":{},",
            "\"boundary_steady_state_p50_ns\":{},\"boundary_steady_state_p95_ns\":{},",
            "\"full_steady_state_p50_ns\":{},\"full_steady_state_p95_ns\":{},\"peak_rss_bytes\":{},",
            "\"image_source_bytes\":{},\"image_object_bytes\":{},\"runtime_archive_bytes\":{},",
            "\"executable_bytes\":{},",
            "\"dynamic_dependencies\":{},\"off_match\":true,\"boundary_match\":true,\"full_match\":true}}"
        ),
        workload.id,
        file_count,
        workload.source_bytes,
        program.profile().nodes,
        artifact.image_encoding.as_nanos(),
        artifact.runtime_compilation.as_nanos(),
        artifact.image_compilation.as_nanos(),
        artifact.linking.as_nanos(),
        percentile(&durations, 50).as_nanos(),
        percentile(&durations, 95).as_nanos(),
        percentile(&steady_off, 50).as_nanos(),
        percentile(&steady_off, 95).as_nanos(),
        percentile(&steady_boundaries, 50).as_nanos(),
        percentile(&steady_boundaries, 95).as_nanos(),
        percentile(&steady_full, 50).as_nanos(),
        percentile(&steady_full, 95).as_nanos(),
        peak_rss_bytes,
        file_size(&artifact.image_source_path)?,
        file_size(&artifact.image_object_path)?,
        file_size(&artifact.runtime_archive_path)?,
        file_size(&artifact.executable_path)?,
        json_string(&dependencies),
    );
    Ok(())
}

fn verify(
    program: &BackendProgram,
    artifact: &BytecodePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
) -> Result<(), String> {
    let report = program.execute_oracle(config(id, control, trace));
    let expected_telemetry = report.render_telemetry_json_lines();
    let expected = report.render_json_lines();
    let output = execute(artifact, id, control, trace)?;
    let actual = output.exported;
    if !output.process.status.success() {
        return Err(format!(
            "{id} exited with {}: {}",
            output.process.status,
            String::from_utf8_lossy(&output.process.stderr)
        ));
    }
    if actual != expected {
        fs::write(
            artifact.executable_path.with_extension("expected.jsonl"),
            &expected,
        )
        .map_err(|e| e.to_string())?;
        fs::write(
            artifact.executable_path.with_extension("actual.jsonl"),
            &actual,
        )
        .map_err(|e| e.to_string())?;
        return Err(format!(
            "{id} {:?} trace differed from the oracle; wrote expected/actual JSONL beside the executable",
            trace
        ));
    }
    verify_benchmark_export(artifact, id, control, trace, &expected_telemetry)?;
    Ok(())
}

fn verify_benchmark_export(
    artifact: &BytecodePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
    oracle_telemetry: &str,
) -> Result<(), String> {
    let telemetry = artifact
        .executable_path
        .with_extension(format!("{id}-{trace:?}-benchmark.jsonl"));
    let _ = fs::remove_file(&telemetry);
    let mut command = Command::new(&artifact.executable_path);
    command
        .arg("--chimi-runner")
        .arg(trace_argument(trace))
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--execution-id={id}"))
        .arg("--benchmark-repeats=1");
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
    command.arg("--");
    let output = command
        .output()
        .map_err(|error| format!("verify {id} {trace:?} benchmark export: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{id} {trace:?} benchmark export exited with {}",
            output.status
        ));
    }
    let actual_telemetry = fs::read_to_string(&telemetry)
        .map_err(|error| format!("read benchmark telemetry: {error}"))?;
    let _ = fs::remove_file(&telemetry);
    if actual_telemetry != oracle_telemetry {
        return Err(format!(
            "{id} {trace:?} benchmark did not use the canonical event export path"
        ));
    }
    String::from_utf8(output.stderr)
        .map_err(|error| format!("benchmark timing was not UTF-8: {error}"))?
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("benchmark timing was invalid: {error}"))?;
    Ok(())
}

struct PrototypeExecution {
    process: Output,
    exported: String,
}

fn execute(
    artifact: &BytecodePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
) -> Result<PrototypeExecution, String> {
    let telemetry = artifact
        .executable_path
        .with_extension(format!("{id}-{trace:?}-telemetry.jsonl"));
    let outcome = artifact
        .executable_path
        .with_extension(format!("{id}-{trace:?}-outcome.jsonl"));
    let _ = fs::remove_file(&telemetry);
    let _ = fs::remove_file(&outcome);
    let mut command = Command::new(&artifact.executable_path);
    command
        .arg("--chimi-runner")
        .arg(trace_argument(trace))
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .arg(format!("--execution-id={id}"));
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
    command.arg("--");
    let process = command
        .output()
        .map_err(|error| format!("run {id}: {error}"))?;
    let mut exported =
        fs::read_to_string(&telemetry).map_err(|error| format!("read telemetry: {error}"))?;
    exported
        .push_str(&fs::read_to_string(&outcome).map_err(|error| format!("read outcome: {error}"))?);
    let _ = fs::remove_file(telemetry);
    let _ = fs::remove_file(outcome);
    Ok(PrototypeExecution { process, exported })
}

fn benchmark_execution(
    artifact: &BytecodePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
    repeats: usize,
) -> Result<Duration, String> {
    let mut command = Command::new(&artifact.executable_path);
    command
        .arg("--chimi-runner")
        .arg(format!("--execution-id={id}"))
        .arg(trace_argument(trace))
        .arg("--telemetry-file=/dev/null")
        .arg(format!("--benchmark-repeats={repeats}"));
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
    command.arg("--");
    let output = command
        .stdout(Stdio::null())
        .output()
        .map_err(|error| format!("benchmark {id}: {error}"))?;
    if !output.status.success() {
        return Err(format!("benchmark {id} exited with {}", output.status));
    }
    let nanoseconds = String::from_utf8(output.stderr)
        .map_err(|error| format!("benchmark output was not UTF-8: {error}"))?
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("benchmark output was invalid: {error}"))?;
    Ok(Duration::from_nanos(nanoseconds))
}

fn benchmark_trace_mode(
    artifact: &BytecodePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
    samples: usize,
) -> Result<Vec<Duration>, String> {
    for _ in 0..3 {
        let _ = benchmark_execution(artifact, id, control, trace, 100)?;
    }
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        durations.push(benchmark_execution(artifact, id, control, trace, 100)?);
    }
    durations.sort_unstable();
    Ok(durations)
}

const fn trace_argument(trace: TraceMode) -> &'static str {
    match trace {
        TraceMode::Off => "--trace=off",
        TraceMode::Boundaries => "--trace=boundaries",
        TraceMode::Full => "--trace=full",
    }
}

fn peak_rss(
    artifact: &BytecodePrototypeArtifact,
    id: &str,
    control: InitialControl,
) -> Result<u64, String> {
    let mut command = Command::new("/usr/bin/time");
    if cfg!(target_os = "macos") {
        command.arg("-l");
    } else {
        command.arg("-v");
    }
    command
        .arg(&artifact.executable_path)
        .arg("--chimi-runner")
        .arg("--trace=boundaries")
        .arg("--telemetry-file=/dev/null")
        .arg(format!("--execution-id={id}"));
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
    command.arg("--");
    let output = command
        .output()
        .map_err(|error| format!("measure peak RSS for {id}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "peak RSS run for {id} exited with {}",
            output.status
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if cfg!(target_os = "macos") {
        stderr
            .lines()
            .find(|line| line.contains("maximum resident set size"))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("could not parse peak RSS from: {stderr}"))
    } else {
        stderr
            .lines()
            .find(|line| line.contains("Maximum resident set size"))
            .and_then(|line| line.rsplit(':').next())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|kilobytes| kilobytes * 1024)
            .ok_or_else(|| format!("could not parse peak RSS from: {stderr}"))
    }
}

fn config(id: &str, control: InitialControl, trace: TraceMode) -> ExecutionConfig {
    let config = ExecutionConfig::new(id)
        .expect("workload IDs are valid")
        .with_trace_mode(trace);
    if matches!(control, InitialControl::Cancel) {
        config.with_cancellation_requested()
    } else {
        config
    }
}

fn percentile(values: &[Duration], percentile: usize) -> Duration {
    values[(values.len() - 1) * percentile / 100]
}

fn file_size(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("inspect {}: {e}", path.display()))
}

fn dynamic_dependencies(executable: &Path) -> Result<String, String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("otool").arg("-L").arg(executable).output()
    } else {
        Command::new("ldd").arg(executable).output()
    }
    .map_err(|error| format!("inspect dynamic dependencies: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn source(module: &str, path: &str, text: &str) -> ProjectSource {
    ProjectSource::new(module, SourceSnapshot::from_text(path, text))
        .expect("prototype module path is valid")
}

fn workloads() -> Vec<Workload> {
    let rich = include_str!("../../../../prototypes/backend-comparison/rich-success.chimi");
    let checkout =
        include_str!("../../../../prototypes/golden/valid/dependencies_and_execution.chimi");
    let retry = include_str!("../../../../prototypes/retry-composition/lib/retry.chimi");
    let retry_main = include_str!("../../../../prototypes/retry-composition/app/main.chimi");
    let defect = include_str!("../../../../prototypes/backend-comparison/defect.chimi");
    let cancellation = include_str!("../../../../prototypes/backend-comparison/cancellation.chimi");
    vec![
        Workload {
            id: "rich-success",
            sources: vec![source(
                "comparison/rich",
                "prototypes/backend-comparison/rich-success.chimi",
                rich,
            )],
            source_bytes: rich.len(),
            control: InitialControl::Run,
        },
        Workload {
            id: "checkout-success",
            sources: vec![source(
                "checkout/main",
                "dependencies_and_execution.chimi",
                checkout,
            )],
            source_bytes: checkout.len(),
            control: InitialControl::Run,
        },
        Workload {
            id: "retry-exhaustion",
            sources: vec![
                source(
                    "lib/retry",
                    "prototypes/retry-composition/lib/retry.chimi",
                    retry,
                ),
                source(
                    "app/main",
                    "prototypes/retry-composition/app/main.chimi",
                    retry_main,
                ),
            ],
            source_bytes: retry.len() + retry_main.len(),
            control: InitialControl::Run,
        },
        Workload {
            id: "defect",
            sources: vec![source(
                "comparison/defect",
                "prototypes/backend-comparison/defect.chimi",
                defect,
            )],
            source_bytes: defect.len(),
            control: InitialControl::Run,
        },
        Workload {
            id: "cancellation",
            sources: vec![source(
                "comparison/cancellation",
                "prototypes/backend-comparison/cancellation.chimi",
                cancellation,
            )],
            source_bytes: cancellation.len(),
            control: InitialControl::Cancel,
        },
    ]
}
