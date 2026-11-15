//! Runner telemetry -> canonical transcript (lab specification,
//! "`expected.transcript`" and "Runner telemetry for process and clock
//! events", which fix the key set of each event family checked here).

use crate::json::{JsonData, decode_json};

const SCHEMA: &str = "chimi.runtime.v1";
const CLOCK_ADVANCED: &str = "clock_advanced";
const SIGNAL_RECEIVED: &str = "clock_signal_received";
const PROCESS_SEND: &str = "process_send";

const ENVELOPE: &[&str] = &[
    "schema",
    "event",
    "sequence",
    "computation",
    "execution_id",
    "task_id",
    "parent_task_id",
    "cause_task_id",
    "channel_id",
    "attributes",
];
const PROCESS_IDENTITIES: &[&str] = &[
    "supervisor_id",
    "membership_id",
    "incarnation_id",
    "parent_supervisor_id",
];
const SCHEDULER_ATTRIBUTES: &[&str] = &[
    "scheduler_sequence",
    "capacity",
    "queue_depth",
    "waiter_depth",
    "discard_count",
];
const PROCESS_ATTRIBUTES: &[&str] = &[
    "time_ns",
    "cause",
    "name",
    "address_kind",
    "outcome",
    "attempts_in_window",
    "defect",
];
const CLOCK_ATTRIBUTES: &[&str] = &["time_ns", "request_kind"];

/// Process-family event names in trace-kind order (kinds 1 to 10).
const PROCESS_EVENTS: &[&str] = &[
    "process_started",
    "admission_rejected",
    "process_terminated",
    "restart_attempted",
    "restart_limit_exceeded",
    "supervisor_escalated",
    "membership_removed",
    "supervisor_stopped",
    PROCESS_SEND,
    "supervisor_started",
];
const CLOCK_EVENTS: &[&str] = &[
    CLOCK_ADVANCED,
    "clock_timer_started",
    "clock_timer_fired",
    "clock_timer_cancelled",
    "clock_deadlock_waiter",
    "clock_scope_fired",
    SIGNAL_RECEIVED,
];

/// Projects telemetry onto the transcript lines the lab compares.
///
/// Keeps the process-family events of the transcript table, the extra events
/// the scenario names, and the clock advances, which set the time printed on
/// later lines but are not printed. Fails on telemetry that breaks the
/// runner's contract: a malformed line, a foreign schema, a `sequence` that
/// does not increase, time that moves backwards, or a kept event with a
/// missing or unknown field.
pub fn project(telemetry: &str, extra_events: &[&str]) -> Result<Vec<String>, String> {
    if let Some(unknown) = extra_events
        .iter()
        .find(|event| !["task_terminal", SIGNAL_RECEIVED].contains(*event))
    {
        return Err(format!("extra event `{unknown}` has no transcript form"));
    }
    let mut lines = Vec::new();
    let mut now: u64 = 0;
    let mut last_sequence: Option<u64> = None;
    for (index, line) in telemetry.lines().enumerate() {
        let event =
            Event::parse(line).map_err(|error| format!("telemetry line {}: {error}", index + 1))?;
        if last_sequence.is_some_and(|last| event.sequence <= last) {
            return Err(format!(
                "telemetry line {}: sequence {} does not follow {}",
                index + 1,
                event.sequence,
                last_sequence.unwrap_or_default()
            ));
        }
        last_sequence = Some(event.sequence);
        let name = event.name.as_str();
        let family = if PROCESS_EVENTS.contains(&name) {
            Some((PROCESS_IDENTITIES, PROCESS_ATTRIBUTES))
        } else if CLOCK_EVENTS.contains(&name) {
            Some((&[][..], CLOCK_ATTRIBUTES))
        } else if extra_events.contains(&name) {
            Some((&[][..], &[][..]))
        } else {
            None
        };
        if let Some((identities, attributes)) = family {
            event
                .expect_keys(identities, attributes)
                .map_err(|error| format!("telemetry line {}: {error}", index + 1))?;
        }
        if CLOCK_EVENTS.contains(&name)
            && lookup(&event.fields, "cause_task_id") != Some(&JsonData::Null)
        {
            return Err(format!(
                "telemetry line {}: `{name}` carries its request kind in attributes, not cause_task_id",
                index + 1
            ));
        }
        let rendered = if event.name == CLOCK_ADVANCED {
            let time = event.time()?;
            if time < now {
                return Err(format!(
                    "telemetry line {}: time moves back to {time}ns",
                    index + 1
                ));
            }
            now = time;
            None
        } else if event.name == SIGNAL_RECEIVED {
            extra_events
                .contains(&SIGNAL_RECEIVED)
                .then(|| event.render_signal())
                .transpose()?
        } else if extra_events.contains(&event.name.as_str()) {
            Some(event.render_task()?)
        } else {
            event.render_process(now)?
        };
        if let Some(fields) = rendered {
            lines.push(format!("@{} {} {fields}", duration_text(now), event.name));
        }
    }
    Ok(lines)
}

/// The `boundaries` view of a full transcript: send events are motion events.
pub fn boundaries(full: &[String]) -> Vec<String> {
    full.iter()
        .filter(|line| line.split(' ').nth(1) != Some(PROCESS_SEND))
        .cloned()
        .collect()
}

/// The canonical `Duration` text form of a non-negative nanosecond count
/// (time-and-outcomes.md, "Text form"): `0s`, `7ns`, `250us`, `1.5ms`,
/// `30.001s`, `2m3.25s`, `1h0m30s`.
pub fn duration_text(nanoseconds: u64) -> String {
    const MICROSECOND: u64 = 1_000;
    const MILLISECOND: u64 = 1_000_000;
    const SECOND: u64 = 1_000_000_000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;
    if nanoseconds == 0 {
        return "0s".into();
    }
    if nanoseconds < SECOND {
        let (unit, suffix, digits) = if nanoseconds >= MILLISECOND {
            (MILLISECOND, "ms", 6)
        } else if nanoseconds >= MICROSECOND {
            (MICROSECOND, "us", 3)
        } else {
            (1, "ns", 0)
        };
        return format!(
            "{}{suffix}",
            decimal(nanoseconds / unit, nanoseconds % unit, digits)
        );
    }
    let hours = nanoseconds / HOUR;
    let minutes = nanoseconds % HOUR / MINUTE;
    let seconds = nanoseconds % MINUTE;
    let mut text = String::new();
    if hours > 0 {
        text.push_str(&format!("{hours}h"));
    }
    if minutes > 0 || hours > 0 && seconds > 0 {
        text.push_str(&format!("{minutes}m"));
    }
    if seconds > 0 {
        text.push_str(&format!(
            "{}s",
            decimal(seconds / SECOND, seconds % SECOND, 9)
        ));
    }
    text
}

fn decimal(whole: u64, fraction: u64, digits: usize) -> String {
    if fraction == 0 {
        return whole.to_string();
    }
    let fraction = format!("{fraction:0digits$}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

struct Event {
    name: String,
    sequence: u64,
    fields: Vec<(String, JsonData)>,
    attributes: Vec<(String, JsonData)>,
}

impl Event {
    fn parse(line: &str) -> Result<Self, String> {
        let JsonData::Object(fields) = decode_json(line).map_err(|error| error.message)? else {
            return Err("an event is not a JSON object".into());
        };
        let text = |key: &str| match lookup(&fields, key) {
            Some(JsonData::Text(value)) => Ok(value.clone()),
            _ => Err(format!("`{key}` is not a string")),
        };
        if text("schema")? != SCHEMA {
            return Err(format!("schema is not {SCHEMA}"));
        }
        let name = text("event")?;
        let sequence = number(lookup(&fields, "sequence"))
            .ok_or_else(|| format!("`{name}` has no sequence number"))?;
        let attributes = match lookup(&fields, "attributes") {
            Some(JsonData::Object(attributes)) => attributes.clone(),
            None => Vec::new(),
            Some(_) => return Err(format!("`{name}` attributes are not an object")),
        };
        Ok(Self {
            name,
            sequence,
            fields,
            attributes,
        })
    }

    /// The event has exactly the envelope plus `identities` at the top level,
    /// and exactly the scheduler attributes plus `attributes`.
    fn expect_keys(&self, identities: &[&str], attributes: &[&str]) -> Result<(), String> {
        if field_keys(&self.fields) != sorted_keys(&[ENVELOPE, identities]) {
            return Err(format!(
                "`{}` has keys {:?}",
                self.name,
                field_keys(&self.fields)
            ));
        }
        if field_keys(&self.attributes) != sorted_keys(&[SCHEDULER_ATTRIBUTES, attributes]) {
            return Err(format!(
                "`{}` has attributes {:?}",
                self.name,
                field_keys(&self.attributes)
            ));
        }
        Ok(())
    }

    fn time(&self) -> Result<u64, String> {
        number(lookup(&self.attributes, "time_ns"))
            .ok_or_else(|| format!("`{}` has no non-negative time_ns", self.name))
    }

    /// `S<n>`, `M<n>`, `I<n>`, `T<n>`, or `-` for an absent identity.
    fn identity(&self, key: &str, prefix: char) -> Result<String, String> {
        match lookup(&self.fields, key) {
            Some(JsonData::Null) => Ok("-".into()),
            value => number(value)
                .filter(|identity| *identity > 0)
                .map(|identity| format!("{prefix}{identity}"))
                .ok_or_else(|| format!("`{}` has no {key}", self.name)),
        }
    }

    fn required_identity(&self, key: &str, prefix: char) -> Result<String, String> {
        let identity = self.identity(key, prefix)?;
        if identity == "-" {
            return Err(format!("`{}` has a null {key}", self.name));
        }
        Ok(identity)
    }

    fn word(&self, key: &str, allowed: &[&str]) -> Result<String, String> {
        match lookup(&self.attributes, key) {
            Some(JsonData::Text(word)) if allowed.contains(&word.as_str()) => Ok(word.clone()),
            other => Err(format!(
                "`{}` has {key} {other:?}, expected one of {allowed:?}",
                self.name
            )),
        }
    }

    fn label(&self) -> Result<Option<String>, String> {
        match lookup(&self.attributes, "name") {
            Some(JsonData::Null) => Ok(None),
            Some(JsonData::Text(label)) if !label.is_empty() => Ok(Some(label.clone())),
            other => Err(format!("`{}` has name {other:?}", self.name)),
        }
    }

    fn required_label(&self) -> Result<String, String> {
        self.label()?
            .ok_or_else(|| format!("`{}` has no name", self.name))
    }

    /// `S<n>`, or `root` for a supervisor with no parent.
    fn parent_supervisor(&self) -> Result<String, String> {
        let parent = self.identity("parent_supervisor_id", 'S')?;
        Ok(if parent == "-" {
            "root".to_owned()
        } else {
            parent
        })
    }

    /// `signal=<number>`: the signal the root was cancelled for.
    fn render_signal(&self) -> Result<String, String> {
        let signal = number(lookup(&self.attributes, "request_kind"))
            .filter(|signal| *signal > 0)
            .ok_or_else(|| format!("`{}` has no signal number", self.name))?;
        Ok(format!("signal={signal}"))
    }

    fn render_task(&self) -> Result<String, String> {
        Ok(format!(
            "task={} parent={} cause={}",
            self.required_identity("task_id", 'T')?,
            self.identity("parent_task_id", 'T')?,
            self.identity("cause_task_id", 'T')?,
        ))
    }

    /// The fields of a transcript event, or `None` for an event the
    /// transcript drops. A kept event's own time must equal the clock.
    fn render_process(&self, now: u64) -> Result<Option<String>, String> {
        let fields = match self.name.as_str() {
            "process_started" => {
                let mut fields = format!(
                    "supervisor={} membership={} incarnation={}",
                    self.required_identity("supervisor_id", 'S')?,
                    self.required_identity("membership_id", 'M')?,
                    self.required_identity("incarnation_id", 'I')?,
                );
                if let Some(label) = self.label()? {
                    fields.push_str(&format!(" name={label}"));
                }
                fields
            }
            "admission_rejected" => {
                let reason = self.word(
                    "cause",
                    &["capacity_reached", "name_taken", "stopped", "start_failed"],
                )?;
                let mut fields = format!(
                    "supervisor={} reason={reason}",
                    self.required_identity("supervisor_id", 'S')?
                );
                if reason == "name_taken" {
                    fields.push_str(&format!(" name={}", self.required_label()?));
                }
                fields
            }
            "process_terminated" => {
                let cause = self.word(
                    "cause",
                    &["returned", "failed", "defected", "cancelled", "forced"],
                )?;
                // The defect message stays in the telemetry; the transcript
                // keeps the prototype's fields.
                match lookup(&self.attributes, "defect") {
                    Some(JsonData::Null) => {}
                    Some(JsonData::Text(_)) if cause == "defected" => {}
                    other => {
                        return Err(format!(
                            "`process_terminated` with cause {cause} has defect {other:?}"
                        ));
                    }
                }
                format!(
                    "membership={} incarnation={} cause={cause}",
                    self.required_identity("membership_id", 'M')?,
                    self.required_identity("incarnation_id", 'I')?,
                )
            }
            "restart_attempted" => format!(
                "supervisor={} membership={} attempts_in_window={}",
                self.required_identity("supervisor_id", 'S')?,
                self.required_identity("membership_id", 'M')?,
                number(lookup(&self.attributes, "attempts_in_window"))
                    .ok_or("`restart_attempted` has no attempts_in_window")?,
            ),
            "restart_limit_exceeded" | "supervisor_stopped" => format!(
                "supervisor={}",
                self.required_identity("supervisor_id", 'S')?
            ),
            "supervisor_escalated" => format!(
                "supervisor={} parent={}",
                self.required_identity("supervisor_id", 'S')?,
                self.parent_supervisor()?,
            ),
            // A nested supervisor also names the membership it fills in its
            // parent; a root has none.
            "supervisor_started" => {
                let parent = self.parent_supervisor()?;
                let membership = self.identity("membership_id", 'M')?;
                let mut fields = format!(
                    "supervisor={} parent={parent}",
                    self.required_identity("supervisor_id", 'S')?,
                );
                match (parent.as_str(), membership.as_str()) {
                    ("root", "-") => {}
                    ("root", _) => {
                        return Err(format!(
                            "root `supervisor_started` has membership {membership}"
                        ));
                    }
                    (_, "-") => {
                        return Err("nested `supervisor_started` has a null membership_id".into());
                    }
                    _ => fields.push_str(&format!(" membership={membership}")),
                }
                fields
            }
            "membership_removed" => format!(
                "membership={}",
                self.required_identity("membership_id", 'M')?
            ),
            PROCESS_SEND => {
                let kind = self.word("address_kind", &["named", "direct"])?;
                let mut fields = format!("kind={kind}");
                if kind == "named" {
                    fields.push_str(&format!(" name={}", self.required_label()?));
                }
                fields.push_str(&format!(
                    " incarnation={} outcome={}",
                    self.identity("incarnation_id", 'I')?,
                    self.word("outcome", &["accepted", "full", "unavailable"])?,
                ));
                fields
            }
            _ => return Ok(None),
        };
        let time = self.time()?;
        if time != now {
            return Err(format!(
                "`{}` at {time}ns after the clock advanced to {now}ns",
                self.name
            ));
        }
        Ok(Some(fields))
    }
}

fn field_keys(fields: &[(String, JsonData)]) -> Vec<&str> {
    let mut keys = fields
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

fn sorted_keys<'a>(parts: &[&[&'a str]]) -> Vec<&'a str> {
    let mut keys = parts.concat();
    keys.sort_unstable();
    keys
}

fn lookup<'a>(fields: &'a [(String, JsonData)], key: &str) -> Option<&'a JsonData> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn number(value: Option<&JsonData>) -> Option<u64> {
    match value {
        Some(JsonData::Number(text)) => text.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENVELOPE: &str =
        r#""schema":"chimi.runtime.v1","computation":"app/main.main","execution_id":"lab""#;

    /// One runner line. `identities` and `attributes` are JSON members
    /// without braces.
    fn line(event: &str, sequence: u64, identities: &str, attributes: &str) -> String {
        let identities = if identities.is_empty() {
            String::new()
        } else {
            format!(",{identities}")
        };
        format!(
            r#"{{{ENVELOPE},"event":"{event}","sequence":{sequence}{identities},"attributes":{{{attributes}}}}}"#
        )
    }

    /// A process-family line with every identity and attribute the runner
    /// writes for that family, absent ones as `null`.
    #[allow(clippy::too_many_arguments)]
    fn process(
        event: &str,
        sequence: u64,
        time: u64,
        [task, supervisor, membership, incarnation, parent]: [Option<u64>; 5],
        cause: Option<&str>,
        name: Option<&str>,
        send: Option<(&str, &str)>,
        attempts: u64,
    ) -> String {
        let id = |value: Option<u64>| value.map_or("null".to_owned(), |value| value.to_string());
        let text =
            |value: Option<&str>| value.map_or("null".to_owned(), |value| format!("\"{value}\""));
        line(
            event,
            sequence,
            &format!(
                r#""task_id":{},"parent_task_id":null,"cause_task_id":null,"channel_id":null,"supervisor_id":{},"membership_id":{},"incarnation_id":{},"parent_supervisor_id":{}"#,
                id(task),
                id(supervisor),
                id(membership),
                id(incarnation),
                id(parent)
            ),
            &format!(
                r#""scheduler_sequence":{sequence},"capacity":0,"queue_depth":0,"waiter_depth":0,"discard_count":0,"time_ns":{time},"cause":{},"name":{},"address_kind":{},"outcome":{},"attempts_in_window":{attempts},"defect":null"#,
                text(cause),
                text(name),
                text(send.map(|send| send.0)),
                text(send.map(|send| send.1)),
            ),
        )
    }

    /// A clock-family line. The request kind lives in attributes, never in
    /// `cause_task_id`.
    fn clock(event: &str, sequence: u64, task: Option<u64>, time: u64) -> String {
        let task = task.map_or("null".to_owned(), |task| task.to_string());
        line(
            event,
            sequence,
            &format!(
                r#""task_id":{task},"parent_task_id":null,"cause_task_id":null,"channel_id":null"#
            ),
            &format!(
                r#""scheduler_sequence":{sequence},"capacity":0,"queue_depth":0,"waiter_depth":0,"discard_count":0,"time_ns":{time},"request_kind":0"#
            ),
        )
    }

    fn advance(sequence: u64, time: u64) -> String {
        clock(CLOCK_ADVANCED, sequence, None, time)
    }

    fn task(
        event: &str,
        sequence: u64,
        task: u64,
        parent: Option<u64>,
        cause: Option<u64>,
    ) -> String {
        let id = |value: Option<u64>| value.map_or("null".to_owned(), |value| value.to_string());
        line(
            event,
            sequence,
            &format!(
                r#""task_id":{task},"parent_task_id":{},"cause_task_id":{},"channel_id":null"#,
                id(parent),
                id(cause)
            ),
            r#""scheduler_sequence":1,"capacity":0,"queue_depth":0,"waiter_depth":0,"discard_count":0"#,
        )
    }

    const NONE: [Option<u64>; 5] = [None; 5];

    fn ids(supervisor: u64, membership: u64, incarnation: u64) -> [Option<u64>; 5] {
        [
            None,
            Some(supervisor),
            Some(membership),
            Some(incarnation),
            None,
        ]
    }

    /// The ES01 run as the runner writes it, with the task and clock events
    /// the transcript drops.
    fn es01_telemetry() -> String {
        const MS: u64 = 1_000_000;
        let counter = Some("app/main.counter");
        let named = Some(("named", "accepted"));
        [
            task("task_started", 1, 1, None, None),
            process(
                "supervisor_started",
                2,
                0,
                [None, Some(1), None, None, None],
                None,
                None,
                None,
                0,
            ),
            process(
                "process_started",
                3,
                0,
                [Some(2), Some(1), Some(1), Some(1), None],
                None,
                counter,
                None,
                0,
            ),
            process(
                "process_started",
                4,
                0,
                [Some(3), Some(1), Some(2), Some(2), None],
                None,
                Some("app/main.peer"),
                None,
                0,
            ),
            process(
                "process_send",
                5,
                0,
                [None, None, None, Some(1), None],
                None,
                counter,
                named,
                0,
            ),
            task("task_parked", 6, 1, None, None),
            process(
                "process_terminated",
                7,
                0,
                [None, None, Some(1), Some(1), None],
                Some("defected"),
                None,
                None,
                0,
            ),
            process(
                "restart_attempted",
                8,
                0,
                [None, Some(1), Some(1), None, None],
                None,
                None,
                None,
                1,
            ),
            process(
                "process_started",
                9,
                0,
                [Some(4), Some(1), Some(1), Some(3), None],
                None,
                counter,
                None,
                0,
            ),
            advance(10, MS),
            clock("clock_timer_fired", 11, Some(1), MS),
            process(
                "process_send",
                12,
                MS,
                [None, None, None, Some(3), None],
                None,
                counter,
                named,
                0,
            ),
            process(
                "process_terminated",
                13,
                MS,
                [None, None, Some(1), Some(3), None],
                Some("defected"),
                None,
                None,
                0,
            ),
            process(
                "restart_limit_exceeded",
                14,
                MS,
                [None, Some(1), None, None, None],
                None,
                None,
                None,
                0,
            ),
            process(
                "process_terminated",
                15,
                MS,
                [None, None, Some(2), Some(2), None],
                Some("cancelled"),
                None,
                None,
                0,
            ),
            process(
                "supervisor_escalated",
                16,
                MS,
                [None, Some(1), None, None, None],
                None,
                None,
                None,
                0,
            ),
            task("task_terminal", 17, 1, None, Some(2)),
        ]
        .join("\n")
            + "\n"
    }

    // Copied from the lab specification's ES01 transcript. Rejects a
    // projector that prints dropped events, prints clock advances, renders
    // time or identities in another form, or orders fields differently.
    #[test]
    fn es01_telemetry_projects_onto_the_specified_transcript() {
        let expected = [
            "@0s supervisor_started supervisor=S1 parent=root",
            "@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter",
            "@0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.peer",
            "@0s process_send kind=named name=app/main.counter incarnation=I1 outcome=accepted",
            "@0s process_terminated membership=M1 incarnation=I1 cause=defected",
            "@0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1",
            "@0s process_started supervisor=S1 membership=M1 incarnation=I3 name=app/main.counter",
            "@1ms process_send kind=named name=app/main.counter incarnation=I3 outcome=accepted",
            "@1ms process_terminated membership=M1 incarnation=I3 cause=defected",
            "@1ms restart_limit_exceeded supervisor=S1",
            "@1ms process_terminated membership=M2 incarnation=I2 cause=cancelled",
            "@1ms supervisor_escalated supervisor=S1 parent=root",
        ];
        assert_eq!(project(&es01_telemetry(), &[]).unwrap(), expected);
        assert_eq!(
            boundaries(&project(&es01_telemetry(), &[]).unwrap()),
            expected
                .iter()
                .filter(|line| !line.contains(" process_send "))
                .map(|line| (*line).to_owned())
                .collect::<Vec<_>>()
        );
    }

    // The U9 form of `supervisor_started` (lab open question 5): a root
    // names `parent=root` and no membership; a nested supervisor names its
    // parent and the membership it fills. Rejects a projector that drops the
    // event, prints a nested one as a root, or omits or invents a membership.
    #[test]
    fn supervisor_started_names_its_parent_and_nested_membership() {
        let telemetry = [
            process(
                "supervisor_started",
                1,
                0,
                [None, Some(1), None, None, None],
                None,
                None,
                None,
                0,
            ),
            process(
                "supervisor_started",
                2,
                0,
                [None, Some(2), Some(3), None, Some(1)],
                None,
                None,
                None,
                0,
            ),
        ]
        .join("\n");
        assert_eq!(
            project(&telemetry, &[]).unwrap(),
            [
                "@0s supervisor_started supervisor=S1 parent=root",
                "@0s supervisor_started supervisor=S2 parent=S1 membership=M3",
            ]
        );
        for malformed in [
            process(
                "supervisor_started",
                1,
                0,
                [None, Some(1), Some(2), None, None],
                None,
                None,
                None,
                0,
            ),
            process(
                "supervisor_started",
                1,
                0,
                [None, Some(2), None, None, Some(1)],
                None,
                None,
                None,
                0,
            ),
        ] {
            assert!(project(&malformed, &[]).is_err(), "{malformed}");
        }
    }

    // Rejects dropping the extra events a scenario asks for, keeping them
    // when it does not, and rendering absent task identities as anything but
    // `-`.
    #[test]
    fn extra_events_are_kept_only_when_named() {
        let telemetry = [
            task("task_started", 1, 1, None, None),
            task("task_terminal", 2, 2, Some(1), None),
            advance(3, 30_001_000_000),
            task("task_terminal", 4, 1, None, Some(2)),
        ]
        .join("\n");
        assert_eq!(project(&telemetry, &[]).unwrap(), Vec::<String>::new());
        assert_eq!(
            project(&telemetry, &["task_terminal"]).unwrap(),
            [
                "@0s task_terminal task=T2 parent=T1 cause=-",
                "@30.001s task_terminal task=T1 parent=- cause=T2",
            ]
        );
        assert!(project(&telemetry, &["channel_closed"]).is_err());
    }

    // Rejects a projector that prints a name on an unnamed start, an
    // incarnation other than `-` on a refused send, or drops the name of a
    // name_taken rejection.
    #[test]
    fn optional_fields_appear_only_where_the_table_allows() {
        let telemetry = [
            process("process_started", 1, 0, ids(1, 3, 4), None, None, None, 0),
            process(
                "process_send",
                2,
                0,
                [None, None, None, None, None],
                None,
                None,
                Some(("direct", "unavailable")),
                0,
            ),
            process(
                "process_send",
                3,
                0,
                [None, None, None, None, None],
                None,
                Some("app/main.p"),
                Some(("named", "full")),
                0,
            ),
            process(
                "admission_rejected",
                4,
                0,
                [None, Some(1), None, None, None],
                Some("name_taken"),
                Some("app/main.counter"),
                None,
                0,
            ),
            process(
                "admission_rejected",
                5,
                0,
                [None, Some(1), None, None, None],
                Some("capacity_reached"),
                None,
                None,
                0,
            ),
            process(
                "supervisor_escalated",
                6,
                0,
                [None, Some(2), None, None, Some(1)],
                None,
                None,
                None,
                0,
            ),
            process(
                "membership_removed",
                7,
                0,
                [None, None, Some(3), None, None],
                None,
                None,
                None,
                0,
            ),
            process(
                "admission_rejected",
                8,
                0,
                [None, Some(1), None, None, None],
                Some("start_failed"),
                None,
                None,
                0,
            ),
        ]
        .join("\n");
        assert_eq!(
            project(&telemetry, &[]).unwrap(),
            [
                "@0s process_started supervisor=S1 membership=M3 incarnation=I4",
                "@0s process_send kind=direct incarnation=- outcome=unavailable",
                "@0s process_send kind=named name=app/main.p incarnation=- outcome=full",
                "@0s admission_rejected supervisor=S1 reason=name_taken name=app/main.counter",
                "@0s admission_rejected supervisor=S1 reason=capacity_reached",
                "@0s supervisor_escalated supervisor=S2 parent=S1",
                "@0s membership_removed membership=M3",
                "@0s admission_rejected supervisor=S1 reason=start_failed",
            ]
        );
    }

    #[test]
    fn off_mode_telemetry_projects_to_nothing() {
        assert_eq!(
            project("", &["task_terminal"]).unwrap(),
            Vec::<String>::new()
        );
    }

    // Each case is telemetry that breaks the runner contract. A projector
    // that tolerated it would let a broken runner pass a transcript.
    #[test]
    fn telemetry_that_breaks_the_runner_contract_is_rejected() {
        let started = |sequence, time| {
            process(
                "process_started",
                sequence,
                time,
                ids(1, 1, 1),
                None,
                None,
                None,
                0,
            )
        };
        // The unbroken lines the cases edit are valid.
        for valid in [
            started(1, 0),
            advance(1, 5),
            task("task_terminal", 1, 1, None, None),
        ] {
            assert!(project(&valid, &["task_terminal"]).is_ok(), "{valid}");
        }
        let cases = [
            ("not JSON", "{".to_owned()),
            (
                "foreign schema",
                started(1, 0).replace("chimi.runtime.v1", "chimi.runtime.v0"),
            ),
            (
                "repeated sequence",
                [started(1, 0), started(1, 0)].join("\n"),
            ),
            ("time moves back", [advance(1, 5), advance(2, 4)].join("\n")),
            (
                "event time disagrees with the clock",
                [advance(1, 5), started(2, 0)].join("\n"),
            ),
            (
                "missing identity",
                process(
                    "process_started",
                    1,
                    0,
                    [None, Some(1), None, Some(1), None],
                    None,
                    None,
                    None,
                    0,
                ),
            ),
            (
                "unknown cause",
                process(
                    "process_terminated",
                    1,
                    0,
                    ids(1, 1, 1),
                    Some("exploded"),
                    None,
                    None,
                    0,
                ),
            ),
            (
                "named send without a name",
                process(
                    "process_send",
                    1,
                    0,
                    NONE,
                    None,
                    None,
                    Some(("named", "accepted")),
                    0,
                ),
            ),
            (
                "advance without time",
                advance(1, 5).replace(r#""time_ns":5,"#, ""),
            ),
            (
                "unexpected attribute on a kept event",
                started(1, 0).replace(
                    r#""attempts_in_window":0"#,
                    r#""attempts_in_window":0,"extra":1"#,
                ),
            ),
            (
                "unexpected identity on a kept event",
                started(1, 0).replace(r#""channel_id":null"#, r#""channel_id":null,"region_id":3"#),
            ),
            (
                "unexpected key on supervisor_started",
                process(
                    "supervisor_started",
                    1,
                    0,
                    [None, Some(1), None, None, None],
                    None,
                    None,
                    None,
                    0,
                )
                .replace(r#""channel_id":null"#, r#""channel_id":null,"region_id":3"#),
            ),
            (
                "clock request kind in cause_task_id",
                advance(1, 5).replace(r#""cause_task_id":null"#, r#""cause_task_id":5"#),
            ),
            (
                "task_terminal without the scheduler attributes",
                task("task_terminal", 1, 1, None, None).replace(r#","discard_count":0"#, ""),
            ),
        ];
        for (case, telemetry) in cases {
            assert!(project(&telemetry, &["task_terminal"]).is_err(), "{case}");
        }
    }

    // Expected strings from time-and-outcomes.md "Text form" and the lab's
    // transcript examples.
    #[test]
    fn times_use_the_duration_text_form() {
        const S: u64 = 1_000_000_000;
        for (nanoseconds, text) in [
            (0, "0s"),
            (7, "7ns"),
            (250_000, "250us"),
            (1_500_000, "1.5ms"),
            (1_000_000, "1ms"),
            (30 * S + 1_000_000, "30.001s"),
            (3_600 * S, "1h"),
            (5_400 * S, "1h30m"),
            (3_630 * S, "1h0m30s"),
            (123 * S + 250_000_000, "2m3.25s"),
            (60 * S, "1m"),
            (S + 1, "1.000000001s"),
        ] {
            assert_eq!(duration_text(nanoseconds), text, "{nanoseconds}");
        }
    }
}
