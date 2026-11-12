//! Locating a fixture, building and running it natively, and comparing its
//! expected files.

use super::Options;
use super::catalogue::Scenario;
use crate::Compiler;
use crate::backend::native::build_application;
use crate::json::{JsonData, decode_json};
use crate::project::LoadedProject;
use std::path::{Path, PathBuf};
use std::process::Command;

const OUTCOME: &str = "expected.outcome";
const TRANSCRIPT: &str = "expected.transcript";
const MEMORY: &str = "expected.memory";

/// The project root of a fixture scenario.
pub(super) fn root(scenario: &Scenario) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if scenario.id == "FP01" {
        return manifest.join("../../programs/supervised-pool");
    }
    manifest
        .join("tests/process-lab")
        .join(format!("{}-{}", scenario.id, scenario.slug))
}

/// The lines of an expected file. A missing file fails the scenario: the lab
/// never writes expected files.
pub(super) fn expected_lines(scenario: &Scenario, file: &str) -> Result<Vec<String>, String> {
    let path = root(scenario).join(file);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(text.lines().map(str::to_owned).collect())
}

/// `Ok` when the lists are equal, otherwise the first differing line.
pub(super) fn compare(file: &str, expected: &[String], actual: &[String]) -> Result<(), String> {
    let absent = "<absent>";
    for index in 0..expected.len().max(actual.len()) {
        let left = expected.get(index).map_or(absent, String::as_str);
        let right = actual.get(index).map_or(absent, String::as_str);
        if left != right {
            return Err(format!(
                "{file} line {}: expected `{left}`, actual `{right}`",
                index + 1
            ));
        }
    }
    Ok(())
}

pub(super) fn print_actual(
    scenario: &Scenario,
    file: &str,
    expected: &[String],
    actual: &[String],
) {
    eprintln!("== {} {file} expected", scenario.id);
    for line in expected {
        eprintln!("{line}");
    }
    eprintln!("== {} {file} actual", scenario.id);
    for line in actual {
        eprintln!("{line}");
    }
}

/// A directory for one scenario's build and runs, removed when dropped.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    pub(super) fn new(scenario: &Scenario) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "chimi-process-lab-{}-{}",
            std::process::id(),
            scenario.id
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("create {}: {error}", path.display()))?;
        Ok(Self(path))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The reports of one run, as `--memory-report-file` and `--telemetry-file`
/// wrote them.
pub(super) struct Run {
    pub(super) memory_report: String,
    /// The `--trace=full` telemetry of a transcript fixture's run; empty for
    /// a fixture without `expected.transcript`.
    pub(super) full_telemetry: String,
}

/// One trace mode of a run (lab specification, "`expected.transcript`").
#[derive(Clone, Copy, Eq, PartialEq)]
enum Trace {
    Off,
    Boundaries,
    Full,
}

impl Trace {
    fn flag(self) -> &'static str {
        match self {
            Self::Off => "--trace=off",
            Self::Boundaries => "--trace=boundaries",
            Self::Full => "--trace=full",
        }
    }
}

/// What one run of the executable wrote.
struct Output {
    outcome: String,
    memory_report: String,
    telemetry: String,
}

/// Builds the fixture with the native backend and runs it once per entry of
/// `scenario.arguments` (once with no arguments when empty), under
/// `--clock=virtual`. Each run must exit with `scenario.exit_code`.
///
/// - The `--trace=off` run writes the outcome and the memory report, and its
///   telemetry file must stay empty. The outcome lines must match
///   `expected.outcome` (`outcome_matches`), one line per run.
/// - With `expected.transcript`, the same run under `--trace=full` must
///   project to it, and under `--trace=boundaries` to it without its
///   `process_send` lines.
pub(super) fn execute(scenario: &Scenario, options: &Options) -> Result<Vec<Run>, String> {
    let expected = expected_lines(scenario, OUTCOME)?;
    let transcript = root(scenario)
        .join(TRANSCRIPT)
        .exists()
        .then(|| expected_lines(scenario, TRANSCRIPT))
        .transpose()?;
    let scratch = Scratch::new(scenario)?;
    let executable = build(scenario, scratch.path())?;
    let arguments: Vec<Option<&str>> = if scenario.arguments.is_empty() {
        vec![None]
    } else {
        scenario.arguments.iter().copied().map(Some).collect()
    };
    let mut outcomes = Vec::with_capacity(arguments.len());
    let mut runs = Vec::with_capacity(arguments.len());
    for (index, argument) in arguments.into_iter().enumerate() {
        let run = |trace: Trace| {
            run_once(
                scenario,
                &executable,
                scratch.path(),
                &format!("{index}-{}", &trace.flag()[8..]),
                trace,
                argument,
            )
        };
        let off = run(Trace::Off)?;
        if !off.telemetry.is_empty() {
            return Err(format!(
                "run `{}` wrote telemetry under --trace=off: {}",
                argument.unwrap_or("<no arguments>"),
                off.telemetry.lines().next().unwrap_or_default()
            ));
        }
        outcomes.extend(off.outcome.lines().map(str::to_owned));
        let mut full_telemetry = String::new();
        if let Some(transcript) = &transcript {
            let full = run(Trace::Full)?;
            let actual = super::transcript::project(&full.telemetry, scenario.extra_events)
                .map_err(|error| format!("--trace=full: {error}"))?;
            if options.print_actual {
                print_actual(scenario, TRANSCRIPT, transcript, &actual);
            }
            compare(TRANSCRIPT, transcript, &actual)?;
            let boundaries = run(Trace::Boundaries)?;
            let actual = super::transcript::project(&boundaries.telemetry, scenario.extra_events)
                .map_err(|error| format!("--trace=boundaries: {error}"))?;
            compare(
                &format!("{TRANSCRIPT} (boundaries)"),
                &super::transcript::boundaries(transcript),
                &actual,
            )?;
            full_telemetry = full.telemetry;
        }
        runs.push(Run {
            memory_report: off.memory_report,
            full_telemetry,
        });
    }
    if options.print_actual {
        print_actual(scenario, OUTCOME, &expected, &outcomes);
    }
    compare_outcomes(&expected, &outcomes)?;
    Ok(runs)
}

/// Runs the executable once with `trace`, in `scratch` files named by `name`.
fn run_once(
    scenario: &Scenario,
    executable: &Path,
    scratch: &Path,
    name: &str,
    trace: Trace,
    argument: Option<&str>,
) -> Result<Output, String> {
    let outcome_path = scratch.join(format!("outcome-{name}.jsonl"));
    let memory_path = scratch.join(format!("memory-{name}.json"));
    let telemetry_path = scratch.join(format!("telemetry-{name}.jsonl"));
    let output = Command::new(executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .arg(trace.flag())
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--memory-report-file={}", memory_path.display()))
        .arg("--")
        .args(argument)
        .output()
        .map_err(|error| format!("run {}: {error}", executable.display()))?;
    let run_name = format!("{} {}", argument.unwrap_or("<no arguments>"), trace.flag());
    if output.status.code() != Some(scenario.exit_code) {
        return Err(format!(
            "run `{run_name}` exited with {}, expected {}: {}",
            output.status,
            scenario.exit_code,
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map_err(|error| format!("run `{run_name}`: read {}: {error}", path.display()))
    };
    Ok(Output {
        outcome: read(&outcome_path)?,
        memory_report: read(&memory_path)?,
        telemetry: read(&telemetry_path)?,
    })
}

/// `Ok` when every outcome line matches its expected line: byte for byte, or,
/// for an expected defect, in its `outcome` and `message` fields only.
pub(super) fn compare_outcomes(expected: &[String], actual: &[String]) -> Result<(), String> {
    let actual = expected
        .iter()
        .zip(actual)
        .map(|(expected, actual)| match defect_fields(expected) {
            Some(_) => defect_fields(actual).unwrap_or_else(|| actual.clone()),
            None => actual.clone(),
        })
        .chain(actual.iter().skip(expected.len()).cloned())
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|line| defect_fields(line).unwrap_or_else(|| line.clone()))
        .collect::<Vec<_>>();
    compare(OUTCOME, &expected, &actual)
}

/// A defect outcome line reduced to its compared fields, rendered as
/// `{"outcome":"defect","message":...}`; `None` for any other line.
fn defect_fields(line: &str) -> Option<String> {
    let JsonData::Object(fields) = decode_json(line).ok()? else {
        return None;
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    if field("outcome") != Some(&JsonData::Text("defect".into())) {
        return None;
    }
    let mut rendered = String::from("{\"outcome\":\"defect\",\"message\":");
    match field("message") {
        Some(JsonData::Text(message)) => crate::json::write_json_string(&mut rendered, message),
        _ => rendered.push_str("null"),
    }
    rendered.push('}');
    Some(rendered)
}

/// IP01: runs the fixture on the reference interpreter, which has no process
/// runtime, and compares its outcome as `execute` does. The exit status a
/// native runner would use for the outcome must be `scenario.exit_code`.
pub(super) fn interpret(scenario: &Scenario, options: &Options) -> Result<(), String> {
    let expected = expected_lines(scenario, OUTCOME)?;
    let project = LoadedProject::load(root(scenario), "app/main")
        .map_err(|error| format!("load fixture: {error}"))?;
    let program = Compiler::prepare_backend(project.sources()).map_err(|diagnostics| {
        format!(
            "fixture does not compile: {}",
            project.render_diagnostics(&diagnostics).trim_end()
        )
    })?;
    let report = program.execute_oracle(crate::execution::ExecutionConfig::for_process());
    let exit_code = match report.outcome {
        crate::execution::ExecutionOutcome::Success(_) => 0,
        crate::execution::ExecutionOutcome::ExpectedFailure(_) => 1,
        crate::execution::ExecutionOutcome::Defect(_) => 70,
        crate::execution::ExecutionOutcome::Cancellation(_) => 130,
    };
    let outcomes = vec![report.render_outcome_json_line().trim_end().to_owned()];
    if options.print_actual {
        print_actual(scenario, OUTCOME, &expected, &outcomes);
    }
    if exit_code != scenario.exit_code {
        return Err(format!(
            "the interpreter's outcome exits with {exit_code}, expected {}",
            scenario.exit_code
        ));
    }
    compare_outcomes(&expected, &outcomes)
}

/// Runs a fixture (`execute`), then checks its `expected.memory` against the
/// runs' memory reports when the scenario has one.
pub(super) fn check(scenario: &Scenario, options: &Options) -> Result<Vec<Run>, String> {
    let runs = execute(scenario, options)?;
    if root(scenario).join(MEMORY).exists() {
        check_memory(scenario, &runs, options)?;
    }
    Ok(runs)
}

/// Checks `expected.memory`. Each line names a top-level integer field of the
/// memory report:
/// - `equal <field>`: the field has the same value in every run;
/// - `at_most <field> <limit>`: the field is at most `limit` in every run;
/// - `exactly <field> <value>`: the field is `value` in every run.
fn check_memory(scenario: &Scenario, runs: &[Run], options: &Options) -> Result<(), String> {
    let lines = expected_lines(scenario, MEMORY)?;
    let reports = runs
        .iter()
        .map(|run| match decode_json(&run.memory_report) {
            Ok(JsonData::Object(fields)) => Ok(fields),
            _ => Err(format!(
                "memory report is not a JSON object: {}",
                run.memory_report
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if options.print_actual {
        eprintln!("== {} memory reports", scenario.id);
        for run in runs {
            eprint!("{}", run.memory_report);
        }
    }
    for (index, line) in lines.iter().enumerate() {
        let malformed = || {
            format!(
                "{MEMORY} line {}: `{line}` is not `equal <field>`, `at_most <field> <limit>`, or `exactly <field> <value>`",
                index + 1
            )
        };
        let (rule, field, bound) = match line.split(' ').collect::<Vec<_>>().as_slice() {
            ["equal", field] => ("equal", *field, None),
            [rule @ ("at_most" | "exactly"), field, bound] => (
                *rule,
                *field,
                Some(bound.parse::<u64>().map_err(|_| malformed())?),
            ),
            _ => return Err(malformed()),
        };
        let values = reports
            .iter()
            .map(|fields| {
                fields
                    .iter()
                    .find(|(key, _)| key == field)
                    .and_then(|(_, value)| match value {
                        JsonData::Number(text) => text.parse::<u64>().ok(),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!("{MEMORY} line {}: no integer field `{field}`", index + 1)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let runs = || {
            let names = if scenario.arguments.is_empty() {
                &["<no arguments>"][..]
            } else {
                scenario.arguments
            };
            names
                .iter()
                .zip(&values)
                .map(|(argument, value)| format!("{argument}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let failed = match (rule, bound) {
            ("equal", _) => values.windows(2).any(|pair| pair[0] != pair[1]),
            ("at_most", Some(limit)) => values.iter().any(|value| *value > limit),
            (_, Some(expected)) => values.iter().any(|value| *value != expected),
            _ => false,
        };
        if failed {
            let claim = match (rule, bound) {
                ("equal", _) => "differs".to_owned(),
                ("at_most", Some(limit)) => format!("exceeds {limit}"),
                (_, bound) => format!("is not {}", bound.unwrap_or_default()),
            };
            return Err(format!(
                "{MEMORY} line {}: {field} {claim} ({})",
                index + 1,
                runs()
            ));
        }
    }
    Ok(())
}

/// The native executable of the fixture, built into `scratch`.
fn build(scenario: &Scenario, scratch: &Path) -> Result<PathBuf, String> {
    let project = LoadedProject::load(root(scenario), "app/main")
        .map_err(|error| format!("load fixture: {error}"))?;
    let program = Compiler::prepare_backend(project.sources()).map_err(|diagnostics| {
        format!(
            "fixture does not compile: {}",
            project.render_diagnostics(&diagnostics).trim_end()
        )
    })?;
    let executable = scratch.join("program");
    build_application(&program, &executable, &scratch.join("artifacts"))
        .map_err(|error| format!("native build failed: {error}"))?;
    Ok(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    // Rejects a comparison that ignores a missing or extra trailing line.
    #[test]
    fn compare_reports_the_first_differing_line_including_length_differences() {
        assert!(compare("f", &lines(&["a", "b"]), &lines(&["a", "b"])).is_ok());
        assert_eq!(
            compare("f", &lines(&["a", "b"]), &lines(&["a", "c"])).unwrap_err(),
            "f line 2: expected `b`, actual `c`"
        );
        assert_eq!(
            compare("f", &lines(&["a"]), &lines(&["a", "b"])).unwrap_err(),
            "f line 2: expected `<absent>`, actual `b`"
        );
        assert!(compare("f", &lines(&["a", "b"]), &lines(&["a"])).is_err());
    }
}
