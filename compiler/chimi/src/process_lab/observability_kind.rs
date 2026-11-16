//! Observability scenarios (`OB`) and backend support (`IP`).
//!
//! A fixture runs like a runtime fixture (`fixture::check`): exact outcomes,
//! transcripts in every trace mode, and `expected.memory` when it has one.
//! IP01 runs its fixture on the reference interpreter instead
//! (`fixture::interpret`), whatever `--backend` says, because the interpreter
//! is what it tests. OB07 is a check over the trace ABI.

use super::catalogue::{self, Form, Scenario};
use super::fixture;
use super::{Options, transcript};
use crate::json::{JsonData, decode_json};

pub(super) fn check(scenario: &Scenario, options: &Options) -> Result<(), String> {
    match scenario.form {
        Form::Fixture if scenario.id.starts_with("IP") => fixture::interpret(scenario, options),
        Form::Fixture => fixture::check(scenario, options).map(|_| ()),
        form => Err(format!(
            "the observability runner does not run {form:?} scenarios"
        )),
    }
}

/// OB04: every transcript fixture unlocked at `CURRENT_STAGE` runs twice under
/// `--trace=full`, and the two telemetry files are byte-identical once
/// `execution_id` is removed. A fixture this lab process already ran supplies
/// the second run from its own executable (`fixture::determinism`); only the
/// others are built again. Scheduling order, identities, virtual times, and
/// sequences all appear in that telemetry, so any nondeterminism in them
/// fails here.
pub(super) fn transcripts_are_deterministic() -> Result<(), String> {
    let mut checked = 0;
    for scenario in &catalogue::SCENARIOS {
        if scenario.unlock > catalogue::CURRENT_STAGE
            || !matches!(scenario.form, Form::Fixture)
            || !fixture::root(scenario).join("expected.transcript").exists()
        {
            continue;
        }
        fixture::determinism(scenario).map_err(|error| format!("{}: {error}", scenario.id))?;
        checked += 1;
    }
    if checked == 0 {
        return Err("no transcript fixture is unlocked".into());
    }
    Ok(())
}

const FIBER_HEADER: &str = include_str!("../../../../runtime/chimi-fiber/include/chimi_fiber.h");

/// OB07: the scheduler the compiler links and the header the runtime is built
/// against both state ABI version 14 (5, plus the capability-violation hook in
/// 6, the process-defect hook in 7, cancellation scopes in 8, the monitor and
/// mailbox requests in 9, the carried capabilities and start argument of
/// G28 in 10, the select request in 11, the resource requests in 13, and the
/// scope cancel request in 14), and the runner writes process events with
/// their own identity fields: SV01's three `process_started` events each name
/// an incarnation and that incarnation's task, which differ (the root is task
/// 1, so the first incarnation runs as task 2).
pub(super) fn trace_abi_version() -> Result<(), String> {
    let header = FIBER_HEADER
        .lines()
        .find_map(|line| line.strip_prefix("#define CHIMI_FIBER_ABI_VERSION "))
        .map(|value| value.trim().trim_end_matches('u'))
        .ok_or("chimi_fiber.h does not define CHIMI_FIBER_ABI_VERSION")?;
    if header != "14" || chimi_fiber::ABI_VERSION != 14 {
        return Err(format!(
            "the trace ABI is {} in chimi_fiber.h and {} in chimi-fiber, expected 14",
            header,
            chimi_fiber::ABI_VERSION
        ));
    }
    let scenario = catalogue::find("SV01").ok_or("the catalogue has no SV01")?;
    let runs = fixture::execute(scenario, &Options::default())?;
    let telemetry = &runs.first().ok_or("SV01 did not run")?.full_telemetry;
    // The projector checks every process event's key set.
    transcript::project(telemetry, &[])?;
    let mut started = 0;
    for line in telemetry.lines() {
        let Ok(JsonData::Object(fields)) = decode_json(line) else {
            return Err(format!("telemetry line is not an object: {line}"));
        };
        let field = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        };
        if field("event") != Some(&JsonData::Text("process_started".into())) {
            continue;
        }
        started += 1;
        let (Some(JsonData::Number(task)), Some(JsonData::Number(incarnation))) =
            (field("task_id"), field("incarnation_id"))
        else {
            return Err(format!(
                "process_started lacks a task or an incarnation: {line}"
            ));
        };
        if task == incarnation {
            return Err(format!(
                "process_started carries incarnation {incarnation} in its task field: {line}"
            ));
        }
    }
    if started != 3 {
        return Err(format!(
            "SV01 wrote {started} process_started events, expected 3"
        ));
    }
    Ok(())
}
