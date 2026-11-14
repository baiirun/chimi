//! Runs the supervised-process conformance lab through its binary. Every
//! scenario at or below `CURRENT_STAGE` must pass; later ones report
//! `locked`. Performance gates measure wall time and resident memory, which a
//! loaded machine skews, so they run only on request: the ignored
//! `performance_gates_pass_on_an_idle_machine`, or
//! `chimi-process-lab performance`.

use chimi::process_lab::catalogue::{CURRENT_STAGE, SCENARIOS, Stage};
use chimi::process_lab::{Kind, transcript};
use std::process::{Command, Output};

fn lab(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chimi-process-lab"))
        .args(arguments)
        .output()
        .expect("run chimi-process-lab")
}

fn statuses(report: &str) -> Vec<(String, String)> {
    report
        .split("{\"id\":\"")
        .skip(1)
        .map(|entry| {
            let (id, rest) = entry.split_once('"').unwrap();
            let status = rest.split("\"status\":\"").nth(1).unwrap();
            (id.to_owned(), status.split('"').next().unwrap().to_owned())
        })
        .collect()
}

/// Every kind except `performance`, in `all`'s order.
fn conformance_kinds() -> Vec<Kind> {
    Kind::ALL
        .into_iter()
        .filter(|kind| *kind != Kind::Performance)
        .collect()
}

// The lab's own gate: every unlocked scenario of every kind but performance
// passes, and each kind reports each of its scenarios once. One run lists
// every kind, so OB04 reuses the executables the earlier kinds built.
#[test]
fn every_scenario_through_the_current_stage_passes() {
    let kinds = conformance_kinds();
    let names: Vec<&str> = kinds.iter().map(|kind| kind.name()).collect();
    let output = lab(&names);
    let reports = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{reports}");
    assert!(output.stderr.is_empty());
    assert_eq!(reports.lines().count(), kinds.len());
    for (kind, report) in kinds.into_iter().zip(reports.lines()) {
        assert!(report.starts_with(&format!(
            "{{\"schema\":\"chimi.process-conformance.v1\",\"kind\":\"{}\",\"stage\":\"{}\",\"scenarios\":[",
            kind.name(),
            CURRENT_STAGE.name()
        )));
        let expected = SCENARIOS
            .iter()
            .filter(|scenario| scenario.kind() == kind)
            .map(|scenario| {
                let status = if scenario.unlock <= CURRENT_STAGE {
                    "pass"
                } else {
                    "locked"
                };
                (scenario.id.to_owned(), status.to_owned())
            })
            .collect::<Vec<_>>();
        assert_eq!(statuses(report), expected, "{}", kind.name());
    }
}

// U10's executed scenarios pass while the stage stays locked on PG02's
// full-trace gate (`prog show ts-8f201b`): the forcing program's exact outcome
// and transcripts, and memory that stays flat over restarts and memberships.
// Redundant once CURRENT_STAGE reaches U10.
#[test]
fn the_forcing_program_and_u10_memory_scenarios_pass() {
    for id in ["FP01", "MF02", "MF03"] {
        let output = lab(&["--scenario", id, "--through", Stage::U10.name()]);
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(output.status.success(), "{report}");
        assert_eq!(statuses(&report), [(id.to_owned(), "pass".to_owned())]);
    }
}

// The explicit performance run: every gate passes and reports its value and
// limit. Run it on an idle machine, in release, through `scripts/heavy.sh`:
// `cargo test --release -p chimi --test process_lab -- --ignored performance`.
#[test]
#[ignore = "measures wall time and resident memory; run explicitly on an idle machine"]
fn performance_gates_pass_on_an_idle_machine() {
    let output = lab(&["performance", "--through", Stage::U10.name()]);
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{report}");
    let gates = SCENARIOS
        .iter()
        .filter(|scenario| scenario.kind() == Kind::Performance)
        .map(|scenario| (scenario.id.to_owned(), "pass".to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(statuses(&report), gates);
    assert!(!report.contains("\"gates\":[]"), "{report}");
}

#[test]
fn all_prints_one_report_per_kind_in_order() {
    let output = lab(&["all", "--through", Stage::U0.name()]);
    let report = String::from_utf8(output.stdout).unwrap();
    let kinds = report
        .lines()
        .map(|line| {
            line.split("\"kind\":\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(kinds, Kind::ALL.map(Kind::name));
    assert_eq!(statuses(&report).len(), SCENARIOS.len());
}

#[test]
fn malformed_arguments_are_usage_errors() {
    for arguments in [
        &[][..],
        &["sideways"],
        &["static", "--through", "U11"],
        &["--scenario", "XX99"],
        &["heap", "--backend", "bytecode"],
        &["static", "core", "static"],
    ] {
        let output = lab(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
    }
}

// Coordinator decision 1: the interpreter has no process runtime, so a fixture
// that executes reports `unsupported`, which is never a pass and never a failure.
// MF05 is a native fixture that passes; the interpreter must not run it or count it.
#[test]
fn the_interpreter_backend_reports_executed_fixtures_unsupported() {
    let output = lab(&[
        "--scenario",
        "MF05",
        "--through",
        Stage::U3.name(),
        "--backend",
        "interpreter",
    ]);
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{report}");
    assert_eq!(
        statuses(&report),
        [("MF05".to_owned(), "unsupported".to_owned())]
    );
}

// Projects task telemetry the native runner really writes. This covers the
// shared envelope, `sequence` order, and the `task_terminal` extra event; every
// runtime transcript fixture projects the process family's. Task identities
// count from 1 in creation order: the root is T1 and its child T2, and
// neither terminal names a defect cause.
#[test]
fn the_projector_reads_real_runner_telemetry() {
    let scratch = std::env::temp_dir().join(format!("chimi-process-lab-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let source = scratch.join("main.chimi");
    std::fs::write(
        &source,
        concat!(
            "import std/task as Task\n",
            "pub fn main() -> Int {\n",
            "  let child = Task.start(fn() -> Int { 41 })\n",
            "  Task.result(child) + 1\n",
            "}\n",
        ),
    )
    .unwrap();
    let executable = scratch.join("main");
    let build = Command::new(env!("CARGO_BIN_EXE_chimi"))
        .current_dir(&scratch)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let telemetry = scratch.join("telemetry.jsonl");
    let run = Command::new(&executable)
        .arg("--chimi-runner")
        .arg("--trace=full")
        .arg(format!("--telemetry-file={}", telemetry.display()))
        .arg(format!(
            "--outcome-file={}",
            scratch.join("outcome.jsonl").display()
        ))
        .arg("--")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let telemetry = std::fs::read_to_string(&telemetry).unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();

    assert_eq!(
        transcript::project(&telemetry, &[]).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        transcript::project(&telemetry, &["task_terminal"]).unwrap(),
        [
            "@0s task_terminal task=T2 parent=T1 cause=-",
            "@0s task_terminal task=T1 parent=- cause=-",
        ]
    );
}
