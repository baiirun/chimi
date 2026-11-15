//! The supervised-process conformance lab, `chimi-process-lab`
//! (`docs/specification/supervised-process-conformance.md`).

pub mod catalogue;
mod core_kind;
mod fixture;
mod heap_kind;
mod observability_kind;
mod performance;
mod static_kind;
pub mod transcript;

pub use performance::baseline_warnings;

use catalogue::{Form, Scenario, Stage};
use std::fmt::Write as _;

/// One evidence kind. `all` on the command line runs each in this order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Static,
    Core,
    Runtime,
    Heap,
    Observability,
    Forcing,
    Performance,
}

impl Kind {
    pub const ALL: [Self; 7] = [
        Self::Static,
        Self::Core,
        Self::Runtime,
        Self::Heap,
        Self::Observability,
        Self::Forcing,
        Self::Performance,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Core => "core",
            Self::Runtime => "runtime",
            Self::Heap => "heap",
            Self::Observability => "observability",
            Self::Forcing => "forcing",
            Self::Performance => "performance",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == text)
    }

    /// The kind that owns a scenario, from its ID prefix.
    pub fn of_scenario(id: &str) -> Option<Self> {
        Some(match id.get(..2)? {
            "SP" | "SF" => Self::Static,
            "CO" => Self::Core,
            "MB" | "RT" | "SV" | "RA" | "ES" | "RR" | "SD" | "SG" => Self::Runtime,
            "CB" | "MF" => Self::Heap,
            "OB" | "IP" => Self::Observability,
            "FP" => Self::Forcing,
            "PG" => Self::Performance,
            _ => return None,
        })
    }
}

/// The backend that executes runtime, heap, observability, and forcing
/// fixtures. Static and Core scenarios, C harnesses, and checks do not depend
/// on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Native,
    /// The reference interpreter has no process runtime, so every executed
    /// fixture reports `unsupported` (coordinator decision 1 on `ts-6ccbc9`).
    Interpreter,
}

impl Backend {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "native" => Some(Self::Native),
            "interpreter" => Some(Self::Interpreter),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Scenarios unlocked at or below this stage must pass.
    pub through: Stage,
    /// Run only this scenario.
    pub scenario: Option<&'static Scenario>,
    /// Print actual diagnostics, outcomes, and transcripts to stderr.
    pub print_actual: bool,
    pub backend: Backend,
    /// Run PG05 with a per-process regression of this factor injected into
    /// its workload, to show that the gate fails on one.
    pub injected_pg05_regression: Option<f64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            through: catalogue::CURRENT_STAGE,
            scenario: None,
            print_actual: false,
            backend: Backend::Native,
            injected_pg05_regression: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    /// `note` is a caveat on the evidence, such as a C harness that ran
    /// without AddressSanitizer. The report shows it as `detail`.
    Pass {
        note: Option<String>,
    },
    /// `detail` is the first differing line.
    Fail {
        detail: String,
    },
    Locked,
    /// The backend cannot run the scenario (the interpreter has no process
    /// runtime). Never counted as a pass.
    Unsupported,
}

/// One measured performance gate: `value` against `limit` under `rule`.
#[derive(Clone, Debug, PartialEq)]
pub struct GateResult {
    pub metric: String,
    pub value: f64,
    pub limit: f64,
    pub rule: Rule,
    pub passed: bool,
}

/// How a gate compares its value with its limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rule {
    /// The value must be strictly below the limit.
    Below,
    /// The value must equal the limit.
    Exactly,
}

impl GateResult {
    pub fn below(metric: &str, value: f64, limit: f64) -> Self {
        Self {
            metric: metric.to_owned(),
            value,
            limit,
            rule: Rule::Below,
            passed: value < limit,
        }
    }

    pub fn exactly(metric: &str, value: f64, limit: f64) -> Self {
        Self {
            metric: metric.to_owned(),
            value,
            limit,
            rule: Rule::Exactly,
            passed: value == limit,
        }
    }

    fn failure(&self) -> String {
        let claim = match self.rule {
            Rule::Below => "is not below",
            Rule::Exactly => "is not exactly",
        };
        format!(
            "{} {} {claim} {}",
            self.metric,
            render_number(self.value),
            render_number(self.limit)
        )
    }
}

/// What one gate scenario measured: its gates, and figures recorded beside
/// them without a limit.
#[derive(Clone, Debug, Default)]
pub struct Measurement {
    pub gates: Vec<GateResult>,
    pub metrics: Vec<(String, f64)>,
}

impl Measurement {
    pub fn gates(gates: Vec<GateResult>) -> Self {
        Self {
            gates,
            metrics: Vec::new(),
        }
    }
}

/// A number for the report: at most three decimals, no trailing zeros, and
/// `null` for a value JSON cannot hold.
pub(crate) fn render_number(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    let rounded = (value * 1_000.0).round() / 1_000.0;
    format!("{rounded}")
}

#[derive(Clone, Debug)]
pub struct Report {
    pub kind: Kind,
    pub stage: Stage,
    pub scenarios: Vec<(&'static str, Status)>,
    /// Every gate the run measured, passed or not.
    pub gates: Vec<GateResult>,
    pub metrics: Vec<(String, f64)>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.scenarios
            .iter()
            .any(|(_, status)| matches!(status, Status::Fail { .. }))
    }

    /// One JSON line. `metrics` and `gates` are empty except in a
    /// performance report.
    pub fn render_json(&self) -> String {
        let mut output = format!(
            "{{\"schema\":\"chimi.process-conformance.v1\",\"kind\":\"{}\",\"stage\":\"{}\",\"scenarios\":[",
            self.kind.name(),
            self.stage.name()
        );
        for (index, (id, status)) in self.scenarios.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            let name = match status {
                Status::Pass { .. } => "pass",
                Status::Fail { .. } => "fail",
                Status::Locked => "locked",
                Status::Unsupported => "unsupported",
            };
            write!(output, "{{\"id\":\"{id}\",\"status\":\"{name}\"").expect("write to a string");
            if let Status::Fail { detail } | Status::Pass { note: Some(detail) } = status {
                output.push_str(",\"detail\":");
                crate::json::write_json_string(&mut output, detail);
            }
            output.push('}');
        }
        output.push_str("],\"metrics\":{");
        for (index, (name, value)) in self.metrics.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            crate::json::write_json_string(&mut output, name);
            write!(output, ":{}", render_number(*value)).expect("write to a string");
        }
        output.push_str("},\"gates\":[");
        for (index, gate) in self.gates.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"metric\":");
            crate::json::write_json_string(&mut output, &gate.metric);
            write!(
                output,
                ",\"value\":{},\"limit\":{},\"rule\":\"{}\",\"status\":\"{}\"}}",
                render_number(gate.value),
                render_number(gate.limit),
                match gate.rule {
                    Rule::Below => "<",
                    Rule::Exactly => "=",
                },
                if gate.passed { "pass" } else { "fail" }
            )
            .expect("write to a string");
        }
        output.push_str("]}\n");
        output
    }
}

/// Runs every scenario of `kind`, or only `options.scenario` when it is set
/// (whatever its kind). Scenarios above `options.through` report `locked`
/// and do not run.
pub fn run(kind: Kind, options: &Options) -> Result<Report, String> {
    run_scenarios(&catalogue::SCENARIOS, kind, options)
}

fn run_scenarios(
    catalogue: &'static [Scenario],
    kind: Kind,
    options: &Options,
) -> Result<Report, String> {
    let mut measured = Measurement::default();
    let scenarios = catalogue
        .iter()
        .filter(|scenario| match options.scenario {
            Some(only) => scenario.id == only.id,
            None => scenario.kind() == kind,
        })
        .map(|scenario| (scenario.id, status(scenario, options, &mut measured)))
        .collect();
    Ok(Report {
        kind: options.scenario.map_or(kind, Scenario::kind),
        stage: options.through,
        scenarios,
        gates: measured.gates,
        metrics: measured.metrics,
    })
}

/// A gate scenario passes when every gate it measured passed; its gates and
/// metrics join `measured` either way.
fn measure(
    gate: impl FnOnce() -> Result<Measurement, String>,
    measured: &mut Measurement,
) -> Result<(), String> {
    let measurement = gate()?;
    let failure = measurement
        .gates
        .iter()
        .find(|gate| !gate.passed)
        .map(GateResult::failure);
    measured.gates.extend(measurement.gates);
    measured.metrics.extend(measurement.metrics);
    failure.map_or(Ok(()), Err)
}

fn status(scenario: &Scenario, options: &Options, measured: &mut Measurement) -> Status {
    if scenario.unlock > options.through {
        return Status::Locked;
    }
    // IP01 is the interpreter's own scenario: it runs on the interpreter always.
    let executed =
        !matches!(scenario.kind(), Kind::Static | Kind::Core) && !scenario.id.starts_with("IP");
    if executed
        && matches!(scenario.form, Form::Fixture | Form::Gate(_))
        && options.backend == Backend::Interpreter
    {
        return Status::Unsupported;
    }
    let result = match (scenario.kind(), scenario.form) {
        (_, Form::Check(check)) => check().map(|()| None),
        (_, Form::Gate(_))
            if scenario.id == "PG05"
                && let Some(factor) = options.injected_pg05_regression =>
        {
            measure(
                || performance::pg05_with_injected_regression(factor),
                measured,
            )
            .map(|()| None)
        }
        (_, Form::Gate(gate)) => measure(gate, measured).map(|()| None),
        (Kind::Static, Form::Fixture) => static_kind::check(scenario, options).map(|()| None),
        (Kind::Heap, Form::Fixture | Form::CHarness(_)) => heap_kind::check(scenario, options),
        (Kind::Runtime, Form::Fixture) => fixture::check(scenario, options).map(|_| None),
        (Kind::Runtime, Form::CHarness(file)) => heap_kind::kernel_harness(scenario, file),
        (Kind::Observability, Form::Fixture) => {
            observability_kind::check(scenario, options).map(|()| None)
        }
        (Kind::Forcing, Form::Fixture) => fixture::check(scenario, options).map(|_| None),
        (kind, form) => Err(format!(
            "the {} runner for {form:?} scenarios has not landed",
            kind.name()
        )),
    };
    match result {
        Ok(note) => Status::Pass { note },
        Err(detail) => Status::Fail { detail },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes() -> Result<(), String> {
        Ok(())
    }

    fn fails() -> Result<(), String> {
        Err("first differing line".into())
    }

    static PROBES: [Scenario; 4] = [
        Scenario {
            id: "CO01",
            ..PASSING
        },
        Scenario {
            id: "CO02",
            form: Form::Check(fails),
            ..PASSING
        },
        Scenario {
            id: "CO03",
            unlock: Stage::U10,
            form: Form::Check(fails),
            ..PASSING
        },
        Scenario {
            id: "SP01",
            form: Form::Check(fails),
            ..PASSING
        },
    ];
    const PASSING: Scenario = Scenario {
        id: "",
        slug: "probe",
        unlock: Stage::U0,
        form: Form::Check(passes),
        exit_code: 0,
        arguments: &[],
        extra_events: &[],
        runner: &[],
    };

    // Rejects a report that hides weaker evidence, such as a C harness that ran
    // without AddressSanitizer, behind a bare pass.
    #[test]
    fn a_pass_with_a_note_reports_the_note_as_its_detail() {
        let report = Report {
            kind: Kind::Heap,
            stage: Stage::U3,
            scenarios: vec![(
                "CB06",
                Status::Pass {
                    note: Some("ran without AddressSanitizer".into()),
                },
            )],
            gates: Vec::new(),
            metrics: Vec::new(),
        };
        assert!(!report.failed());
        assert!(
            report.render_json().contains(
                r#"{"id":"CB06","status":"pass","detail":"ran without AddressSanitizer"}"#
            )
        );
    }

    // Independent of CURRENT_STAGE and of which runners have landed: rejects
    // a lab that reports an unlocked failure as a pass, runs a locked
    // scenario, or mixes kinds into one report.
    #[test]
    fn unlocked_failures_fail_the_report_and_locked_scenarios_do_not_run() {
        let below_the_last_stage = Options {
            through: Stage::U9,
            ..Options::default()
        };
        let report = run_scenarios(&PROBES, Kind::Core, &below_the_last_stage).unwrap();
        assert_eq!(
            report.scenarios,
            [
                ("CO01", Status::Pass { note: None }),
                (
                    "CO02",
                    Status::Fail {
                        detail: "first differing line".into()
                    }
                ),
                ("CO03", Status::Locked),
            ]
        );
        assert!(report.failed());
        assert!(
            report
                .render_json()
                .contains(r#"{"id":"CO02","status":"fail","detail":"first differing line"}"#)
        );
        let only = Options {
            scenario: Some(&PROBES[0]),
            ..below_the_last_stage
        };
        assert!(
            !run_scenarios(&PROBES, Kind::Static, &only)
                .unwrap()
                .failed()
        );
    }
}
