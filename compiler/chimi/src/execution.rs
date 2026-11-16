mod bytes_operations;
mod causes;
mod constants;
pub(crate) mod core_operations;
mod debug;
mod host_operations;
mod decimal_operations;
mod durations;
mod json_operations;
mod map_operations;
mod persistent_list;
mod processes;
mod resource_operations;
mod structural;
mod teardown;

use crate::decimal::decimal_binary;
use crate::executable::{
    CoreArm, CoreLiteral, CoreOperation, CorePattern, CoreProgram, CoreStep, CoreSymbolKind,
    LegacyCoreProgram,
};
use crate::ids::{CoreNodeId, FileId, OriginId, SemanticId, SymbolId, TextRange};
use crate::json::write_json_string;
use crate::numeric::{
    ArithmeticOperator, FloatParseError, FloatToIntError, IntParseError, float_binary,
    float_to_int, integer_binary, integer_modulo, integer_negate, parse_float, parse_int,
    render_float,
};
use crate::semantic::{BinaryOperator, UnaryOperator};
use crate::standard::{StandardConstructor, StandardFunction, StandardSymbol, StandardType};
use causes::{Cause, CauseKind};
pub(crate) use constants::{ConstantLimits, evaluate_constants};
#[cfg(test)]
pub(crate) use constants::{evaluate_constants_with, measured_values_to_evaluate};
use persistent_list::PersistentList;
use std::cmp::Ordering as Comparison;
use std::collections::HashMap;
use std::fmt::Write;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! success_or_return {
    ($control:expr) => {
        match $control {
            Control::Success(value) => value,
            other => return other,
        }
    };
}

static NEXT_EXECUTION_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceMode {
    Off,
    Boundaries,
    Full,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionConfig {
    execution_id: String,
    trace_mode: TraceMode,
    cancellation_requested: bool,
    application_arguments: Vec<String>,
    standard_input: Vec<u8>,
    max_line_bytes: usize,
    /// The `random` provider's seed; `None` seeds it from the operating system.
    random_seed: Option<u64>,
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
        Ok(Self {
            execution_id,
            trace_mode: TraceMode::Full,
            cancellation_requested: false,
            application_arguments: Vec::new(),
            standard_input: Vec::new(),
            max_line_bytes: 1024 * 1024,
            random_seed: None,
        })
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

    pub const fn with_trace_mode(mut self, trace_mode: TraceMode) -> Self {
        self.trace_mode = trace_mode;
        self
    }

    pub const fn with_cancellation_requested(mut self) -> Self {
        self.cancellation_requested = true;
        self
    }

    pub fn with_application_arguments(
        mut self,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.application_arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_standard_input(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.standard_input = input.into();
        self
    }

    /// Seeds the `random` provider, as `chimi test` does for each test.
    pub const fn with_random_seed(mut self, seed: u64) -> Self {
        self.random_seed = Some(seed);
        self
    }

    pub const fn with_max_line_bytes(mut self, max_line_bytes: usize) -> Self {
        assert!(max_line_bytes > 0, "maximum line bytes must be positive");
        self.max_line_bytes = max_line_bytes;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    ComputationStarted,
    OperationStarted,
    OperationCompleted,
    DependencyCalled,
    ComputationCompleted,
}

impl EventKind {
    fn name(self) -> &'static str {
        match self {
            Self::ComputationStarted => "computation_started",
            Self::OperationStarted => "operation_started",
            Self::OperationCompleted => "operation_completed",
            Self::DependencyCalled => "dependency_called",
            Self::ComputationCompleted => "computation_completed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionAttribute {
    pub name: String,
    pub value: ExecutionAttributeValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionAttributeValue {
    Text(String),
    Integer(i64),
    Symbol(u32),
    Semantic(u32),
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
    pub operation: String,
    pub attributes: Vec<ExecutionAttribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueField {
    pub name: String,
    pub value: ExecutionValue,
}

/// A finite `Float` result, compared by bit pattern so that `-0.0` stays distinct
/// from `0.0` when outcomes are compared.
#[derive(Clone, Copy, Debug)]
pub struct FloatValue(pub f64);

impl PartialEq for FloatValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for FloatValue {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionValue {
    Void,
    Integer(i64),
    Float(FloatValue),
    Text(String),
    Boolean(bool),
    List(Vec<ExecutionValue>),
    Tuple(Vec<ExecutionValue>),
    Range {
        start: i64,
        end: i64,
    },
    /// Nanoseconds.
    Duration(i64),
    /// A `std/decimal.Decimal` in its normalized text, as `Debug.render` writes
    /// it, so equal values are equal outcomes.
    Decimal(String),
    /// A `std/bytes.Bytes`.
    Bytes(Vec<u8>),
    Record(Vec<ValueField>),
    /// A map whose keys are not Text, as its entries in key order. A Text-keyed
    /// map is a `Record` of its entries, and an empty map is an empty `Record`.
    Map(Vec<(ExecutionValue, ExecutionValue)>),
    Constructed {
        type_name: String,
        variant_name: Option<String>,
        fields: Vec<ValueField>,
    },
    Callable {
        name: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDefect {
    pub message: String,
    pub core_node_id: Option<u32>,
    pub origin_id: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionCancellation {
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionOutcome {
    Success(ExecutionValue),
    ExpectedFailure(ExecutionValue),
    Defect(ExecutionDefect),
    Cancellation(ExecutionCancellation),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionReport {
    pub events: Vec<ExecutionEvent>,
    pub outcome: ExecutionOutcome,
    /// The root outcome's cause tree (`Cause.to_text`), when it has context
    /// frames, a `caused_by` link, or suppressed causes.
    pub cause: Option<String>,
    /// An expected failure's value in the indented debug format, which
    /// `chimi test` reports; `None` for any other outcome.
    pub failure_rendering: Option<String>,
    pub application_stdout: String,
    pub application_stderr: String,
    /// Whether the execution drew from its `random` provider, so a failing
    /// test's report names the seed.
    pub random_read: bool,
}

impl ExecutionReport {
    pub fn render_json_lines(&self) -> String {
        let mut output = self.render_telemetry_json_lines();
        output.push_str(&self.render_outcome_json_line());
        output
    }

    pub fn render_telemetry_json_lines(&self) -> String {
        let mut output = String::new();
        for event in &self.events {
            write!(
                output,
                concat!(
                    "{{\"schema\":\"chimi.runtime.v1\",",
                    "\"event\":\"{}\",\"sequence\":{},\"computation\":"
                ),
                event.kind.name(),
                event.sequence,
            )
            .expect("writing JSON to a string cannot fail");
            write_json_string(&mut output, &event.computation);
            write!(
                output,
                concat!(
                    ",\"execution_id\":\"{}\",\"span_id\":{},\"parent_span_id\":{},",
                    "\"core_node_id\":{},\"origin_id\":{},\"semantic_id\":{},",
                    "\"file_id\":{},\"source_start\":{},\"source_end\":{},\"operation\":"
                ),
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
            )
            .expect("writing JSON to a string cannot fail");
            write_json_string(&mut output, &event.operation);
            output.push_str(",\"attributes\":{");
            for (index, attribute) in event.attributes.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_string(&mut output, &attribute.name);
                output.push(':');
                write_json_attribute(&mut output, &attribute.value);
            }
            output.push_str("}}\n");
        }
        output
    }

    pub fn render_outcome_json_line(&self) -> String {
        let mut output = String::new();
        match &self.outcome {
            ExecutionOutcome::Success(value) => {
                output.push_str(
                    "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"success\",\"value\":",
                );
                write_json_value(&mut output, value);
            }
            ExecutionOutcome::ExpectedFailure(error) => {
                output.push_str(concat!(
                    "{\"schema\":\"chimi.runtime.v1\",",
                    "\"outcome\":\"expected_failure\",\"error\":"
                ));
                write_json_value(&mut output, error);
            }
            ExecutionOutcome::Defect(defect) => {
                output.push_str(
                    "{\"schema\":\"chimi.runtime.v1\",\"outcome\":\"defect\",\"message\":",
                );
                write_json_string(&mut output, &defect.message);
                write!(
                    output,
                    ",\"core_node_id\":{},\"origin_id\":{}",
                    optional_u32(defect.core_node_id),
                    optional_u32(defect.origin_id),
                )
                .expect("writing JSON to a string cannot fail");
            }
            ExecutionOutcome::Cancellation(cancellation) => {
                output.push_str(concat!(
                    "{\"schema\":\"chimi.runtime.v1\",",
                    "\"outcome\":\"cancellation\",\"reason\":"
                ));
                write_json_string(&mut output, &cancellation.reason);
            }
        }
        if let Some(cause) = &self.cause {
            output.push_str(",\"cause\":");
            write_json_string(&mut output, cause);
        }
        output.push_str("}\n");
        output
    }
}

#[derive(Clone, Debug)]
enum RuntimeValue {
    Void,
    Integer(i64),
    /// Always finite; every operation that would leave the finite domain defects.
    Float(f64),
    Text(String),
    Boolean(bool),
    List(PersistentList<RuntimeValue>),
    /// Two or more elements; typing fixes the arity.
    Tuple(Arc<Vec<RuntimeValue>>),
    /// The half-open run `start..end`; `..=` is already normalized to it.
    Range {
        start: i64,
        end: i64,
    },
    /// A `std/duration.Duration`: a signed count of nanoseconds.
    Duration(i64),
    /// A `std/decimal.Decimal`.
    Decimal(crate::decimal::Decimal),
    /// A `std/bytes.Bytes`, shared because values are copied on every reference.
    Bytes(Arc<[u8]>),
    /// A named `std/process.Address`: the `process name` declaration's symbol.
    ProcessAddress(SymbolId),
    /// Fields of records and constructed values are shared like list nodes and
    /// closure environments: values are copied on every reference, and a structure
    /// that holds an earlier one twice would otherwise double with each level.
    Record(Arc<Vec<RuntimeField>>),
    Constructed {
        constructor: SymbolId,
        type_name: String,
        variant_name: Option<String>,
        fields: Arc<Vec<RuntimeField>>,
        /// The cause a stored `Err` keeps, outside the value: equality, order,
        /// matching and rendering ignore it.
        cause: Option<Arc<Cause>>,
    },
    Map(RuntimeMap),
    /// A `std/set.Set`: a map from each item to `Void`, kept a separate kind so
    /// rendering and outcomes show a set.
    Set(RuntimeMap),
    JsonNumber(String),
    Callable(RuntimeCallable),
    /// A `std/cause.Cause`.
    Cause(Arc<Cause>),
    /// A `std/test.Recorder` or `std/test.Script`: a handle to a log the test
    /// execution owns. The handle never changes; its log does.
    TestLog(Arc<core_operations::TestLog>),
}

thread_local! {
    static RUNTIME_VALUE_RELEASES: teardown::ReleaseQueue<RuntimeValue> =
        const { teardown::ReleaseQueue::new() };
    static EXECUTION_VALUE_RELEASES: teardown::ReleaseQueue<ExecutionValue> =
        const { teardown::ReleaseQueue::new() };
}

impl RuntimeValue {
    /// Whether the value can hold other values, so freeing it could recurse.
    fn has_parts(&self) -> bool {
        matches!(
            self,
            Self::List(_)
                | Self::Tuple(_)
                | Self::Record(_)
                | Self::Constructed { .. }
                | Self::Map(_)
                | Self::Set(_)
                | Self::Callable(_)
        )
    }
}

/// Frees a value of any nesting depth in bounded host stack (E18; see `teardown`).
impl Drop for RuntimeValue {
    fn drop(&mut self) {
        if self.has_parts() {
            teardown::release(&RUNTIME_VALUE_RELEASES, self, |value| {
                std::mem::replace(value, Self::Void)
            });
        }
    }
}

/// A program's outcome value is freed by the caller, often on a thread with a small
/// stack, so it is freed without recursing too.
impl Drop for ExecutionValue {
    fn drop(&mut self) {
        if matches!(
            self,
            Self::List(_) | Self::Tuple(_) | Self::Record(_) | Self::Constructed { .. }
        ) {
            teardown::release(&EXECUTION_VALUE_RELEASES, self, |value| {
                std::mem::replace(value, Self::Void)
            });
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RuntimeMap {
    root: Option<Arc<RuntimeMapNode>>,
}

#[derive(Clone, Debug)]
struct RuntimeMapNode {
    key: RuntimeValue,
    value: RuntimeValue,
    height: u16,
    left: Option<Arc<Self>>,
    right: Option<Arc<Self>>,
}

/// The order over keys: structural order, which is consistent with equality
/// for every equatable type. Enumeration visits keys in this order, which
/// `std/map` exposes only for an ordered key type.
type KeyOrder<'order> = dyn Fn(&RuntimeValue, &RuntimeValue) -> Comparison + 'order;

impl RuntimeMap {
    fn get(&self, key: &RuntimeValue, order: &KeyOrder<'_>) -> Option<RuntimeValue> {
        let mut node = self.root.as_deref();
        while let Some(current) = node {
            match order(key, &current.key) {
                Comparison::Less => node = current.left.as_deref(),
                Comparison::Equal => return Some(current.value.clone()),
                Comparison::Greater => node = current.right.as_deref(),
            }
        }
        None
    }

    fn put(&self, key: RuntimeValue, value: RuntimeValue, order: &KeyOrder<'_>) -> Self {
        Self {
            root: Some(map_insert(self.root.clone(), key, value, order)),
        }
    }

    /// The map without `key`, or `None` when it has no such key.
    fn remove(&self, key: &RuntimeValue, order: &KeyOrder<'_>) -> Option<Self> {
        map_remove(self.root.clone(), key, order).map(|root| Self { root })
    }

    fn size(&self) -> usize {
        self.entry_refs().len()
    }

    /// Entries in key order, borrowed. The walk keeps its own stack; the tree is
    /// balanced, so it never holds more than the tree height.
    fn entry_refs(&self) -> Vec<(&RuntimeValue, &RuntimeValue)> {
        let mut entries = Vec::new();
        let mut pending = Vec::new();
        let mut node = self.root.as_deref();
        loop {
            while let Some(current) = node {
                pending.push(current);
                node = current.left.as_deref();
            }
            let Some(current) = pending.pop() else {
                return entries;
            };
            entries.push((&current.key, &current.value));
            node = current.right.as_deref();
        }
    }

    fn entries(&self) -> Vec<(RuntimeValue, RuntimeValue)> {
        self.entry_refs()
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

fn map_insert(
    node: Option<Arc<RuntimeMapNode>>,
    key: RuntimeValue,
    value: RuntimeValue,
    order: &KeyOrder<'_>,
) -> Arc<RuntimeMapNode> {
    let Some(node) = node else {
        return map_node(key, value, None, None);
    };
    let updated = match order(&key, &node.key) {
        Comparison::Less => map_node(
            node.key.clone(),
            node.value.clone(),
            Some(map_insert(node.left.clone(), key, value, order)),
            node.right.clone(),
        ),
        // An equal key already held stays; only the value changes.
        Comparison::Equal => map_node(
            node.key.clone(),
            value,
            node.left.clone(),
            node.right.clone(),
        ),
        Comparison::Greater => map_node(
            node.key.clone(),
            node.value.clone(),
            node.left.clone(),
            Some(map_insert(node.right.clone(), key, value, order)),
        ),
    };
    map_balance(updated)
}

/// `node` without `key`, rebalanced; `None` when `key` is absent. The recursion
/// is bounded by the tree height.
fn map_remove(
    node: Option<Arc<RuntimeMapNode>>,
    key: &RuntimeValue,
    order: &KeyOrder<'_>,
) -> Option<Option<Arc<RuntimeMapNode>>> {
    let node = node?;
    let rebuilt = |left, right| {
        Some(Some(map_balance(map_node(
            node.key.clone(),
            node.value.clone(),
            left,
            right,
        ))))
    };
    match order(key, &node.key) {
        Comparison::Less => {
            let left = map_remove(node.left.clone(), key, order)?;
            rebuilt(left, node.right.clone())
        }
        Comparison::Greater => {
            let right = map_remove(node.right.clone(), key, order)?;
            rebuilt(node.left.clone(), right)
        }
        Comparison::Equal => Some(match (&node.left, &node.right) {
            (None, right) => right.clone(),
            (left, None) => left.clone(),
            (left, Some(right)) => {
                let (least, rest) = map_remove_least(right.clone());
                Some(map_balance(map_node(
                    least.key.clone(),
                    least.value.clone(),
                    left.clone(),
                    rest,
                )))
            }
        }),
    }
}

/// The least node of `node` and the rest of the tree without it.
fn map_remove_least(
    node: Arc<RuntimeMapNode>,
) -> (Arc<RuntimeMapNode>, Option<Arc<RuntimeMapNode>>) {
    let Some(left) = node.left.clone() else {
        let rest = node.right.clone();
        return (node, rest);
    };
    let (least, rest) = map_remove_least(left);
    let rebuilt = map_balance(map_node(
        node.key.clone(),
        node.value.clone(),
        rest,
        node.right.clone(),
    ));
    (least, Some(rebuilt))
}

fn map_height(node: &Option<Arc<RuntimeMapNode>>) -> u16 {
    node.as_ref().map_or(0, |node| node.height)
}

fn map_node(
    key: RuntimeValue,
    value: RuntimeValue,
    left: Option<Arc<RuntimeMapNode>>,
    right: Option<Arc<RuntimeMapNode>>,
) -> Arc<RuntimeMapNode> {
    Arc::new(RuntimeMapNode {
        key,
        value,
        height: 1 + map_height(&left).max(map_height(&right)),
        left,
        right,
    })
}

fn map_balance(node: Arc<RuntimeMapNode>) -> Arc<RuntimeMapNode> {
    let balance = i32::from(map_height(&node.left)) - i32::from(map_height(&node.right));
    if balance > 1 {
        let mut root = node;
        let left = root
            .left
            .as_ref()
            .expect("left-heavy map node has a left child");
        if map_height(&left.left) < map_height(&left.right) {
            root = map_node(
                root.key.clone(),
                root.value.clone(),
                Some(map_rotate_left(left.clone())),
                root.right.clone(),
            );
        }
        return map_rotate_right(root);
    }
    if balance < -1 {
        let mut root = node;
        let right = root
            .right
            .as_ref()
            .expect("right-heavy map node has a right child");
        if map_height(&right.right) < map_height(&right.left) {
            root = map_node(
                root.key.clone(),
                root.value.clone(),
                root.left.clone(),
                Some(map_rotate_right(right.clone())),
            );
        }
        return map_rotate_left(root);
    }
    node
}

fn map_rotate_left(root: Arc<RuntimeMapNode>) -> Arc<RuntimeMapNode> {
    let right = root
        .right
        .as_ref()
        .expect("left rotation requires a right child")
        .clone();
    let left = map_node(
        root.key.clone(),
        root.value.clone(),
        root.left.clone(),
        right.left.clone(),
    );
    map_node(
        right.key.clone(),
        right.value.clone(),
        Some(left),
        right.right.clone(),
    )
}

fn map_rotate_right(root: Arc<RuntimeMapNode>) -> Arc<RuntimeMapNode> {
    let left = root
        .left
        .as_ref()
        .expect("right rotation requires a left child")
        .clone();
    let right = map_node(
        root.key.clone(),
        root.value.clone(),
        left.right.clone(),
        root.right.clone(),
    );
    map_node(
        left.key.clone(),
        left.value.clone(),
        left.left.clone(),
        Some(right),
    )
}

#[derive(Clone, Debug)]
struct RuntimeField {
    name: String,
    value: RuntimeValue,
}

#[derive(Clone, Debug)]
enum RuntimeCallable {
    Declared {
        symbol: SymbolId,
        requirements: HashMap<String, RuntimeValue>,
    },
    Closure {
        parameters: Vec<SymbolId>,
        body: CoreNodeId,
        /// Shared, so copying a closure value never copies what it captured. Values
        /// are copied freely, and a closure that captures an earlier closure would
        /// otherwise copy a tree that doubles with every capture level.
        environment: Arc<Environment>,
        name: String,
    },
    Dependency {
        callable: Box<RuntimeCallable>,
        requirement: SymbolId,
        interface: SymbolId,
        method: SemanticId,
        name: String,
    },
    Intrinsic(StandardFunction),
    Host(HostFunction),
    /// A constructor with fields named as a value; calling it constructs.
    Constructor {
        symbol: SymbolId,
        name: String,
    },
}

#[derive(Clone, Copy, Debug)]
enum HostFunction {
    ProcessUtf8Lines,
    WriteLine,
    WriteError,
    RandomNextInt,
}

#[derive(Clone, Debug, Default)]
struct Environment {
    locals: HashMap<SymbolId, RuntimeValue>,
    requirements: HashMap<String, RuntimeValue>,
}

#[derive(Clone, Debug)]
enum Control {
    Success(RuntimeValue),
    Failure(RuntimeValue, Trail),
    Defect {
        message: String,
        node: Option<CoreNodeId>,
        cause: Option<Arc<Cause>>,
    },
    Cancellation(Option<Arc<Cause>>),
    /// Unwinds to the loop whose state symbol is `target`. Verified programs never let it
    /// cross a function boundary, so it never reaches a call, task, or root outcome.
    Break {
        target: SymbolId,
        value: RuntimeValue,
    },
}

/// The cause of a live expected failure. Until something reads or extends it,
/// it stays implicit: a fresh cause of the failure's value, started at `origin`.
#[derive(Clone, Debug, Default)]
struct Trail {
    cause: Option<Arc<Cause>>,
    origin: Option<CoreNodeId>,
}

impl Trail {
    const fn at(origin: CoreNodeId) -> Self {
        Self {
            cause: None,
            origin: Some(origin),
        }
    }

    fn built(cause: Cause) -> Self {
        Self {
            origin: cause.origin,
            cause: Some(Arc::new(cause)),
        }
    }
}

impl Control {
    /// An expected failure whose cause starts where the caller stamps it.
    fn failure(value: RuntimeValue) -> Self {
        Self::Failure(value, Trail::default())
    }

    /// The cause of a terminal outcome, built from the outcome when still implicit.
    fn cause(&self) -> Option<Arc<Cause>> {
        match self {
            Self::Success(_) | Self::Break { .. } => None,
            Self::Failure(value, trail) => Some(trail.cause.clone().unwrap_or_else(|| {
                Arc::new(Cause::new(
                    CauseKind::ExpectedFailure(value.clone()),
                    trail.origin,
                ))
            })),
            Self::Defect {
                message,
                node,
                cause,
            } => Some(cause.clone().unwrap_or_else(|| {
                Arc::new(Cause::new(CauseKind::Defect(message.clone()), *node))
            })),
            Self::Cancellation(cause) => Some(cause.clone().unwrap_or_else(|| {
                Arc::new(Cause::new(
                    CauseKind::Cancellation(CANCELLATION_REASON.to_owned()),
                    None,
                ))
            })),
        }
    }

    /// This outcome with its cause replaced; a success or break is unchanged.
    fn with_cause(self, cause: Cause) -> Self {
        match self {
            Self::Failure(value, _) => Self::Failure(value, Trail::built(cause)),
            Self::Defect { message, node, .. } => Self::Defect {
                message,
                node,
                cause: Some(Arc::new(cause)),
            },
            Self::Cancellation(_) => Self::Cancellation(Some(Arc::new(cause))),
            other @ (Self::Success(_) | Self::Break { .. }) => other,
        }
    }
}

/// The reason the interpreter gives every cancellation it observes.
const CANCELLATION_REASON: &str = "requested before the next semantic operation";

/// Runs a program on the reference interpreter, on a thread with a large stack.
pub(crate) fn execute(core: &CoreProgram, config: ExecutionConfig) -> ExecutionReport {
    on_evaluation_stack("execution", || {
        Evaluator::new(core, config.clone()).execute()
    })
}

/// The host stack of a thread that runs the interpreter. The interpreter recurses on
/// the host stack, a few frames per Core operation, so this must hold
/// `ConstantLimits::BUILD.call_depth` nested calls of an ordinary function body even in
/// an unoptimized compiler, which spends about 55 KiB of stack per call. Only the pages
/// a deep evaluation touches are committed.
const EVALUATION_STACK_BYTES: usize = 1 << 30;

/// Runs `evaluate` on a named thread with `EVALUATION_STACK_BYTES` of stack, so a
/// program's recursion depth does not depend on the caller's thread. A host that
/// cannot reserve the stack, such as one under a small `ulimit -v`, runs it on the
/// current thread instead.
fn on_evaluation_stack<T: Send>(name: &str, evaluate: impl Fn() -> T + Sync) -> T {
    std::thread::scope(|scope| {
        match std::thread::Builder::new()
            .name(name.to_owned())
            .stack_size(EVALUATION_STACK_BYTES)
            .spawn_scoped(scope, &evaluate)
        {
            Ok(thread) => thread
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(_) => evaluate(),
        }
    })
}

struct Evaluator<'core> {
    core: &'core CoreProgram,
    config: ExecutionConfig,
    events: Vec<ExecutionEvent>,
    next_sequence: u64,
    next_span: u64,
    /// Every constant's value. At run time it holds them all from the start; during
    /// build-time evaluation it fills as constants are evaluated.
    constants: HashMap<SymbolId, RuntimeValue>,
    /// Present only while the compiler evaluates constants.
    constant_evaluation: Option<constants::ConstantEvaluation>,
    application_stdout: String,
    application_stderr: String,
    /// `CoreProgram::assertion_calls`, computed on the first `capture_defect`.
    assertion_calls: std::cell::OnceCell<std::collections::HashSet<u32>>,
    /// The id the next recorder or script takes, from 1 in creation order, as
    /// the native runtime numbers them, so both order handles alike.
    next_test_log: u64,
    /// The `random` provider's generator, made on the first draw.
    random: Option<crate::random_source::Generator>,
}

impl<'core> Evaluator<'core> {
    fn new(core: &'core CoreProgram, config: ExecutionConfig) -> Self {
        Self {
            core,
            config,
            events: Vec::new(),
            next_sequence: 0,
            next_span: 2,
            constants: core
                .constants
                .iter()
                .map(|constant| {
                    (
                        constant.symbol,
                        constants::runtime_value(core, &constant.value),
                    )
                })
                .collect(),
            constant_evaluation: None,
            application_stdout: String::new(),
            application_stderr: String::new(),
            assertion_calls: std::cell::OnceCell::new(),
            next_test_log: 1,
            random: None,
        }
    }

    fn execute(mut self) -> ExecutionReport {
        let entry = self
            .core
            .functions
            .iter()
            .find(|function| function.symbol == self.core.entry)
            .expect("well-formed core contains its entry function")
            .clone();
        if self.config.trace_mode != TraceMode::Off {
            self.emit(
                EventKind::ComputationStarted,
                entry.body,
                1,
                None,
                "computation",
                Vec::new(),
            );
        }
        let control = if self.config.cancellation_requested {
            Control::Cancellation(None)
        } else {
            let mut environment = Environment::default();
            if let Some(parameter) = entry.parameters.first() {
                environment.locals.insert(
                    *parameter,
                    RuntimeValue::List(
                        self.config
                            .application_arguments
                            .iter()
                            .cloned()
                            .map(RuntimeValue::Text)
                            .collect(),
                    ),
                );
            }
            self.eval_node(entry.body, 1, environment)
        };
        if self.config.trace_mode != TraceMode::Off {
            self.emit(
                EventKind::ComputationCompleted,
                entry.body,
                1,
                None,
                "computation",
                vec![text_attribute("outcome", control_name(&control))],
            );
        }
        let cause = control
            .cause()
            .filter(|cause| causes::has_detail(cause))
            .map(|cause| causes::render(self.core, &cause));
        let failure_rendering = match &control {
            Control::Failure(value, _) => Some(debug::render(self.core, value, true)),
            _ => None,
        };
        let outcome = self.public_outcome(control);
        ExecutionReport {
            events: self.events,
            outcome,
            cause,
            failure_rendering,
            application_stdout: self.application_stdout,
            application_stderr: self.application_stderr,
            random_read: self.random.is_some(),
        }
    }

    fn eval_node(
        &mut self,
        node_id: CoreNodeId,
        parent_span: u64,
        environment: Environment,
    ) -> Control {
        if self.config.cancellation_requested {
            return Control::Cancellation(None);
        }
        if let Some(stop) = self.count_constant_operation(node_id) {
            return stop;
        }
        let node = self.core.nodes[node_id.raw() as usize].clone();
        let span = if self.config.trace_mode == TraceMode::Off {
            parent_span
        } else {
            let span = self.next_span;
            self.next_span += 1;
            span
        };
        let operation = if self.config.trace_mode == TraceMode::Full {
            operation_name(&node.operation)
        } else {
            ""
        };
        if self.config.trace_mode == TraceMode::Full {
            self.emit(
                EventKind::OperationStarted,
                node_id,
                span,
                Some(parent_span),
                operation,
                Vec::new(),
            );
        }
        let copies = self.constant_evaluation.is_some() && constants::copies_value(&node.operation);
        let mut control = self.eval_operation(node_id, span, environment, node.operation);
        if copies && let Some(stop) = self.measure_constant_value(&control) {
            control = stop;
        }
        if self.config.trace_mode == TraceMode::Full {
            self.emit(
                EventKind::OperationCompleted,
                node_id,
                span,
                Some(parent_span),
                operation,
                vec![text_attribute("outcome", control_name(&control))],
            );
        }
        control
    }

    fn eval_operation(
        &mut self,
        node_id: CoreNodeId,
        span: u64,
        mut environment: Environment,
        operation: CoreOperation,
    ) -> Control {
        match operation {
            CoreOperation::Constant(literal) => Control::Success(runtime_literal(literal)),
            CoreOperation::Reference(symbol) => self.eval_reference(symbol, span, environment),
            CoreOperation::Select { receiver, member } => {
                let value = success_or_return!(self.eval_node(receiver, span, environment));
                select_field(&value, &member).unwrap_or_else(|| Control::Defect {
                    message: format!("verified value has no field {member}"),
                    node: Some(node_id),
                    cause: None,
                })
            }
            CoreOperation::DependencyMethod {
                requirement,
                interface,
                method,
                name,
            } => {
                let requirement_name = self.core.symbol(requirement).name.clone();
                let callable = if let Some(provider) =
                    environment.requirements.get(&requirement_name).cloned()
                {
                    let selected = select_field(&provider, &name);
                    let Some(Control::Success(RuntimeValue::Callable(callable))) = &selected else {
                        return Control::Defect {
                            message: format!(
                                "provider {requirement_name} has no callable field {name}"
                            ),
                            node: Some(node_id),
                            cause: None,
                        };
                    };
                    callable.clone()
                } else if let Some(host) = self.host_function(interface, method) {
                    RuntimeCallable::Host(host)
                } else {
                    return Control::Defect {
                        message: format!("closed core has no provider for {requirement_name}"),
                        node: Some(node_id),
                        cause: None,
                    };
                };
                Control::Success(RuntimeValue::Callable(RuntimeCallable::Dependency {
                    callable: Box::new(callable),
                    requirement,
                    interface,
                    method,
                    name,
                }))
            }
            CoreOperation::Construct {
                constructor,
                arguments,
                cause,
            } => {
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(success_or_return!(self.eval_node(
                        argument,
                        span,
                        environment.clone()
                    )));
                }
                let control = self.construct(constructor, values, node_id);
                match cause {
                    Some(cause) => attach_matched_cause(control, &environment, cause, node_id),
                    None => control,
                }
            }
            CoreOperation::Call { callee, arguments } => {
                let callable =
                    success_or_return!(self.eval_node(callee, span, environment.clone()));
                let RuntimeValue::Callable(callable) = &callable else {
                    return Control::Defect {
                        message: "verified call target is not callable".to_owned(),
                        node: Some(node_id),
                        cause: None,
                    };
                };
                let callable = callable.clone();
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(success_or_return!(self.eval_node(
                        argument,
                        span,
                        environment.clone(),
                    )));
                }
                match self.invoke(callable, values, span, node_id, environment) {
                    // A failure an operation raised without a source construction
                    // starts at its call.
                    Control::Failure(value, trail)
                        if trail.cause.is_none() && trail.origin.is_none() =>
                    {
                        Control::Failure(value, Trail::at(node_id))
                    }
                    control => control,
                }
            }
            CoreOperation::List(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    values.push(success_or_return!(self.eval_node(
                        element,
                        span,
                        environment.clone()
                    )));
                }
                Control::Success(RuntimeValue::List(values.into_iter().collect()))
            }
            CoreOperation::Tuple(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    values.push(success_or_return!(self.eval_node(
                        element,
                        span,
                        environment.clone()
                    )));
                }
                Control::Success(RuntimeValue::Tuple(Arc::new(values)))
            }
            CoreOperation::Record(fields) => {
                let mut values = Vec::with_capacity(fields.len());
                for field in fields {
                    values.push(RuntimeField {
                        name: field.name,
                        value: success_or_return!(self.eval_node(
                            field.value,
                            span,
                            environment.clone()
                        )),
                    });
                }
                Control::Success(RuntimeValue::Record(Arc::new(values)))
            }
            CoreOperation::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => match success_or_return!(self.eval_node(operand, span, environment)) {
                RuntimeValue::Integer(value) => match integer_negate(value) {
                    Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                    Err(message) => Control::Defect {
                        message,
                        node: Some(node_id),
                        cause: None,
                    },
                },
                RuntimeValue::Float(value) => Control::Success(RuntimeValue::Float(-value)),
                RuntimeValue::Decimal(value) => {
                    Control::Success(RuntimeValue::Decimal(value.negate()))
                }
                _ => Control::Defect {
                    message: "verified unary operand is not an integer".to_owned(),
                    node: Some(node_id),
                    cause: None,
                },
            },
            CoreOperation::Unary {
                operator: UnaryOperator::Not,
                ..
            } => unreachable!("Core lowers `not` to a Bool match"),
            CoreOperation::Binary {
                operator,
                left,
                right,
            } => {
                let left = success_or_return!(self.eval_node(left, span, environment.clone()));
                let right = success_or_return!(self.eval_node(right, span, environment));
                self.eval_binary(node_id, operator, left, right)
            }
            CoreOperation::Identity(value) => self.eval_node(value, span, environment),
            CoreOperation::Provision {
                expression,
                providers,
            } => {
                for provider in providers {
                    let value = success_or_return!(self.eval_node(
                        provider.value,
                        span,
                        environment.clone(),
                    ));
                    environment.requirements.insert(provider.name, value);
                }
                self.eval_node(expression, span, environment)
            }
            CoreOperation::Sequence(steps) => {
                let mut last = None;
                for step in steps {
                    match step {
                        CoreStep::Bind { symbol, value } => {
                            let value = success_or_return!(self.eval_node(
                                value,
                                span,
                                environment.clone(),
                            ));
                            environment.locals.insert(symbol, value.clone());
                            last = Some(value);
                        }
                        CoreStep::Expression(value) => {
                            last = Some(success_or_return!(self.eval_node(
                                value,
                                span,
                                environment.clone(),
                            )));
                        }
                    }
                }
                Control::Success(last.expect("verified sequence is nonempty"))
            }
            CoreOperation::Closure {
                parameters, body, ..
            } => Control::Success(RuntimeValue::Callable(RuntimeCallable::Closure {
                parameters,
                body,
                environment: Arc::new(environment),
                name: "closure".to_owned(),
            })),
            CoreOperation::Match { subjects, arms } => {
                self.eval_match(node_id, span, environment, subjects, arms)
            }
            CoreOperation::Loop {
                state,
                initial,
                body,
            } => self.eval_loop(span, environment, state, initial, body),
            CoreOperation::Break { target, value } => {
                self.eval_break(span, environment, target, value)
            }
        }
    }

    /// Ensures: passes run iteratively, so host stack use is independent of the pass
    /// count; each pass is evaluated under the loop's span, so pass operations are
    /// siblings; a `Break` for this loop becomes `Success`, every other non-success
    /// control propagates unchanged; cancellation is observed before each pass.
    fn eval_loop(
        &mut self,
        span: u64,
        environment: Environment,
        state: SymbolId,
        initial: CoreNodeId,
        body: CoreNodeId,
    ) -> Control {
        let mut value = success_or_return!(self.eval_node(initial, span, environment.clone()));
        loop {
            if self.config.cancellation_requested {
                return Control::Cancellation(None);
            }
            let mut pass = environment.clone();
            pass.locals.insert(state, value);
            match self.eval_node(body, span, pass) {
                Control::Success(next) => value = next,
                Control::Break { target, value } if target == state => {
                    return Control::Success(value);
                }
                other => return other,
            }
        }
    }

    fn eval_break(
        &mut self,
        span: u64,
        environment: Environment,
        target: SymbolId,
        value: Option<CoreNodeId>,
    ) -> Control {
        let value = match value {
            Some(value) => success_or_return!(self.eval_node(value, span, environment)),
            None => RuntimeValue::Void,
        };
        Control::Break { target, value }
    }

    fn eval_reference(&mut self, symbol: SymbolId, span: u64, environment: Environment) -> Control {
        let core_symbol = self.core.symbol(symbol).clone();
        match core_symbol.kind {
            CoreSymbolKind::Function => {
                Control::Success(RuntimeValue::Callable(RuntimeCallable::Declared {
                    symbol,
                    requirements: environment.requirements,
                }))
            }
            CoreSymbolKind::ProcessName => Control::Success(RuntimeValue::ProcessAddress(symbol)),
            CoreSymbolKind::Constant => match self.constants.get(&symbol) {
                Some(value) => Control::Success(value.clone()),
                None => self.evaluate_constant_reference(symbol, span),
            },
            CoreSymbolKind::Requirement => environment
                .requirements
                .get(&core_symbol.name)
                .cloned()
                .map(Control::Success)
                .unwrap_or_else(|| Control::Defect {
                    message: format!("closed core has no provider for {}", core_symbol.name),
                    node: None,
                    cause: None,
                }),
            CoreSymbolKind::Local => environment
                .locals
                .get(&symbol)
                .cloned()
                .map(Control::Success)
                .unwrap_or_else(|| Control::Defect {
                    message: format!("verified local {} is not bound", core_symbol.name),
                    node: None,
                    cause: None,
                }),
            CoreSymbolKind::Intrinsic => match core_symbol.standard {
                Some(StandardSymbol::Function(function)) => {
                    Control::Success(RuntimeValue::Callable(RuntimeCallable::Intrinsic(function)))
                }
                _ => Control::Defect {
                    message: format!("intrinsic symbol {} has no operation", core_symbol.name),
                    node: None,
                    cause: None,
                },
            },
            CoreSymbolKind::Constructor => {
                Control::Success(RuntimeValue::Callable(RuntimeCallable::Constructor {
                    symbol,
                    name: core_symbol.name.clone(),
                }))
            }
            CoreSymbolKind::Other => Control::Defect {
                message: format!("symbol {} is not a runtime value", core_symbol.name),
                node: None,
                cause: None,
            },
        }
    }

    /// Constructs a value, or a live outcome; a failure's cause starts at `node`.
    fn construct(
        &self,
        constructor: SymbolId,
        values: Vec<RuntimeValue>,
        node: CoreNodeId,
    ) -> Control {
        let symbol = self.core.symbol(constructor);
        let shape = symbol
            .constructor
            .as_ref()
            .expect("core constructor symbol owns shape")
            .clone();
        if let Some(success) = shape.outcome_success {
            let value = values
                .into_iter()
                .next()
                .expect("outcome constructor has one argument");
            return if success {
                Control::Success(value)
            } else {
                Control::Failure(value, Trail::at(node))
            };
        }
        debug_assert_eq!(values.len(), shape.fields.len());
        let fields = shape
            .fields
            .iter()
            .zip(values)
            .map(|(field_name, value)| RuntimeField {
                name: field_name.clone(),
                value,
            })
            .collect::<Vec<_>>();
        Control::Success(RuntimeValue::Constructed {
            constructor,
            type_name: shape.type_name,
            variant_name: shape.variant_name,
            fields: Arc::new(fields),
            cause: None,
        })
    }

    fn invoke(
        &mut self,
        callable: RuntimeCallable,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        call_environment: Environment,
    ) -> Control {
        match callable {
            RuntimeCallable::Declared {
                symbol,
                mut requirements,
            } => {
                requirements.extend(call_environment.requirements);
                let function = self
                    .core
                    .functions
                    .iter()
                    .find(|function| function.symbol == symbol)
                    .expect("reachable function exists")
                    .clone();
                let mut environment = Environment {
                    locals: HashMap::new(),
                    requirements,
                };
                for (parameter, argument) in function.parameters.iter().zip(arguments) {
                    environment.locals.insert(*parameter, argument);
                }
                if let Some(stop) = self.enter_constant_call(Some(symbol)) {
                    return stop;
                }
                let control = self.eval_node(function.body, span, environment);
                self.leave_constant_call(Some(symbol));
                control
            }
            RuntimeCallable::Closure {
                parameters,
                body,
                environment,
                name: _,
            } => {
                let mut environment = Arc::unwrap_or_clone(environment);
                environment
                    .requirements
                    .extend(call_environment.requirements);
                for (parameter, argument) in parameters.iter().zip(arguments) {
                    environment.locals.insert(*parameter, argument);
                }
                if let Some(stop) = self.enter_constant_call(None) {
                    return stop;
                }
                let control = self.eval_node(body, span, environment);
                self.leave_constant_call(None);
                control
            }
            RuntimeCallable::Dependency {
                callable,
                requirement,
                interface,
                method,
                name,
            } => {
                if self.config.trace_mode != TraceMode::Off {
                    let dependency_span = self.next_span;
                    self.next_span += 1;
                    self.emit(
                        EventKind::DependencyCalled,
                        call_node,
                        dependency_span,
                        Some(span),
                        "dependency_call",
                        vec![
                            text_attribute(
                                "requirement",
                                self.core.symbol(requirement).name.clone(),
                            ),
                            symbol_attribute("requirement_symbol_id", requirement),
                            symbol_attribute("interface_symbol_id", interface),
                            text_attribute("method", name),
                            semantic_attribute("method_semantic_id", method),
                        ],
                    );
                }
                self.invoke(*callable, arguments, span, call_node, call_environment)
            }
            RuntimeCallable::Intrinsic(function) => {
                self.invoke_intrinsic(function, arguments, span, call_node, call_environment)
            }
            RuntimeCallable::Host(function) => {
                self.invoke_host(function, arguments, span, call_node, call_environment)
            }
            RuntimeCallable::Constructor { symbol, .. } => {
                self.construct(symbol, arguments, call_node)
            }
        }
    }

    fn host_function(&self, interface: SymbolId, method: SemanticId) -> Option<HostFunction> {
        match (self.core.symbol(interface).standard, method.raw()) {
            (Some(StandardSymbol::Type(StandardType::FileSystem)), 0) => {
                Some(HostFunction::ProcessUtf8Lines)
            }
            (Some(StandardSymbol::Type(StandardType::Console)), 0) => Some(HostFunction::WriteLine),
            (Some(StandardSymbol::Type(StandardType::Random)), 0) => {
                Some(HostFunction::RandomNextInt)
            }
            (Some(StandardSymbol::Type(StandardType::Console)), 1) => {
                Some(HostFunction::WriteError)
            }
            _ => None,
        }
    }

    fn invoke_host(
        &mut self,
        function: HostFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        if let Some(stop) =
            self.reject_constant_operation(constants::Operation::Host(function), call_node)
        {
            return stop;
        }
        match function {
            HostFunction::RandomNextInt => {
                let seed = self.config.random_seed;
                let generator = self.random.get_or_insert_with(|| {
                    seed.map_or_else(
                        crate::random_source::Generator::from_os,
                        crate::random_source::Generator::seeded,
                    )
                });
                Control::Success(RuntimeValue::Integer(generator.next().cast_signed()))
            }
            HostFunction::WriteLine | HostFunction::WriteError => {
                let [RuntimeValue::Text(line)] = arguments.as_slice() else {
                    return Control::Defect {
                        message: "verified console call has invalid runtime arguments".to_owned(),
                        node: Some(call_node),
                        cause: None,
                    };
                };
                let output = if matches!(function, HostFunction::WriteLine) {
                    &mut self.application_stdout
                } else {
                    &mut self.application_stderr
                };
                output.push_str(line);
                output.push('\n');
                Control::Success(RuntimeValue::Void)
            }
            HostFunction::ProcessUtf8Lines => {
                let [
                    RuntimeValue::Text(path),
                    initial,
                    RuntimeValue::Callable(callback),
                ] = arguments.as_slice()
                else {
                    return Control::Defect {
                        message: "verified filesystem call has invalid runtime arguments"
                            .to_owned(),
                        node: Some(call_node),
                        cause: None,
                    };
                };
                let mut input: Box<dyn Read> = if path == "-" {
                    Box::new(std::io::Cursor::new(self.config.standard_input.clone()))
                } else {
                    match std::fs::File::open(path) {
                        Ok(file) => Box::new(file),
                        Err(error) => {
                            return Control::failure(self.standard_value(
                                StandardConstructor::FileOpen,
                                vec![
                                    RuntimeValue::Text(path.clone()),
                                    RuntimeValue::Text(error.to_string()),
                                ],
                            ));
                        }
                    }
                };
                let mut state = initial.clone();
                let mut line = Vec::with_capacity(self.config.max_line_bytes.min(4096));
                let mut line_number = 1_i64;
                let mut byte = [0_u8; 1];
                loop {
                    if self.config.cancellation_requested {
                        return Control::Cancellation(None);
                    }
                    match input.read(&mut byte) {
                        Ok(0) => {
                            if line.is_empty() {
                                return Control::Success(state);
                            }
                            return self.invoke_utf8_line(
                                callback.clone(),
                                state,
                                &line,
                                path,
                                line_number,
                                span,
                                call_node,
                                environment,
                            );
                        }
                        Ok(_) if byte[0] == b'\n' => {
                            let length = if line.last() == Some(&b'\r') {
                                line.len() - 1
                            } else {
                                line.len()
                            };
                            state = success_or_return!(self.invoke_utf8_line(
                                callback.clone(),
                                state,
                                &line[..length],
                                path,
                                line_number,
                                span,
                                call_node,
                                environment.clone(),
                            ));
                            line.clear();
                            line_number += 1;
                        }
                        Ok(_) => {
                            if line.len() == self.config.max_line_bytes {
                                return Control::failure(self.standard_value(
                                    StandardConstructor::FileLineTooLong,
                                    vec![
                                        RuntimeValue::Text(path.clone()),
                                        RuntimeValue::Integer(
                                            i64::try_from(self.config.max_line_bytes)
                                                .unwrap_or(i64::MAX),
                                        ),
                                    ],
                                ));
                            }
                            line.push(byte[0]);
                        }
                        Err(error) => {
                            return Control::failure(self.standard_value(
                                StandardConstructor::FileRead,
                                vec![
                                    RuntimeValue::Text(path.clone()),
                                    RuntimeValue::Text(error.to_string()),
                                ],
                            ));
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_utf8_line(
        &mut self,
        callback: RuntimeCallable,
        state: RuntimeValue,
        bytes: &[u8],
        path: &str,
        line_number: i64,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        let Ok(line) = std::str::from_utf8(bytes) else {
            return Control::failure(self.standard_value(
                StandardConstructor::Utf8Invalid,
                vec![
                    RuntimeValue::Text(path.to_owned()),
                    RuntimeValue::Integer(line_number),
                ],
            ));
        };
        if line.contains('\0') {
            return Control::failure(self.standard_value(
                StandardConstructor::Utf8Invalid,
                vec![
                    RuntimeValue::Text(path.to_owned()),
                    RuntimeValue::Integer(line_number),
                ],
            ));
        }
        self.invoke(
            callback,
            vec![state, RuntimeValue::Text(line.to_owned())],
            span,
            call_node,
            environment,
        )
    }

    fn invoke_intrinsic(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        if let Some(stop) =
            self.reject_constant_operation(constants::Operation::Standard(function), call_node)
        {
            return stop;
        }
        match function {
            StandardFunction::DurationNanoseconds
            | StandardFunction::DurationMicroseconds
            | StandardFunction::DurationMilliseconds
            | StandardFunction::DurationSeconds
            | StandardFunction::DurationMinutes
            | StandardFunction::DurationHours
            | StandardFunction::DurationToNanoseconds
            | StandardFunction::DurationToMicroseconds
            | StandardFunction::DurationToMilliseconds
            | StandardFunction::DurationToSeconds
            | StandardFunction::DurationAdd
            | StandardFunction::DurationSubtract
            | StandardFunction::DurationMultiply
            | StandardFunction::DurationDivide
            | StandardFunction::DurationNegate
            | StandardFunction::DurationAbsolute
            | StandardFunction::DurationToText
            | StandardFunction::DurationParse => {
                self.invoke_duration(function, &arguments, call_node)
            }
            StandardFunction::ProcessSpec
            | StandardFunction::ProcessRegister
            | StandardFunction::ProcessReceive
            | StandardFunction::ProcessSend
            | StandardFunction::ProcessOwnAddress
            | StandardFunction::ProcessQueued
            | StandardFunction::ProcessCapacity
            | StandardFunction::ProcessMonitor
            | StandardFunction::ProcessUnmonitor
            | StandardFunction::ProcessHandoffSpec
            | StandardFunction::SupervisorProcess
            | StandardFunction::SupervisorSupervisor
            | StandardFunction::SupervisorPolicy
            | StandardFunction::SupervisorRestarts
            | StandardFunction::SupervisorStart
            | StandardFunction::SupervisorStartProcess
            | StandardFunction::SupervisorStartSupervisor
            | StandardFunction::SupervisorStartHandoff
            | StandardFunction::SupervisorAddress
            | StandardFunction::SupervisorRemoveProcess
            | StandardFunction::SupervisorRemoveSupervisor
            | StandardFunction::SupervisorStop
            | StandardFunction::SupervisorWait
            | StandardFunction::ClockSleep => self.invoke_process(function, arguments, call_node),
            StandardFunction::ClockNow
            | StandardFunction::ClockTimestamp
            | StandardFunction::ClockSince
            | StandardFunction::ClockSleepUntil
            | StandardFunction::ClockAdd
            | StandardFunction::ClockBetween
            | StandardFunction::ClockTimestampToText
            | StandardFunction::ClockUnixNanoseconds
            | StandardFunction::ClockFromUnixNanoseconds
            | StandardFunction::ClockTimestampBetween
            | StandardFunction::ClockStartingAt
            | StandardFunction::TaskTimeout
            | StandardFunction::TaskDeadline => self.invoke_process(function, arguments, call_node),
            StandardFunction::SelectReceive
            | StandardFunction::SelectInbox
            | StandardFunction::SelectTask
            | StandardFunction::SelectAfter
            | StandardFunction::SelectAt
            | StandardFunction::SelectFirst
            | StandardFunction::SelectWatch => Control::Defect {
                message: format!(
                    "Select.{} requires the native runtime; build the program with `chimi build`",
                    function.name()
                ),
                node: Some(call_node),
                cause: None,
            },
            StandardFunction::TestAssertEqual
            | StandardFunction::TestAssertNotEqual
            | StandardFunction::TestAssert
            | StandardFunction::TestFail => self.invoke_assertion(function, arguments, call_node),
            StandardFunction::TestCaptureDefect => {
                self.invoke_capture_defect(arguments, span, call_node, environment)
            }
            StandardFunction::TestRecorder
            | StandardFunction::TestRecord
            | StandardFunction::TestRecorded
            | StandardFunction::TestScript
            | StandardFunction::TestNext => self.invoke_test_log(function, arguments, call_node),
            StandardFunction::TestMailbox | StandardFunction::TestReceived => Control::Defect {
                message: format!(
                    "Test.{} requires the native backend, as processes do",
                    function.name()
                ),
                node: Some(call_node),
                cause: None,
            },
            // Setup refuses a program that reaches a binding (`E5204`), so only
            // a caller that skipped it gets here.
            StandardFunction::BindingCall => Control::Defect {
                message: "E5204: the interpreter cannot call C bindings; build the program with `chimi build`".to_owned(),
                node: Some(call_node),
                cause: None,
            },
            StandardFunction::DebugCrash
            | StandardFunction::OptionUnwrapOrFail
            | StandardFunction::TextConcat
            | StandardFunction::TextByteLength
            | StandardFunction::TextByteSlice
            | StandardFunction::TextContains
            | StandardFunction::TextStartsWith
            | StandardFunction::TextEndsWith
            | StandardFunction::TextSplit
            | StandardFunction::TextReplace
            | StandardFunction::TextTrimStart
            | StandardFunction::TextTrimEnd
            | StandardFunction::TextLowercase
            | StandardFunction::TextUppercase
            | StandardFunction::TextGraphemes
            | StandardFunction::ListGet
            | StandardFunction::ListPush
            | StandardFunction::ListSlice
            | StandardFunction::ListSet => {
                self.invoke_core_operation(function, arguments, call_node)
            }
            StandardFunction::MapEmpty
            | StandardFunction::MapGet
            | StandardFunction::MapPut
            | StandardFunction::MapRemove
            | StandardFunction::MapSize
            | StandardFunction::MapReduceEntries => {
                self.invoke_map_operation(function, arguments, span, call_node, environment)
            }
            StandardFunction::SetEmpty
            | StandardFunction::SetContains
            | StandardFunction::SetAdd
            | StandardFunction::SetRemove
            | StandardFunction::SetSize
            | StandardFunction::SetReduceItems => {
                self.invoke_set_operation(function, arguments, span, call_node, environment)
            }
            StandardFunction::JsonDecode
            | StandardFunction::JsonEncode
            | StandardFunction::JsonInt
            | StandardFunction::JsonNumberToInt
            | StandardFunction::JsonNumberToDecimal => {
                self.invoke_json_operation(function, arguments, span, call_node, environment)
            }
            StandardFunction::DecimalScaled
            | StandardFunction::DecimalRead
            | StandardFunction::DecimalToText
            | StandardFunction::DecimalAddChecked
            | StandardFunction::DecimalSubtractChecked
            | StandardFunction::DecimalMultiplyChecked
            | StandardFunction::DecimalDivideRounded
            | StandardFunction::DecimalToFloat => {
                self.invoke_decimal_operation(function, arguments, span, call_node, environment)
            }
            StandardFunction::BytesFrom
            | StandardFunction::BytesLength
            | StandardFunction::BytesGet
            | StandardFunction::BytesIndexOf
            | StandardFunction::BytesConcat
            | StandardFunction::BytesSlice
            | StandardFunction::BytesToList
            | StandardFunction::BytesFromText
            | StandardFunction::BytesToTextLossy
            | StandardFunction::BytesValidUtf8Length
            | StandardFunction::BytesReadIntCode
            | StandardFunction::BytesIntBytes
            | StandardFunction::EmbedText
            | StandardFunction::EmbedBytes => {
                self.invoke_bytes_operation(function, arguments, call_node)
            }
            StandardFunction::CauseContext
            | StandardFunction::CauseOf
            | StandardFunction::CauseKindCode
            | StandardFunction::CauseSummary
            | StandardFunction::CauseFrames
            | StandardFunction::CauseCausedBy
            | StandardFunction::CauseSuppressed
            | StandardFunction::CauseReasonCode
            | StandardFunction::CauseToText => {
                self.invoke_cause_operation(function, arguments, span, call_node, environment)
            }
            StandardFunction::ResourceScoped => {
                self.invoke_resource_scoped(arguments, span, call_node, environment)
            }
            StandardFunction::EnvHostVariables | StandardFunction::EnvHostCurrentDirectory => {
                self.invoke_host_operation(function, arguments, call_node)
            }
            StandardFunction::IoPath
            | StandardFunction::PathFrom
            | StandardFunction::PathToText => {
                let [RuntimeValue::Text(path)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Text(path.clone()))
            }
            StandardFunction::ListFirst => {
                let [RuntimeValue::List(values)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(match values.first() {
                    Some(value) => {
                        self.standard_value(StandardConstructor::OptionSome, vec![value.clone()])
                    }
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                })
            }
            StandardFunction::ListLength => {
                let [RuntimeValue::List(values)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Integer(
                    i64::try_from(values.len()).unwrap_or(i64::MAX),
                ))
            }
            StandardFunction::TextAppend => {
                let [RuntimeValue::Text(left), RuntimeValue::Text(right)] = arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Text(format!("{left}{right}")))
            }
            StandardFunction::TextEqual => {
                let [RuntimeValue::Text(left), RuntimeValue::Text(right)] = arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Boolean(left == right))
            }
            StandardFunction::IntParse => {
                let [RuntimeValue::Text(text)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                match parse_int(text) {
                    Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                    Err(error) => Control::failure(self.standard_value(
                        match error {
                            IntParseError::Empty => StandardConstructor::IntParseEmpty,
                            IntParseError::InvalidDigit => {
                                StandardConstructor::IntParseInvalidDigit
                            }
                            IntParseError::OutOfRange => StandardConstructor::IntParseOutOfRange,
                        },
                        Vec::new(),
                    )),
                }
            }
            StandardFunction::IntToText => {
                let [RuntimeValue::Integer(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Text(value.to_string()))
            }
            StandardFunction::IntAddChecked
            | StandardFunction::IntSubtractChecked
            | StandardFunction::IntMultiplyChecked
            | StandardFunction::IntDivideChecked
            | StandardFunction::IntRemainderChecked => {
                let [RuntimeValue::Integer(left), RuntimeValue::Integer(right)] =
                    arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                let operator = match function {
                    StandardFunction::IntAddChecked => ArithmeticOperator::Add,
                    StandardFunction::IntSubtractChecked => ArithmeticOperator::Subtract,
                    StandardFunction::IntMultiplyChecked => ArithmeticOperator::Multiply,
                    StandardFunction::IntDivideChecked => ArithmeticOperator::Divide,
                    _ => ArithmeticOperator::Remainder,
                };
                Control::Success(match integer_binary(operator, *left, *right) {
                    Ok(value) => self.standard_value(
                        StandardConstructor::OptionSome,
                        vec![RuntimeValue::Integer(value)],
                    ),
                    Err(_) => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                })
            }
            StandardFunction::OrderCompare
            | StandardFunction::OrderMin
            | StandardFunction::OrderMax => {
                let [left, right] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                let order = self.structural_order(left, right);
                Control::Success(match function {
                    StandardFunction::OrderCompare => self.standard_value(
                        match order {
                            Comparison::Less => StandardConstructor::OrderingLess,
                            Comparison::Equal => StandardConstructor::OrderingEqual,
                            Comparison::Greater => StandardConstructor::OrderingGreater,
                        },
                        Vec::new(),
                    ),
                    // Both return the left operand when the two are equal.
                    StandardFunction::OrderMin if order.is_gt() => right.clone(),
                    StandardFunction::OrderMax if order.is_lt() => right.clone(),
                    _ => left.clone(),
                })
            }
            StandardFunction::IntModulo => {
                let [RuntimeValue::Integer(value), RuntimeValue::Integer(modulus)] =
                    arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                match integer_modulo(*value, *modulus) {
                    Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                    Err(message) => Control::Defect {
                        message,
                        node: Some(call_node),
                        cause: None,
                    },
                }
            }
            StandardFunction::RangeStart
            | StandardFunction::RangeEnd
            | StandardFunction::RangeLength => {
                let [RuntimeValue::Range { start, end }] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Integer(match function {
                    StandardFunction::RangeStart => *start,
                    StandardFunction::RangeEnd => *end,
                    _ => match range_length(*start, *end) {
                        Some(length) => length,
                        None => {
                            return Control::Defect {
                                message: RANGE_LENGTH_OVERFLOW.to_owned(),
                                node: Some(call_node),
                                cause: None,
                            };
                        }
                    },
                }))
            }
            StandardFunction::RangeContains => {
                let [
                    RuntimeValue::Range { start, end },
                    RuntimeValue::Integer(value),
                ] = arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Boolean(start <= value && value < end))
            }
            StandardFunction::FloatFromInt => {
                let [RuntimeValue::Integer(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                // `as` rounds to nearest, ties to even.
                Control::Success(RuntimeValue::Float(*value as f64))
            }
            StandardFunction::FloatTruncate
            | StandardFunction::FloatFloor
            | StandardFunction::FloatCeiling
            | StandardFunction::FloatRound => {
                let [RuntimeValue::Float(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Float(match function {
                    StandardFunction::FloatTruncate => value.trunc(),
                    StandardFunction::FloatFloor => value.floor(),
                    StandardFunction::FloatCeiling => value.ceil(),
                    // Half away from zero, like C `round`.
                    _ => value.round(),
                }))
            }
            StandardFunction::FloatToInt => {
                let [RuntimeValue::Float(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                match float_to_int(*value) {
                    Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                    Err(error) => Control::failure(self.standard_value(
                        match error {
                            FloatToIntError::NotInteger => StandardConstructor::FloatNotInteger,
                            FloatToIntError::OutOfRange => StandardConstructor::FloatOutOfRange,
                        },
                        Vec::new(),
                    )),
                }
            }
            StandardFunction::DebugRender | StandardFunction::DebugRenderIndented => {
                let [value] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Text(debug::render(
                    self.core,
                    value,
                    function == StandardFunction::DebugRenderIndented,
                )))
            }
            StandardFunction::FloatToText => {
                let [RuntimeValue::Float(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Text(render_float(*value)))
            }
            StandardFunction::FloatParse => {
                let [RuntimeValue::Text(text)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                match parse_float(text) {
                    Ok(value) => Control::Success(RuntimeValue::Float(value)),
                    Err(error) => Control::failure(self.standard_value(
                        match error {
                            FloatParseError::Empty => StandardConstructor::FloatParseEmpty,
                            FloatParseError::Invalid => StandardConstructor::FloatParseInvalid,
                            FloatParseError::OutOfRange => {
                                StandardConstructor::FloatParseOutOfRange
                            }
                        },
                        Vec::new(),
                    )),
                }
            }
            StandardFunction::TaskStart
            | StandardFunction::TaskResult
            | StandardFunction::TaskCancel
            | StandardFunction::TaskAll
            | StandardFunction::TaskResults
            | StandardFunction::TaskRace => Control::Defect {
                message: format!(
                    "standard function {} requires the finite Task runtime",
                    function.name()
                ),
                node: Some(call_node),
                cause: None,
            },
            StandardFunction::ChannelSend
            | StandardFunction::ChannelReceive
            | StandardFunction::ChannelClose
            | StandardFunction::ChannelAbort
            | StandardFunction::ChannelBounded => Control::Defect {
                message: format!(
                    "standard function {} requires the bounded Channel runtime",
                    function.name()
                ),
                node: Some(call_node),
                cause: None,
            },
            StandardFunction::ChannelAddReceiver
            | StandardFunction::BroadcastBounded
            | StandardFunction::BroadcastPublish
            | StandardFunction::BroadcastSubscribe
            | StandardFunction::WatchStartingWith
            | StandardFunction::WatchPublish
            | StandardFunction::WatchSubscribe
            | StandardFunction::WatchReceive => Control::Defect {
                message: format!(
                    "{}.{} requires the bounded Channel runtime; build the program with `chimi build`",
                    function
                        .module()
                        .map_or_else(String::new, |module| module.qualifier()),
                    function.name()
                ),
                node: Some(call_node),
                cause: None,
            },
        }
    }

    fn intrinsic_shape_defect(&self, function: StandardFunction, call_node: CoreNodeId) -> Control {
        Control::Defect {
            message: format!(
                "verified arguments for standard function {} have invalid runtime shapes",
                function.name()
            ),
            node: Some(call_node),
            cause: None,
        }
    }

    fn standard_value(
        &self,
        constructor: StandardConstructor,
        values: Vec<RuntimeValue>,
    ) -> RuntimeValue {
        let symbol = self.standard_constructor_symbol(constructor);
        let shape = self
            .core
            .symbol(symbol)
            .constructor
            .as_ref()
            .expect("standard constructor owns a core shape");
        RuntimeValue::Constructed {
            constructor: symbol,
            type_name: shape.type_name.clone(),
            variant_name: shape.variant_name.clone(),
            fields: Arc::new(
                shape
                    .fields
                    .iter()
                    .cloned()
                    .zip(values)
                    .map(|(name, value)| RuntimeField { name, value })
                    .collect(),
            ),
            cause: None,
        }
    }

    fn standard_constructor_symbol(&self, constructor: StandardConstructor) -> SymbolId {
        self.core
            .symbols
            .iter()
            .find_map(|symbol| {
                (symbol.standard == Some(StandardSymbol::Constructor(constructor)))
                    .then_some(symbol.id)
            })
            .expect("core retains every standard constructor")
    }

    fn key_order(&self) -> impl Fn(&RuntimeValue, &RuntimeValue) -> Comparison + '_ {
        |left, right| self.structural_order(left, right)
    }

    fn structural_order(&self, left: &RuntimeValue, right: &RuntimeValue) -> Comparison {
        structural::structural_order(left, right, &|constructor| {
            self.core
                .symbol(constructor)
                .constructor
                .as_ref()
                .map_or(0, |shape| shape.ordinal)
        })
    }

    fn eval_binary(
        &self,
        node: CoreNodeId,
        operator: BinaryOperator,
        left: RuntimeValue,
        right: RuntimeValue,
    ) -> Control {
        let defect = |message| Control::Defect {
            message,
            node: Some(node),
            cause: None,
        };
        if let Some(arithmetic) = ArithmeticOperator::from_binary(operator) {
            return match (left, right) {
                (RuntimeValue::Integer(left), RuntimeValue::Integer(right)) => {
                    match integer_binary(arithmetic, left, right) {
                        Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                        Err(message) => defect(message),
                    }
                }
                (RuntimeValue::Float(left), RuntimeValue::Float(right)) => {
                    match float_binary(arithmetic, left, right) {
                        Ok(value) => Control::Success(RuntimeValue::Float(value)),
                        Err(message) => defect(message),
                    }
                }
                (RuntimeValue::Decimal(left), RuntimeValue::Decimal(right)) => {
                    match decimal_binary(arithmetic, left, right) {
                        Ok(value) => Control::Success(RuntimeValue::Decimal(value)),
                        Err(message) => defect(message),
                    }
                }
                _ => defect("verified binary operands are not numbers".to_owned()),
            };
        }
        match (operator, &left, &right) {
            (BinaryOperator::Range, RuntimeValue::Integer(start), RuntimeValue::Integer(end)) => {
                return Control::Success(RuntimeValue::Range {
                    start: *start,
                    end: *end,
                });
            }
            (
                BinaryOperator::RangeInclusive,
                RuntimeValue::Integer(start),
                RuntimeValue::Integer(end),
            ) => {
                return match end.checked_add(1) {
                    Some(end) => Control::Success(RuntimeValue::Range { start: *start, end }),
                    None => defect(INCLUSIVE_RANGE_OVERFLOW.to_owned()),
                };
            }
            (BinaryOperator::Range | BinaryOperator::RangeInclusive, _, _) => {
                return defect("verified range bounds are not integers".to_owned());
            }
            _ => {}
        }
        let ordering = self.structural_order(&left, &right);
        let result = match operator {
            BinaryOperator::Less => ordering.is_lt(),
            BinaryOperator::LessEqual => ordering.is_le(),
            BinaryOperator::Greater => ordering.is_gt(),
            BinaryOperator::GreaterEqual => ordering.is_ge(),
            BinaryOperator::Equal => ordering.is_eq(),
            BinaryOperator::And | BinaryOperator::Or | BinaryOperator::NotEqual => {
                unreachable!("Core lowers `and`, `or`, and `!=` to a Bool match")
            }
            BinaryOperator::Range | BinaryOperator::RangeInclusive => {
                unreachable!("ranges returned above")
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => unreachable!("arithmetic returned above"),
        };
        Control::Success(RuntimeValue::Boolean(result))
    }

    fn eval_match(
        &mut self,
        node_id: CoreNodeId,
        span: u64,
        environment: Environment,
        subjects: Vec<CoreNodeId>,
        arms: Vec<CoreArm>,
    ) -> Control {
        let contextual = (0..subjects.len())
            .map(|index| {
                arms.iter()
                    .any(|arm| matches!(arm.patterns.get(index), Some(CorePattern::Outcome { .. })))
            })
            .collect::<Vec<_>>();
        let mut values = Vec::with_capacity(subjects.len());
        for (index, subject) in subjects.into_iter().enumerate() {
            let control = self.eval_node(subject, span, environment.clone());
            if contextual[index] {
                match control {
                    Control::Success(value) => values.push(MatchValue::Success(value)),
                    Control::Failure(value, trail) => {
                        values.push(MatchValue::Failure(value, trail));
                    }
                    other => return other,
                }
            } else {
                values.push(MatchValue::Plain(success_or_return!(control)));
            }
        }
        for arm in arms {
            let mut arm_environment = environment.clone();
            let mut matched = true;
            for (pattern, value) in arm.patterns.iter().zip(&values) {
                match match_pattern(pattern, value, &mut arm_environment.locals) {
                    Ok(true) => {}
                    Ok(false) => {
                        matched = false;
                        break;
                    }
                    Err(message) => {
                        return Control::Defect {
                            message: message.to_owned(),
                            node: Some(node_id),
                            cause: None,
                        };
                    }
                }
            }
            if !matched {
                continue;
            }
            if let Some(guard) = arm.guard {
                match self.eval_node(guard, span, arm_environment.clone()) {
                    Control::Success(RuntimeValue::Boolean(true)) => {}
                    Control::Success(RuntimeValue::Boolean(false)) => continue,
                    Control::Success(_) => {
                        return Control::Defect {
                            message: "verified case guard is not Boolean".to_owned(),
                            node: Some(node_id),
                            cause: None,
                        };
                    }
                    other => return other,
                }
            }
            if let Some(cause) = arm.cause {
                arm_environment
                    .locals
                    .insert(cause.local, matched_cause(&values[cause.subject]));
            }
            return self.eval_node(arm.body, span, arm_environment);
        }
        values
            .into_iter()
            .find_map(|value| match value {
                MatchValue::Failure(error, trail) => Some(Control::Failure(error, trail)),
                MatchValue::Plain(_) | MatchValue::Success(_) => None,
            })
            .unwrap_or_else(|| Control::Defect {
                message: "verified exhaustive match had no matching arm".to_owned(),
                node: Some(node_id),
                cause: None,
            })
    }

    fn emit(
        &mut self,
        kind: EventKind,
        node_id: CoreNodeId,
        span_id: u64,
        parent_span_id: Option<u64>,
        operation: &str,
        attributes: Vec<ExecutionAttribute>,
    ) {
        let node = &self.core.nodes[node_id.raw() as usize];
        let origin = &self.core.origins[node.origin.raw() as usize];
        self.events.push(ExecutionEvent {
            sequence: self.next_sequence,
            kind,
            computation: self.core.computation.clone(),
            execution_id: self.config.execution_id.clone(),
            span_id,
            parent_span_id,
            core_node_id: node.id.raw(),
            origin_id: origin.origin_id.raw(),
            semantic_id: origin.semantic_id.raw(),
            file_id: origin.file_id.raw(),
            source_range: origin.range,
            operation: operation.to_owned(),
            attributes,
        });
        self.next_sequence += 1;
    }

    fn public_outcome(&self, control: Control) -> ExecutionOutcome {
        match control {
            Control::Success(value) => ExecutionOutcome::Success(public_value(&value)),
            Control::Failure(value, _) => ExecutionOutcome::ExpectedFailure(public_value(&value)),
            Control::Defect { message, node, .. } => {
                let origin = node.map(|node| {
                    let node = &self.core.nodes[node.raw() as usize];
                    node.origin.raw()
                });
                ExecutionOutcome::Defect(ExecutionDefect {
                    message,
                    core_node_id: node.map(CoreNodeId::raw),
                    origin_id: origin,
                })
            }
            Control::Cancellation(_) => ExecutionOutcome::Cancellation(ExecutionCancellation {
                reason: CANCELLATION_REASON.to_owned(),
            }),
            Control::Break { .. } => ExecutionOutcome::Defect(ExecutionDefect {
                message: "verified break escaped its loop".to_owned(),
                core_node_id: None,
                origin_id: None,
            }),
        }
    }
}

#[derive(Clone, Debug)]
enum MatchValue {
    Plain(RuntimeValue),
    Success(RuntimeValue),
    Failure(RuntimeValue, Trail),
}

/// The cause a cause arm binds for its subject: a live failure's, or the one a
/// stored `Err` keeps, or `Void` when it keeps none.
fn matched_cause(value: &MatchValue) -> RuntimeValue {
    let cause = match value {
        MatchValue::Failure(error, trail) => Some(trail.cause.clone().unwrap_or_else(|| {
            Arc::new(Cause::new(
                CauseKind::ExpectedFailure(error.clone()),
                trail.origin,
            ))
        })),
        MatchValue::Plain(RuntimeValue::Constructed { cause, .. }) => cause.clone(),
        MatchValue::Plain(_) | MatchValue::Success(_) => None,
    };
    cause.map_or(RuntimeValue::Void, RuntimeValue::Cause)
}

/// Gives a `Failure(...)` or `Result.Err(...)` built in a cause arm the arm's
/// matched cause, read from the arm's generated local: the same cause when it
/// keeps it, otherwise a new cause started at `node` and linked to it.
fn attach_matched_cause(
    control: Control,
    environment: &Environment,
    cause: crate::executable::ConstructCause,
    node: CoreNodeId,
) -> Control {
    let matched = match environment.locals.get(&cause.local) {
        Some(RuntimeValue::Cause(matched)) => Some(Arc::clone(matched)),
        _ => None,
    };
    let derive = |error: &RuntimeValue| -> Option<Arc<Cause>> {
        if cause.keep {
            return matched.clone();
        }
        let fresh = Cause::new(CauseKind::ExpectedFailure(error.clone()), Some(node));
        Some(Arc::new(match &matched {
            Some(matched) => fresh.linked_to(Arc::clone(matched)),
            None => fresh,
        }))
    };
    match control {
        Control::Failure(error, trail) => {
            let cause = derive(&error);
            let trail = match cause {
                Some(cause) => Trail {
                    origin: cause.origin,
                    cause: Some(cause),
                },
                None => trail,
            };
            Control::Failure(error, trail)
        }
        Control::Success(mut value) => {
            if let RuntimeValue::Constructed { fields, cause, .. } = &mut value {
                *cause = fields.first().and_then(|field| derive(&field.value));
            }
            Control::Success(value)
        }
        other => other,
    }
}

/// Matches `value` against `pattern`, binding into `bindings`. `Err` is a defect: a
/// verified pattern asked for a list element or view the value does not have.
fn match_pattern(
    pattern: &CorePattern,
    value: &MatchValue,
    bindings: &mut HashMap<SymbolId, RuntimeValue>,
) -> Result<bool, &'static str> {
    match pattern {
        CorePattern::Outcome {
            success: expected,
            value: pattern,
        } => match (expected, value) {
            (true, MatchValue::Success(value)) | (false, MatchValue::Failure(value, _)) => {
                match_plain_pattern(pattern, value, bindings)
            }
            _ => Ok(false),
        },
        _ => match value {
            MatchValue::Plain(value) => match_plain_pattern(pattern, value, bindings),
            MatchValue::Success(_) | MatchValue::Failure(..) => Ok(false),
        },
    }
}

fn match_plain_pattern(
    pattern: &CorePattern,
    value: &RuntimeValue,
    bindings: &mut HashMap<SymbolId, RuntimeValue>,
) -> Result<bool, &'static str> {
    match pattern {
        CorePattern::Wildcard => Ok(true),
        CorePattern::Binding(symbol) => {
            bindings.insert(*symbol, value.clone());
            Ok(true)
        }
        CorePattern::Alias { pattern, binding } => {
            if !match_plain_pattern(pattern, value, bindings)? {
                return Ok(false);
            }
            bindings.insert(*binding, value.clone());
            Ok(true)
        }
        CorePattern::Literal(literal) => Ok(literal_matches(literal, value)),
        CorePattern::Constructor { symbol, fields } => {
            let RuntimeValue::Constructed {
                constructor,
                fields: values,
                ..
            } = value
            else {
                return Ok(false);
            };
            if constructor != symbol || fields.len() != values.len() {
                return Ok(false);
            }
            for (pattern, field) in fields.iter().zip(values.iter()) {
                if !match_plain_pattern(pattern, &field.value, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        CorePattern::List {
            prefix,
            rest,
            suffix,
        } => {
            let RuntimeValue::List(values) = value else {
                return Ok(false);
            };
            match_list_pattern(prefix, rest.as_deref(), suffix, values, bindings)
        }
        CorePattern::Tuple(elements) => {
            let RuntimeValue::Tuple(values) = value else {
                return Ok(false);
            };
            if elements.len() != values.len() {
                return Ok(false);
            }
            for (pattern, value) in elements.iter().zip(values.iter()) {
                if !match_plain_pattern(pattern, value, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        CorePattern::Outcome { .. } => Ok(false),
    }
}

/// Checks the length first, then reads each fixed element by index; `rest` receives an
/// O(1) view of the elements between the prefix and the suffix.
fn match_list_pattern(
    prefix: &[CorePattern],
    rest: Option<&CorePattern>,
    suffix: &[CorePattern],
    values: &PersistentList<RuntimeValue>,
    bindings: &mut HashMap<SymbolId, RuntimeValue>,
) -> Result<bool, &'static str> {
    let length = values.len();
    let fits = match rest {
        None => length == prefix.len() && suffix.is_empty(),
        Some(_) => length >= prefix.len() + suffix.len(),
    };
    if !fits {
        return Ok(false);
    }
    let suffix_start = length - suffix.len();
    let fixed = prefix
        .iter()
        .enumerate()
        .chain(
            suffix
                .iter()
                .enumerate()
                .map(|(offset, pattern)| (suffix_start + offset, pattern)),
        )
        .map(|(index, pattern)| (pattern, index));
    for (pattern, index) in fixed {
        let element = values
            .get(index)
            .ok_or("verified list pattern read past the end of its list")?;
        if !match_plain_pattern(pattern, element, bindings)? {
            return Ok(false);
        }
    }
    match rest {
        None | Some(CorePattern::Wildcard) => Ok(true),
        Some(rest) => {
            let view = values
                .slice(prefix.len()..suffix_start)
                .ok_or("verified list pattern took a view outside its list")?;
            match_plain_pattern(rest, &RuntimeValue::List(view), bindings)
        }
    }
}

/// The defect of `Range.length` when the count of a range exceeds `Int`.
const RANGE_LENGTH_OVERFLOW: &str =
    "Range.length overflows Int: the range holds more than the largest Int elements";

/// `end - start` for a nonempty range, 0 for an empty one, or `None` when the count
/// exceeds `Int`.
fn range_length(start: i64, end: i64) -> Option<i64> {
    if end <= start {
        Some(0)
    } else {
        end.checked_sub(start)
    }
}

/// The defect of `start..=end` when `end` is the largest `Int`.
const INCLUSIVE_RANGE_OVERFLOW: &str =
    "the inclusive range `..=` ends at the largest Int, so its exclusive end overflows";

fn literal_matches(literal: &CoreLiteral, value: &RuntimeValue) -> bool {
    match (literal, value) {
        (CoreLiteral::Integer(left), RuntimeValue::Integer(right)) => left == right,
        (CoreLiteral::Text(left), RuntimeValue::Text(right)) => left == right,
        (CoreLiteral::Boolean(left), RuntimeValue::Boolean(right)) => left == right,
        _ => false,
    }
}

fn select_field(value: &RuntimeValue, member: &str) -> Option<Control> {
    let fields = match value {
        RuntimeValue::Record(fields) | RuntimeValue::Constructed { fields, .. } => fields,
        _ => return None,
    };
    fields
        .iter()
        .find(|field| field.name == member)
        .map(|field| Control::Success(field.value.clone()))
}

fn runtime_literal(literal: CoreLiteral) -> RuntimeValue {
    match literal {
        CoreLiteral::Integer(value) => RuntimeValue::Integer(value),
        CoreLiteral::Float(value) => RuntimeValue::Float(value),
        CoreLiteral::Text(value) => RuntimeValue::Text(value),
        CoreLiteral::Boolean(value) => RuntimeValue::Boolean(value),
        CoreLiteral::Void => RuntimeValue::Void,
    }
}

fn public_value(value: &RuntimeValue) -> ExecutionValue {
    match value {
        RuntimeValue::Void => ExecutionValue::Void,
        RuntimeValue::Integer(value) => ExecutionValue::Integer(*value),
        RuntimeValue::Float(value) => ExecutionValue::Float(FloatValue(*value)),
        RuntimeValue::Text(value) | RuntimeValue::JsonNumber(value) => {
            ExecutionValue::Text(value.clone())
        }
        RuntimeValue::Boolean(value) => ExecutionValue::Boolean(*value),
        RuntimeValue::List(values) => {
            ExecutionValue::List(values.iter().map(public_value).collect())
        }
        RuntimeValue::Tuple(values) => {
            ExecutionValue::Tuple(values.iter().map(public_value).collect())
        }
        RuntimeValue::Range { start, end } => ExecutionValue::Range {
            start: *start,
            end: *end,
        },
        RuntimeValue::Duration(value) => ExecutionValue::Duration(*value),
        RuntimeValue::Decimal(value) => ExecutionValue::Decimal(value.normalized_text()),
        RuntimeValue::Bytes(value) => ExecutionValue::Bytes(value.to_vec()),
        RuntimeValue::Record(fields) => ExecutionValue::Record(public_fields(fields)),
        RuntimeValue::Constructed {
            type_name,
            variant_name,
            fields,
            ..
        } => ExecutionValue::Constructed {
            type_name: type_name.clone(),
            variant_name: variant_name.clone(),
            fields: public_fields(fields),
        },
        RuntimeValue::Map(map) => {
            let entries = map.entry_refs();
            let text_keyed = entries
                .iter()
                .map(|(key, value)| match key {
                    RuntimeValue::Text(name) => Some(ValueField {
                        name: name.clone(),
                        value: public_value(value),
                    }),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>();
            match text_keyed {
                Some(fields) => ExecutionValue::Record(fields),
                None => ExecutionValue::Map(
                    entries
                        .into_iter()
                        .map(|(key, value)| (public_value(key), public_value(value)))
                        .collect(),
                ),
            }
        }
        RuntimeValue::Set(set) => ExecutionValue::List(
            set.entry_refs()
                .into_iter()
                .map(|(item, _)| public_value(item))
                .collect(),
        ),
        RuntimeValue::Callable(callable) => ExecutionValue::Callable {
            name: callable_name(callable).to_owned(),
        },
        // Process programs never reach an outcome on the interpreter; an address is
        // opaque, so its public form names only what it is.
        RuntimeValue::ProcessAddress(_) => ExecutionValue::Constructed {
            type_name: "Address".to_owned(),
            variant_name: None,
            fields: Vec::new(),
        },
        // Diagnostic data: its public form names only what it is.
        RuntimeValue::Cause(_) => ExecutionValue::Constructed {
            type_name: "Cause".to_owned(),
            variant_name: None,
            fields: Vec::new(),
        },
        RuntimeValue::TestLog(log) => ExecutionValue::Constructed {
            type_name: match log.kind {
                core_operations::TestLogKind::Recorder => "Recorder",
                core_operations::TestLogKind::Script => "Script",
            }
            .to_owned(),
            variant_name: None,
            fields: Vec::new(),
        },
    }
}

fn public_fields(fields: &[RuntimeField]) -> Vec<ValueField> {
    fields
        .iter()
        .map(|field| ValueField {
            name: field.name.clone(),
            value: public_value(&field.value),
        })
        .collect()
}

fn callable_name(callable: &RuntimeCallable) -> &str {
    match callable {
        RuntimeCallable::Declared { .. } => "function",
        RuntimeCallable::Closure { name, .. } => name,
        RuntimeCallable::Dependency { name, .. } => name,
        RuntimeCallable::Intrinsic(function) => function.name(),
        RuntimeCallable::Host(HostFunction::ProcessUtf8Lines) => "process_utf8_lines",
        RuntimeCallable::Host(HostFunction::WriteLine) => "write_line",
        RuntimeCallable::Host(HostFunction::WriteError) => "write_error",
        RuntimeCallable::Host(HostFunction::RandomNextInt) => "next_int",
        RuntimeCallable::Constructor { name, .. } => name,
    }
}

fn operation_name(operation: &CoreOperation) -> &'static str {
    match operation {
        CoreOperation::Constant(_) => "constant",
        CoreOperation::Reference(_) => "reference",
        CoreOperation::Select { .. } => "select",
        CoreOperation::DependencyMethod { .. } => "dependency_method",
        CoreOperation::Construct { .. } => "construct",
        CoreOperation::Call { .. } => "call",
        CoreOperation::List(_) => "list",
        CoreOperation::Tuple(_) => "tuple",
        CoreOperation::Record(_) => "record",
        CoreOperation::Unary { .. } => "unary",
        CoreOperation::Binary { .. } => "binary",
        CoreOperation::Identity(_) => "identity",
        CoreOperation::Provision { .. } => "provision",
        CoreOperation::Sequence(_) => "sequence",
        CoreOperation::Closure { .. } => "closure",
        CoreOperation::Match { .. } => "match",
        CoreOperation::Loop { .. } => "loop",
        CoreOperation::Break { .. } => "break",
    }
}

fn control_name(control: &Control) -> &'static str {
    match control {
        Control::Success(_) => "success",
        Control::Failure(..) => "expected_failure",
        Control::Defect { .. } => "defect",
        Control::Cancellation(_) => "cancellation",
        Control::Break { .. } => "break",
    }
}

fn text_attribute(name: impl Into<String>, value: impl Into<String>) -> ExecutionAttribute {
    ExecutionAttribute {
        name: name.into(),
        value: ExecutionAttributeValue::Text(value.into()),
    }
}

fn symbol_attribute(name: impl Into<String>, value: SymbolId) -> ExecutionAttribute {
    ExecutionAttribute {
        name: name.into(),
        value: ExecutionAttributeValue::Symbol(value.raw()),
    }
}

fn semantic_attribute(name: impl Into<String>, value: SemanticId) -> ExecutionAttribute {
    ExecutionAttribute {
        name: name.into(),
        value: ExecutionAttributeValue::Semantic(value.raw()),
    }
}

/// One step of writing an outcome value as JSON. The writer keeps its own stack, so
/// a value of any nesting depth costs heap, not host stack: the caller of an
/// execution renders its outcome on whatever thread it has.
enum JsonStep<'value> {
    Value(&'value ExecutionValue),
    Punctuation(&'static str),
    /// A field name and its `:`.
    Name(&'value str),
}

fn write_json_value(output: &mut String, value: &ExecutionValue) {
    let mut steps = vec![JsonStep::Value(value)];
    while let Some(step) = steps.pop() {
        let value = match step {
            JsonStep::Punctuation(text) => {
                output.push_str(text);
                continue;
            }
            JsonStep::Name(name) => {
                write_json_string(output, name);
                output.push(':');
                continue;
            }
            JsonStep::Value(value) => value,
        };
        match value {
            ExecutionValue::Void => output.push_str("{\"type\":\"Void\"}"),
            ExecutionValue::Integer(value) => {
                write!(output, "{value}").expect("writing JSON to a string cannot fail")
            }
            ExecutionValue::Float(value) => output.push_str(&render_float(value.0)),
            ExecutionValue::Text(value) => write_json_string(output, value),
            ExecutionValue::Boolean(value) => {
                output.push_str(if *value { "true" } else { "false" })
            }
            ExecutionValue::List(values) => {
                output.push('[');
                push_json_elements(&mut steps, values, "]");
            }
            ExecutionValue::Tuple(values) => {
                output.push_str("{\"tuple\":[");
                push_json_elements(&mut steps, values, "]}");
            }
            ExecutionValue::Range { start, end } => {
                write!(output, "{{\"range\":{{\"start\":{start},\"end\":{end}}}}}")
                    .expect("writing JSON to a string cannot fail")
            }
            ExecutionValue::Duration(nanoseconds) => {
                write!(output, "{{\"duration\":{nanoseconds}}}")
                    .expect("writing JSON to a string cannot fail")
            }
            ExecutionValue::Decimal(text) => {
                output.push_str("{\"decimal\":");
                write_json_string(output, text);
                output.push('}');
            }
            // Lowercase hex, two digits per byte.
            ExecutionValue::Bytes(bytes) => {
                output.push_str("{\"bytes\":\"");
                output.push_str(&bytes_operations::hex(bytes));
                output.push_str("\"}");
            }
            ExecutionValue::Record(fields) => {
                output.push('{');
                push_json_fields(&mut steps, fields, "}");
            }
            ExecutionValue::Map(entries) => {
                output.push_str("{\"map\":[");
                steps.push(JsonStep::Punctuation("]}"));
                for (index, (key, value)) in entries.iter().enumerate().rev() {
                    steps.push(JsonStep::Punctuation("]"));
                    steps.push(JsonStep::Value(value));
                    steps.push(JsonStep::Punctuation(","));
                    steps.push(JsonStep::Value(key));
                    steps.push(JsonStep::Punctuation(if index > 0 { ",[" } else { "[" }));
                }
            }
            ExecutionValue::Constructed {
                type_name,
                variant_name,
                fields,
            } => {
                output.push_str("{\"type\":");
                write_json_string(output, type_name);
                if let Some(variant_name) = variant_name {
                    output.push_str(",\"variant\":");
                    write_json_string(output, variant_name);
                }
                output.push_str(",\"fields\":{");
                push_json_fields(&mut steps, fields, "}}");
            }
            ExecutionValue::Callable { name } => {
                output.push_str("{\"callable\":");
                write_json_string(output, name);
                output.push('}');
            }
        }
    }
}

/// Queues `values` comma-separated, then `close`. Steps pop in reverse.
fn push_json_elements<'value>(
    steps: &mut Vec<JsonStep<'value>>,
    values: &'value [ExecutionValue],
    close: &'static str,
) {
    steps.push(JsonStep::Punctuation(close));
    for (index, value) in values.iter().enumerate().rev() {
        steps.push(JsonStep::Value(value));
        if index > 0 {
            steps.push(JsonStep::Punctuation(","));
        }
    }
}

/// Queues `fields` as comma-separated `"name":value` pairs, then `close`.
fn push_json_fields<'value>(
    steps: &mut Vec<JsonStep<'value>>,
    fields: &'value [ValueField],
    close: &'static str,
) {
    steps.push(JsonStep::Punctuation(close));
    for (index, field) in fields.iter().enumerate().rev() {
        steps.push(JsonStep::Value(&field.value));
        steps.push(JsonStep::Name(&field.name));
        if index > 0 {
            steps.push(JsonStep::Punctuation(","));
        }
    }
}

fn write_json_attribute(output: &mut String, value: &ExecutionAttributeValue) {
    match value {
        ExecutionAttributeValue::Text(value) => write_json_string(output, value),
        ExecutionAttributeValue::Integer(value) => {
            write!(output, "{value}").expect("writing JSON to a string cannot fail")
        }
        ExecutionAttributeValue::Symbol(value) | ExecutionAttributeValue::Semantic(value) => {
            write!(output, "{value}").expect("writing JSON to a string cannot fail");
        }
    }
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

pub(crate) fn execute_legacy(core: LegacyCoreProgram, config: ExecutionConfig) -> ExecutionReport {
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
        operation: "legacy_construction".to_owned(),
        attributes: Vec::new(),
    };

    ExecutionReport {
        failure_rendering: None,
        events: vec![
            event(0, EventKind::ComputationStarted),
            event(1, EventKind::ComputationCompleted),
        ],
        cause: None,
        outcome: ExecutionOutcome::Success(ExecutionValue::Constructed {
            type_name: core.constructor_name,
            variant_name: None,
            fields: vec![ValueField {
                name: core.field_name,
                value: ExecutionValue::Integer(core.value),
            }],
        }),
        application_stdout: String::new(),
        application_stderr: String::new(),
        random_read: false,
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
    use super::{
        Environment, ExecutionConfig, ExecutionValue, RuntimeCallable, RuntimeField, RuntimeMap,
        RuntimeValue, TraceMode, ValueField, write_json_value,
    };
    use crate::ids::{CoreNodeId, SymbolId};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn execution_ids_and_trace_configuration_are_validated_at_the_boundary() {
        assert!(ExecutionConfig::new("test-run_1.2:3").is_ok());
        assert!(ExecutionConfig::new("").is_err());
        assert!(ExecutionConfig::new("not json safe").is_err());
        assert_eq!(
            ExecutionConfig::new("test")
                .unwrap()
                .with_trace_mode(TraceMode::Boundaries),
            ExecutionConfig {
                execution_id: "test".to_owned(),
                trace_mode: TraceMode::Boundaries,
                cancellation_requested: false,
                application_arguments: Vec::new(),
                standard_input: Vec::new(),
                max_line_bytes: 1024 * 1024,
                random_seed: None,
            }
        );
        assert_eq!(
            ExecutionConfig::new("test")
                .unwrap()
                .with_trace_mode(TraceMode::Off)
                .trace_mode,
            TraceMode::Off,
        );
        assert_ne!(
            ExecutionConfig::for_process(),
            ExecutionConfig::for_process()
        );
    }

    fn order(left: &RuntimeValue, right: &RuntimeValue) -> std::cmp::Ordering {
        super::structural::structural_order(left, right, &|_| 0)
    }

    fn text(value: &str) -> RuntimeValue {
        RuntimeValue::Text(value.to_owned())
    }

    #[test]
    fn persistent_maps_retain_prior_values_and_share_untouched_branches() {
        let mut map = RuntimeMap::default();
        for (index, key) in ["d", "b", "f", "a", "c", "e", "g"].into_iter().enumerate() {
            map = map.put(text(key), RuntimeValue::Integer(index as i64), &order);
        }
        let prior = map.clone();
        let updated = map.put(text("g"), RuntimeValue::Integer(99), &order);

        assert!(matches!(
            prior.get(&text("g"), &order),
            Some(RuntimeValue::Integer(6))
        ));
        assert!(matches!(
            updated.get(&text("g"), &order),
            Some(RuntimeValue::Integer(99))
        ));
        let prior_left = prior.root.as_ref().unwrap().left.as_ref().unwrap();
        let updated_left = updated.root.as_ref().unwrap().left.as_ref().unwrap();
        assert!(Arc::ptr_eq(prior_left, updated_left));
    }

    #[test]
    fn ordered_insertions_keep_map_height_logarithmic() {
        let mut map = RuntimeMap::default();
        for value in 0..10_000 {
            map = map.put(
                RuntimeValue::Integer(value),
                RuntimeValue::Integer(value),
                &order,
            );
        }

        assert!(map.root.as_ref().unwrap().height <= 20);
        assert_eq!(map.entries().len(), 10_000);
    }

    /// The AVL bound: a tree of `n` nodes is at most `1.45 log2(n + 2)` high.
    /// Keeping only the keys on the leftmost (or rightmost) path and removing
    /// every other key leaves a path as long as the old height unless removal
    /// rebalances. Each order of removal reaches the rebalancing on one side of
    /// the descent. `tests/fixtures/native-map-runtime.c` is the same check of
    /// `runtime.c`.
    #[test]
    fn removals_that_keep_one_path_keep_map_height_logarithmic() {
        const COUNT: i64 = 4_096;
        for (leftmost, ascending) in [(true, true), (false, true), (true, false), (false, false)] {
            let mut map = RuntimeMap::default();
            for value in 0..COUNT {
                map = map.put(RuntimeValue::Integer(value), RuntimeValue::Void, &order);
            }
            let mut path = Vec::new();
            let mut node = map.root.as_deref();
            while let Some(current) = node {
                let RuntimeValue::Integer(key) = current.key else {
                    unreachable!("every key is an Int")
                };
                path.push(key);
                node = if leftmost {
                    current.left.as_deref()
                } else {
                    current.right.as_deref()
                };
            }
            let removals = (0..COUNT).map(|step| if ascending { step } else { COUNT - 1 - step });
            for value in removals.filter(|value| !path.contains(value)) {
                map = map
                    .remove(&RuntimeValue::Integer(value), &order)
                    .expect("the key is held");
            }

            let height = f64::from(map.root.as_ref().unwrap().height);
            let bound = 1.45 * ((path.len() + 2) as f64).log2();
            assert!(
                height <= bound,
                "leftmost {leftmost}, ascending {ascending}: height {height} for {} keys",
                path.len()
            );
            path.sort_unstable();
            let keys = map
                .entries()
                .into_iter()
                .map(|(key, _)| match key {
                    RuntimeValue::Integer(key) => key,
                    _ => unreachable!("every key is an Int"),
                })
                .collect::<Vec<_>>();
            assert_eq!(keys, path);
        }
    }

    /// Nesting depth for the teardown tests: far more levels than `SMALL_STACK` holds
    /// frames, so freeing or writing that recurses once per level overflows it.
    const DEPTH: usize = 100_000;
    const SMALL_STACK: usize = 256 * 1024;

    fn on_small_stack(work: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(work)
            .expect("spawn a test thread")
            .join()
            .expect("the test thread finishes");
    }

    fn field(value: RuntimeValue) -> Arc<Vec<RuntimeField>> {
        Arc::new(vec![RuntimeField {
            name: "inner".to_owned(),
            value,
        }])
    }

    /// One level of each kind of value that holds other values.
    const WRAPPERS: [fn(RuntimeValue) -> RuntimeValue; 7] = [
        |inner| RuntimeValue::List(std::iter::once(inner).collect()),
        |inner| RuntimeValue::Tuple(Arc::new(vec![inner, RuntimeValue::Void])),
        |inner| RuntimeValue::Record(field(inner)),
        |inner| RuntimeValue::Constructed {
            constructor: SymbolId::new(0),
            type_name: "Nest".to_owned(),
            variant_name: Some("Wrap".to_owned()),
            fields: field(inner),
            cause: None,
        },
        |inner| {
            RuntimeValue::Map(RuntimeMap::default().put(RuntimeValue::Integer(0), inner, &order))
        },
        |inner| {
            RuntimeValue::Callable(RuntimeCallable::Closure {
                parameters: Vec::new(),
                body: CoreNodeId::new(0),
                environment: Arc::new(Environment {
                    locals: HashMap::from([(SymbolId::new(0), inner)]),
                    requirements: HashMap::new(),
                }),
                name: "closure".to_owned(),
            })
        },
        |inner| {
            RuntimeValue::Callable(RuntimeCallable::Declared {
                symbol: SymbolId::new(0),
                requirements: HashMap::from([("Provider".to_owned(), inner)]),
            })
        },
    ];

    // ts-0e026f, E18: a value nested far deeper than the stack has frames frees
    // without recursing, through every kind of part that holds other values.
    #[test]
    fn freeing_a_deeply_nested_value_does_not_recurse_on_the_host_stack() {
        for wrap in WRAPPERS {
            let mut value = RuntimeValue::Integer(0);
            for _ in 0..DEPTH {
                value = wrap(value);
            }
            on_small_stack(move || drop(value));
        }
    }

    // A part another value still holds survives the release of its first owner.
    #[test]
    fn freeing_one_owner_keeps_parts_that_another_value_shares() {
        let shared = RuntimeValue::List((0..3).map(RuntimeValue::Integer).collect());
        let outer = RuntimeValue::Tuple(Arc::new(vec![shared.clone(), shared.clone()]));
        drop(outer);
        let RuntimeValue::List(values) = &shared else {
            unreachable!("built as a list");
        };
        assert_eq!(values.len(), 3);
        assert!(matches!(values.get(2), Some(RuntimeValue::Integer(2))));
    }

    // A program's outcome is freed and written as JSON by the caller, so neither may
    // recurse per level either.
    #[test]
    fn deep_outcome_values_write_and_free_without_recursing() {
        let wrappers: [fn(ExecutionValue) -> ExecutionValue; 4] = [
            |inner| ExecutionValue::List(vec![inner]),
            |inner| ExecutionValue::Tuple(vec![inner, ExecutionValue::Void]),
            |inner| {
                ExecutionValue::Record(vec![ValueField {
                    name: "inner".to_owned(),
                    value: inner,
                }])
            },
            |inner| ExecutionValue::Constructed {
                type_name: "Nest".to_owned(),
                variant_name: None,
                fields: vec![ValueField {
                    name: "inner".to_owned(),
                    value: inner,
                }],
            },
        ];
        let levels = [
            ("[", "]"),
            ("{\"tuple\":[", ",{\"type\":\"Void\"}]}"),
            ("{\"inner\":", "}"),
            ("{\"type\":\"Nest\",\"fields\":{\"inner\":", "}}"),
        ];
        for (wrap, (open, close)) in wrappers.into_iter().zip(levels) {
            let mut value = ExecutionValue::Integer(7);
            for _ in 0..DEPTH {
                value = wrap(value);
            }
            on_small_stack(move || {
                let mut json = String::new();
                write_json_value(&mut json, &value);
                assert_eq!(
                    json,
                    format!("{}7{}", open.repeat(DEPTH), close.repeat(DEPTH))
                );
                drop(value);
            });
        }
    }
}
