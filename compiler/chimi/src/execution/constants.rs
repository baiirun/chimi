//! Build-time evaluation of module constants on the reference interpreter.
//!
//! Every constant runs once, here, with no scheduler, no providers, and no trace.
//! The rules the type checker cannot see are enforced as the initializer runs: only
//! `pure` operations (`E5201`), no defect (`E5202`), bounded work, call depth, and
//! size (`E5203`), and no cycle through constants (`E5205`). Each broken constant
//! yields exactly one diagnostic; a constant that only failed because another one did
//! reports nothing.

use super::{
    Control, Environment, Evaluator, ExecutionConfig, HostFunction, RuntimeCallable, RuntimeField,
    RuntimeMap, RuntimeMapNode, RuntimeValue, TraceMode,
};
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::executable::{
    ConstantFunction, ConstantRequirements, ConstantValue, CoreConstant, CoreOperation,
    CoreProgram, CoreSymbolKind,
};
use crate::ids::{CoreNodeId, SymbolId};
use crate::standard::{StandardFunction, StandardModule, StandardSymbol};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

/// The provisional budgets of one constant's evaluation (`E5203`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConstantLimits {
    pub(crate) operations: u64,
    /// The most function and closure calls that may be active at once.
    pub(crate) call_depth: usize,
    /// The largest value the evaluation may build, in stored bytes (see `Measure`).
    pub(crate) value_bytes: usize,
}

impl ConstantLimits {
    pub(crate) const BUILD: Self = Self {
        operations: 50_000_000,
        call_depth: 10_000,
        value_bytes: 16 * 1024 * 1024,
    };
}

/// An operation build-time evaluation may refuse because it is not `pure`.
#[derive(Clone, Copy, Debug)]
pub(super) enum Operation {
    Standard(StandardFunction),
    Host(HostFunction),
}

impl Operation {
    /// The qualified name of a `runtime` or `host` operation, or `None` when it is pure.
    fn impure_name(self) -> Option<String> {
        match self {
            Self::Standard(function) => match function.module() {
                Some(module @ (StandardModule::Task | StandardModule::Channel)) => {
                    Some(format!("{}.{}", module.path(), function.name()))
                }
                Some(module) if function.requires_process_runtime() => {
                    Some(format!("{}.{}", module.path(), function.name()))
                }
                // A test-only operation observes defects, and constant evaluation
                // reports its own stops as defects, so a constant must not call it.
                _ => crate::operations::test_only_name(function),
            },
            Self::Host(function) => Some(
                match function {
                    HostFunction::ProcessUtf8Lines => "FileSystem.process_utf8_lines",
                    HostFunction::WriteLine => "Console.write_line",
                    HostFunction::WriteError => "Console.write_error",
                    HostFunction::RandomNextInt => "Random.next_int",
                }
                .to_owned(),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    Constant(SymbolId),
    Function(SymbolId),
}

/// Why the innermost constant stopped, set where the rule broke and taken by the
/// constant that owns the evaluation.
#[derive(Debug)]
enum Stop {
    Impure {
        operation: String,
        node: CoreNodeId,
        chain: Vec<Frame>,
    },
    Operations,
    Depth,
    Size,
    Cycle(Vec<Frame>),
    /// A constant this one depends on already reported its own diagnostic.
    Reported,
}

pub(super) struct ConstantEvaluation {
    limits: ConstantLimits,
    operations: u64,
    /// Function and closure calls active in the current constant.
    calls: usize,
    frames: Vec<Frame>,
    closures: HashMap<CoreNodeId, CoreNodeId>,
    shared: SharedSizes,
    stop: Option<Stop>,
    failed: HashSet<SymbolId>,
    diagnostics: Vec<Diagnostic>,
}

/// Evaluates every constant initializer in `core`.
///
/// Ensures: on success, one value per initializer in initializer order, each free of
/// capabilities and of providers other than constant ones; on failure, one diagnostic per constant that broke a
/// build-time rule itself. Must not emit trace events or touch the host.
pub(crate) fn evaluate_constants(core: &CoreProgram) -> Result<Vec<CoreConstant>, Vec<Diagnostic>> {
    evaluate_constants_with(core, ConstantLimits::BUILD)
}

pub(crate) fn evaluate_constants_with(
    core: &CoreProgram,
    limits: ConstantLimits,
) -> Result<Vec<CoreConstant>, Vec<Diagnostic>> {
    if core.constant_initializers.is_empty() {
        return Ok(Vec::new());
    }
    super::on_evaluation_stack("constant-evaluation", || {
        evaluate_on_this_thread(core, limits)
    })
}

fn evaluate_on_this_thread(
    core: &CoreProgram,
    limits: ConstantLimits,
) -> Result<Vec<CoreConstant>, Vec<Diagnostic>> {
    let mut config =
        ExecutionConfig::new("constant-evaluation").expect("fixed execution id is valid");
    config.trace_mode = TraceMode::Off;
    let mut evaluator = Evaluator::new(core, config);
    evaluator.constant_evaluation = Some(ConstantEvaluation {
        limits,
        operations: 0,
        calls: 0,
        frames: Vec::new(),
        closures: closure_nodes(core),
        shared: SharedSizes::default(),
        stop: None,
        failed: HashSet::new(),
        diagnostics: Vec::new(),
    });
    for initializer in &core.constant_initializers {
        if !evaluator.constants.contains_key(&initializer.symbol) {
            let _ = evaluator.evaluate_constant(initializer.symbol);
        }
    }
    let state = evaluator
        .constant_evaluation
        .take()
        .expect("constant evaluation state is present");
    let mut diagnostics = state.diagnostics;
    let (closures, mut shared) = (state.closures, state.shared);
    let mut constants = Vec::new();
    for initializer in &core.constant_initializers {
        let Some(value) = evaluator.constants.get(&initializer.symbol) else {
            continue;
        };
        let size = Measure {
            core,
            closures: &closures,
            shared: &mut shared,
            limit: limits.value_bytes,
        }
        .value(value);
        if size.is_none() {
            diagnostics.push(diagnostic_at(
                core,
                initializer.value,
                "E5203",
                too_large(&core.symbol(initializer.symbol).name, limits),
            ));
            continue;
        }
        match constant_value(core, &closures, value) {
            Ok(value) => constants.push(CoreConstant {
                symbol: initializer.symbol,
                value,
            }),
            Err(part) => diagnostics.push(diagnostic_at(
                core,
                initializer.value,
                "E5201",
                format!(
                    "constant `{}` holds {part}, which only exists at run time",
                    core.symbol(initializer.symbol).name
                ),
            )),
        }
    }
    if diagnostics.is_empty() {
        Ok(constants)
    } else {
        Err(diagnostics)
    }
}

impl Evaluator<'_> {
    /// Evaluates the constant `symbol` and stores its value.
    ///
    /// Requires: constant evaluation is active and `symbol` has no value yet.
    /// Ensures: `Success` with the value, or a defect after recording this constant's
    /// diagnostic (or `Stop::Reported` when a dependency already reported one).
    fn evaluate_constant(&mut self, symbol: SymbolId) -> Control {
        let Some(initializer) = self
            .core
            .constant_initializers
            .iter()
            .find(|initializer| initializer.symbol == symbol)
            .map(|initializer| initializer.value)
        else {
            return Control::Defect {
                message: format!(
                    "constant {} has no initializer",
                    self.core.symbol(symbol).name
                ),
                node: None,
                cause: None,
            };
        };
        let state = self.constant_state();
        if state.failed.contains(&symbol) {
            state.stop = Some(Stop::Reported);
            return reported_defect();
        }
        state.stop = None;
        let saved_operations = std::mem::take(&mut state.operations);
        let saved_calls = std::mem::take(&mut state.calls);
        state.frames.push(Frame::Constant(symbol));
        let control = self.eval_node(initializer, 1, Environment::default());
        let state = self.constant_state();
        state.frames.pop();
        state.operations = saved_operations;
        state.calls = saved_calls;
        match control {
            Control::Success(value) => {
                self.constants.insert(symbol, value.clone());
                Control::Success(value)
            }
            other => {
                let state = self.constant_state();
                state.failed.insert(symbol);
                let stop = state.stop.take();
                if let Some(diagnostic) = self.constant_diagnostic(symbol, initializer, stop, other)
                {
                    self.constant_state().diagnostics.push(diagnostic);
                }
                self.constant_state().stop = Some(Stop::Reported);
                reported_defect()
            }
        }
    }

    /// Loads a constant that has no value yet. At run time every constant has one, so
    /// this only happens during build-time evaluation, where it evaluates the constant
    /// or reports the cycle that reached it.
    pub(super) fn evaluate_constant_reference(&mut self, symbol: SymbolId, _span: u64) -> Control {
        let Some(state) = self.constant_evaluation.as_mut() else {
            return Control::Defect {
                message: format!("constant {} has no value", self.core.symbol(symbol).name),
                node: None,
                cause: None,
            };
        };
        if let Some(start) = state
            .frames
            .iter()
            .position(|frame| matches!(frame, Frame::Constant(active) if *active == symbol))
        {
            let mut path = state.frames[start..].to_vec();
            path.push(Frame::Constant(symbol));
            state.stop = Some(Stop::Cycle(path));
            return reported_defect();
        }
        self.evaluate_constant(symbol)
    }

    /// Counts one executed Core operation against the current constant's budget.
    pub(super) fn count_constant_operation(&mut self, _node: CoreNodeId) -> Option<Control> {
        let state = self.constant_evaluation.as_mut()?;
        state.operations += 1;
        if state.operations <= state.limits.operations {
            return None;
        }
        state.stop = Some(Stop::Operations);
        Some(reported_defect())
    }

    /// Refuses a `runtime` or `host` operation while a constant is being evaluated.
    pub(super) fn reject_constant_operation(
        &mut self,
        operation: Operation,
        call_node: CoreNodeId,
    ) -> Option<Control> {
        let state = self.constant_evaluation.as_mut()?;
        let operation = operation.impure_name()?;
        state.stop = Some(Stop::Impure {
            operation,
            node: call_node,
            chain: state.frames.clone(),
        });
        Some(reported_defect())
    }

    /// Starts a call of the declared function `function`, or of a closure when `None`,
    /// unless the current constant already has its budget of active calls.
    ///
    /// Ensures: on `None`, a matching `leave_constant_call` must follow the call.
    pub(super) fn enter_constant_call(&mut self, function: Option<SymbolId>) -> Option<Control> {
        let state = self.constant_evaluation.as_mut()?;
        if state.calls >= state.limits.call_depth {
            state.stop = Some(Stop::Depth);
            return Some(reported_defect());
        }
        state.calls += 1;
        if let Some(function) = function {
            state.frames.push(Frame::Function(function));
        }
        None
    }

    pub(super) fn leave_constant_call(&mut self, function: Option<SymbolId>) {
        if let Some(state) = self.constant_evaluation.as_mut() {
            state.calls -= 1;
            if function.is_some() {
                state.frames.pop();
            }
        }
    }

    /// Stops the current constant when `control` carries a value larger than the size
    /// budget, so a value that doubles each step stops near the budget instead of
    /// after it has been built and converted.
    pub(super) fn measure_constant_value(&mut self, control: &Control) -> Option<Control> {
        let Control::Success(value) = control else {
            return None;
        };
        let core = self.core;
        let state = self.constant_evaluation.as_mut()?;
        let fits = Measure {
            core,
            closures: &state.closures,
            shared: &mut state.shared,
            limit: state.limits.value_bytes,
        }
        .value(value)
        .is_some();
        if fits {
            return None;
        }
        state.stop = Some(Stop::Size);
        Some(reported_defect())
    }

    fn constant_state(&mut self) -> &mut ConstantEvaluation {
        self.constant_evaluation
            .as_mut()
            .expect("constant evaluation is active")
    }

    fn constant_diagnostic(
        &self,
        symbol: SymbolId,
        initializer: CoreNodeId,
        stop: Option<Stop>,
        control: Control,
    ) -> Option<Diagnostic> {
        let core = self.core;
        let name = &core.symbol(symbol).name;
        let path = |frames: &[Frame]| {
            frames
                .iter()
                .map(|frame| match frame {
                    Frame::Constant(symbol) | Frame::Function(symbol) => {
                        core.symbol(*symbol).name.as_str()
                    }
                })
                .collect::<Vec<_>>()
                .join(" -> ")
        };
        Some(match stop {
            Some(Stop::Reported) => return None,
            Some(Stop::Impure {
                operation,
                node,
                chain,
            }) => diagnostic_at(
                core,
                node,
                "E5201",
                format!(
                    "constant `{name}` cannot be evaluated at build time: it reaches \
                     `{operation}`, which needs the running program (through {})",
                    path(&chain)
                ),
            ),
            Some(Stop::Operations) => diagnostic_at(
                core,
                initializer,
                "E5203",
                format!(
                    "constant `{name}` did not finish within {} operations",
                    self.constant_evaluation
                        .as_ref()
                        .map_or(0, |state| state.limits.operations)
                ),
            ),
            Some(Stop::Depth) => diagnostic_at(
                core,
                initializer,
                "E5203",
                format!(
                    "constant `{name}` nested more than {} calls",
                    self.constant_evaluation
                        .as_ref()
                        .map_or(0, |state| state.limits.call_depth)
                ),
            ),
            Some(Stop::Size) => diagnostic_at(
                core,
                initializer,
                "E5203",
                too_large(
                    name,
                    self.constant_evaluation
                        .as_ref()
                        .map_or(ConstantLimits::BUILD, |state| state.limits),
                ),
            ),
            Some(Stop::Cycle(frames)) => diagnostic_at(
                core,
                initializer,
                "E5205",
                format!(
                    "constants depend on each other in a cycle: {}",
                    path(&frames)
                ),
            ),
            None => match control {
                Control::Defect { message, node, .. } => diagnostic_at(
                    core,
                    node.unwrap_or(initializer),
                    "E5202",
                    format!("constant `{name}` defected at build time: {message}"),
                ),
                Control::Failure(..) | Control::Cancellation(_) | Control::Break { .. } => {
                    diagnostic_at(
                        core,
                        initializer,
                        "E5202",
                        format!("constant `{name}` did not produce a value at build time"),
                    )
                }
                Control::Success(_) => return None,
            },
        })
    }
}

fn too_large(name: &str, limits: ConstantLimits) -> String {
    format!(
        "constant `{name}` builds a value larger than the {} MiB a constant may store",
        limits.value_bytes / (1024 * 1024)
    )
}

/// Whether the result of `operation` is measured against the size budget while a
/// constant evaluates: the operations that copy a value out of the environment.
///
/// A value can only outgrow the budget step after step by holding copies of an
/// earlier value, and every copy starts at one of these. One expression without them
/// grows a value by at most the sum of the copies it combines, so the overshoot is
/// bounded by the source, and the final measurement still decides what is stored.
/// Measuring here also costs no more than the interpreter's own copy, which is linear
/// in the parts it does not share, while measuring every construction would make a
/// recursive builder such as `Link(chain(n - 1))` quadratic.
pub(super) fn copies_value(operation: &CoreOperation) -> bool {
    matches!(
        operation,
        CoreOperation::Reference(_) | CoreOperation::Closure { .. }
    )
}

fn reported_defect() -> Control {
    Control::Defect {
        message: "constant evaluation stopped".to_owned(),
        node: None,
        cause: None,
    }
}

fn diagnostic_at(
    core: &CoreProgram,
    node: CoreNodeId,
    code: &'static str,
    message: String,
) -> Diagnostic {
    let origin = &core.origins[core.nodes[node.raw() as usize].origin.raw() as usize];
    Diagnostic::error(
        DiagnosticStage::ExecutionSetup,
        code,
        origin.file_id,
        origin.range,
        message,
    )
}

/// Each closure body's `Closure` node, so a closure value can name the node that made it.
fn closure_nodes(core: &CoreProgram) -> HashMap<CoreNodeId, CoreNodeId> {
    core.nodes
        .iter()
        .filter_map(|node| match &node.operation {
            CoreOperation::Closure { body, .. } => Some((*body, node.id)),
            _ => None,
        })
        .collect()
}

/// Stored sizes of the parts that interpreter values share instead of copying: lists,
/// map nodes, closure environments, tuples, and the fields of records and constructed
/// values, keyed by address. Measuring a value that grew
/// by one step then costs the new parts, not the whole value.
///
/// Each entry holds a weak handle to its part, which keeps the allocation, and so the
/// address, from being reused while the entry lives.
#[derive(Default)]
struct SharedSizes {
    known: HashMap<usize, (Weak<dyn Any>, usize)>,
    /// Entry count at which parts no value holds any more are dropped.
    sweep_at: usize,
}

impl SharedSizes {
    fn get(&self, address: usize) -> Option<usize> {
        self.known.get(&address).map(|(_, size)| *size)
    }

    fn insert(&mut self, address: usize, part: Weak<dyn Any>, size: usize) {
        if self.known.len() >= self.sweep_at {
            self.known.retain(|_, (part, _)| part.strong_count() > 0);
            self.sweep_at = (2 * self.known.len()).max(4096);
        }
        self.known.insert(address, (part, size));
    }
}

#[cfg(test)]
thread_local! {
    /// Values `Measure` has visited on this thread, cached parts included, so a test
    /// can bound measuring work without timing it.
    static MEASURED_VALUES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Evaluates `core`'s constants as `evaluate_constants_with` does and returns how many
/// values `Measure` visited, with whether evaluation succeeded.
#[cfg(test)]
pub(crate) fn measured_values_to_evaluate(
    core: &CoreProgram,
    limits: ConstantLimits,
) -> (usize, bool) {
    super::on_evaluation_stack("constant-evaluation", || {
        MEASURED_VALUES.with(|count| count.set(0));
        let evaluated = evaluate_on_this_thread(core, limits).is_ok();
        (MEASURED_VALUES.with(std::cell::Cell::get), evaluated)
    })
}

/// Bytes of one `CHIMI_CONSTANT_VALUES` row (`ChimiConstantValue` in `runtime.h`).
const ROW_BYTES: usize = 24;
/// Bytes of one entry of `CHIMI_IDS`.
const ID_BYTES: usize = 4;

/// Measures the stored size of an evaluated value without building it.
///
/// A value is charged what the native table stores for it: one row, one id per part it
/// refers to (two for a named or keyed part: name or key, and value), the bytes of its
/// text, and its parts. A part reached twice is charged twice, because the stored form,
/// the interpreter's loaded copy, and the native table all hold a copy per reference.
struct Measure<'a> {
    core: &'a CoreProgram,
    closures: &'a HashMap<CoreNodeId, CoreNodeId>,
    shared: &'a mut SharedSizes,
    limit: usize,
}

impl Measure<'_> {
    /// The stored size of `value`, or `None` as soon as it exceeds `limit`.
    fn value(&mut self, value: &RuntimeValue) -> Option<usize> {
        #[cfg(test)]
        MEASURED_VALUES.with(|count| count.set(count.get() + 1));
        match value {
            RuntimeValue::Void
            | RuntimeValue::Integer(_)
            | RuntimeValue::Duration(_)
            | RuntimeValue::Decimal(_)
            | RuntimeValue::ProcessAddress(_)
            | RuntimeValue::Float(_)
            | RuntimeValue::Boolean(_) => Some(ROW_BYTES),
            // Its row refers to one `Int` row per bound.
            RuntimeValue::Range { .. } => self.sum(ROW_BYTES, 2 * (ID_BYTES + ROW_BYTES)),
            RuntimeValue::Text(text) | RuntimeValue::JsonNumber(text) => {
                self.sum(ROW_BYTES, text.len())
            }
            // Stored as hex text: two characters per byte.
            RuntimeValue::Bytes(bytes) => self.sum(ROW_BYTES, 2 * bytes.len()),
            RuntimeValue::List(values) => {
                let identity = values.identity();
                let address = Weak::as_ptr(&identity) as *const () as usize;
                if let Some(size) = self.shared.get(address) {
                    return Some(size);
                }
                let mut size = ROW_BYTES;
                for value in values.iter() {
                    let part = self.value(value)?;
                    size = self.sum(size, ID_BYTES + part)?;
                }
                self.shared.insert(address, identity, size);
                Some(size)
            }
            RuntimeValue::Record(fields) => self.fields(fields, 2 * ID_BYTES),
            RuntimeValue::Tuple(values) => {
                let address = Arc::as_ptr(values) as usize;
                if let Some(size) = self.shared.get(address) {
                    return Some(size);
                }
                let mut size = ROW_BYTES;
                for value in values.iter() {
                    let part = self.value(value)?;
                    size = self.sum(size, ID_BYTES + part)?;
                }
                self.shared
                    .insert(address, Arc::downgrade(values) as Weak<dyn Any>, size);
                Some(size)
            }
            RuntimeValue::Constructed { fields, .. } => self.fields(fields, ID_BYTES),
            RuntimeValue::Map(map) | RuntimeValue::Set(map) => {
                let entries = self.map_node(map.root.as_ref())?;
                self.sum(ROW_BYTES, entries)
            }
            RuntimeValue::Callable(RuntimeCallable::Declared { requirements, .. }) => {
                let providers = self.requirements(requirements)?;
                self.sum(ROW_BYTES, providers)
            }
            RuntimeValue::Callable(RuntimeCallable::Closure {
                body, environment, ..
            }) => {
                let address = Arc::as_ptr(environment) as usize;
                if let Some(size) = self.shared.get(address) {
                    return Some(size);
                }
                let mut size = ROW_BYTES;
                for capture in closure_captures(self.core, self.closures, *body).1 {
                    if let Some(value) = environment.locals.get(capture) {
                        let part = self.value(value)?;
                        size = self.sum(size, 2 * ID_BYTES + part)?;
                    }
                }
                let providers = self.requirements(&environment.requirements)?;
                size = self.sum(size, providers)?;
                let part = Arc::downgrade(environment) as Weak<dyn Any>;
                self.shared.insert(address, part, size);
                Some(size)
            }
            RuntimeValue::Callable(_) | RuntimeValue::Cause(_) | RuntimeValue::TestLog(_) => {
                Some(ROW_BYTES)
            }
        }
    }

    fn fields(&mut self, fields: &Arc<Vec<RuntimeField>>, ids: usize) -> Option<usize> {
        let address = Arc::as_ptr(fields) as usize;
        if let Some(size) = self.shared.get(address) {
            return Some(size);
        }
        let mut size = ROW_BYTES;
        for field in fields.iter() {
            let part = self.value(&field.value)?;
            size = self.sum(size, ids + part)?;
        }
        self.shared
            .insert(address, Arc::downgrade(fields) as Weak<dyn Any>, size);
        Some(size)
    }

    /// The entries of the map subtree at `node`, without the map's own row.
    fn map_node(&mut self, node: Option<&Arc<RuntimeMapNode>>) -> Option<usize> {
        let Some(node) = node else {
            return Some(0);
        };
        let address = Arc::as_ptr(node) as usize;
        if let Some(size) = self.shared.get(address) {
            return Some(size);
        }
        let mut size = 2 * ID_BYTES;
        for part in [&node.key, &node.value] {
            let part = self.value(part)?;
            size = self.sum(size, part)?;
        }
        for child in [node.left.as_ref(), node.right.as_ref()] {
            let part = self.map_node(child)?;
            size = self.sum(size, part)?;
        }
        let part = Arc::downgrade(node) as Weak<dyn Any>;
        self.shared.insert(address, part, size);
        Some(size)
    }

    fn requirements(&mut self, requirements: &HashMap<String, RuntimeValue>) -> Option<usize> {
        let mut size = 0;
        for value in requirements.values() {
            let part = self.value(value)?;
            size = self.sum(size, 2 * ID_BYTES + part)?;
        }
        Some(size)
    }

    fn sum(&self, left: usize, right: usize) -> Option<usize> {
        left.checked_add(right).filter(|size| *size <= self.limit)
    }
}

/// The `Closure` node that made the closure whose body is `body`, and its captures.
fn closure_captures<'core>(
    core: &'core CoreProgram,
    closures: &HashMap<CoreNodeId, CoreNodeId>,
    body: CoreNodeId,
) -> (CoreNodeId, &'core [SymbolId]) {
    let node = closures[&body];
    let CoreOperation::Closure { captures, .. } = &core.nodes[node.raw() as usize].operation else {
        unreachable!("closure node map holds closure nodes");
    };
    (node, captures)
}

/// Converts an evaluated value to stored data. `Err` names a part that cannot
/// outlive the evaluation.
///
/// Requires: `Measure` found the value within the size budget, so this costs at most
/// time linear in the budget.
fn constant_value(
    core: &CoreProgram,
    closures: &HashMap<CoreNodeId, CoreNodeId>,
    value: &RuntimeValue,
) -> Result<ConstantValue, String> {
    let convert = |value: &RuntimeValue| constant_value(core, closures, value);
    Ok(match value {
        RuntimeValue::Void => ConstantValue::Void,
        RuntimeValue::Integer(value) => ConstantValue::Integer(*value),
        RuntimeValue::Duration(value) => ConstantValue::Duration(*value),
        RuntimeValue::Decimal(value) => ConstantValue::Decimal(*value),
        RuntimeValue::ProcessAddress(symbol) => ConstantValue::ProcessAddress(*symbol),
        RuntimeValue::Float(value) => ConstantValue::Float(*value),
        RuntimeValue::Text(value) => ConstantValue::Text(value.clone()),
        RuntimeValue::Bytes(value) => ConstantValue::Bytes(super::bytes_operations::hex(value)),
        RuntimeValue::Boolean(value) => ConstantValue::Boolean(*value),
        RuntimeValue::List(values) => {
            ConstantValue::List(values.iter().map(convert).collect::<Result<_, _>>()?)
        }
        RuntimeValue::Range { start, end } => ConstantValue::Range {
            start: *start,
            end: *end,
        },
        RuntimeValue::Tuple(values) => {
            ConstantValue::Tuple(values.iter().map(convert).collect::<Result<_, _>>()?)
        }
        RuntimeValue::Record(fields) => ConstantValue::Record(
            fields
                .iter()
                .map(|field| Ok((field.name.clone(), convert(&field.value)?)))
                .collect::<Result<_, String>>()?,
        ),
        RuntimeValue::Constructed {
            constructor,
            fields,
            ..
        } => ConstantValue::Constructed {
            constructor: *constructor,
            fields: fields
                .iter()
                .map(|field| convert(&field.value))
                .collect::<Result<_, _>>()?,
        },
        RuntimeValue::Map(map) => ConstantValue::Map(
            map.entries()
                .into_iter()
                .map(|(key, value)| Ok((convert(&key)?, convert(&value)?)))
                .collect::<Result<_, String>>()?,
        ),
        RuntimeValue::Set(set) => ConstantValue::Set(
            set.entries()
                .into_iter()
                .map(|(item, _)| convert(&item))
                .collect::<Result<_, String>>()?,
        ),
        RuntimeValue::JsonNumber(text) => ConstantValue::JsonNumber(text.clone()),
        RuntimeValue::Cause(_) => return Err("a `Cause`".to_owned()),
        RuntimeValue::TestLog(_) => return Err("a test recorder or script".to_owned()),
        RuntimeValue::Callable(callable) => ConstantValue::Function(match callable {
            RuntimeCallable::Declared {
                symbol,
                requirements,
            } => ConstantFunction::Declared {
                symbol: *symbol,
                requirements: constant_requirements(core, closures, requirements)?,
            },
            RuntimeCallable::Closure {
                body, environment, ..
            } => {
                let (node, captures) = closure_captures(core, closures, *body);
                let captures = captures
                    .iter()
                    .filter_map(|capture| {
                        environment
                            .locals
                            .get(capture)
                            .map(|value| Ok((*capture, convert(value)?)))
                    })
                    .collect::<Result<_, String>>()?;
                ConstantFunction::Closure {
                    node,
                    captures,
                    requirements: constant_requirements(core, closures, &environment.requirements)?,
                }
            }
            RuntimeCallable::Intrinsic(function) => ConstantFunction::Intrinsic(
                core.symbols
                    .iter()
                    .find(|symbol| {
                        symbol.kind == CoreSymbolKind::Intrinsic
                            && symbol.standard == Some(StandardSymbol::Function(*function))
                    })
                    .map(|symbol| symbol.id)
                    .ok_or_else(|| format!("the operation `{}`", function.name()))?,
            ),
            RuntimeCallable::Constructor { symbol, .. } => ConstantFunction::Constructor(*symbol),
            RuntimeCallable::Dependency { .. } => return Err("a provider method".to_owned()),
            RuntimeCallable::Host(_) => return Err("a host operation".to_owned()),
        }),
    })
}

/// Bound providers in ascending name order, so the stored data is reproducible.
fn constant_requirements(
    core: &CoreProgram,
    closures: &HashMap<CoreNodeId, CoreNodeId>,
    requirements: &HashMap<String, RuntimeValue>,
) -> Result<ConstantRequirements, String> {
    let mut requirements = requirements.iter().collect::<Vec<_>>();
    requirements.sort_by(|left, right| left.0.cmp(right.0));
    requirements
        .into_iter()
        .map(|(name, value)| Ok((name.clone(), constant_value(core, closures, value)?)))
        .collect()
}

/// The interpreter value of a stored constant.
pub(super) fn runtime_value(core: &CoreProgram, value: &ConstantValue) -> RuntimeValue {
    match value {
        ConstantValue::Void => RuntimeValue::Void,
        ConstantValue::Integer(value) => RuntimeValue::Integer(*value),
        ConstantValue::Duration(value) => RuntimeValue::Duration(*value),
        ConstantValue::Decimal(value) => RuntimeValue::Decimal(*value),
        ConstantValue::ProcessAddress(symbol) => RuntimeValue::ProcessAddress(*symbol),
        ConstantValue::Float(value) => RuntimeValue::Float(*value),
        ConstantValue::Text(value) => RuntimeValue::Text(value.clone()),
        ConstantValue::Bytes(hex) => {
            RuntimeValue::Bytes(super::bytes_operations::from_hex(hex).into())
        }
        ConstantValue::Boolean(value) => RuntimeValue::Boolean(*value),
        ConstantValue::List(values) => RuntimeValue::List(
            values
                .iter()
                .map(|value| runtime_value(core, value))
                .collect(),
        ),
        ConstantValue::Range { start, end } => RuntimeValue::Range {
            start: *start,
            end: *end,
        },
        ConstantValue::Tuple(values) => RuntimeValue::Tuple(Arc::new(
            values
                .iter()
                .map(|value| runtime_value(core, value))
                .collect(),
        )),
        ConstantValue::Record(fields) => RuntimeValue::Record(Arc::new(
            fields
                .iter()
                .map(|(name, value)| RuntimeField {
                    name: name.clone(),
                    value: runtime_value(core, value),
                })
                .collect(),
        )),
        ConstantValue::Constructed {
            constructor,
            fields,
        } => {
            let shape = core
                .symbol(*constructor)
                .constructor
                .as_ref()
                .expect("constant constructor owns a shape");
            RuntimeValue::Constructed {
                constructor: *constructor,
                type_name: shape.type_name.clone(),
                variant_name: shape.variant_name.clone(),
                fields: Arc::new(
                    shape
                        .fields
                        .iter()
                        .zip(fields)
                        .map(|(name, value)| RuntimeField {
                            name: name.clone(),
                            value: runtime_value(core, value),
                        })
                        .collect(),
                ),
                cause: None,
            }
        }
        ConstantValue::Map(entries) => RuntimeValue::Map(runtime_map(
            core,
            entries
                .iter()
                .map(|(key, value)| (runtime_value(core, key), runtime_value(core, value))),
        )),
        ConstantValue::Set(items) => RuntimeValue::Set(runtime_map(
            core,
            items
                .iter()
                .map(|item| (runtime_value(core, item), RuntimeValue::Void)),
        )),
        ConstantValue::JsonNumber(text) => RuntimeValue::JsonNumber(text.clone()),
        ConstantValue::Function(function) => RuntimeValue::Callable(match function {
            ConstantFunction::Declared {
                symbol,
                requirements,
            } => RuntimeCallable::Declared {
                symbol: *symbol,
                requirements: runtime_requirements(core, requirements),
            },
            ConstantFunction::Intrinsic(symbol) => match core.symbol(*symbol).standard {
                Some(StandardSymbol::Function(function)) => RuntimeCallable::Intrinsic(function),
                _ => unreachable!("constant intrinsic names a standard function"),
            },
            ConstantFunction::Constructor(symbol) => RuntimeCallable::Constructor {
                symbol: *symbol,
                name: core.symbol(*symbol).name.clone(),
            },
            ConstantFunction::Closure {
                node,
                captures,
                requirements,
            } => {
                let CoreOperation::Closure {
                    parameters, body, ..
                } = &core.nodes[node.raw() as usize].operation
                else {
                    unreachable!("constant closure names a closure node");
                };
                RuntimeCallable::Closure {
                    parameters: parameters.clone(),
                    body: *body,
                    environment: std::sync::Arc::new(Environment {
                        locals: captures
                            .iter()
                            .map(|(symbol, value)| (*symbol, runtime_value(core, value)))
                            .collect(),
                        requirements: runtime_requirements(core, requirements),
                    }),
                    name: "closure".to_owned(),
                }
            }
        }),
    }
}

/// A map of `entries` in the structural key order every map uses.
fn runtime_map(
    core: &CoreProgram,
    entries: impl Iterator<Item = (RuntimeValue, RuntimeValue)>,
) -> RuntimeMap {
    let order = |left: &RuntimeValue, right: &RuntimeValue| {
        super::structural::structural_order(left, right, &|constructor| {
            core.symbol(constructor)
                .constructor
                .as_ref()
                .map_or(0, |shape| shape.ordinal)
        })
    };
    entries.fold(RuntimeMap::default(), |map, (key, value)| {
        map.put(key, value, &order)
    })
}

fn runtime_requirements(
    core: &CoreProgram,
    requirements: &ConstantRequirements,
) -> HashMap<String, RuntimeValue> {
    requirements
        .iter()
        .map(|(name, value)| (name.clone(), runtime_value(core, value)))
        .collect()
}
