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
        "{\"outcome\":\"success\",\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}"
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
    let execution = Command::new(&executable)
        .args(["--trace=boundaries", "--execution-id=cli-build"])
        .output()
        .expect("run built Chimi executable");
    assert!(execution.status.success());
    assert!(execution.stderr.is_empty());
    let stdout = String::from_utf8(execution.stdout).unwrap();
    assert!(
        stdout
            .lines()
            .next()
            .unwrap()
            .contains("\"execution_id\":\"cli-build\"")
    );
    assert_eq!(
        stdout.lines().last().unwrap(),
        "{\"outcome\":\"success\",\"value\":{\"type\":\"Answer\",\"fields\":{\"value\":42}}}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
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
    let execution = Command::new(&executable)
        .args(["--trace=off", "--execution-id=project-build"])
        .output()
        .expect("run built multi-module executable");
    assert!(execution.status.success());
    assert_eq!(
        String::from_utf8(execution.stdout).unwrap(),
        "{\"outcome\":\"success\",\"value\":42}\n"
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
