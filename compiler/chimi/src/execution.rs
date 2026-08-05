use crate::executable::CoreProgram;
use crate::ids::{CoreNodeId, FileId, OriginId, SemanticId, TextRange};
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_EXECUTION_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionConfig {
    execution_id: String,
}

impl ExecutionConfig {
    pub fn new(execution_id: impl Into<String>) -> Result<Self, &'static str> {
        let execution_id = execution_id.into();
        if execution_id.is_empty()
            || !execution_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err("execution ID must use letters, digits, '-', '_', '.', or ':'");
        }
        Ok(Self { execution_id })
    }

    pub fn for_process() -> Self {
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after Unix epoch");
        Self::new(format!(
            "execution-{}-{}-{}",
            std::process::id(),
            elapsed.as_nanos(),
            NEXT_EXECUTION_ID.fetch_add(1, Ordering::Relaxed),
        ))
        .expect("generated execution ID is valid")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    ComputationStarted,
    ComputationCompleted,
}

impl EventKind {
    fn name(self) -> &'static str {
        match self {
            Self::ComputationStarted => "computation_started",
            Self::ComputationCompleted => "computation_completed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionEvent {
    pub sequence: u64,
    pub kind: EventKind,
    pub computation: String,
    pub execution_id: String,
    pub span_id: u64,
    pub parent_span_id: Option<u64>,
    pub core_node_id: u32,
    pub origin_id: u32,
    pub semantic_id: u32,
    pub file_id: u32,
    pub source_range: TextRange,
    pub constructor_symbol_id: u32,
    pub field_symbol_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructValue {
    pub type_name: String,
    pub field_name: String,
    pub integer_value: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionOutcome {
    Success(StructValue),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionReport {
    pub events: Vec<ExecutionEvent>,
    pub outcome: ExecutionOutcome,
}

impl ExecutionReport {
    pub fn render_json_lines(&self) -> String {
        let mut output = String::new();
        for event in &self.events {
            writeln!(
                output,
                concat!(
                    "{{\"event\":\"{}\",\"sequence\":{},\"computation\":\"{}\",",
                    "\"execution_id\":\"{}\",\"span_id\":{},\"parent_span_id\":{},",
                    "\"core_node_id\":{},\"origin_id\":{},\"semantic_id\":{},",
                    "\"file_id\":{},\"source_start\":{},\"source_end\":{},",
                    "\"constructor_symbol_id\":{},\"field_symbol_id\":{}}}"
                ),
                event.kind.name(),
                event.sequence,
                event.computation,
                event.execution_id,
                event.span_id,
                event
                    .parent_span_id
                    .map_or_else(|| "null".to_owned(), |identifier| identifier.to_string()),
                event.core_node_id,
                event.origin_id,
                event.semantic_id,
                event.file_id,
                event.source_range.start(),
                event.source_range.end(),
                event.constructor_symbol_id,
                event.field_symbol_id,
            )
            .expect("writing JSON to a string cannot fail");
        }

        match &self.outcome {
            ExecutionOutcome::Success(value) => {
                writeln!(
                    output,
                    concat!(
                        "{{\"outcome\":\"success\",\"value\":{{\"type\":\"{}\",",
                        "\"fields\":{{\"{}\":{}}}}}}}"
                    ),
                    value.type_name, value.field_name, value.integer_value,
                )
                .expect("writing JSON to a string cannot fail");
            }
        }
        output
    }
}

pub(crate) fn execute(core: CoreProgram, config: ExecutionConfig) -> ExecutionReport {
    let event = |sequence, kind| ExecutionEvent {
        sequence,
        kind,
        computation: core.computation.clone(),
        execution_id: config.execution_id.clone(),
        span_id: 1,
        parent_span_id: None,
        core_node_id: id(core.root_node_id),
        origin_id: origin_id(core.origin.origin_id),
        semantic_id: semantic_id(core.origin.semantic_id),
        file_id: file_id(core.origin.file_id),
        source_range: core.origin.range,
        constructor_symbol_id: core.constructor.raw(),
        field_symbol_id: core.field.raw(),
    };

    ExecutionReport {
        events: vec![
            event(0, EventKind::ComputationStarted),
            event(1, EventKind::ComputationCompleted),
        ],
        outcome: ExecutionOutcome::Success(StructValue {
            type_name: core.constructor_name,
            field_name: core.field_name,
            integer_value: core.value,
        }),
    }
}

const fn id(identifier: CoreNodeId) -> u32 {
    identifier.raw()
}

const fn origin_id(identifier: OriginId) -> u32 {
    identifier.raw()
}

const fn semantic_id(identifier: SemanticId) -> u32 {
    identifier.raw()
}

const fn file_id(identifier: FileId) -> u32 {
    identifier.raw()
}

#[cfg(test)]
mod tests {
    use super::ExecutionConfig;

    #[test]
    fn execution_ids_are_safe_structured_event_values() {
        assert!(ExecutionConfig::new("test-run_1.2:3").is_ok());
        assert!(ExecutionConfig::new("").is_err());
        assert!(ExecutionConfig::new("not json safe").is_err());
        assert_ne!(
            ExecutionConfig::for_process(),
            ExecutionConfig::for_process()
        );
    }
}
