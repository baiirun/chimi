//! Runs `chimi run` for tests that read the reference interpreter's trace and
//! outcome. Since CHI-315 `chimi run` writes those only to the files it is
//! given, as a native runner does, so this adds the sinks and hands back their
//! contents where these tests used to read them.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs `command` (a `chimi run <input>` with no `--` yet) under
/// `--trace=full`, and returns its output with `stdout` replaced by the trace
/// followed by the outcome record.
pub fn traced(command: &mut Command) -> std::io::Result<Output> {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let base = std::env::temp_dir().join(format!(
        "chimi-traced-run-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    let telemetry = PathBuf::from(format!("{}.trace.jsonl", base.display()));
    let outcome = PathBuf::from(format!("{}.outcome.jsonl", base.display()));
    let _ = std::fs::remove_file(&telemetry);
    let _ = std::fs::remove_file(&outcome);
    let mut output = command
        .arg("--trace=full")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!("--outcome-file={}", outcome.display()))
        .output()?;
    let mut records = std::fs::read(&telemetry).unwrap_or_default();
    records.extend(std::fs::read(&outcome).unwrap_or_default());
    let _ = std::fs::remove_file(telemetry);
    let _ = std::fs::remove_file(outcome);
    output.stdout = records;
    Ok(output)
}
