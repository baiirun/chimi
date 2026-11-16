//! `chimi trace show`: a post-mortem reading of one run's `chimi.runtime.v1`
//! telemetry, and its outcome record when it is there, as text.
//!
//! The report answers "what went wrong": the outcome, the path of operations
//! that did not succeed down to the one where the failure started, the
//! processes that did not return, and the tasks that never ended. Operations
//! that succeeded are folded into a count unless every operation is asked for.

use crate::json::{JsonData, decode_json};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

const SCHEMA: &str = "chimi.runtime.v1";

/// Where a source range is, for the report: `path:line:column` when the
/// trace's source is known, otherwise `None`.
pub type Locate<'a> = &'a dyn Fn(u32, u32) -> Option<String>;

/// A trace that cannot be read: the one-based line and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

/// The report for `text`, JSON Lines of telemetry events and at most one
/// outcome record per execution. With `all`, every operation is listed, not
/// only the ones on a failure path.
pub fn render_trace(text: &str, locate: Locate<'_>, all: bool) -> Result<String, TraceError> {
    let trace = Trace::read(text)?;
    let mut report = String::new();
    for (index, execution) in trace.executions.iter().enumerate() {
        if index > 0 {
            report.push('\n');
        }
        execution.render(&mut report, locate, all);
    }
    if trace.executions.is_empty() {
        report.push_str("no events\n");
    }
    Ok(report)
}

#[derive(Default)]
struct Trace {
    executions: Vec<Execution>,
}

/// One `execution_id`'s records, in sequence order.
#[derive(Default)]
struct Execution {
    id: String,
    spans: Vec<Span>,
    span_index: HashMap<u64, usize>,
    roots: Vec<usize>,
    tasks: BTreeMap<u64, Task>,
    processes: Vec<ProcessEnd>,
    outcome: Option<String>,
    events: usize,
}

struct Span {
    label: String,
    location: Option<(u32, u32)>,
    outcome: Option<String>,
    children: Vec<usize>,
}

#[derive(Default)]
struct Task {
    parent: Option<u64>,
    state: &'static str,
    channel: Option<u64>,
}

/// A process incarnation that ended other than by returning.
struct ProcessEnd {
    name: Option<String>,
    incarnation: Option<u64>,
    cause: String,
    defect: Option<String>,
}

impl Trace {
    fn read(text: &str) -> Result<Self, TraceError> {
        let mut trace = Trace::default();
        let mut by_id: HashMap<String, usize> = HashMap::new();
        // An outcome record carries no execution id: it belongs to the
        // execution whose event came just before it, or to the first one
        // when it comes first.
        let mut early_outcome: Option<String> = None;
        let mut last_slot: Option<usize> = None;
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let error = |message: &str| TraceError {
                line: index + 1,
                message: message.to_owned(),
            };
            let record = decode_json(line).map_err(|_| error("not a JSON record"))?;
            let JsonData::Object(fields) = record else {
                return Err(error("not a JSON object"));
            };
            let record = Record(&fields);
            if record.text("schema") != Some(SCHEMA) {
                return Err(error("not a chimi.runtime.v1 record"));
            }
            if let Some(outcome) = record.text("outcome") {
                let outcome = describe_outcome(outcome, &record);
                match last_slot {
                    Some(slot) => trace.executions[slot].outcome = Some(outcome),
                    None => early_outcome = Some(outcome),
                }
                continue;
            }
            let Some(event) = record.text("event") else {
                return Err(error("neither an event nor an outcome"));
            };
            let id = record.text("execution_id").unwrap_or("").to_owned();
            let slot = *by_id.entry(id.clone()).or_insert_with(|| {
                trace.executions.push(Execution {
                    id,
                    ..Execution::default()
                });
                trace.executions.len() - 1
            });
            last_slot = Some(slot);
            let execution = &mut trace.executions[slot];
            if let Some(outcome) = early_outcome.take() {
                execution.outcome = Some(outcome);
            }
            execution.add(event, &record);
        }
        if let Some(outcome) = early_outcome {
            trace.executions.push(Execution {
                outcome: Some(outcome),
                ..Execution::default()
            });
        }
        Ok(trace)
    }
}

impl Execution {
    fn add(&mut self, event: &str, record: &Record<'_>) {
        self.events += 1;
        match event {
            "computation_started" | "operation_started" | "dependency_called" => {
                self.start_span(event, record)
            }
            "computation_completed" | "operation_completed" => {
                if let Some(&span) = record
                    .number("span_id")
                    .and_then(|id| self.span_index.get(&id))
                {
                    self.spans[span].outcome = record.attribute_text("outcome").map(str::to_owned);
                }
            }
            "process_terminated" => {
                let cause = record.attribute_text("cause").unwrap_or("ended");
                if cause != "returned" {
                    self.processes.push(ProcessEnd {
                        name: record.attribute_text("name").map(str::to_owned),
                        incarnation: record.number("incarnation_id"),
                        cause: cause.to_owned(),
                        defect: record.attribute_text("defect").map(str::to_owned),
                    });
                }
            }
            _ if event.starts_with("task_") => {
                let Some(id) = record.number("task_id") else {
                    return;
                };
                let task = self.tasks.entry(id).or_default();
                if event == "task_started" {
                    task.parent = record.number("parent_task_id");
                }
                task.state = match event {
                    "task_parked" => "parked",
                    "task_cancellation_requested" => "cancelling",
                    "task_cleanup_completed" | "task_terminal" => "ended",
                    _ => "running",
                };
                if event == "task_parked" {
                    // The channel comes on the `channel_*_parked` event
                    // that follows, when the wait is on a channel.
                    task.channel = record.number("channel_id");
                }
            }
            "channel_receive_parked" | "channel_send_parked" => {
                let task = record
                    .number("task_id")
                    .and_then(|id| self.tasks.get_mut(&id));
                if let Some(task) = task {
                    task.channel = record.number("channel_id");
                }
            }
            _ => {}
        }
    }

    fn start_span(&mut self, event: &str, record: &Record<'_>) {
        let operation = record.text("operation").unwrap_or("operation");
        let label = match event {
            "computation_started" => {
                format!("computation {}", record.text("computation").unwrap_or("?"))
            }
            "dependency_called" => format!(
                "dependency {}.{}",
                record.attribute_text("requirement").unwrap_or("?"),
                record.attribute_text("method").unwrap_or("?")
            ),
            _ => operation.to_owned(),
        };
        let location = record
            .number("file_id")
            .zip(record.number("source_start"))
            .map(|(file, start)| (file as u32, start as u32));
        let span = self.spans.len();
        self.spans.push(Span {
            label,
            location,
            outcome: (event == "dependency_called").then(|| "success".to_owned()),
            children: Vec::new(),
        });
        if let Some(id) = record.number("span_id") {
            self.span_index.insert(id, span);
        }
        match record
            .number("parent_span_id")
            .and_then(|parent| self.span_index.get(&parent))
        {
            Some(&parent) => self.spans[parent].children.push(span),
            None => self.roots.push(span),
        }
    }

    fn render(&self, report: &mut String, locate: Locate<'_>, all: bool) {
        if !self.id.is_empty() {
            let _ = writeln!(report, "execution {}", self.id);
        }
        let _ = writeln!(
            report,
            "outcome: {}",
            self.outcome.as_deref().unwrap_or("none recorded")
        );
        let _ = writeln!(report, "events: {}", self.events);
        if !self.roots.is_empty() {
            report.push('\n');
            for &root in &self.roots {
                self.render_tree(report, root, locate, all);
            }
        }
        if !self.processes.is_empty() {
            report.push_str("\nprocesses that did not return:\n");
            for process in &self.processes {
                let name = process.name.as_deref().unwrap_or("(unnamed)");
                let _ = write!(report, "  {name}");
                if let Some(incarnation) = process.incarnation {
                    let _ = write!(report, " incarnation {incarnation}");
                }
                let _ = write!(report, ": {}", process.cause);
                if let Some(defect) = &process.defect {
                    let _ = write!(report, ": {defect}");
                }
                report.push('\n');
            }
        }
        let unfinished: Vec<_> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.state != "ended")
            .collect();
        if !unfinished.is_empty() {
            report.push_str("\ntasks that never ended:\n");
            for (id, task) in unfinished {
                let _ = write!(report, "  task {id}");
                if let Some(parent) = task.parent {
                    let _ = write!(report, " (started by task {parent})");
                }
                let _ = write!(report, ": {}", task.state);
                if let Some(channel) = task.channel.filter(|_| task.state == "parked") {
                    let _ = write!(report, " on channel {channel}");
                }
                report.push('\n');
            }
        }
    }

    fn failed(&self, span: usize) -> bool {
        self.spans[span]
            .outcome
            .as_deref()
            .is_none_or(|outcome| outcome != "success")
    }

    /// The tree under `root`, depth first. It keeps its own stack, since a
    /// deep recursion in the program makes a span chain as deep, and caps
    /// the indentation, writing the depth instead past the cap.
    fn render_tree(&self, report: &mut String, root: usize, locate: Locate<'_>, all: bool) {
        enum Step {
            Span(usize, usize),
            Folded(usize, usize),
        }
        let mut stack = vec![Step::Span(root, 0)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Folded(depth, folded) => {
                    let noun = if folded == 1 {
                        "operation"
                    } else {
                        "operations"
                    };
                    let _ = writeln!(report, "{}({folded} succeeded {noun})", indent(depth));
                }
                Step::Span(index, depth) => {
                    let shown = self.render_line(report, index, depth, locate, all);
                    let folded = self.spans[index].children.len() - shown.len();
                    if folded > 0 {
                        stack.push(Step::Folded(depth + 1, folded));
                    }
                    stack.extend(
                        shown
                            .iter()
                            .rev()
                            .map(|&child| Step::Span(child, depth + 1)),
                    );
                }
            }
        }
    }

    /// Writes span `index`'s own line and returns the children to list.
    fn render_line(
        &self,
        report: &mut String,
        index: usize,
        depth: usize,
        locate: Locate<'_>,
        all: bool,
    ) -> Vec<usize> {
        let span = &self.spans[index];
        let _ = write!(report, "{}{}", indent(depth), span.label);
        if let Some((file, start)) = span.location {
            let location = locate(file, start).unwrap_or(format!("file {file} at {start}"));
            let _ = write!(report, "  {location}");
        }
        let failed_children: Vec<usize> = span
            .children
            .iter()
            .copied()
            .filter(|&child| self.failed(child))
            .collect();
        match span.outcome.as_deref() {
            Some("success") => {}
            Some(outcome) => {
                let _ = write!(report, "  {outcome}");
                if failed_children.is_empty() {
                    report.push_str("  <- started here");
                }
            }
            None => report.push_str("  (did not finish)"),
        }
        report.push('\n');
        if all {
            span.children.clone()
        } else {
            failed_children
        }
    }
}

/// Indentation stops growing at this depth, so a deep chain stays linear in
/// size; deeper lines carry their depth.
const MAX_INDENT: usize = 24;

fn indent(depth: usize) -> String {
    if depth <= MAX_INDENT {
        "  ".repeat(depth)
    } else {
        format!("{}[depth {depth}] ", "  ".repeat(MAX_INDENT))
    }
}

fn describe_outcome(outcome: &str, record: &Record<'_>) -> String {
    let mut text = outcome.to_owned();
    if let Some(message) = record.text("message") {
        let _ = write!(text, ": {message}");
    }
    if let Some(cause) = record.text("cause") {
        let _ = write!(text, "\n  {}", cause.replace('\n', "\n  "));
    }
    text
}

/// One JSON object's fields, read by name.
struct Record<'a>(&'a [(String, JsonData)]);

impl<'a> Record<'a> {
    fn get(&self, name: &str) -> Option<&'a JsonData> {
        self.0
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value)
    }

    fn text(&self, name: &str) -> Option<&'a str> {
        match self.get(name)? {
            JsonData::Text(text) => Some(text),
            _ => None,
        }
    }

    fn number(&self, name: &str) -> Option<u64> {
        match self.get(name)? {
            JsonData::Number(number) => number.parse().ok(),
            _ => None,
        }
    }

    fn attribute_text(&self, name: &str) -> Option<&'a str> {
        let JsonData::Object(attributes) = self.get("attributes")? else {
            return None;
        };
        Record(attributes).text(name)
    }
}
