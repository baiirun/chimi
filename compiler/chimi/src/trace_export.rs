//! `chimi trace export --otlp`: one run's `chimi.runtime.v1` telemetry as an
//! OpenTelemetry trace, in the OTLP/JSON encoding of `TracesData`, so a
//! collector or tracing backend can show it.
//!
//! Each execution is one trace. The computation and every operation are a
//! span, from its `*_started` event to its `*_completed` event, and a
//! dependency call is a span from the call to the end of the provider's last
//! operation inside it. Scheduler, task and process events are not spans and
//! are left out. Times are `start_unix_ns` plus each event's `time_ns`
//! (runtime-output-channels.md, "Event times").

use crate::json::{JsonData, decode_json, write_json_string};
use crate::trace_view::{Locate, Record, SCHEMA, TraceError};
use std::collections::HashMap;
use std::fmt::Write;

/// The OTLP/JSON document for `text`. `service_name` names the resource; the
/// computation's name is used when it is `None`.
pub fn export_trace_otlp(
    text: &str,
    locate: Locate<'_>,
    service_name: Option<&str>,
) -> Result<String, TraceError> {
    let executions = read(text)?;
    let mut output = String::from("{\"resourceSpans\":[");
    for (index, execution) in executions.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        execution.write(&mut output, locate, service_name);
    }
    output.push_str("]}\n");
    Ok(output)
}

struct Execution {
    id: String,
    computation: String,
    start_unix_ns: i64,
    spans: Vec<Span>,
    span_index: HashMap<u64, usize>,
}

struct Span {
    id: u64,
    parent: Option<u64>,
    name: String,
    start: i64,
    end: Option<i64>,
    /// A completion event ended it.
    completed: bool,
    operation: String,
    core_node_id: u64,
    file_id: u64,
    source: (u64, u64),
    dependency: Option<(String, String)>,
    outcome: Option<String>,
}

fn read(text: &str) -> Result<Vec<Execution>, TraceError> {
    let mut executions: Vec<Execution> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let error = |message: String| TraceError {
            line: index + 1,
            message,
        };
        let record = decode_json(line).map_err(|_| error("not a JSON record".to_owned()))?;
        let JsonData::Object(fields) = record else {
            return Err(error("not a JSON object".to_owned()));
        };
        let record = Record(&fields);
        if record.text("schema") != Some(SCHEMA) {
            return Err(error("not a chimi.runtime.v1 record".to_owned()));
        }
        let Some(event) = record.text("event") else {
            if record.text("outcome").is_some() {
                continue;
            }
            return Err(error("neither an event nor an outcome".to_owned()));
        };
        let started = match event {
            "computation_started" | "operation_started" | "dependency_called" => true,
            "computation_completed" | "operation_completed" => false,
            _ => continue,
        };
        let time = record.attribute_number("time_ns").ok_or_else(|| {
            error(format!(
                "{event} has no time_ns; the trace was written by a runtime without event times"
            ))
        })?;
        let id = record.text("execution_id").unwrap_or("").to_owned();
        let slot = match by_id.get(&id) {
            Some(&slot) => slot,
            None if event == "computation_started" => {
                let start_unix_ns = record
                    .attribute_number("start_unix_ns")
                    .ok_or_else(|| error("computation_started has no start_unix_ns".to_owned()))?;
                executions.push(Execution {
                    id: id.clone(),
                    computation: record.text("computation").unwrap_or("").to_owned(),
                    start_unix_ns,
                    spans: Vec::new(),
                    span_index: HashMap::new(),
                });
                by_id.insert(id, executions.len() - 1);
                executions.len() - 1
            }
            None => {
                return Err(error(format!(
                    "{event} comes before its execution's computation_started"
                )));
            }
        };
        let execution = &mut executions[slot];
        let span_id = record
            .number("span_id")
            .filter(|&span| span != 0)
            .ok_or_else(|| error(format!("{event} has no span_id")))?;
        if started {
            execution.start_span(event, &record, span_id, time);
        } else if let Some(&span) = execution.span_index.get(&span_id) {
            let span = &mut execution.spans[span];
            span.end = Some(time);
            span.completed = true;
            span.outcome = record.attribute_text("outcome").map(str::to_owned);
        }
    }
    for execution in &mut executions {
        execution.close_open_spans();
    }
    Ok(executions)
}

impl Execution {
    fn start_span(&mut self, event: &str, record: &Record<'_>, id: u64, time: i64) {
        let operation = record.text("operation").unwrap_or("operation").to_owned();
        let dependency = (event == "dependency_called").then(|| {
            (
                record
                    .attribute_text("requirement")
                    .unwrap_or("?")
                    .to_owned(),
                record.attribute_text("method").unwrap_or("?").to_owned(),
            )
        });
        let name = match (&dependency, event) {
            (Some((requirement, method)), _) => format!("dependency {requirement}.{method}"),
            (None, "computation_started") => format!("computation {}", self.computation),
            _ => operation.clone(),
        };
        self.span_index.insert(id, self.spans.len());
        self.spans.push(Span {
            id,
            parent: record.number("parent_span_id"),
            name,
            start: time,
            end: None,
            completed: false,
            operation,
            core_node_id: record.number("core_node_id").unwrap_or(0),
            file_id: record.number("file_id").unwrap_or(0),
            source: (
                record.number("source_start").unwrap_or(0),
                record.number("source_end").unwrap_or(0),
            ),
            dependency,
            outcome: None,
        });
    }

    /// A span with no completion event (a dependency call, or an operation a
    /// run never finished) ends with the last of its descendants. Children
    /// start after their parent, so one pass from the newest span carries each
    /// end up to its parent.
    fn close_open_spans(&mut self) {
        for index in (0..self.spans.len()).rev() {
            let span = &mut self.spans[index];
            let end = *span.end.get_or_insert(span.start);
            let parent = span
                .parent
                .and_then(|parent| self.span_index.get(&parent).copied());
            if let Some(parent) = parent.map(|parent| &mut self.spans[parent])
                && !parent.completed
            {
                parent.end = Some(parent.end.unwrap_or(parent.start).max(end));
            }
        }
    }

    fn write(&self, output: &mut String, locate: Locate<'_>, service_name: Option<&str>) {
        output.push_str("{\"resource\":{\"attributes\":[");
        string_attribute(
            output,
            "service.name",
            service_name.unwrap_or(&self.computation),
        );
        output.push(',');
        string_attribute(output, "chimi.execution_id", &self.id);
        output.push_str("]},\"scopeSpans\":[{\"scope\":{\"name\":\"chimi\",\"version\":");
        write_json_string(output, env!("CARGO_PKG_VERSION"));
        output.push_str("},\"spans\":[");
        let trace_id = trace_id(&self.id);
        for (index, span) in self.spans.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            self.write_span(output, span, &trace_id, locate);
        }
        output.push_str("]}]}");
    }

    fn write_span(&self, output: &mut String, span: &Span, trace_id: &str, locate: Locate<'_>) {
        let _ = write!(
            output,
            "{{\"traceId\":\"{trace_id}\",\"spanId\":\"{:016x}\"",
            span.id
        );
        if let Some(parent) = span
            .parent
            .filter(|parent| self.span_index.contains_key(parent))
        {
            let _ = write!(output, ",\"parentSpanId\":\"{parent:016x}\"");
        }
        output.push_str(",\"name\":");
        write_json_string(output, &span.name);
        let end = span.end.unwrap_or(span.start);
        let _ = write!(
            output,
            ",\"kind\":1,\"startTimeUnixNano\":\"{}\",\"endTimeUnixNano\":\"{}\",\"attributes\":[",
            unix_nanos(self.start_unix_ns, span.start),
            unix_nanos(self.start_unix_ns, end),
        );
        string_attribute(output, "chimi.operation", &span.operation);
        for (key, value) in [
            ("chimi.core_node_id", span.core_node_id),
            ("chimi.file_id", span.file_id),
            ("chimi.source_start", span.source.0),
            ("chimi.source_end", span.source.1),
        ] {
            output.push(',');
            int_attribute(output, key, value);
        }
        if let Some(place) = locate(span.file_id as u32, span.source.0 as u32) {
            output.push(',');
            string_attribute(output, "chimi.source", &place);
        }
        if let Some((requirement, method)) = &span.dependency {
            output.push(',');
            string_attribute(output, "chimi.requirement", requirement);
            output.push(',');
            string_attribute(output, "chimi.method", method);
        }
        if let Some(outcome) = &span.outcome {
            output.push(',');
            string_attribute(output, "chimi.outcome", outcome);
        }
        output.push(']');
        if !span.completed && span.dependency.is_none() {
            output.push_str(",\"status\":{\"code\":2,\"message\":\"did not complete\"}");
        } else if let Some(outcome @ ("expected_failure" | "defect")) = span.outcome.as_deref() {
            output.push_str(",\"status\":{\"code\":2,\"message\":");
            write_json_string(output, outcome);
            output.push('}');
        }
        output.push('}');
    }
}

/// Unix nanoseconds as OTLP/JSON writes a fixed64: a decimal string. A time
/// before 1970 cannot be written and is clamped to 0.
fn unix_nanos(start: i64, time: i64) -> u64 {
    u64::try_from(start.saturating_add(time)).unwrap_or(0)
}

fn string_attribute(output: &mut String, key: &str, value: &str) {
    output.push_str("{\"key\":");
    write_json_string(output, key);
    output.push_str(",\"value\":{\"stringValue\":");
    write_json_string(output, value);
    output.push_str("}}");
}

fn int_attribute(output: &mut String, key: &str, value: u64) {
    output.push_str("{\"key\":");
    write_json_string(output, key);
    let _ = write!(output, ",\"value\":{{\"intValue\":\"{value}\"}}}}");
}

/// A 16-byte trace id from the execution id, in hex: two FNV-1a hashes, so
/// one execution id always gives the same trace. All zeros is invalid in
/// OTLP, so it becomes 1.
fn trace_id(execution_id: &str) -> String {
    let hash = |basis: u64| {
        execution_id.bytes().fold(basis, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    let (high, low) = (hash(0xcbf2_9ce4_8422_2325), hash(0x6c62_272e_07bb_0142));
    let low = if high == 0 && low == 0 { 1 } else { low };
    format!("{high:016x}{low:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trace_id_is_32_hex_digits_and_stable() {
        let id = trace_id("run-1");
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|digit| digit.is_ascii_hexdigit()));
        assert_eq!(id, trace_id("run-1"));
        assert_ne!(id, trace_id("run-2"));
    }
}
