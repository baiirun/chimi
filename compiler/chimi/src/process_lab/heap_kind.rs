//! Heap scenarios: copy-boundary safety (`CB`) and memory flatness (`MF`).
//!
//! A C harness scenario compiles `tests/process-lab/c/<file>`, which includes
//! `runtime.c`, and runs it with the scenario ID as its only argument. It
//! passes when the harness exits 0. AddressSanitizer is used when the C
//! compiler supports it, so a freed value that is still reachable fails the
//! scenario even when its bytes look right.
//!
//! A fixture scenario runs natively once per argument (`fixture::execute`),
//! then checks `expected.memory` against the runs' memory reports. Each line
//! is `equal <field>`: the top-level integer field must have the same value in
//! every run.

use super::Options;
use super::catalogue::{Form, Scenario};
use super::fixture::{self, Run, Scratch};
use crate::json::{JsonData, decode_json};
use std::path::{Path, PathBuf};
use std::process::Command;

const MEMORY: &str = "expected.memory";

pub(super) fn check(scenario: &Scenario, options: &Options) -> Result<(), String> {
    match scenario.form {
        Form::CHarness(file) => harness(scenario, file),
        Form::Fixture => {
            let runs = fixture::execute(scenario, options)?;
            check_memory(scenario, &runs, options)
        }
        form => Err(format!("the heap runner does not run {form:?} scenarios")),
    }
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn harness(scenario: &Scenario, file: &str) -> Result<(), String> {
    let scratch = Scratch::new(scenario)?;
    let executable = scratch.path().join("harness");
    let source = manifest_path("tests/process-lab/c").join(file);
    let compile = |flags: &[&str]| {
        Command::new("cc")
            .args(["-std=c11", "-O1", "-g"])
            .args(flags)
            .arg("-I")
            .arg(manifest_path("runtime/native"))
            .arg("-I")
            .arg(manifest_path("../../runtime/chimi-fiber/include"))
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .map_err(|error| format!("run cc: {error}"))
    };
    let sanitized = compile(&["-fsanitize=address,undefined", "-fno-omit-frame-pointer"])?;
    if !sanitized.status.success() {
        let plain = compile(&[])?;
        if !plain.status.success() {
            return Err(format!(
                "compile {file}: {}",
                compile_error_line(&plain.stderr)
            ));
        }
    }
    let run = Command::new(&executable)
        .arg(scenario.id)
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .map_err(|error| format!("run {file}: {error}"))?;
    if !run.status.success() {
        return Err(format!(
            "{file} {} exited with {}: {}",
            scenario.id,
            run.status,
            first_error_line(&run.stderr)
        ));
    }
    if run.stdout != b"ok\n" {
        return Err(format!(
            "{file} {} printed `{}`, expected `ok`",
            scenario.id,
            String::from_utf8_lossy(&run.stdout).trim_end()
        ));
    }
    Ok(())
}

fn compile_error_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .find(|line| line.contains("error"))
        .or_else(|| text.lines().last())
        .unwrap_or("")
        .to_owned()
}

/// The harness's own failure line, or the sanitizer's summary.
fn first_error_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .find(|line| line.contains("check failed") || line.contains("ERROR:"))
        .or_else(|| text.lines().next())
        .unwrap_or("")
        .to_owned()
}

fn check_memory(scenario: &Scenario, runs: &[Run], options: &Options) -> Result<(), String> {
    let lines = fixture::expected_lines(scenario, MEMORY)?;
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
        let Some(field) = line.strip_prefix("equal ") else {
            return Err(format!(
                "{MEMORY} line {}: `{line}` is not `equal <field>`",
                index + 1
            ));
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
        if values.windows(2).any(|pair| pair[0] != pair[1]) {
            let runs = scenario
                .arguments
                .iter()
                .zip(&values)
                .map(|(argument, value)| format!("{argument}: {value}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "{MEMORY} line {}: {field} differs ({runs})",
                index + 1
            ));
        }
    }
    Ok(())
}
