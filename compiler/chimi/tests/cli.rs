use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}.chimi", env!("CARGO_MANIFEST_DIR"))
}

fn repository_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
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
        "{\"outcome\":\"success\",\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}"
    );
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
            "{\"outcome\":\"success\",\"value\":{\"type\":\"Order\",",
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
            "usage: chimi --version | chimi check <path> | chimi run <path> | ",
            "chimi check --root <project-dir> <entry-module> | ",
            "chimi run --root <project-dir> <entry-module>\n"
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
        "{\"outcome\":\"success\",\"value\":42}"
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
