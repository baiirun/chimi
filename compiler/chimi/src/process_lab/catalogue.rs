//! The scenario catalogue of the supervised-process conformance lab
//! (`docs/specification/supervised-process-conformance.md`, "Scenario
//! catalogue"), and the stage the implementation has reached.

use super::Kind;

/// The last stage whose scenarios must pass. Each ts-6ccbc9 commit that
/// completes a stage bumps this in the same commit.
pub const CURRENT_STAGE: Stage = Stage::U12;

/// An implementation stage from the lab's unlock order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Stage {
    U0,
    U1,
    U2,
    U3,
    U4,
    U5,
    U6,
    U7,
    U8,
    U9,
    U10,
    U11,
    U12,
}

impl Stage {
    pub const ALL: [Self; 13] = [
        Self::U0,
        Self::U1,
        Self::U2,
        Self::U3,
        Self::U4,
        Self::U5,
        Self::U6,
        Self::U7,
        Self::U8,
        Self::U9,
        Self::U10,
        Self::U11,
        Self::U12,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::U0 => "U0",
            Self::U1 => "U1",
            Self::U2 => "U2",
            Self::U3 => "U3",
            Self::U4 => "U4",
            Self::U5 => "U5",
            Self::U6 => "U6",
            Self::U7 => "U7",
            Self::U8 => "U8",
            Self::U9 => "U9",
            Self::U10 => "U10",
            Self::U11 => "U11",
            Self::U12 => "U12",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.name() == text)
    }
}

/// A Rust check that has no fixture of its own.
pub type Check = fn() -> Result<(), String>;

/// A performance gate's measurement (`performance.rs`).
pub type Measure = fn() -> Result<super::Measurement, String>;

/// How a scenario produces its evidence.
#[derive(Clone, Copy, Debug)]
pub enum Form {
    /// A Chimi project at `tests/process-lab/<ID>-<slug>/` with the expected
    /// files its kind needs. FP01's project is `programs/supervised-pool/`, FP02's
    /// `programs/service-pool/`.
    Fixture,
    /// A white-box C harness under `tests/process-lab/c/`.
    CHarness(&'static str),
    /// A Rust check over verified Core, built artifacts, or the telemetry of
    /// other fixtures (CO01-CO04, MF06, OB04, OB07).
    Check(Check),
    /// A measured performance gate.
    Gate(Measure),
}

#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub id: &'static str,
    pub slug: &'static str,
    pub unlock: Stage,
    pub form: Form,
    /// Runner exit code of an executed fixture: 0 success, 1 expected
    /// failure, 70 defect. Static and Core scenarios do not run and keep 0.
    pub exit_code: i32,
    /// One entry per run. Each run passes its entry to `main(arguments)`, and
    /// `expected.outcome` holds one line per run in this order. Empty means
    /// one run with no arguments.
    pub arguments: &'static [&'static str],
    /// Events kept in the transcript beyond the process family.
    pub extra_events: &'static [&'static str],
    /// Runner options every run adds after `--clock=virtual`, such as an
    /// injected `--signal`.
    pub runner: &'static [&'static str],
}

impl Scenario {
    pub fn kind(&self) -> Kind {
        Kind::of_scenario(self.id).expect("every catalogue ID has a known prefix")
    }

    const fn new(id: &'static str, slug: &'static str, unlock: Stage, form: Form) -> Self {
        Self {
            id,
            slug,
            unlock,
            form,
            exit_code: 0,
            arguments: &[],
            extra_events: &[],
            runner: &[],
        }
    }

    const fn defects(mut self) -> Self {
        self.exit_code = 70;
        self
    }

    const fn runs(mut self, arguments: &'static [&'static str]) -> Self {
        self.arguments = arguments;
        self
    }

    const fn with_events(mut self, extra_events: &'static [&'static str]) -> Self {
        self.extra_events = extra_events;
        self
    }

    /// The run ends by an injected signal (`--signal`): it exits 128 plus
    /// the signal's number.
    const fn signalled(mut self, runner: &'static [&'static str], exit_code: i32) -> Self {
        self.runner = runner;
        self.exit_code = exit_code;
        self
    }
}

const fn fixture(id: &'static str, slug: &'static str, unlock: Stage) -> Scenario {
    Scenario::new(id, slug, unlock, Form::Fixture)
}

const fn c_harness(
    id: &'static str,
    slug: &'static str,
    unlock: Stage,
    file: &'static str,
) -> Scenario {
    Scenario::new(id, slug, unlock, Form::CHarness(file))
}

const fn gate(id: &'static str, slug: &'static str, measure: Measure) -> Scenario {
    Scenario::new(id, slug, Stage::U10, Form::Gate(measure))
}

const HEAP_BOUNDARY: &str = "heap_boundary.c";
const KERNEL_FAULTS: &str = "kernel_faults.c";
const TASK_TERMINAL: &[&str] = &["task_terminal"];

use Stage::*;

pub static SCENARIOS: [Scenario; 149] = [
    // Static: compile-pass.
    fixture("SP01", "static-contract-example", U1),
    fixture("SP02", "spec-inside-using-shareable-provider", U2),
    fixture("SP03", "start-process-matches-every-start-error", U1),
    fixture("SP04", "running-sent-in-a-message-starts-a-sibling", U2),
    fixture("SP05", "managed-process-sent-through-a-channel", U2),
    fixture("SP06", "body-captures-address-and-running", U2),
    fixture("SP07", "zero-deadline-and-zero-limit-in-range", U1),
    fixture("SP08", "rendered-send-and-start-contracts", U1),
    fixture("SP09", "same-process-name-in-two-modules", U1),
    // Static: compile-fail.
    fixture("SF01", "body-needs-unshareable-provider", U2),
    fixture("SF02", "body-captures-channel-receiver", U2),
    fixture("SF03", "body-captures-managed-process", U2),
    fixture("SF04", "process-name-message-holds-task", U1),
    fixture("SF05", "spec-message-holds-task", U1),
    fixture("SF06", "register-local-copy-of-name", U1),
    fixture("SF07", "register-supervisor-address", U1),
    fixture("SF08", "register-through-helper-parameter", U1),
    fixture("SF09", "zero-mailbox-capacity", U1),
    fixture("SF10", "zero-supervisor-capacity", U1),
    fixture("SF11", "negative-restart-limit", U1),
    fixture("SF12", "members-literal-over-capacity", U1),
    fixture("SF13", "negative-shutdown-within", U1),
    fixture("SF14", "zero-restart-window", U1),
    fixture("SF15", "duplicate-name-in-policy-literal", U1),
    fixture("SF16", "send-wrong-message-type", U1),
    fixture("SF17", "register-name-of-other-message-type", U1),
    fixture("SF18", "spec-directly-in-members-list", U1),
    fixture("SF19", "address-after-remove-process", U2),
    fixture("SF20", "receiver-used-after-sending-in-message", U2),
    fixture("SF21", "inbox-captured-by-task", U2),
    fixture("SF22", "inbox-returned-from-helper", U2),
    fixture("SF23", "inbox-sent-in-message", U2),
    fixture("SF24", "managed-handle-removed-in-loop", U2),
    fixture("SF25", "start-process-case-omits-stopped", U1),
    fixture("SF26", "start-case-has-capacity-reached-arm", U1),
    fixture("SF27", "monitor-notice-must-not-fail", U11),
    fixture("SF28", "monitor-notice-is-kept-by-the-monitor", U11),
    // Verified Core.
    Scenario::new(
        "CO01",
        "process-name-constant-address-identity",
        U2,
        Form::Check(super::core_kind::co01),
    ),
    Scenario::new(
        "CO02",
        "register-takes-constant-address",
        U2,
        Form::Check(super::core_kind::co02),
    ),
    Scenario::new(
        "CO03",
        "process-operations-are-intrinsics",
        U2,
        Form::Check(super::core_kind::co03),
    ),
    Scenario::new(
        "CO04",
        "verifier-rejects-inbox-outside-body",
        U2,
        Form::Check(super::core_kind::co04),
    ),
    // Runtime: mailbox and delivery.
    fixture("MB01", "acceptance-order-across-senders", U5),
    fixture("MB02", "full-returns-the-message", U5),
    fixture("MB03", "body-send-to-full-peer-does-not-park", U5),
    fixture("MB04", "declared-member-routable-before-first-run", U5),
    fixture("MB05", "unread-messages-not-carried-into-replacement", U6),
    fixture("MB06", "send-after-return-is-unavailable", U5),
    fixture("MB07", "process-failure-stays-in-process", U5),
    fixture("MB08", "removal-drains-queued-then-cancels", U7),
    fixture("MB09", "no-routing-during-drain", U7),
    fixture("MB10", "computed-zero-capacity-defects-at-spec", U6).defects(),
    fixture("MB11", "terminal-ends-are-lifecycle-only", U5),
    fixture("MB12", "own-address-is-direct", U11),
    fixture("MB13", "queued-and-capacity", U11),
    fixture("MB14", "monitor-reports-each-end", U11),
    fixture("MB15", "notice-passes-full-mailbox", U11),
    fixture("MB16", "unmonitor-removes-queued-notice", U11),
    fixture("MB17", "watcher-end-releases-notices", U11),
    fixture("MB18", "named-monitor-watches-one-incarnation", U11),
    fixture("MB19", "notice-during-drain-is-dropped", U12),
    // Runtime: routing.
    fixture("RT01", "named-follows-replacement-direct-goes-stale", U6),
    fixture("RT02", "never-member-name-released", U6),
    fixture("RT03", "name-taken-text-matches-trace-label", U7),
    fixture("RT04", "same-name-in-two-modules-routes-apart", U7),
    fixture("RT05", "name-reusable-after-removal", U7),
    fixture("RT06", "second-root-name-taken-rolls-back", U7),
    fixture("RT07", "computed-double-register-defects", U7).defects(),
    fixture("RT08", "address-is-a-shareable-value", U5),
    // Runtime: supervisor lifecycle and admission.
    fixture("SV01", "declared-members-start-in-order", U5),
    fixture("SV02", "fail-fast-capacity-admission", U7),
    fixture("SV03", "on-defect-not-restarted-on-other-ends", U6),
    fixture("SV04", "never-member-not-restarted", U6),
    fixture("SV05", "member-stops-its-own-supervisor", U7),
    fixture("SV06", "operations-on-stopped-supervisor", U7),
    fixture("SV07", "shared-running-starts-a-sibling", U7),
    fixture("SV08", "managed-process-removed-by-another-task", U7),
    fixture("SV09", "declared-nested-supervisor-order", U9),
    fixture("SV10", "dynamic-nested-supervisor", U9),
    fixture("SV11", "computed-out-of-range-values-defect", U6)
        .defects()
        .runs(&["capacity", "limit", "window", "members", "shutdown_within"]),
    fixture("SV12", "remove-through-foreign-running-defects", U7).defects(),
    // Runtime: restart accounting.
    fixture("RA01", "replacement-fresh-peer-keeps-state", U6),
    fixture("RA02", "limit-is-per-supervisor", U6).defects(),
    fixture("RA03", "window-rolls", U6),
    fixture("RA04", "window-edge-is-outside", U6),
    fixture("RA05", "zero-limit-escalates-first-defect", U6).defects(),
    c_harness("RA06", "failed-replacement-is-charged", U7, KERNEL_FAULTS),
    fixture("RA07", "child-tasks-end-before-replacement", U8).with_events(TASK_TERMINAL),
    fixture("RA08", "restart-storm-is-deterministic", U6),
    // Runtime: escalation.
    fixture("ES01", "root-escalation-transcript", U6).defects(),
    fixture("ES02", "nested-escalation-restarts-subtree", U9),
    fixture("ES03", "nested-never-escalation-stays-stopped", U9),
    fixture("ES04", "nested-escalation-chains-to-root", U9).defects(),
    fixture("ES05", "escalation-stop-order", U6).defects(),
    // Runtime: removal races.
    fixture("RR01", "removal-waits-for-termination", U7),
    fixture("RR02", "removal-during-replacement-start", U7),
    fixture("RR03", "defect-during-removal-drain", U7),
    fixture("RR04", "removal-after-return", U7),
    fixture("RR05", "removal-lets-handler-finish", U12),
    fixture("RR06", "removal-during-stop", U8),
    // Runtime: shutdown and forced termination.
    fixture("SD01", "stop-order", U5),
    fixture("SD02", "zero-deadline-forces", U8),
    fixture("SD03", "deadline-unused-when-drained", U8),
    c_harness(
        "SD04",
        "reparked-cleanup-forced-at-deadline",
        U8,
        KERNEL_FAULTS,
    ),
    fixture("SD05", "member-children-end-before-next-member", U8).with_events(TASK_TERMINAL),
    fixture("SD06", "older-member-routable-while-newer-drains", U12),
    fixture("SD07", "shutdown-ends-not-restarted", U8),
    fixture("SD08", "member-stops-own-root", U7),
    fixture("SD09", "second-stop-is-silent", U5),
    fixture("SD10", "deadline-forces-in-flight-handler", U12),
    fixture("SD11", "draining-handler-awaits-child-task", U12),
    fixture("SD12", "draining-handler-times-out", U12),
    fixture("SG01", "terminate-drains-then-exits", U12)
        .signalled(&["--signal=TERM@1500ms"], 143)
        .with_events(&["clock_signal_received"]),
    fixture("SG02", "interrupt-cancels-main-without-supervisors", U12)
        .signalled(&["--signal=INT@2s"], 130)
        .with_events(&["clock_signal_received"]),
    fixture("SG03", "signal-after-main-ends-changes-nothing", U12)
        .signalled(&["--signal=TERM@10s"], 0),
    // Heap: copy-boundary safety.
    fixture("CB01", "message-survives-sender-reclamation", U5),
    fixture("CB02", "copy-preserves-sharing", U5),
    fixture("CB03", "failed-send-returns-intact-value", U5),
    fixture("CB04", "channel-value-survives-process-removal", U7),
    fixture("CB05", "task-result-survives-child-heap", U8),
    c_harness("CB06", "foreign-pointer-not-followed", U3, HEAP_BOUNDARY),
    c_harness(
        "CB07",
        "foreign-pointer-send-defects-sender",
        U5,
        HEAP_BOUNDARY,
    ),
    fixture("CB08", "provider-closure-per-incarnation", U6),
    // Heap: memory flatness.
    fixture("MF01", "mailbox-memory-flat-over-uptime", U5).runs(&["10000", "1000000"]),
    fixture("MF02", "memory-flat-over-restarts", U10).runs(&["100", "10000"]),
    fixture("MF03", "memory-flat-over-memberships", U10).runs(&["100", "10000"]),
    fixture("MF04", "small-fragments-share-slabs", U5),
    fixture("MF05", "task-loop-reclaims", U3).runs(&["10000", "1000000"]),
    Scenario::new(
        "MF06",
        "pay-for-play-linking",
        U5,
        Form::Check(super::heap_kind::pay_for_play_linking),
    ),
    // Observability and backend support.
    fixture("OB01", "trace-off-constructs-nothing", U5),
    fixture("OB02", "boundaries-omit-sends", U5),
    fixture("OB03", "full-send-fields", U5),
    Scenario::new(
        "OB04",
        "transcripts-are-deterministic",
        U6,
        Form::Check(super::observability_kind::transcripts_are_deterministic),
    ),
    fixture("OB05", "one-shared-sequence", U5),
    fixture("OB06", "virtual-time-costs-no-wall-time", U6),
    Scenario::new(
        "OB07",
        "trace-abi-version",
        U5,
        Form::Check(super::observability_kind::trace_abi_version),
    ),
    fixture("OB08", "trace-off-root-names-process-defect", U5)
        .defects()
        .runs(&["overflow", "divide"]),
    fixture("IP01", "interpreter-rejects-processes", U5).defects(),
    // Forcing program.
    fixture("FP01", "supervised-pool", U10),
    fixture("FP02", "service-pool", U12)
        .signalled(&["--signal=TERM@1500ms"], 143)
        .with_events(&["clock_signal_received"]),
    // Performance gates.
    gate("PG01", "message-cost-trace-off", super::performance::pg01),
    gate("PG02", "trace-overhead", super::performance::pg02),
    gate("PG03", "restart-cycle-cost", super::performance::pg03),
    gate("PG04", "start-and-stop-cost", super::performance::pg04),
    gate("PG05", "start-and-stop-scaling", super::performance::pg05),
    gate("PG06", "message-cost-by-payload", super::performance::pg06),
    gate("PG07", "resident-memory-growth", super::performance::pg07),
    gate("PG08", "parked-resident-memory", super::performance::pg08),
    gate("PG09", "cold-start", super::performance::pg09),
    gate("PG10", "trace-off-events", super::performance::pg10),
];

pub fn find(id: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|scenario| scenario.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    const SPECIFICATION: &str =
        include_str!("../../../../docs/specification/supervised-process-conformance.md");

    fn section<'a>(heading: &str, text: &'a str) -> &'a str {
        let start = text
            .find(heading)
            .expect("heading is in the lab specification");
        let rest = &text[start + heading.len()..];
        let end = rest.find("\n## ").unwrap_or(rest.len());
        &rest[..end]
    }

    fn cells(row: &str) -> Vec<&str> {
        row.trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect()
    }

    fn is_scenario_id(text: &str) -> bool {
        text.len() == 4
            && text[..2].bytes().all(|byte| byte.is_ascii_uppercase())
            && text[2..].bytes().all(|byte| byte.is_ascii_digit())
    }

    /// Expands `SP01, SP03, SP07–SP09` into each ID.
    fn expand(list: &str) -> Vec<String> {
        let mut ids = Vec::new();
        for item in list
            .split(',')
            .map(str::trim)
            .filter(|item| *item != "none")
        {
            match item.split_once('–') {
                Some((first, last)) => {
                    let prefix = &first[..2];
                    assert_eq!(prefix, &last[..2], "range {item} spans prefixes");
                    let first: u32 = first[2..].parse().unwrap();
                    let last: u32 = last[2..].parse().unwrap();
                    ids.extend((first..=last).map(|number| format!("{prefix}{number:02}")));
                }
                None => ids.push(item.to_owned()),
            }
        }
        ids
    }

    fn catalogue_unlocks() -> BTreeMap<&'static str, Stage> {
        SCENARIOS
            .iter()
            .map(|scenario| (scenario.id, scenario.unlock))
            .collect()
    }

    // Rejects a catalogue whose unlock stages drift from the lab's unlock-order
    // table, which is what decides the scenarios each stage commit turns green.
    #[test]
    fn unlock_stages_match_the_unlock_order_table() {
        let mut specified = BTreeMap::new();
        for row in section("## Unlock order", SPECIFICATION).lines() {
            let row = cells(row);
            let Some(stage) = row.first().and_then(|cell| Stage::parse(cell)) else {
                continue;
            };
            for id in expand(row[2]) {
                assert!(
                    specified.insert(id.clone(), stage).is_none(),
                    "{id} unlocks twice"
                );
            }
        }
        let catalogue = catalogue_unlocks()
            .into_iter()
            .map(|(id, stage)| (id.to_owned(), stage))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(catalogue, specified);
    }

    // Rejects a catalogue row that disagrees with the scenario tables, where
    // each row states its own unlock stage.
    #[test]
    fn unlock_stages_match_the_scenario_tables() {
        let unlocks = catalogue_unlocks();
        let mut rows = 0;
        let tables = section("## Scenario catalogue", SPECIFICATION);
        let tables = &tables[..tables
            .find("### Catalogue data")
            .expect("catalogue data table")];
        for row in tables.lines() {
            let row = cells(row);
            if row.len() < 2 || !is_scenario_id(row[0]) {
                continue;
            }
            let stage = Stage::parse(row[1]).expect("a scenario row names its unlock stage");
            assert_eq!(unlocks.get(row[0]), Some(&stage), "{}", row[0]);
            rows += 1;
        }
        // Every scenario except the forcing programs and the gates, which
        // carry no unlock column.
        assert_eq!(rows, SCENARIOS.len() - 2 - 10);
    }

    // Rejects a duplicated or missing ID, and an ID whose prefix maps to no
    // evidence kind.
    #[test]
    fn the_catalogue_has_every_counted_scenario_once() {
        let ids = SCENARIOS
            .iter()
            .map(|scenario| scenario.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), SCENARIOS.len());
        let mut counts = BTreeMap::<&str, usize>::new();
        for id in &ids {
            assert!(Kind::of_scenario(id).is_some(), "{id} has no evidence kind");
            *counts.entry(&id[..2]).or_default() += 1;
        }
        let mut counted = 0;
        for row in section("## Scenario counts", SPECIFICATION).lines() {
            let row = cells(row);
            let Some(Ok(count)) = row.get(1).map(|cell| cell.parse::<usize>()) else {
                continue;
            };
            let actual = row[0]
                .match_indices('(')
                .map(|(start, _)| {
                    counts
                        .get(&row[0][start + 1..start + 3])
                        .copied()
                        .unwrap_or(0)
                })
                .sum::<usize>();
            assert_eq!(actual, count, "{}", row[0]);
            counted += count;
        }
        assert_eq!(counted, SCENARIOS.len());
    }

    fn form_name(form: Form) -> String {
        match form {
            Form::Fixture => "fixture".into(),
            Form::CHarness(file) => format!("C `{file}`"),
            Form::Check(_) => "check".into(),
            Form::Gate(_) => "gate".into(),
        }
    }

    fn listed(cell: &str) -> Vec<String> {
        if cell == "-" {
            return Vec::new();
        }
        cell.split(", ")
            .map(|item| item.trim_matches('`').to_owned())
            .collect()
    }

    // Rejects a catalogue whose form, harness file, exit code, runs, extra
    // events, or runner options differ from the specification's catalogue data. Unlisted
    // scenarios must keep every default.
    #[test]
    fn forms_exit_codes_runs_and_extra_events_match_the_catalogue_data() {
        let default = ("fixture".to_owned(), 0, Vec::new(), Vec::new(), Vec::new());
        let mut specified = BTreeMap::new();
        let table = section("### Catalogue data", SPECIFICATION);
        for row in table.lines() {
            let row = cells(row);
            if row.len() != 6 || !is_scenario_id(&row[0][..4.min(row[0].len())]) {
                continue;
            }
            let data = (
                row[1].to_owned(),
                row[2].parse::<i32>().expect("an exit code"),
                listed(row[3]),
                listed(row[4]),
                listed(row[5]),
            );
            for id in expand(row[0]) {
                assert!(
                    specified.insert(id.clone(), data.clone()).is_none(),
                    "{id} listed twice"
                );
            }
        }
        for scenario in &SCENARIOS {
            let actual = (
                form_name(scenario.form),
                scenario.exit_code,
                scenario
                    .arguments
                    .iter()
                    .map(|item| (*item).to_owned())
                    .collect::<Vec<_>>(),
                scenario
                    .extra_events
                    .iter()
                    .map(|item| (*item).to_owned())
                    .collect::<Vec<_>>(),
                scenario
                    .runner
                    .iter()
                    .map(|item| (*item).to_owned())
                    .collect::<Vec<_>>(),
            );
            let expected = specified
                .remove(scenario.id)
                .unwrap_or_else(|| default.clone());
            assert_eq!(actual, expected, "{}", scenario.id);
        }
        assert!(
            specified.is_empty(),
            "listed but not catalogued: {specified:?}"
        );
    }
}
