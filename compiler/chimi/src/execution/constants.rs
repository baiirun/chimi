//! Build-time evaluation of module constants on the reference interpreter.
//!
//! Every constant runs once, here, with no scheduler, no providers, and no trace.
//! The rules the type checker cannot see are enforced as the initializer runs: only
//! `pure` operations (`E5201`), no defect (`E5202`), bounded work and size (`E5203`),
//! and no cycle through constants (`E5205`). Each broken constant yields exactly one
//! diagnostic; a constant that only failed because another one did reports nothing.

use super::{
    Control, Environment, Evaluator, ExecutionConfig, HostFunction, RuntimeCallable, RuntimeField,
    RuntimeMap, RuntimeValue, TraceMode,
};
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::executable::{
    ConstantFunction, ConstantValue, CoreConstant, CoreOperation, CoreProgram, CoreSymbolKind,
};
use crate::ids::{CoreNodeId, SymbolId};
use crate::standard::{StandardFunction, StandardModule, StandardSymbol};
use std::collections::{HashMap, HashSet};

/// The provisional budgets of one constant's evaluation (`E5203`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConstantLimits {
    pub(crate) operations: u64,
    pub(crate) value_bytes: usize,
}

impl ConstantLimits {
    pub(crate) const BUILD: Self = Self {
        operations: 50_000_000,
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
                _ => None,
            },
            Self::Host(function) => Some(
                match function {
                    HostFunction::ProcessUtf8Lines => "FileSystem.process_utf8_lines",
                    HostFunction::WriteLine => "Console.write_line",
                    HostFunction::WriteError => "Console.write_error",
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
    Cycle(Vec<Frame>),
    /// A constant this one depends on already reported its own diagnostic.
    Reported,
}

pub(super) struct ConstantEvaluation {
    limits: ConstantLimits,
    operations: u64,
    frames: Vec<Frame>,
    stop: Option<Stop>,
    failed: HashSet<SymbolId>,
    diagnostics: Vec<Diagnostic>,
}

/// Evaluates every constant initializer in `core`.
///
/// Ensures: on success, one value per initializer in initializer order, each free of
/// capabilities and providers; on failure, one diagnostic per constant that broke a
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
    let mut config =
        ExecutionConfig::new("constant-evaluation").expect("fixed execution id is valid");
    config.trace_mode = TraceMode::Off;
    let mut evaluator = Evaluator::new(core, config);
    evaluator.constant_evaluation = Some(ConstantEvaluation {
        limits,
        operations: 0,
        frames: Vec::new(),
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
    let closures = closure_nodes(core);
    let mut constants = Vec::new();
    for initializer in &core.constant_initializers {
        let Some(value) = evaluator.constants.get(&initializer.symbol) else {
            continue;
        };
        let mut bytes = 0;
        match constant_value(core, &closures, value, &mut bytes) {
            Ok(value) if bytes <= limits.value_bytes => constants.push(CoreConstant {
                symbol: initializer.symbol,
                value,
            }),
            Ok(_) => diagnostics.push(diagnostic_at(
                core,
                initializer.value,
                "E5203",
                format!(
                    "constant `{}` is larger than the {} MiB a constant may store",
                    core.symbol(initializer.symbol).name,
                    limits.value_bytes / (1024 * 1024)
                ),
            )),
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
            };
        };
        let state = self.constant_state();
        if state.failed.contains(&symbol) {
            state.stop = Some(Stop::Reported);
            return reported_defect();
        }
        state.stop = None;
        let saved_operations = std::mem::take(&mut state.operations);
        state.frames.push(Frame::Constant(symbol));
        let control = self.eval_node(initializer, 1, Environment::default());
        let state = self.constant_state();
        state.frames.pop();
        state.operations = saved_operations;
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

    pub(super) fn enter_constant_call(&mut self, function: SymbolId) {
        if let Some(state) = self.constant_evaluation.as_mut() {
            state.frames.push(Frame::Function(function));
        }
    }

    pub(super) fn leave_constant_call(&mut self) {
        if let Some(state) = self.constant_evaluation.as_mut() {
            state.frames.pop();
        }
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
                Control::Defect { message, node } => diagnostic_at(
                    core,
                    node.unwrap_or(initializer),
                    "E5202",
                    format!("constant `{name}` defected at build time: {message}"),
                ),
                Control::Failure(_) | Control::Cancellation | Control::Break { .. } => {
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

fn reported_defect() -> Control {
    Control::Defect {
        message: "constant evaluation stopped".to_owned(),
        node: None,
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

/// Converts an evaluated value to stored data, adding its approximate stored size to
/// `bytes`. `Err` names a part that cannot outlive the evaluation.
fn constant_value(
    core: &CoreProgram,
    closures: &HashMap<CoreNodeId, CoreNodeId>,
    value: &RuntimeValue,
    bytes: &mut usize,
) -> Result<ConstantValue, String> {
    *bytes += 16;
    let convert =
        |value: &RuntimeValue, bytes: &mut usize| constant_value(core, closures, value, bytes);
    Ok(match value {
        RuntimeValue::Void => ConstantValue::Void,
        RuntimeValue::Integer(value) => ConstantValue::Integer(*value),
        RuntimeValue::Float(value) => ConstantValue::Float(*value),
        RuntimeValue::Text(value) => {
            *bytes += value.len();
            ConstantValue::Text(value.clone())
        }
        RuntimeValue::Boolean(value) => ConstantValue::Boolean(*value),
        RuntimeValue::List(values) => ConstantValue::List(
            values
                .iter()
                .map(|value| convert(value, bytes))
                .collect::<Result<_, _>>()?,
        ),
        RuntimeValue::Record(fields) => ConstantValue::Record(
            fields
                .iter()
                .map(|field| Ok((field.name.clone(), convert(&field.value, bytes)?)))
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
                .map(|field| convert(&field.value, bytes))
                .collect::<Result<_, _>>()?,
        },
        RuntimeValue::Map(map) => ConstantValue::Map(
            map.entries()
                .into_iter()
                .map(|(key, value)| {
                    *bytes += key.len();
                    Ok((key, convert(&value, bytes)?))
                })
                .collect::<Result<_, String>>()?,
        ),
        RuntimeValue::JsonNumber(text) => {
            *bytes += text.len();
            ConstantValue::JsonNumber(text.clone())
        }
        RuntimeValue::Callable(callable) => ConstantValue::Function(match callable {
            RuntimeCallable::Declared {
                symbol,
                requirements,
            } if requirements.is_empty() => ConstantFunction::Declared(*symbol),
            RuntimeCallable::Closure {
                body, environment, ..
            } if environment.requirements.is_empty() => {
                let node = closures[body];
                let CoreOperation::Closure { captures, .. } =
                    &core.nodes[node.raw() as usize].operation
                else {
                    unreachable!("closure node map holds closure nodes");
                };
                let captures = captures
                    .iter()
                    .filter_map(|capture| {
                        environment
                            .locals
                            .get(capture)
                            .map(|value| Ok((*capture, convert(value, bytes)?)))
                    })
                    .collect::<Result<_, String>>()?;
                ConstantFunction::Closure { node, captures }
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
            RuntimeCallable::Constructor(symbol) => ConstantFunction::Constructor(*symbol),
            RuntimeCallable::Declared { .. }
            | RuntimeCallable::Closure { .. }
            | RuntimeCallable::Dependency { .. }
            | RuntimeCallable::Host(_) => return Err("a function bound to a provider".to_owned()),
        }),
    })
}

/// The interpreter value of a stored constant.
pub(super) fn runtime_value(core: &CoreProgram, value: &ConstantValue) -> RuntimeValue {
    match value {
        ConstantValue::Void => RuntimeValue::Void,
        ConstantValue::Integer(value) => RuntimeValue::Integer(*value),
        ConstantValue::Float(value) => RuntimeValue::Float(*value),
        ConstantValue::Text(value) => RuntimeValue::Text(value.clone()),
        ConstantValue::Boolean(value) => RuntimeValue::Boolean(*value),
        ConstantValue::List(values) => RuntimeValue::List(
            values
                .iter()
                .map(|value| runtime_value(core, value))
                .collect(),
        ),
        ConstantValue::Record(fields) => RuntimeValue::Record(
            fields
                .iter()
                .map(|(name, value)| RuntimeField {
                    name: name.clone(),
                    value: runtime_value(core, value),
                })
                .collect(),
        ),
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
                fields: shape
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|(name, value)| RuntimeField {
                        name: name.clone(),
                        value: runtime_value(core, value),
                    })
                    .collect(),
            }
        }
        ConstantValue::Map(entries) => RuntimeValue::Map(
            entries
                .iter()
                .fold(RuntimeMap::default(), |map, (key, value)| {
                    map.put(key.clone(), runtime_value(core, value))
                }),
        ),
        ConstantValue::JsonNumber(text) => RuntimeValue::JsonNumber(text.clone()),
        ConstantValue::Function(function) => RuntimeValue::Callable(match function {
            ConstantFunction::Declared(symbol) => RuntimeCallable::Declared {
                symbol: *symbol,
                requirements: HashMap::new(),
            },
            ConstantFunction::Intrinsic(symbol) => match core.symbol(*symbol).standard {
                Some(StandardSymbol::Function(function)) => RuntimeCallable::Intrinsic(function),
                _ => unreachable!("constant intrinsic names a standard function"),
            },
            ConstantFunction::Constructor(symbol) => RuntimeCallable::Constructor(*symbol),
            ConstantFunction::Closure { node, captures } => {
                let CoreOperation::Closure {
                    parameters, body, ..
                } = &core.nodes[node.raw() as usize].operation
                else {
                    unreachable!("constant closure names a closure node");
                };
                RuntimeCallable::Closure {
                    parameters: parameters.clone(),
                    body: *body,
                    environment: Environment {
                        locals: captures
                            .iter()
                            .map(|(symbol, value)| (*symbol, runtime_value(core, value)))
                            .collect(),
                        requirements: HashMap::new(),
                    },
                    name: "closure".to_owned(),
                }
            }
        }),
    }
}
