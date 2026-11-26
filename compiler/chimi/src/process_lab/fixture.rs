//! Locating a fixture, building and running it natively, and comparing its
//! expected files.

use super::Options;
use super::catalogue::Scenario;
use crate::Compiler;
use crate::backend::native::build_application;
use crate::json::{JsonData, decode_json};
use crate::project::{LoadedProject, StandardTree};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const OUTCOME: &str = "expected.outcome";
const TRANSCRIPT: &str = "expected.transcript";
const MEMORY: &str = "expected.memory";
const EVENTS: &str = "expected.events";
/// The fields that split an event kind into the classes `expected.events`
/// counts.
const CLASS_FIELDS: [&str; 4] = ["cause", "reason", "outcome", "attempts_in_window"];
/// The longest one fixture or C harness run may take (see
/// `wait_within_run_limit`).
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// The project root of a fixture scenario.
pub(super) fn root(scenario: &Scenario) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match scenario.id {
        "FP01" => return manifest.join("../../programs/supervised-pool"),
        "FP02" => return manifest.join("../../programs/service-pool"),
        _ => {}
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
        Self::named(scenario.id)
    }

    /// A scratch directory for `name`, unique to this lab process.
    pub(super) fn named(name: &str) -> Result<Self, String> {
        let path =
            std::env::temp_dir().join(format!("chimi-process-lab-{}-{name}", std::process::id()));
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

/// Waits for a lab child process. Fixtures and C harnesses run under virtual
/// time, so none needs long: a run past `RUN_LIMIT` is a runtime that lost a
/// fast path or deadlocked, killed and reported as a failure instead of a hang.
pub(super) fn wait_within_run_limit(
    child: &mut Child,
    run_name: &str,
) -> Result<ExitStatus, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("run `{run_name}`: {error}"))?
        {
            return Ok(status);
        }
        if started.elapsed() > RUN_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "run `{run_name}` exceeded the {} s run limit",
                RUN_LIMIT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
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
/// - With `expected.events` (RA08, whose transcript is too long to write by
///   hand), the run goes twice under `--trace=full`: the two telemetry files
///   must be identical apart from `execution_id`, and the projected events,
///   counted by `event_classes`, must match the file in any order.
pub(super) fn execute(scenario: &Scenario, options: &Options) -> Result<Vec<Run>, String> {
    let expected = expected_lines(scenario, OUTCOME)?;
    let optional = |file: &str| {
        root(scenario)
            .join(file)
            .exists()
            .then(|| expected_lines(scenario, file))
            .transpose()
    };
    let transcript = optional(TRANSCRIPT)?;
    let events = optional(EVENTS)?;
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
        let run = |trace: Trace, repeat: u32| {
            run_once(
                scenario,
                &executable,
                scratch.path(),
                &format!("{index}-{}-{repeat}", &trace.flag()[8..]),
                trace,
                argument,
            )
        };
        let off = run(Trace::Off, 0)?;
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
            let full = run(Trace::Full, 0)?;
            let actual = super::transcript::project(&full.telemetry, scenario.extra_events)
                .map_err(|error| format!("--trace=full: {error}"))?;
            if options.print_actual {
                print_actual(scenario, TRANSCRIPT, transcript, &actual);
            }
            compare(TRANSCRIPT, transcript, &actual)?;
            let boundaries = run(Trace::Boundaries, 0)?;
            let actual = super::transcript::project(&boundaries.telemetry, scenario.extra_events)
                .map_err(|error| format!("--trace=boundaries: {error}"))?;
            compare(
                &format!("{TRANSCRIPT} (boundaries)"),
                &super::transcript::boundaries(transcript),
                &actual,
            )?;
            // OB04's evidence, gathered on the executable this run built: the
            // first run's telemetry against a second full run's.
            if index == 0 {
                let again = run(Trace::Full, 3).map(|second| second.telemetry);
                record_determinism(
                    scenario,
                    again.and_then(|second| same_telemetry(&full.telemetry, &second)),
                );
            }
            full_telemetry = full.telemetry;
        }
        if let Some(events) = &events {
            let first = run(Trace::Full, 1)?.telemetry;
            let second = run(Trace::Full, 2)?.telemetry;
            same_telemetry(&first, &second)?;
            let actual = event_classes(
                &super::transcript::project(&first, scenario.extra_events)
                    .map_err(|error| format!("--trace=full: {error}"))?,
            );
            let mut expected = events.clone();
            expected.sort();
            if options.print_actual {
                print_actual(scenario, EVENTS, &expected, &actual);
            }
            compare(EVENTS, &expected, &actual)?;
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
    let run_name = format!("{} {}", argument.unwrap_or("<no arguments>"), trace.flag());
    let stderr_path = scratch.join(format!("stderr-{name}.txt"));
    let mut child = Command::new(executable)
        .arg("--chimi-runner")
        .arg("--clock=virtual")
        .args(scenario.runner)
        .arg(trace.flag())
        .arg(format!("--telemetry-file={}", telemetry_path.display()))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg(format!("--memory-report-file={}", memory_path.display()))
        .arg("--")
        .args(argument)
        .stdout(Stdio::null())
        .stderr(
            std::fs::File::create(&stderr_path)
                .map_err(|error| format!("create {}: {error}", stderr_path.display()))?,
        )
        .spawn()
        .map_err(|error| format!("run {}: {error}", executable.display()))?;
    let status = wait_within_run_limit(&mut child, &run_name)?;
    if status.code() != Some(scenario.exit_code) {
        return Err(format!(
            "run `{run_name}` exited with {}, expected {}: {}",
            status,
            scenario.exit_code,
            std::fs::read_to_string(&stderr_path)
                .unwrap_or_default()
                .trim_end()
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

/// The telemetry with each line's `execution_id` field removed: the only
/// field the lab lets differ between two runs of one executable.
fn without_execution_id(telemetry: &str) -> Vec<String> {
    const KEY: &str = "\"execution_id\":\"";
    telemetry
        .lines()
        .map(|line| match line.find(KEY) {
            Some(start) => {
                let value = start + KEY.len();
                // An execution id holds no quote; the field's comma goes with it.
                let end = line[value..]
                    .find('"')
                    .map_or(line.len(), |close| value + close + 1);
                let end = if line[end..].starts_with(',') {
                    end + 1
                } else {
                    end
                };
                format!("{}{}", &line[..start], &line[end..])
            }
            None => line.to_owned(),
        })
        .collect()
}

/// `Ok` when two `--trace=full` telemetry files are byte-identical apart from
/// `execution_id` (OB04, RA08); otherwise the first differing line.
fn same_telemetry(first: &str, second: &str) -> Result<(), String> {
    compare(
        "telemetry of the second --trace=full run",
        &without_execution_id(first),
        &without_execution_id(second),
    )
}

/// The `expected.events` lines of a projected transcript: one line per class
/// of event, `<event> [<field>=<value>...] <count>`, sorted. A class is the
/// event name with its `CLASS_FIELDS` (the terminal cause, the rejection
/// reason, the send outcome, the restart attempt), so identities and times
/// do not split it.
fn event_classes(transcript: &[String]) -> Vec<String> {
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    for line in transcript {
        let mut words = line.split(' ').skip(1);
        let mut class = words.next().unwrap_or_default().to_owned();
        for word in words {
            if CLASS_FIELDS
                .iter()
                .any(|field| word.split_once('=').is_some_and(|(key, _)| key == *field))
            {
                class.push(' ');
                class.push_str(word);
            }
        }
        *counts.entry(class).or_default() += 1;
    }
    let mut lines: Vec<String> = counts
        .into_iter()
        .map(|(class, count)| format!("{class} {count}"))
        .collect();
    lines.sort();
    lines
}

/// The OB04 results of the transcript fixtures this lab process has already
/// run, by scenario ID, so OB04 need not rebuild them.
static DETERMINISM: std::sync::Mutex<Vec<(&'static str, Result<(), String>)>> =
    std::sync::Mutex::new(Vec::new());

fn record_determinism(scenario: &Scenario, result: Result<(), String>) {
    let mut results = DETERMINISM
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    results.retain(|(id, _)| *id != scenario.id);
    results.push((scenario.id, result));
}

/// OB04 for one transcript fixture: the result its own run recorded in this
/// process, or else a fresh build run twice (`deterministic`).
pub(super) fn determinism(scenario: &Scenario) -> Result<(), String> {
    let recorded = DETERMINISM
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .find(|(id, _)| *id == scenario.id)
        .map(|(_, result)| result.clone());
    recorded.unwrap_or_else(|| deterministic(scenario))
}

/// OB04 for one transcript fixture: builds it and runs it twice under
/// `--trace=full`, with no arguments or with its first, and requires
/// byte-identical telemetry apart from `execution_id`.
fn deterministic(scenario: &Scenario) -> Result<(), String> {
    let scratch = Scratch::new(scenario)?;
    let executable = build(scenario, scratch.path())?;
    let argument = scenario.arguments.first().copied();
    let first = run_once(
        scenario,
        &executable,
        scratch.path(),
        "first",
        Trace::Full,
        argument,
    )?;
    let second = run_once(
        scenario,
        &executable,
        scratch.path(),
        "second",
        Trace::Full,
        argument,
    )?;
    same_telemetry(&first.telemetry, &second.telemetry)
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
    let project = LoadedProject::load(root(scenario), "app/main", &StandardTree::embedded())
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
/// - `at_least <field> <limit>`: the field is at least `limit` in every run,
///   which catches a count that stops being kept and reads 0 in every run;
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
                "{MEMORY} line {}: `{line}` is not `equal <field>`, `at_most <field> <limit>`, `at_least <field> <limit>`, or `exactly <field> <value>`",
                index + 1
            )
        };
        let (rule, field, bound) = match line.split(' ').collect::<Vec<_>>().as_slice() {
            ["equal", field] => ("equal", *field, None),
            [rule @ ("at_most" | "at_least" | "exactly"), field, bound] => (
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
            ("at_least", Some(limit)) => values.iter().any(|value| *value < limit),
            (_, Some(expected)) => values.iter().any(|value| *value != expected),
            _ => false,
        };
        if failed {
            let claim = match (rule, bound) {
                ("equal", _) => "differs".to_owned(),
                ("at_most", Some(limit)) => format!("exceeds {limit}"),
                ("at_least", Some(limit)) => format!("is below {limit}"),
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
    build_entry(&root(scenario), "app/main", scratch)
}

/// The native executable of the project at `root` with entry module `entry`,
/// built into `scratch` as `program`.
pub(super) fn build_entry(root: &Path, entry: &str, scratch: &Path) -> Result<PathBuf, String> {
    let project = LoadedProject::load(root, entry, &StandardTree::embedded())
        .map_err(|error| format!("load fixture: {error}"))?;
    let program = Compiler::prepare_backend(project.sources()).map_err(|diagnostics| {
        format!(
            "fixture does not compile: {}",
            project.render_diagnostics(&diagnostics).trim_end()
        )
    })?;
    let executable = scratch.join("program");
    build_application(
        &program,
        &StandardTree::embedded(),
        &executable,
        &scratch.join("artifacts"),
        Vec::new(),
    )
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

    // Rejects a determinism check that ignores more than `execution_id`, or
    // that strips the field and leaves the lines unequal.
    #[test]
    fn same_telemetry_ignores_only_the_execution_id() {
        let line = |id: &str, sequence: u32| {
            format!(
                "{{\"schema\":\"s\",\"event\":\"process_started\",\"sequence\":{sequence},\"computation\":\"main\",\"execution_id\":\"{id}\",\"task_id\":2}}\n"
            )
        };
        let first = line("execution-1", 1) + &line("execution-1", 2);
        assert!(
            same_telemetry(&first, &(line("execution-2", 1) + &line("execution-2", 2))).is_ok()
        );
        assert!(
            same_telemetry(&first, &(line("execution-2", 1) + &line("execution-2", 3))).is_err()
        );
        assert!(same_telemetry(&first, &line("execution-2", 1)).is_err());
        assert_eq!(
            without_execution_id(&line("x", 1)),
            [
                "{\"schema\":\"s\",\"event\":\"process_started\",\"sequence\":1,\"computation\":\"main\",\"task_id\":2}"
            ]
        );
    }

    // Rejects classes split by identities or times, and classes that drop the
    // cause, outcome, or attempt that RA08 counts.
    #[test]
    fn event_classes_count_by_event_and_class_fields_only() {
        let transcript = lines(&[
            "@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.w",
            "@0s process_send kind=named name=app/main.w incarnation=I1 outcome=accepted",
            "@0s process_terminated membership=M1 incarnation=I1 cause=defected",
            "@0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1",
            "@1ms process_started supervisor=S1 membership=M1 incarnation=I2 name=app/main.w",
            "@1ms process_send kind=named name=app/main.w incarnation=- outcome=full",
            "@1ms restart_attempted supervisor=S1 membership=M1 attempts_in_window=2",
            "@2ms process_terminated membership=M1 incarnation=I2 cause=cancelled",
        ]);
        assert_eq!(
            event_classes(&transcript),
            [
                "process_send outcome=accepted 1",
                "process_send outcome=full 1",
                "process_started 2",
                "process_terminated cause=cancelled 1",
                "process_terminated cause=defected 1",
                "restart_attempted attempts_in_window=1 1",
                "restart_attempted attempts_in_window=2 1",
            ]
        );
    }
}
