use crate::executable::{
    CoreArm, CoreLiteral, CoreOperation, CorePattern, CoreProgram, CoreStep, CoreSymbolKind,
    LegacyCoreProgram,
};
use crate::ids::{CoreNodeId, FileId, OriginId, SemanticId, SymbolId, TextRange};
use crate::json::{JsonData, decode_json, write_json_data, write_json_string};
use crate::semantic::{BinaryOperator, UnaryOperator};
use crate::standard::{StandardConstructor, StandardFunction, StandardSymbol};
use std::cmp::Ordering as Comparison;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionValue {
    Integer(i64),
    Text(String),
    Boolean(bool),
    List(Vec<ExecutionValue>),
    Record(Vec<ValueField>),
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
}

impl ExecutionReport {
    pub fn render_json_lines(&self) -> String {
        let mut output = String::new();
        for event in &self.events {
            write!(
                output,
                "{{\"event\":\"{}\",\"sequence\":{},\"computation\":",
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

        match &self.outcome {
            ExecutionOutcome::Success(value) => {
                output.push_str("{\"outcome\":\"success\",\"value\":");
                write_json_value(&mut output, value);
            }
            ExecutionOutcome::ExpectedFailure(error) => {
                output.push_str("{\"outcome\":\"expected_failure\",\"error\":");
                write_json_value(&mut output, error);
            }
            ExecutionOutcome::Defect(defect) => {
                output.push_str("{\"outcome\":\"defect\",\"message\":");
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
                output.push_str("{\"outcome\":\"cancellation\",\"reason\":");
                write_json_string(&mut output, &cancellation.reason);
            }
        }
        output.push_str("}\n");
        output
    }
}

#[derive(Clone, Debug)]
enum RuntimeValue {
    Integer(i64),
    Text(String),
    Boolean(bool),
    List(Vec<RuntimeValue>),
    Record(Vec<RuntimeField>),
    Constructed {
        constructor: SymbolId,
        type_name: String,
        variant_name: Option<String>,
        fields: Vec<RuntimeField>,
    },
    Map(RuntimeMap),
    JsonNumber(String),
    Callable(RuntimeCallable),
}

#[derive(Clone, Debug, Default)]
struct RuntimeMap {
    root: Option<Arc<RuntimeMapNode>>,
}

#[derive(Clone, Debug)]
struct RuntimeMapNode {
    key: String,
    value: RuntimeValue,
    height: u16,
    left: Option<Arc<Self>>,
    right: Option<Arc<Self>>,
}

impl RuntimeMap {
    fn get(&self, key: &str) -> Option<RuntimeValue> {
        let mut node = self.root.as_deref();
        while let Some(current) = node {
            match key.as_bytes().cmp(current.key.as_bytes()) {
                Comparison::Less => node = current.left.as_deref(),
                Comparison::Equal => return Some(current.value.clone()),
                Comparison::Greater => node = current.right.as_deref(),
            }
        }
        None
    }

    fn put(&self, key: String, value: RuntimeValue) -> Self {
        Self {
            root: Some(map_insert(self.root.clone(), key, value)),
        }
    }

    fn entries(&self) -> Vec<(String, RuntimeValue)> {
        fn visit(node: &Option<Arc<RuntimeMapNode>>, entries: &mut Vec<(String, RuntimeValue)>) {
            if let Some(node) = node {
                visit(&node.left, entries);
                entries.push((node.key.clone(), node.value.clone()));
                visit(&node.right, entries);
            }
        }
        let mut entries = Vec::new();
        visit(&self.root, &mut entries);
        entries
    }
}

fn map_insert(
    node: Option<Arc<RuntimeMapNode>>,
    key: String,
    value: RuntimeValue,
) -> Arc<RuntimeMapNode> {
    let Some(node) = node else {
        return map_node(key, value, None, None);
    };
    let updated = match key.as_bytes().cmp(node.key.as_bytes()) {
        Comparison::Less => map_node(
            node.key.clone(),
            node.value.clone(),
            Some(map_insert(node.left.clone(), key, value)),
            node.right.clone(),
        ),
        Comparison::Equal => map_node(key, value, node.left.clone(), node.right.clone()),
        Comparison::Greater => map_node(
            node.key.clone(),
            node.value.clone(),
            node.left.clone(),
            Some(map_insert(node.right.clone(), key, value)),
        ),
    };
    map_balance(updated)
}

fn map_height(node: &Option<Arc<RuntimeMapNode>>) -> u16 {
    node.as_ref().map_or(0, |node| node.height)
}

fn map_node(
    key: String,
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
        environment: Environment,
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
}

#[derive(Clone, Debug, Default)]
struct Environment {
    locals: HashMap<SymbolId, RuntimeValue>,
    requirements: HashMap<String, RuntimeValue>,
}

#[derive(Clone, Debug)]
enum Control {
    Success(RuntimeValue),
    Failure(RuntimeValue),
    Defect {
        message: String,
        node: Option<CoreNodeId>,
    },
    Cancellation,
}

pub(crate) fn execute(core: &CoreProgram, config: ExecutionConfig) -> ExecutionReport {
    Evaluator::new(core, config).execute()
}

struct Evaluator<'core> {
    core: &'core CoreProgram,
    config: ExecutionConfig,
    events: Vec<ExecutionEvent>,
    next_sequence: u64,
    next_span: u64,
    active_bindings: HashSet<SymbolId>,
}

impl<'core> Evaluator<'core> {
    fn new(core: &'core CoreProgram, config: ExecutionConfig) -> Self {
        Self {
            core,
            config,
            events: Vec::new(),
            next_sequence: 0,
            next_span: 2,
            active_bindings: HashSet::new(),
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
            Control::Cancellation
        } else {
            self.eval_node(entry.body, 1, Environment::default())
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
        let outcome = self.public_outcome(control);
        ExecutionReport {
            events: self.events,
            outcome,
        }
    }

    fn eval_node(
        &mut self,
        node_id: CoreNodeId,
        parent_span: u64,
        environment: Environment,
    ) -> Control {
        if self.config.cancellation_requested {
            return Control::Cancellation;
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
        let control = self.eval_operation(node_id, span, environment, node.operation);
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
                select_field(value, &member).unwrap_or_else(|| Control::Defect {
                    message: format!("verified value has no field {member}"),
                    node: Some(node_id),
                })
            }
            CoreOperation::DependencyMethod {
                requirement,
                interface,
                method,
                name,
            } => {
                let requirement_name = self.core.symbol(requirement).name.clone();
                let Some(provider) = environment.requirements.get(&requirement_name).cloned()
                else {
                    return Control::Defect {
                        message: format!("closed core has no provider for {requirement_name}"),
                        node: Some(node_id),
                    };
                };
                let Some(Control::Success(RuntimeValue::Callable(callable))) =
                    select_field(provider, &name)
                else {
                    return Control::Defect {
                        message: format!(
                            "provider {requirement_name} has no callable field {name}"
                        ),
                        node: Some(node_id),
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
            } => {
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    let value = success_or_return!(self.eval_node(
                        argument.value,
                        span,
                        environment.clone()
                    ));
                    values.push((argument.label, value));
                }
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
                        .expect("outcome constructor has one argument")
                        .1;
                    return if success {
                        Control::Success(value)
                    } else {
                        Control::Failure(value)
                    };
                }
                let mut fields = Vec::with_capacity(shape.fields.len());
                for (index, field_name) in shape.fields.iter().enumerate() {
                    let value = values
                        .iter()
                        .find(|(label, _)| label.as_deref() == Some(field_name))
                        .or_else(|| values.get(index))
                        .expect("verified constructor supplies every field")
                        .1
                        .clone();
                    fields.push(RuntimeField {
                        name: field_name.clone(),
                        value,
                    });
                }
                Control::Success(RuntimeValue::Constructed {
                    constructor,
                    type_name: shape.type_name,
                    variant_name: shape.variant_name,
                    fields,
                })
            }
            CoreOperation::Call { callee, arguments } => {
                let callable =
                    success_or_return!(self.eval_node(callee, span, environment.clone()));
                let RuntimeValue::Callable(callable) = callable else {
                    return Control::Defect {
                        message: "verified call target is not callable".to_owned(),
                        node: Some(node_id),
                    };
                };
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(success_or_return!(self.eval_node(
                        argument.value,
                        span,
                        environment.clone(),
                    )));
                }
                self.invoke(callable, values, span, node_id, environment)
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
                Control::Success(RuntimeValue::List(values))
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
                Control::Success(RuntimeValue::Record(values))
            }
            CoreOperation::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => match success_or_return!(self.eval_node(operand, span, environment)) {
                RuntimeValue::Integer(value) => value
                    .checked_neg()
                    .map(RuntimeValue::Integer)
                    .map(Control::Success)
                    .unwrap_or_else(|| Control::Defect {
                        message: "integer negation overflow".to_owned(),
                        node: Some(node_id),
                    }),
                _ => Control::Defect {
                    message: "verified unary operand is not an integer".to_owned(),
                    node: Some(node_id),
                },
            },
            CoreOperation::Binary {
                operator: BinaryOperator::Pipe,
                left,
                right,
            } => {
                let value = success_or_return!(self.eval_node(left, span, environment.clone()));
                let callable = success_or_return!(self.eval_node(right, span, environment.clone()));
                let RuntimeValue::Callable(callable) = callable else {
                    return Control::Defect {
                        message: "verified pipeline target is not callable".to_owned(),
                        node: Some(node_id),
                    };
                };
                self.invoke(callable, vec![value], span, node_id, environment)
            }
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
            CoreOperation::Closure { parameters, body } => {
                Control::Success(RuntimeValue::Callable(RuntimeCallable::Closure {
                    parameters,
                    body,
                    environment,
                    name: "closure".to_owned(),
                }))
            }
            CoreOperation::Match { subjects, arms } => {
                self.eval_match(node_id, span, environment, subjects, arms)
            }
        }
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
            CoreSymbolKind::Binding => {
                if !self.active_bindings.insert(symbol) {
                    return Control::Defect {
                        message: format!("module binding {} is cyclic", core_symbol.name),
                        node: None,
                    };
                }
                let binding = self
                    .core
                    .bindings
                    .iter()
                    .find(|binding| binding.symbol == symbol)
                    .expect("reachable binding exists")
                    .clone();
                let result = self.eval_node(binding.value, span, environment);
                self.active_bindings.remove(&symbol);
                result
            }
            CoreSymbolKind::Requirement => environment
                .requirements
                .get(&core_symbol.name)
                .cloned()
                .map(Control::Success)
                .unwrap_or_else(|| Control::Defect {
                    message: format!("closed core has no provider for {}", core_symbol.name),
                    node: None,
                }),
            CoreSymbolKind::Local => environment
                .locals
                .get(&symbol)
                .cloned()
                .map(Control::Success)
                .unwrap_or_else(|| Control::Defect {
                    message: format!("verified local {} is not bound", core_symbol.name),
                    node: None,
                }),
            CoreSymbolKind::Intrinsic => match core_symbol.standard {
                Some(StandardSymbol::Function(function)) => {
                    Control::Success(RuntimeValue::Callable(RuntimeCallable::Intrinsic(function)))
                }
                _ => Control::Defect {
                    message: format!("intrinsic symbol {} has no operation", core_symbol.name),
                    node: None,
                },
            },
            CoreSymbolKind::Constructor | CoreSymbolKind::Other => Control::Defect {
                message: format!("symbol {} is not a runtime value", core_symbol.name),
                node: None,
            },
        }
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
                self.eval_node(function.body, span, environment)
            }
            RuntimeCallable::Closure {
                parameters,
                body,
                mut environment,
                name: _,
            } => {
                environment
                    .requirements
                    .extend(call_environment.requirements);
                for (parameter, argument) in parameters.iter().zip(arguments) {
                    environment.locals.insert(*parameter, argument);
                }
                self.eval_node(body, span, environment)
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
        }
    }

    fn invoke_intrinsic(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        match function {
            StandardFunction::MapEmpty => {
                Control::Success(RuntimeValue::Map(RuntimeMap::default()))
            }
            StandardFunction::MapGet => {
                let [RuntimeValue::Map(map), RuntimeValue::Text(key)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                let value = map.get(key);
                Control::Success(match value {
                    Some(value) => {
                        self.standard_value(StandardConstructor::OptionSome, vec![value])
                    }
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                })
            }
            StandardFunction::MapPut => {
                let [RuntimeValue::Map(map), RuntimeValue::Text(key), value] = arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Map(map.put(key.clone(), value.clone())))
            }
            StandardFunction::MapUpdate => {
                let [
                    RuntimeValue::Map(map),
                    RuntimeValue::Text(key),
                    RuntimeValue::Callable(callback),
                ] = arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                let current = match map.get(key) {
                    Some(value) => {
                        self.standard_value(StandardConstructor::OptionSome, vec![value])
                    }
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                };
                let value = success_or_return!(self.invoke(
                    callback.clone(),
                    vec![current],
                    span,
                    call_node,
                    environment,
                ));
                Control::Success(RuntimeValue::Map(map.put(key.clone(), value)))
            }
            StandardFunction::MapMapValues => {
                let [RuntimeValue::Map(map), RuntimeValue::Callable(callback)] =
                    arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                let mut output = RuntimeMap::default();
                for (key, value) in map.entries() {
                    let value = success_or_return!(self.invoke(
                        callback.clone(),
                        vec![value],
                        span,
                        call_node,
                        environment.clone(),
                    ));
                    output = output.put(key, value);
                }
                Control::Success(RuntimeValue::Map(output))
            }
            StandardFunction::JsonDecode => {
                let [RuntimeValue::Text(source)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                match decode_json(source) {
                    Ok(value) => Control::Success(self.json_to_runtime(value)),
                    Err(error) => Control::Failure(self.standard_value(
                        StandardConstructor::DecodeInvalidJson,
                        vec![
                            RuntimeValue::Text(error.message),
                            RuntimeValue::Integer(i64::try_from(error.offset).unwrap_or(i64::MAX)),
                        ],
                    )),
                }
            }
            StandardFunction::JsonEncode => {
                let [value] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                let Ok(value) = self.runtime_to_json(value) else {
                    return Control::Defect {
                        message: "verified JSON value has an invalid runtime shape".to_owned(),
                        node: Some(call_node),
                    };
                };
                let mut output = String::new();
                write_json_data(&mut output, &value);
                Control::Success(RuntimeValue::Text(output))
            }
            StandardFunction::JsonBoolean => {
                let [RuntimeValue::Boolean(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(self.standard_value(
                    StandardConstructor::JsonBoolean,
                    vec![RuntimeValue::Boolean(*value)],
                ))
            }
            StandardFunction::JsonInt => {
                let [RuntimeValue::Integer(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(self.standard_value(
                    StandardConstructor::JsonNumber,
                    vec![RuntimeValue::JsonNumber(value.to_string())],
                ))
            }
            StandardFunction::JsonText => {
                let [RuntimeValue::Text(value)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(self.standard_value(
                    StandardConstructor::JsonText,
                    vec![RuntimeValue::Text(value.clone())],
                ))
            }
            StandardFunction::JsonArray => {
                let [RuntimeValue::List(values)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(self.standard_value(
                    StandardConstructor::JsonArray,
                    vec![RuntimeValue::List(values.clone())],
                ))
            }
            StandardFunction::JsonObject => {
                let [RuntimeValue::Map(fields)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(self.standard_value(
                    StandardConstructor::JsonObject,
                    vec![RuntimeValue::Map(fields.clone())],
                ))
            }
            StandardFunction::JsonNumberToInt => {
                let [RuntimeValue::JsonNumber(number)] = arguments.as_slice() else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                if number
                    .bytes()
                    .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
                {
                    return Control::Failure(self.standard_value(
                        StandardConstructor::NumberNotInteger,
                        vec![RuntimeValue::JsonNumber(number.clone())],
                    ));
                }
                match number.parse::<i64>() {
                    Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                    Err(_) => Control::Failure(self.standard_value(
                        StandardConstructor::NumberOutOfRange,
                        vec![RuntimeValue::JsonNumber(number.clone())],
                    )),
                }
            }
            StandardFunction::TextAppend => {
                let [RuntimeValue::Text(left), RuntimeValue::Text(right)] = arguments.as_slice()
                else {
                    return self.intrinsic_shape_defect(function, call_node);
                };
                Control::Success(RuntimeValue::Text(format!("{left}{right}")))
            }
        }
    }

    fn intrinsic_shape_defect(&self, function: StandardFunction, call_node: CoreNodeId) -> Control {
        Control::Defect {
            message: format!(
                "verified arguments for standard function {} have invalid runtime shapes",
                function.name()
            ),
            node: Some(call_node),
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
            fields: shape
                .fields
                .iter()
                .cloned()
                .zip(values)
                .map(|(name, value)| RuntimeField { name, value })
                .collect(),
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

    fn json_to_runtime(&self, value: JsonData) -> RuntimeValue {
        match value {
            JsonData::Null => self.standard_value(StandardConstructor::JsonNull, Vec::new()),
            JsonData::Boolean(value) => self.standard_value(
                StandardConstructor::JsonBoolean,
                vec![RuntimeValue::Boolean(value)],
            ),
            JsonData::Number(value) => self.standard_value(
                StandardConstructor::JsonNumber,
                vec![RuntimeValue::JsonNumber(value)],
            ),
            JsonData::Text(value) => self.standard_value(
                StandardConstructor::JsonText,
                vec![RuntimeValue::Text(value)],
            ),
            JsonData::Array(values) => self.standard_value(
                StandardConstructor::JsonArray,
                vec![RuntimeValue::List(
                    values
                        .into_iter()
                        .map(|value| self.json_to_runtime(value))
                        .collect(),
                )],
            ),
            JsonData::Object(fields) => {
                let mut map = RuntimeMap::default();
                for (name, value) in fields {
                    map = map.put(name, self.json_to_runtime(value));
                }
                self.standard_value(
                    StandardConstructor::JsonObject,
                    vec![RuntimeValue::Map(map)],
                )
            }
        }
    }

    fn runtime_to_json(&self, value: &RuntimeValue) -> Result<JsonData, ()> {
        let RuntimeValue::Constructed {
            constructor,
            fields,
            ..
        } = value
        else {
            return Err(());
        };
        let Some(StandardSymbol::Constructor(constructor)) =
            self.core.symbol(*constructor).standard
        else {
            return Err(());
        };
        let field = |index: usize| fields.get(index).map(|field| &field.value).ok_or(());
        match constructor {
            StandardConstructor::JsonNull => Ok(JsonData::Null),
            StandardConstructor::JsonBoolean => match field(0)? {
                RuntimeValue::Boolean(value) => Ok(JsonData::Boolean(*value)),
                _ => Err(()),
            },
            StandardConstructor::JsonNumber => match field(0)? {
                RuntimeValue::JsonNumber(value) => Ok(JsonData::Number(value.clone())),
                _ => Err(()),
            },
            StandardConstructor::JsonText => match field(0)? {
                RuntimeValue::Text(value) => Ok(JsonData::Text(value.clone())),
                _ => Err(()),
            },
            StandardConstructor::JsonArray => match field(0)? {
                RuntimeValue::List(values) => values
                    .iter()
                    .map(|value| self.runtime_to_json(value))
                    .collect::<Result<Vec<_>, _>>()
                    .map(JsonData::Array),
                _ => Err(()),
            },
            StandardConstructor::JsonObject => match field(0)? {
                RuntimeValue::Map(fields) => fields
                    .entries()
                    .into_iter()
                    .map(|(name, value)| self.runtime_to_json(&value).map(|value| (name, value)))
                    .collect::<Result<Vec<_>, _>>()
                    .map(JsonData::Object),
                _ => Err(()),
            },
            _ => Err(()),
        }
    }

    fn eval_binary(
        &self,
        node: CoreNodeId,
        operator: BinaryOperator,
        left: RuntimeValue,
        right: RuntimeValue,
    ) -> Control {
        let (RuntimeValue::Integer(left), RuntimeValue::Integer(right)) = (left, right) else {
            return Control::Defect {
                message: "verified binary operands are not integers".to_owned(),
                node: Some(node),
            };
        };
        match operator {
            BinaryOperator::Add => checked_integer(node, left.checked_add(right), "addition"),
            BinaryOperator::Subtract => {
                checked_integer(node, left.checked_sub(right), "subtraction")
            }
            BinaryOperator::Multiply => {
                checked_integer(node, left.checked_mul(right), "multiplication")
            }
            BinaryOperator::Divide if right == 0 => Control::Defect {
                message: "integer division by zero".to_owned(),
                node: Some(node),
            },
            BinaryOperator::Divide => checked_integer(node, left.checked_div(right), "division"),
            BinaryOperator::Less => Control::Success(RuntimeValue::Boolean(left < right)),
            BinaryOperator::LessEqual => Control::Success(RuntimeValue::Boolean(left <= right)),
            BinaryOperator::Greater => Control::Success(RuntimeValue::Boolean(left > right)),
            BinaryOperator::GreaterEqual => Control::Success(RuntimeValue::Boolean(left >= right)),
            BinaryOperator::Pipe => unreachable!("pipelines are evaluated separately"),
        }
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
                    Control::Failure(value) => values.push(MatchValue::Failure(value)),
                    other => return other,
                }
            } else {
                values.push(MatchValue::Plain(success_or_return!(control)));
            }
        }
        for arm in arms {
            let mut arm_environment = environment.clone();
            if !arm
                .patterns
                .iter()
                .zip(&values)
                .all(|(pattern, value)| match_pattern(pattern, value, &mut arm_environment.locals))
            {
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
                        };
                    }
                    other => return other,
                }
            }
            return self.eval_node(arm.body, span, arm_environment);
        }
        values
            .into_iter()
            .find_map(|value| match value {
                MatchValue::Failure(error) => Some(Control::Failure(error)),
                MatchValue::Plain(_) | MatchValue::Success(_) => None,
            })
            .unwrap_or_else(|| Control::Defect {
                message: "verified exhaustive match had no matching arm".to_owned(),
                node: Some(node_id),
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
            Control::Success(value) => ExecutionOutcome::Success(public_value(value)),
            Control::Failure(value) => ExecutionOutcome::ExpectedFailure(public_value(value)),
            Control::Defect { message, node } => {
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
            Control::Cancellation => ExecutionOutcome::Cancellation(ExecutionCancellation {
                reason: "requested before the next semantic operation".to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug)]
enum MatchValue {
    Plain(RuntimeValue),
    Success(RuntimeValue),
    Failure(RuntimeValue),
}

fn match_pattern(
    pattern: &CorePattern,
    value: &MatchValue,
    bindings: &mut HashMap<SymbolId, RuntimeValue>,
) -> bool {
    match pattern {
        CorePattern::Outcome {
            success: expected,
            value: pattern,
        } => match (expected, value) {
            (true, MatchValue::Success(value)) | (false, MatchValue::Failure(value)) => {
                match_plain_pattern(pattern, value, bindings)
            }
            _ => false,
        },
        _ => match value {
            MatchValue::Plain(value) => match_plain_pattern(pattern, value, bindings),
            MatchValue::Success(_) | MatchValue::Failure(_) => false,
        },
    }
}

fn match_plain_pattern(
    pattern: &CorePattern,
    value: &RuntimeValue,
    bindings: &mut HashMap<SymbolId, RuntimeValue>,
) -> bool {
    match pattern {
        CorePattern::Wildcard => true,
        CorePattern::Binding(symbol) => {
            bindings.insert(*symbol, value.clone());
            true
        }
        CorePattern::Literal(literal) => literal_matches(literal, value),
        CorePattern::Constructor { symbol, fields } => {
            let RuntimeValue::Constructed {
                constructor,
                fields: values,
                ..
            } = value
            else {
                return false;
            };
            constructor == symbol
                && fields.len() == values.len()
                && fields
                    .iter()
                    .zip(values)
                    .all(|(pattern, field)| match_plain_pattern(pattern, &field.value, bindings))
        }
        CorePattern::Outcome { .. } => false,
    }
}

fn literal_matches(literal: &CoreLiteral, value: &RuntimeValue) -> bool {
    match (literal, value) {
        (CoreLiteral::Integer(left), RuntimeValue::Integer(right)) => left == right,
        (CoreLiteral::Text(left), RuntimeValue::Text(right)) => left == right,
        (CoreLiteral::Boolean(left), RuntimeValue::Boolean(right)) => left == right,
        _ => false,
    }
}

fn select_field(value: RuntimeValue, member: &str) -> Option<Control> {
    let fields = match value {
        RuntimeValue::Record(fields) | RuntimeValue::Constructed { fields, .. } => fields,
        _ => return None,
    };
    fields
        .into_iter()
        .find(|field| field.name == member)
        .map(|field| Control::Success(field.value))
}

fn runtime_literal(literal: CoreLiteral) -> RuntimeValue {
    match literal {
        CoreLiteral::Integer(value) => RuntimeValue::Integer(value),
        CoreLiteral::Text(value) => RuntimeValue::Text(value),
        CoreLiteral::Boolean(value) => RuntimeValue::Boolean(value),
    }
}

fn public_value(value: RuntimeValue) -> ExecutionValue {
    match value {
        RuntimeValue::Integer(value) => ExecutionValue::Integer(value),
        RuntimeValue::Text(value) => ExecutionValue::Text(value),
        RuntimeValue::Boolean(value) => ExecutionValue::Boolean(value),
        RuntimeValue::List(values) => {
            ExecutionValue::List(values.into_iter().map(public_value).collect())
        }
        RuntimeValue::Record(fields) => ExecutionValue::Record(public_fields(fields)),
        RuntimeValue::Constructed {
            type_name,
            variant_name,
            fields,
            ..
        } => ExecutionValue::Constructed {
            type_name,
            variant_name,
            fields: public_fields(fields),
        },
        RuntimeValue::Map(map) => ExecutionValue::Record(
            map.entries()
                .into_iter()
                .map(|(name, value)| ValueField {
                    name,
                    value: public_value(value),
                })
                .collect(),
        ),
        RuntimeValue::JsonNumber(value) => ExecutionValue::Text(value),
        RuntimeValue::Callable(callable) => ExecutionValue::Callable {
            name: callable_name(&callable).to_owned(),
        },
    }
}

fn public_fields(fields: Vec<RuntimeField>) -> Vec<ValueField> {
    fields
        .into_iter()
        .map(|field| ValueField {
            name: field.name,
            value: public_value(field.value),
        })
        .collect()
}

fn callable_name(callable: &RuntimeCallable) -> &str {
    match callable {
        RuntimeCallable::Declared { .. } => "function",
        RuntimeCallable::Closure { name, .. } => name,
        RuntimeCallable::Dependency { name, .. } => name,
        RuntimeCallable::Intrinsic(function) => function.name(),
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
        CoreOperation::Record(_) => "record",
        CoreOperation::Unary { .. } => "unary",
        CoreOperation::Binary { .. } => "binary",
        CoreOperation::Identity(_) => "identity",
        CoreOperation::Provision { .. } => "provision",
        CoreOperation::Sequence(_) => "sequence",
        CoreOperation::Closure { .. } => "closure",
        CoreOperation::Match { .. } => "match",
    }
}

fn control_name(control: &Control) -> &'static str {
    match control {
        Control::Success(_) => "success",
        Control::Failure(_) => "expected_failure",
        Control::Defect { .. } => "defect",
        Control::Cancellation => "cancellation",
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

fn checked_integer(node: CoreNodeId, value: Option<i64>, operation: &str) -> Control {
    value
        .map(RuntimeValue::Integer)
        .map(Control::Success)
        .unwrap_or_else(|| Control::Defect {
            message: format!("integer {operation} overflow"),
            node: Some(node),
        })
}

fn write_json_value(output: &mut String, value: &ExecutionValue) {
    match value {
        ExecutionValue::Integer(value) => {
            write!(output, "{value}").expect("writing JSON to a string cannot fail")
        }
        ExecutionValue::Text(value) => write_json_string(output, value),
        ExecutionValue::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        ExecutionValue::List(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_value(output, value);
            }
            output.push(']');
        }
        ExecutionValue::Record(fields) => write_json_fields(output, fields),
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
            output.push_str(",\"fields\":");
            write_json_fields(output, fields);
            output.push('}');
        }
        ExecutionValue::Callable { name } => {
            output.push_str("{\"callable\":");
            write_json_string(output, name);
            output.push('}');
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

fn write_json_fields(output: &mut String, fields: &[ValueField]) {
    output.push('{');
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write_json_string(output, &field.name);
        output.push(':');
        write_json_value(output, &field.value);
    }
    output.push('}');
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
        events: vec![
            event(0, EventKind::ComputationStarted),
            event(1, EventKind::ComputationCompleted),
        ],
        outcome: ExecutionOutcome::Success(ExecutionValue::Constructed {
            type_name: core.constructor_name,
            variant_name: None,
            fields: vec![ValueField {
                name: core.field_name,
                value: ExecutionValue::Integer(core.value),
            }],
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
    use super::{ExecutionConfig, RuntimeMap, RuntimeValue, TraceMode};
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

    #[test]
    fn persistent_maps_retain_prior_values_and_share_untouched_branches() {
        let mut map = RuntimeMap::default();
        for (index, key) in ["d", "b", "f", "a", "c", "e", "g"].into_iter().enumerate() {
            map = map.put(key.to_owned(), RuntimeValue::Integer(index as i64));
        }
        let prior = map.clone();
        let updated = map.put("g".to_owned(), RuntimeValue::Integer(99));

        assert!(matches!(prior.get("g"), Some(RuntimeValue::Integer(6))));
        assert!(matches!(updated.get("g"), Some(RuntimeValue::Integer(99))));
        let prior_left = prior.root.as_ref().unwrap().left.as_ref().unwrap();
        let updated_left = updated.root.as_ref().unwrap().left.as_ref().unwrap();
        assert!(Arc::ptr_eq(prior_left, updated_left));
    }

    #[test]
    fn ordered_insertions_keep_map_height_logarithmic() {
        let mut map = RuntimeMap::default();
        for value in 0..10_000 {
            map = map.put(format!("{value:05}"), RuntimeValue::Integer(value));
        }

        assert!(map.root.as_ref().unwrap().height <= 20);
        assert_eq!(map.entries().len(), 10_000);
    }
}
