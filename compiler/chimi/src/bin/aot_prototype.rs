//! PROTOTYPE: build and validate the disposable native AOT backend candidate.

use chimi::backend::native_aot_prototype::{NativePrototypeArtifact, build_executable};
use chimi::execution::{ExecutionConfig, TraceMode};
use chimi::source::SourceSnapshot;
use chimi::{BackendProgram, Compiler, ProjectSource};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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
        "{{\"schema\":\"chimi.native-aot-prototype.v1\",\"kind\":\"environment\",\"os\":\"{}\",\"arch\":\"{}\",\"samples\":{}}}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        samples,
    );
    for workload in workloads() {
        if let Err(error) = run_workload(workload, samples) {
            eprintln!("native AOT prototype failed: {error}");
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
    let output_directory = PathBuf::from("target/native-aot-prototype").join(workload.id);
    let artifact = build_executable(&program, &output_directory)?;
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
        if !output.status.success() {
            return Err(format!("{} exited with {}", workload.id, output.status));
        }
    }
    durations.sort_unstable();
    let mut steady_state = Vec::with_capacity(samples);
    for _ in 0..samples {
        steady_state.push(benchmark_execution(
            &artifact,
            workload.id,
            workload.control,
            100,
        )?);
    }
    steady_state.sort_unstable();
    let peak_rss_bytes = peak_rss(&artifact, workload.id, workload.control)?;
    let dependencies = dynamic_dependencies(&artifact.executable_path)?;
    println!(
        concat!(
            "{{\"schema\":\"chimi.native-aot-prototype.v1\",\"kind\":\"workload\",",
            "\"id\":\"{}\",\"files\":{},\"source_bytes\":{},\"core_nodes\":{},",
            "\"object_emission_ns\":{},\"runtime_compilation_ns\":{},\"data_compilation_ns\":{},",
            "\"link_ns\":{},\"cold_start_p50_ns\":{},\"cold_start_p95_ns\":{},",
            "\"steady_state_p50_ns\":{},\"steady_state_p95_ns\":{},\"peak_rss_bytes\":{},",
            "\"object_bytes\":{},\"runtime_archive_bytes\":{},\"executable_bytes\":{},",
            "\"dynamic_dependencies\":{},\"boundary_match\":true,\"full_match\":true}}"
        ),
        workload.id,
        file_count,
        workload.source_bytes,
        program.profile().nodes,
        artifact.object_emission.as_nanos(),
        artifact.runtime_compilation.as_nanos(),
        artifact.data_compilation.as_nanos(),
        artifact.linking.as_nanos(),
        percentile(&durations, 50).as_nanos(),
        percentile(&durations, 95).as_nanos(),
        percentile(&steady_state, 50).as_nanos(),
        percentile(&steady_state, 95).as_nanos(),
        peak_rss_bytes,
        file_size(&artifact.object_path)?,
        file_size(&artifact.runtime_archive_path)?,
        file_size(&artifact.executable_path)?,
        json_string(&dependencies),
    );
    Ok(())
}

fn verify(
    program: &BackendProgram,
    artifact: &NativePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
) -> Result<(), String> {
    let expected = program
        .execute_oracle(config(id, control, trace))
        .render_json_lines();
    let output = execute(artifact, id, control, trace)?;
    let actual = String::from_utf8(output.stdout)
        .map_err(|error| format!("native output was not UTF-8: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{id} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
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
    Ok(())
}

fn execute(
    artifact: &NativePrototypeArtifact,
    id: &str,
    control: InitialControl,
    trace: TraceMode,
) -> Result<Output, String> {
    let mut command = Command::new(&artifact.executable_path);
    command.arg(match trace {
        TraceMode::Boundaries => "--trace=boundaries",
        TraceMode::Full => "--trace=full",
    });
    command.arg(format!("--execution-id={id}"));
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
    command
        .output()
        .map_err(|error| format!("run {id}: {error}"))
}

fn benchmark_execution(
    artifact: &NativePrototypeArtifact,
    id: &str,
    control: InitialControl,
    repeats: usize,
) -> Result<Duration, String> {
    let mut command = Command::new(&artifact.executable_path);
    command.arg(format!("--execution-id={id}"));
    command.arg(format!("--benchmark-repeats={repeats}"));
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
    let output = command
        .output()
        .map_err(|error| format!("benchmark {id}: {error}"))?;
    if !output.status.success() {
        return Err(format!("benchmark {id} exited with {}", output.status));
    }
    let nanoseconds = String::from_utf8(output.stdout)
        .map_err(|error| format!("benchmark output was not UTF-8: {error}"))?
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("benchmark output was invalid: {error}"))?;
    Ok(Duration::from_nanos(nanoseconds))
}

fn peak_rss(
    artifact: &NativePrototypeArtifact,
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
        .arg("--trace=boundaries")
        .arg(format!("--execution-id={id}"));
    if matches!(control, InitialControl::Cancel) {
        command.arg("--cancel");
    }
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
