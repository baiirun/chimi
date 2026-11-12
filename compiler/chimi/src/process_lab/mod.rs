//! The supervised-process conformance lab, `chimi-process-lab`
//! (`docs/specification/supervised-process-conformance.md`).

pub mod catalogue;
pub mod transcript;

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
            "MB" | "RT" | "SV" | "RA" | "ES" | "RR" | "SD" => Self::Runtime,
            "CB" | "MF" => Self::Heap,
            "OB" | "IP" => Self::Observability,
            "FP" => Self::Forcing,
            "PG" => Self::Performance,
            _ => return None,
        })
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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            through: catalogue::CURRENT_STAGE,
            scenario: None,
            print_actual: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    Pass,
    /// `detail` is the first differing line.
    Fail {
        detail: String,
    },
    Locked,
    /// The backend cannot run the scenario (the interpreter has no process
    /// runtime). Never counted as a pass.
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub kind: Kind,
    pub stage: Stage,
    pub scenarios: Vec<(&'static str, Status)>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.scenarios
            .iter()
            .any(|(_, status)| matches!(status, Status::Fail { .. }))
    }

    /// One JSON line. `metrics` and `gates` stay empty until the performance
    /// harness lands (U10).
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
                Status::Pass => "pass",
                Status::Fail { .. } => "fail",
                Status::Locked => "locked",
                Status::Unsupported => "unsupported",
            };
            write!(output, "{{\"id\":\"{id}\",\"status\":\"{name}\"").expect("write to a string");
            if let Status::Fail { detail } = status {
                output.push_str(",\"detail\":");
                crate::json::write_json_string(&mut output, detail);
            }
            output.push('}');
        }
        output.push_str("],\"metrics\":{},\"gates\":[]}\n");
        output
    }
}

/// Runs every scenario of `kind`, or only `options.scenario` when it is set
/// (whatever its kind). Scenarios above `options.through` report `locked`
/// and do not run.
pub fn run(kind: Kind, options: &Options) -> Result<Report, String> {
    let scenarios = catalogue::SCENARIOS
        .iter()
        .filter(|scenario| match options.scenario {
            Some(only) => scenario.id == only.id,
            None => scenario.kind() == kind,
        })
        .map(|scenario| (scenario.id, status(scenario, options)))
        .collect();
    Ok(Report {
        kind: options.scenario.map_or(kind, Scenario::kind),
        stage: options.through,
        scenarios,
    })
}

fn status(scenario: &Scenario, options: &Options) -> Status {
    if scenario.unlock > options.through {
        return Status::Locked;
    }
    let result = match (scenario.kind(), scenario.form) {
        (_, Form::Check(check) | Form::Gate(check)) => check(),
        (kind, form) => Err(format!(
            "the {} runner for {form:?} scenarios has not landed",
            kind.name()
        )),
    };
    match result {
        Ok(()) => Status::Pass,
        Err(detail) => Status::Fail { detail },
    }
}
