//! Locating a fixture, building and running it natively, and comparing its
//! expected files.

use super::Options;
use super::catalogue::Scenario;
use crate::Compiler;
use crate::backend::native::build_application;
use crate::project::LoadedProject;
use std::path::{Path, PathBuf};
use std::process::Command;

const OUTCOME: &str = "expected.outcome";
const TRANSCRIPT: &str = "expected.transcript";

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

/// The memory report of one run, as `--memory-report-file` wrote it.
pub(super) struct Run {
    pub(super) memory_report: String,
}

/// Builds the fixture with the native backend and runs it once per entry of
/// `scenario.arguments` (once with no arguments when empty), with
/// `--trace=off` and a memory report. Each run must exit with
/// `scenario.exit_code`, and the outcome lines must equal `expected.outcome`
/// byte for byte, one line per run.
///
/// Transcript fixtures and defect outcomes are compared from U5, which lands
/// the process trace family and `--clock=virtual`; until then a fixture that
/// has either fails rather than passing on a partial comparison.
pub(super) fn execute(scenario: &Scenario, options: &Options) -> Result<Vec<Run>, String> {
    if root(scenario).join(TRANSCRIPT).exists() {
        return Err(format!(
            "{TRANSCRIPT} comparison lands with U5 (it needs --clock=virtual)"
        ));
    }
    let expected = expected_lines(scenario, OUTCOME)?;
    if expected
        .iter()
        .any(|line| line.contains("\"outcome\":\"defect\""))
    {
        return Err(format!("{OUTCOME}: defect comparison lands with U5"));
    }
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
        let outcome_path = scratch.path().join(format!("outcome-{index}.jsonl"));
        let memory_path = scratch.path().join(format!("memory-{index}.json"));
        let mut command = Command::new(&executable);
        command
            .arg("--chimi-runner")
            .arg("--trace=off")
            .arg(format!("--outcome-file={}", outcome_path.display()))
            .arg(format!("--memory-report-file={}", memory_path.display()))
            .arg("--")
            .args(argument);
        let output = command
            .output()
            .map_err(|error| format!("run {}: {error}", executable.display()))?;
        let run_name = argument.unwrap_or("<no arguments>");
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
        outcomes.extend(read(&outcome_path)?.lines().map(str::to_owned));
        runs.push(Run {
            memory_report: read(&memory_path)?,
        });
    }
    if options.print_actual {
        print_actual(scenario, OUTCOME, &expected, &outcomes);
    }
    compare(OUTCOME, &expected, &outcomes)?;
    Ok(runs)
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
