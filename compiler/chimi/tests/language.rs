//! Language-feature conformance through both execution paths.
//!
//! Every program here runs on the reference interpreter (`chimi run`) and as a
//! native AOT executable (`chimi build`). Both outcomes must be identical and
//! must match the expected outcome. Each program also round-trips through the
//! formatter and survives inspection at every byte offset.

use chimi::source::SourceSnapshot;
use chimi::{Compiler, ProjectSource};
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chimi-language-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn interpreter_outcome(source: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .arg("run")
        .arg(source)
        .output()
        .expect("run the reference interpreter");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .last()
        .unwrap_or_else(|| {
            panic!(
                "no interpreter outcome: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .to_owned()
}

fn native_outcome(scratch: &Path, source: &Path) -> String {
    let executable = scratch.join("program");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(scratch)
        .arg("build")
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build the native executable");
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let outcome_path = scratch.join("outcome.jsonl");
    Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=off")
        .arg(format!(
            "--telemetry-file={}",
            scratch.join("telemetry.jsonl").display()
        ))
        .arg(format!("--outcome-file={}", outcome_path.display()))
        .arg("--execution-id=language")
        .arg("--")
        .output()
        .expect("execute the native program");
    std::fs::read_to_string(outcome_path)
        .expect("native outcome file")
        .trim_end()
        .to_owned()
}

fn assert_formatter_round_trips(source: &str) {
    let first = Compiler::format(SourceSnapshot::from_text("main.chimi", source));
    assert!(!first.is_blocked(), "{}", first.render_diagnostics());
    let second = Compiler::format(SourceSnapshot::from_text("main.chimi", first.text()));
    assert_eq!(second.text(), first.text(), "formatting is not idempotent");
}

fn assert_inspection_survives_every_offset(source: &str) {
    for offset in 0..=source.len() {
        if !source.is_char_boundary(offset) {
            continue;
        }
        let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
            .expect("valid module path");
        let _ = Compiler::inspect_project(vec![module], "main", offset);
    }
}

/// Runs `source` on both execution paths and returns their shared outcome value.
///
/// The returned text is the outcome line without its schema prefix, such as
/// `"outcome":"success","value":42`.
fn run_both(name: &str, source: &str) -> String {
    let scratch = scratch_directory(name);
    let path = scratch.join("main.chimi");
    std::fs::write(&path, source).unwrap();
    let interpreted = interpreter_outcome(&path);
    let native = native_outcome(&scratch, &path);
    assert_eq!(interpreted, native, "execution paths disagree for {name}");
    assert_formatter_round_trips(source);
    assert_inspection_survives_every_offset(source);
    std::fs::remove_dir_all(scratch).unwrap();
    interpreted
        .strip_prefix("{\"schema\":\"chimi.runtime.v1\",")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("unexpected outcome line: {interpreted}"))
        .to_owned()
}

fn check_codes(source: &str) -> Vec<String> {
    let module = ProjectSource::new("main", SourceSnapshot::from_text("main.chimi", source))
        .expect("valid module path");
    Compiler::type_project(vec![module])
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect()
}

#[test]
fn pipeline_inserts_the_subject_as_the_first_argument() {
    let outcome = run_both(
        "pipe-first-argument",
        concat!(
            "fn subtract(left: Int, right: Int) -> Int { left - right }\n",
            "fn scale(value: Int, factor: Int, offset: Int) -> Int { value * factor + offset }\n",
            "fn increment(value: Int) -> Int { value + 1 }\n",
            "pub fn main() -> Int {\n",
            "  10\n",
            "  |> subtract(3)\n",
            "  |> scale(10, 2)\n",
            "  |> increment\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":73");
}

#[test]
fn pipeline_applies_a_non_call_right_side_as_a_unary_function() {
    let outcome = run_both(
        "pipe-unary-forms",
        concat!(
            "fn adder(amount: Int) -> fn(Int) -> Int { fn(value: Int) { value + amount } }\n",
            "pub fn main() -> Int {\n",
            "  let add_two = adder(2)\n",
            "  1 |> add_two |> (adder(5)) |> (fn(value: Int) { value * 3 })\n",
            "}\n",
        ),
    );
    assert_eq!(outcome, "\"outcome\":\"success\",\"value\":24");
}

#[test]
fn pipeline_into_a_call_that_returns_a_function_does_not_apply_the_result() {
    // `1 |> adder(2)` is `adder(1, 2)`, which has too many arguments; the
    // pipeline never falls back to calling the returned function.
    let codes = check_codes(concat!(
        "fn adder(amount: Int) -> fn(Int) -> Int { fn(value: Int) { value + amount } }\n",
        "pub fn main() -> Int { 1 |> adder(2) }\n",
    ));
    assert_eq!(
        codes.first().map(String::as_str),
        Some("T4202"),
        "{codes:?}"
    );
}
