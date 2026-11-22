//! Flow-sensitive checks for values that cross task and process ownership boundaries.
//!
//! Process rules (`process-and-supervisor-static-contracts.md`):
//! - `T4260`: a `Process.spec` body must be shareable, because every incarnation runs it
//!   again. A capture that is not shareable is reported at the body argument. A provider
//!   that is not shareable is reported at the body argument when `using` binds it to the
//!   body, and at the provider when a `using` binds it to a need that flowed out of a
//!   `Process.spec` call, directly or through calls of named functions.
//! - `Process.send` is a transfer boundary for its message, like `Channel.send`.
//! - `Supervisor.remove_process` and `remove_supervisor` consume their handle.
//!
//! Exclusive values move (`task-and-channel-capabilities.md`): a value built from an
//! exclusive binding carries it (`FlowValue::carries`), and the carried bindings move
//! when the value is bound by `let` or a loop state, taken apart by patterns, or
//! crosses a transfer boundary.

use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{ModuleId, SemanticId, SymbolId, TextRange};
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::{
    BinaryOperator, FunctionSignature, ModuleBlockItem, ModuleDeclaration, ModuleDeclarationKind,
    ModuleExpression, Pattern,
};
use crate::standard::{StandardFunction, StandardSymbol};
use crate::typing::{CapabilityClass, PropertyDerivation, TypedProject, ValueProperties};
use std::collections::{BTreeSet, HashMap, HashSet};

pub(crate) struct CapabilityReport {
    pub(crate) expression_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnershipState {
    Available,
    Transferred {
        at: TextRange,
        boundary: TransferBoundary,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferBoundary {
    ClosureCapture,
    TaskStart,
    ChannelSend,
    ProcessSend,
    /// The notice of `Process.monitor`, kept by the monitor until it is delivered.
    ProcessMonitor,
    /// The argument of `Supervisor.start_handoff`, which the new incarnation owns.
    StartHandoff,
    ProcessRemoval,
    SupervisorRemoval,
    /// A `let` or loop state binding took the value that carried it.
    IntoBinding(SymbolId),
    /// The patterns of a `case`, or of a destructuring `let`, took apart a value
    /// that carried it.
    IntoPattern,
}

impl TransferBoundary {
    fn description(self, typed: &TypedProject) -> String {
        match self {
            Self::ClosureCapture => "into a one-shot closure".to_owned(),
            Self::TaskStart => "by Task.start".to_owned(),
            Self::ChannelSend => "by Channel.send".to_owned(),
            Self::ProcessSend => "by Process.send".to_owned(),
            Self::ProcessMonitor => "by Process.monitor".to_owned(),
            Self::StartHandoff => "by Supervisor.start_handoff".to_owned(),
            Self::ProcessRemoval => "by Supervisor.remove_process".to_owned(),
            Self::SupervisorRemoval => "by Supervisor.remove_supervisor".to_owned(),
            Self::IntoBinding(owner) => format!("into `{}`", typed.resolved.symbol(owner).name),
            Self::IntoPattern => "into a pattern".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
struct FlowValue {
    properties: ValueProperties,
    /// What makes the value never transferable, for diagnostics.
    blocked_by: Option<String>,
    /// What makes the value never shareable, for diagnostics.
    unshared_by: Option<String>,
    nonshareable_provider: Option<String>,
    /// The exclusive bindings this value may hold: the binding itself when the value
    /// is one, and those it was built from by constructors, literals, blocks, `case`
    /// arms, `break`, and calls whose result type can hold an argument's type. Taking
    /// the value moves them. Empty for a value read out of a binding's field.
    carries: BTreeSet<SymbolId>,
    /// The exclusive bindings this value reads without holding them: the receiver
    /// of a `Select.receive` or `Select.watch` event. Taking the value moves
    /// nothing, but using it after one of them moved is `T4241`.
    borrows: BTreeSet<SymbolId>,
}

impl FlowValue {
    fn from_type(typed: &TypedProject, type_id: crate::ids::TypeId) -> Self {
        let properties = typed.value_properties(type_id);
        Self {
            properties,
            blocked_by: (properties.transferable == PropertyDerivation::Never)
                .then(|| typed.format_type(type_id)),
            unshared_by: (properties.shareable == PropertyDerivation::Never)
                .then(|| typed.format_type(type_id)),
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            borrows: BTreeSet::new(),
        }
    }

    fn shareable() -> Self {
        Self {
            properties: ValueProperties::SHAREABLE,
            blocked_by: None,
            unshared_by: None,
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            borrows: BTreeSet::new(),
        }
    }

    fn invalid() -> Self {
        Self {
            properties: ValueProperties::INVALID,
            blocked_by: None,
            unshared_by: None,
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            borrows: BTreeSet::new(),
        }
    }

    /// Lowers each of this value's transferability and shareability that its
    /// type leaves to the value (a type holding functions) to `other`'s,
    /// keeping its capability class, carries and borrows. A property the type
    /// settles, as for an `Address`, stays as the type says.
    fn take_sharing_of(&mut self, other: &Self) {
        let mut properties = self.properties;
        if properties.transferable == PropertyDerivation::ValueDependent {
            properties.transferable = properties.transferable.combine(other.properties.transferable);
        }
        if properties.shareable == PropertyDerivation::ValueDependent {
            properties.shareable = properties.shareable.combine(other.properties.shareable);
        }
        if properties.transferable == PropertyDerivation::Never && self.blocked_by.is_none() {
            self.blocked_by = other.blocked_by.clone();
        }
        if properties.shareable == PropertyDerivation::Never && self.unshared_by.is_none() {
            self.unshared_by = other.unshared_by.clone();
        }
        self.properties = properties;
    }

    fn combine(mut self, mut other: Self) -> Self {
        self.carries.append(&mut other.carries);
        self.borrows.append(&mut other.borrows);
        let properties = self.properties.combine(other.properties);
        let blocked_by = if properties.transferable == PropertyDerivation::Never {
            self.blocked_by.or(other.blocked_by)
        } else {
            None
        };
        let unshared_by = if properties.shareable == PropertyDerivation::Never {
            self.unshared_by.or(other.unshared_by)
        } else {
            None
        };
        Self {
            properties,
            blocked_by,
            unshared_by,
            nonshareable_provider: self.nonshareable_provider.or(other.nonshareable_provider),
            carries: self.carries,
            borrows: self.borrows,
        }
    }
}

#[derive(Clone, Debug)]
struct FlowBinding {
    value: FlowValue,
    ownership: OwnershipState,
    requirement: bool,
}

type FlowEnvironment = HashMap<SymbolId, FlowBinding>;

/// For each named function, the needs (by lexical name) that flow out of a
/// `Process.spec` call in its body and stay open: a `using` around a call of the
/// function snapshots its providers into a spec.
type SpecNeedSummaries = HashMap<SymbolId, BTreeSet<String>>;

/// Ensures: the report of a pass whose spec-need summaries are a fixed point, so a
/// function's summary is the same whether its callers come before or after it. A
/// project without `Process.spec` calls takes one pass.
pub(crate) fn check(typed: &TypedProject) -> CapabilityReport {
    let mut summaries = SpecNeedSummaries::new();
    loop {
        let mut checker = CapabilityChecker {
            typed,
            summaries: &summaries,
            expression_properties: HashMap::new(),
            diagnostics: Vec::new(),
            spec_needs: BTreeSet::new(),
            next_summaries: SpecNeedSummaries::new(),
            loop_breaks: Vec::new(),
        };
        checker.check_modules();
        if checker.next_summaries == summaries {
            return CapabilityReport {
                expression_properties: checker.expression_properties,
                diagnostics: checker.diagnostics,
            };
        }
        summaries = checker.next_summaries;
    }
}

struct CapabilityChecker<'a> {
    typed: &'a TypedProject,
    /// The previous pass's summaries, read at calls.
    summaries: &'a SpecNeedSummaries,
    expression_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    diagnostics: Vec<Diagnostic>,
    /// Needs that flowed out of `Process.spec` calls evaluated so far in the current
    /// declaration and that no enclosing `using` has bound yet.
    spec_needs: BTreeSet<String>,
    next_summaries: SpecNeedSummaries,
    /// For each enclosing loop of the function being evaluated, innermost last, what
    /// its `break` values carry: the loop's value carries them.
    loop_breaks: Vec<BTreeSet<SymbolId>>,
}

impl CapabilityChecker<'_> {
    fn check_modules(&mut self) {
        for module in &self.typed.resolved.semantic.modules {
            for declaration in &module.declarations {
                self.check_declaration(module.id, declaration);
            }
        }
    }

    fn check_declaration(&mut self, module_id: ModuleId, declaration: &ModuleDeclaration) {
        let Some(body) = &declaration.body else {
            return;
        };
        let mut environment = FlowEnvironment::new();
        self.spec_needs.clear();
        let mut function = None;
        if declaration.kind == ModuleDeclarationKind::Function {
            let Some(symbol) = self.typed.resolved.source_symbol(
                module_id,
                declaration.semantic_id,
                ProjectSymbolKind::Function,
                &declaration.name.text,
            ) else {
                return;
            };
            let Some(contract) = self.typed.contracts.get(&symbol) else {
                return;
            };
            self.add_signature_bindings(
                module_id,
                declaration
                    .function
                    .as_ref()
                    .expect("function declaration owns a signature"),
                contract,
                &mut environment,
            );
            function = Some(symbol);
        }
        self.evaluate(module_id, body, &mut environment);
        if let Some(symbol) = function
            && !self.spec_needs.is_empty()
        {
            self.next_summaries
                .insert(symbol, std::mem::take(&mut self.spec_needs));
        }
    }

    fn add_signature_bindings(
        &self,
        module_id: ModuleId,
        signature: &FunctionSignature,
        contract: &crate::typing::CallableContract,
        environment: &mut FlowEnvironment,
    ) {
        for (parameter, type_id) in signature.parameters.iter().zip(&contract.inputs) {
            if let Some(symbol) = self.typed.resolved.source_symbol(
                module_id,
                parameter.semantic_id,
                ProjectSymbolKind::Parameter,
                &parameter.name.text,
            ) {
                environment.insert(
                    symbol,
                    FlowBinding {
                        value: FlowValue::from_type(self.typed, *type_id),
                        ownership: OwnershipState::Available,
                        requirement: false,
                    },
                );
            }
        }
        for need in &self.typed.needs_rows[contract.needs.raw() as usize].entries {
            environment.insert(
                need.requirement,
                FlowBinding {
                    value: FlowValue::from_type(self.typed, need.interface),
                    ownership: OwnershipState::Available,
                    requirement: true,
                },
            );
        }
    }

    fn evaluate(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let value = match expression {
            ModuleExpression::Literal(_) => self.type_value(module_id, expression),
            ModuleExpression::Name { semantic_id, name } => {
                let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id) else {
                    return self.record(module_id, expression, FlowValue::invalid());
                };
                if let Some(binding) = environment.get(&symbol).cloned() {
                    if let OwnershipState::Transferred { boundary, .. } = binding.ownership {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticStage::Type,
                            "T4241",
                            self.file_id(module_id),
                            name.range,
                            format!(
                                "`{}` was transferred {} and cannot be used by its previous owner",
                                name.text,
                                boundary.description(self.typed)
                            ),
                        ));
                    }
                    self.check_borrows(module_id, name, &binding.value, environment);
                    let mut value = binding.value;
                    if !binding.requirement
                        && value.properties.capability == CapabilityClass::Exclusive
                    {
                        value.carries.insert(symbol);
                    }
                    value
                } else if matches!(
                    self.typed.resolved.symbol(symbol).kind,
                    ProjectSymbolKind::Function
                        | ProjectSymbolKind::StructConstructor
                        | ProjectSymbolKind::EnumConstructor
                        | ProjectSymbolKind::BuiltinValue
                ) {
                    FlowValue::shareable()
                } else {
                    self.type_value(module_id, expression)
                }
            }
            // A qualified constructor or function, such as `Ask.Done` or
            // `List.map`, names a declaration as a bare name does; it reads no
            // field of its receiver, which is a module or type name.
            ModuleExpression::Select { .. }
                if self
                    .expression_binding_symbol(module_id, expression)
                    .is_some_and(|symbol| {
                        matches!(
                            self.typed.resolved.symbol(symbol).kind,
                            ProjectSymbolKind::Function
                                | ProjectSymbolKind::StructConstructor
                                | ProjectSymbolKind::EnumConstructor
                                | ProjectSymbolKind::BuiltinValue
                        )
                    }) =>
            {
                FlowValue::shareable()
            }
            ModuleExpression::Select {
                receiver: receiver_expression,
                ..
            } => {
                let receiver = self.evaluate(module_id, receiver_expression, environment);
                let mut value = self.type_value(module_id, expression);
                let carries = if value.properties.shareable == PropertyDerivation::Always
                    || reads_a_binding(receiver_expression)
                {
                    BTreeSet::new()
                } else {
                    receiver.carries.clone()
                };
                if value.properties.transferable == PropertyDerivation::ValueDependent {
                    value = receiver;
                }
                value.carries = carries;
                value
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                let callee_value = self.evaluate(module_id, callee, environment);
                let arguments = arguments
                    .iter()
                    .map(|argument| {
                        let value = self.evaluate(module_id, &argument.value, environment);
                        (&argument.value, value)
                    })
                    .collect::<Vec<_>>();
                let standard = self.standard_function(module_id, callee);
                match standard {
                    Some(StandardFunction::TestRecord) => {
                        if let Some((argument, value)) = arguments.get(1) {
                            self.check_test_log_value(module_id, "Test.record", argument, value);
                        }
                    }
                    Some(StandardFunction::TestScript) => {
                        if let Some((argument, replies)) = arguments.first() {
                            self.check_test_log_value(module_id, "Test.script", argument, replies);
                        }
                    }
                    Some(
                        function @ (StandardFunction::TaskStart
                        | StandardFunction::TaskAll
                        | StandardFunction::TaskResults
                        | StandardFunction::TaskRace),
                    ) => {
                        // A combinator starts every thunk of its list as a
                        // child, so the list carries what its thunks capture.
                        if let Some((argument, thunk)) = arguments.first() {
                            self.check_task_transfer(
                                module_id,
                                function,
                                argument,
                                thunk,
                                environment,
                            );
                        }
                    }
                    Some(StandardFunction::ChannelSend) => {
                        if let Some((argument, payload)) = arguments.get(1) {
                            self.check_message_transfer(
                                module_id,
                                argument,
                                payload,
                                TransferBoundary::ChannelSend,
                                environment,
                            );
                        }
                    }
                    Some(
                        function @ (StandardFunction::BroadcastPublish
                        | StandardFunction::WatchPublish
                        | StandardFunction::WatchStartingWith),
                    ) => {
                        let index = usize::from(function != StandardFunction::WatchStartingWith);
                        if let Some((argument, value)) = arguments.get(index) {
                            self.check_published(module_id, function, argument, value);
                        }
                    }
                    Some(StandardFunction::ProcessSend) => {
                        if let Some((argument, message)) = arguments.get(1) {
                            self.check_message_transfer(
                                module_id,
                                argument,
                                message,
                                TransferBoundary::ProcessSend,
                                environment,
                            );
                        }
                    }
                    Some(StandardFunction::ProcessMonitor) => {
                        if let Some((argument, notice)) = arguments.get(2) {
                            self.check_message_transfer(
                                module_id,
                                argument,
                                notice,
                                TransferBoundary::ProcessMonitor,
                                environment,
                            );
                        }
                    }
                    Some(StandardFunction::SupervisorStartHandoff) => {
                        if let Some((argument, start)) = arguments.get(2) {
                            self.check_message_transfer(
                                module_id,
                                argument,
                                start,
                                TransferBoundary::StartHandoff,
                                environment,
                            );
                        }
                    }
                    Some(
                        function @ (StandardFunction::ProcessSpec
                        | StandardFunction::ProcessHandoffSpec),
                    ) => {
                        if let Some((argument, body)) = arguments.first() {
                            self.check_spec_body(module_id, function, argument, body);
                        }
                    }
                    Some(
                        function @ (StandardFunction::SupervisorRemoveProcess
                        | StandardFunction::SupervisorRemoveSupervisor),
                    ) => {
                        let boundary = if function == StandardFunction::SupervisorRemoveProcess {
                            TransferBoundary::ProcessRemoval
                        } else {
                            TransferBoundary::SupervisorRemoval
                        };
                        if let Some((argument, handle)) = arguments.get(1) {
                            self.transfer_carried(handle, argument.range(), boundary, environment);
                        }
                    }
                    Some(_) => {}
                    None => {
                        if let Some(symbol) = self.expression_binding_symbol(module_id, callee)
                            && let Some(needs) = self.summaries.get(&symbol)
                        {
                            self.spec_needs.extend(needs.iter().cloned());
                        }
                    }
                }
                // The callee is not followed: a result that could hold an exclusive
                // value may hold the closure called and every argument whose type
                // occurs in the result's type.
                // `Channel.add_receiver` returns a new Receiver of the same queue and
                // leaves its argument with its owner, so the result holds nothing.
                let mut result = self.type_value(module_id, expression);
                let holds_carried = result.properties.shareable != PropertyDerivation::Always
                    && standard != Some(StandardFunction::ChannelAddReceiver);
                if holds_carried {
                    result.carries = callee_value.carries;
                }
                let result_type = self.expression_type(module_id, expression);
                let constructs = self.is_constructor(module_id, callee);
                for (argument, value) in &arguments {
                    let embeds = match (result_type, self.expression_type(module_id, argument)) {
                        (Some(result_type), Some(argument_type)) => {
                            self.typed.type_may_hold(result_type, argument_type)
                        }
                        _ => true,
                    };
                    if embeds {
                        if holds_carried {
                            result.carries.extend(value.carries.iter().copied());
                        }
                        result.borrows.extend(value.borrows.iter().copied());
                        // A constructor holds its arguments, so a closure argument
                        // keeps what its captures make it: `Conn(fn() { child })`
                        // is no more shareable than the closure. Other calls are
                        // not followed, and their result may hold none of it.
                        if constructs {
                            result.take_sharing_of(value);
                        }
                    }
                }
                if let Some(StandardFunction::SelectReceive | StandardFunction::SelectWatch) =
                    standard
                    && let Some((argument, receiver)) = arguments.first()
                {
                    result.borrows.extend(receiver.carries.iter().copied());
                    // A receiver read out of a field, like `jobs.receiver`, carries
                    // nothing, so the event borrows the binding the field is read from.
                    result
                        .borrows
                        .extend(self.field_root_binding(module_id, argument, environment));
                }
                result
            }
            ModuleExpression::List { elements, .. } | ModuleExpression::Tuple { elements, .. } => {
                elements
                    .iter()
                    .map(|element| self.evaluate(module_id, element, environment))
                    .fold(FlowValue::shareable(), FlowValue::combine)
            }
            ModuleExpression::Interpolation { parts, .. } => {
                for part in crate::semantic::interpolated_expressions(parts) {
                    self.evaluate(module_id, part, environment);
                }
                FlowValue::shareable()
            }
            ModuleExpression::Record { fields, .. } => fields
                .iter()
                .map(|field| self.evaluate(module_id, &field.value, environment))
                .fold(FlowValue::shareable(), FlowValue::combine),
            ModuleExpression::Update { base, fields, .. } => {
                let base = self.evaluate(module_id, base, environment);
                fields
                    .iter()
                    .map(|field| self.evaluate(module_id, &field.value, environment))
                    .fold(base, FlowValue::combine)
            }
            ModuleExpression::Unary { operand, .. } => {
                self.evaluate(module_id, operand, environment);
                self.type_value(module_id, expression)
            }
            ModuleExpression::Binary {
                operator: BinaryOperator::And | BinaryOperator::Or,
                left,
                right,
                ..
            } => {
                // The right operand runs only on one branch of the left, like a `case` arm,
                // so its transfers are possible transfers.
                self.evaluate(module_id, left, environment);
                let mut branch = environment.clone();
                self.evaluate(module_id, right, &mut branch);
                self.merge_transfers(environment, &[branch]);
                self.type_value(module_id, expression)
            }
            ModuleExpression::Binary { left, right, .. } => {
                self.evaluate(module_id, left, environment);
                self.evaluate(module_id, right, environment);
                self.type_value(module_id, expression)
            }
            ModuleExpression::Group { expression, .. } => {
                self.evaluate(module_id, expression, environment)
            }
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => {
                let outer_spec_needs = std::mem::take(&mut self.spec_needs);
                let mut value = self.evaluate(module_id, expression, environment);
                let mut inner_spec_needs = std::mem::take(&mut self.spec_needs);
                for provider in providers {
                    let mut provided = self.evaluate(module_id, &provider.value, environment);
                    if inner_spec_needs.remove(&provider.name.text)
                        && provided.properties.shareable == PropertyDerivation::Never
                    {
                        self.nonshareable_spec_provider(
                            module_id,
                            provider.value.range(),
                            &provider.name.text,
                        );
                    }
                    if provided.properties.shareable == PropertyDerivation::Never {
                        provided.nonshareable_provider = Some(provider.name.text.clone());
                    }
                    self.transfer_carried(
                        &provided,
                        provider.value.range(),
                        TransferBoundary::ClosureCapture,
                        environment,
                    );
                    value = value.combine(provided);
                }
                self.spec_needs.extend(outer_spec_needs);
                self.spec_needs.append(&mut inner_spec_needs);
                value
            }
            ModuleExpression::UsingRecord {
                semantic_id,
                expression,
                record,
                ..
            } => {
                let outer_spec_needs = std::mem::take(&mut self.spec_needs);
                let value = self.evaluate(module_id, expression, environment);
                let mut inner_spec_needs = std::mem::take(&mut self.spec_needs);
                let mut provided = self.evaluate(module_id, record, environment);
                // The record is shared or not as a whole, so each provider it
                // supplies is judged by the record.
                let names = self
                    .typed
                    .using_records
                    .get(&(module_id, *semantic_id))
                    .map(|fields| fields.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>())
                    .unwrap_or_default();
                let nonshareable = provided.properties.shareable == PropertyDerivation::Never;
                for name in &names {
                    if inner_spec_needs.remove(name) && nonshareable {
                        self.nonshareable_spec_provider(module_id, record.range(), name);
                    }
                }
                if nonshareable {
                    provided.nonshareable_provider = names.first().cloned();
                }
                self.transfer_carried(
                    &provided,
                    record.range(),
                    TransferBoundary::ClosureCapture,
                    environment,
                );
                self.spec_needs.extend(outer_spec_needs);
                self.spec_needs.append(&mut inner_spec_needs);
                value.combine(provided)
            }
            ModuleExpression::Sequence { expressions, .. } => expressions
                .iter()
                .map(|child| self.evaluate(module_id, child, environment))
                .last()
                .unwrap_or_else(FlowValue::invalid),
            ModuleExpression::Block { items, .. } => {
                self.evaluate_block(module_id, items, environment)
            }
            ModuleExpression::Function {
                signature, body, ..
            } => self.evaluate_function(module_id, expression, signature, body, environment),
            ModuleExpression::Loop {
                state_id,
                initial,
                body,
                ..
            } => {
                let mut loop_value = self.type_value(module_id, expression);
                loop_value.carries =
                    self.evaluate_loop(module_id, *state_id, initial, body, environment);
                loop_value
            }
            ModuleExpression::Break { value, .. } => {
                if let Some(value) = value {
                    let carries = self.evaluate(module_id, value, environment).carries;
                    if let Some(breaks) = self.loop_breaks.last_mut() {
                        breaks.extend(carries);
                    }
                }
                FlowValue::invalid()
            }
            ModuleExpression::Case { subjects, arms, .. } => {
                for subject in subjects {
                    // A subject built here, not read from a binding, is taken apart by
                    // the patterns, as by a destructuring `let`: they take what it
                    // carries.
                    let value = self.evaluate(module_id, subject, environment);
                    if !reads_a_binding(subject) {
                        self.transfer_carried(
                            &value,
                            subject.range(),
                            TransferBoundary::IntoPattern,
                            environment,
                        );
                    }
                }
                let mut values = Vec::new();
                let mut arm_environments = Vec::new();
                for arm in arms {
                    let mut arm_environment = environment.clone();
                    for pattern in &arm.patterns {
                        self.add_pattern_bindings(module_id, pattern, &mut arm_environment);
                    }
                    if let Some(guard) = &arm.guard {
                        self.evaluate(module_id, guard, &mut arm_environment);
                    }
                    values.push(self.evaluate(module_id, &arm.body, &mut arm_environment));
                    arm_environments.push(arm_environment);
                }
                self.merge_transfers(environment, &arm_environments);
                values
                    .into_iter()
                    .fold(FlowValue::shareable(), FlowValue::combine)
            }
            ModuleExpression::Empty { .. } => FlowValue::invalid(),
        };
        self.record(module_id, expression, value)
    }

    fn evaluate_block(
        &mut self,
        module_id: ModuleId,
        items: &[ModuleBlockItem],
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let outer_symbols = environment.keys().copied().collect::<HashSet<_>>();
        let mut value = FlowValue::invalid();
        for item in items {
            match item {
                ModuleBlockItem::Binding {
                    name,
                    value: expression,
                    ..
                } => {
                    value = self.evaluate(module_id, expression, environment);
                    if let Some(symbol) = self.typed.resolved.source_symbol(
                        module_id,
                        expression.semantic_id(),
                        ProjectSymbolKind::LocalBinding,
                        &name.text,
                    ) {
                        self.bind_owner(symbol, value.clone(), expression.range(), environment);
                    }
                }
                ModuleBlockItem::Expression(expression) => {
                    value = self.evaluate(module_id, expression, environment);
                }
            }
        }
        environment.retain(|symbol, _| outer_symbols.contains(symbol));
        value
    }

    fn evaluate_function(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        signature: &FunctionSignature,
        body: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let captures = self.capture_symbols(module_id, body, environment);
        let outer_loop_breaks = std::mem::take(&mut self.loop_breaks);
        let mut closure = FlowValue::shareable();
        let mut function_environment = environment.clone();
        for symbol in captures {
            let Some(binding) = environment.get(&symbol).cloned() else {
                continue;
            };
            if binding.requirement {
                continue;
            }
            if let OwnershipState::Transferred { boundary, .. } = binding.ownership {
                self.use_after_transfer(module_id, symbol, body.range(), boundary);
            }
            let mut captured = binding.value.clone();
            if captured.properties.transferable == PropertyDerivation::Never {
                captured.blocked_by = Some(format!(
                    "capture `{}` ({})",
                    self.typed.resolved.symbol(symbol).name,
                    captured
                        .blocked_by
                        .as_deref()
                        .unwrap_or("non-transferable value")
                ));
            }
            if captured.properties.shareable == PropertyDerivation::Never {
                captured.unshared_by = Some(format!(
                    "capture `{}` ({})",
                    self.typed.resolved.symbol(symbol).name,
                    captured
                        .unshared_by
                        .as_deref()
                        .unwrap_or("non-shareable value")
                ));
            }
            closure = closure.combine(captured);
            if binding.value.properties.capability == CapabilityClass::Exclusive {
                self.transfer_binding(
                    symbol,
                    expression.range(),
                    TransferBoundary::ClosureCapture,
                    environment,
                );
            }
            if let Some(function_binding) = function_environment.get_mut(&symbol) {
                function_binding.ownership = OwnershipState::Available;
            }
        }

        if let Some(fact) = self
            .typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            && let Some(contract) = self.typed.callable_contract_for_type(fact.success)
        {
            self.add_signature_bindings(module_id, signature, contract, &mut function_environment);
        }
        self.evaluate(module_id, body, &mut function_environment);
        self.loop_breaks = outer_loop_breaks;
        closure
    }

    fn check_task_transfer(
        &mut self,
        module_id: ModuleId,
        function: StandardFunction,
        argument: &ModuleExpression,
        thunk: &FlowValue,
        environment: &mut FlowEnvironment,
    ) {
        if let Some(provider) = &thunk.nonshareable_provider {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4242",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "Task.{} requires provider `{provider}` to be shareable across tasks",
                    function.name()
                ),
            ));
            return;
        }
        if thunk.properties.transferable == PropertyDerivation::Never {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4240",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "Task.{} cannot transfer {} because {} is parent-local",
                    function.name(),
                    if function == StandardFunction::TaskStart {
                        "thunk"
                    } else {
                        "its thunks"
                    },
                    thunk.blocked_by.as_deref().unwrap_or("a captured value")
                ),
            ));
            return;
        }
        self.transfer_carried(
            thunk,
            argument.range(),
            TransferBoundary::TaskStart,
            environment,
        );
    }

    /// `Channel.send` and `Process.send`: the payload moves to the receiver.
    /// `Process.monitor`: the notice moves into the monitor, which runs it at most once.
    /// `Supervisor.start_handoff`: the argument moves into the new incarnation.
    ///
    /// Ensures: `T4243` when the payload is never transferable; otherwise every
    /// exclusive binding the payload carries is transferred, so a later use is `T4241`.
    fn check_message_transfer(
        &mut self,
        module_id: ModuleId,
        argument: &ModuleExpression,
        payload: &FlowValue,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        let operation = match boundary {
            TransferBoundary::ProcessSend => "Process.send",
            TransferBoundary::ProcessMonitor => "Process.monitor",
            TransferBoundary::StartHandoff => "Supervisor.start_handoff",
            _ => "Channel.send",
        };
        if payload.properties.transferable == PropertyDerivation::Never {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4243",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "{operation} cannot transfer {} because it is parent-local",
                    payload.blocked_by.as_deref().unwrap_or("the payload")
                ),
            ));
            return;
        }
        self.transfer_carried(payload, argument.range(), boundary, environment);
    }

    /// `T4403`: a recorder hands its values out again, to any task, and a script
    /// is a shareable handle any task may take from, so both keep shareable
    /// values only. (testing.md allows a script any transferable value; moving an
    /// exclusive one through a script is not built yet.)
    fn check_test_log_value(
        &mut self,
        module_id: ModuleId,
        operation: &str,
        argument: &ModuleExpression,
        value: &FlowValue,
    ) {
        if value.properties.shareable != PropertyDerivation::Never {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4403",
            self.file_id(module_id),
            argument.range(),
            format!(
                "{operation} keeps its values for any task to read, so they must be shareable, \
                 but {} is not shareable",
                value.unshared_by.as_deref().unwrap_or("this value")
            ),
        ));
    }

    /// `Broadcast.publish`, `Watch.publish` and `Watch.starting_with`: every
    /// subscription gets its own copy of the value, so it must be shareable.
    ///
    /// Ensures: `T4244` when the value is never shareable. Nothing moves, so the
    /// publisher keeps every binding the value reads.
    fn check_published(
        &mut self,
        module_id: ModuleId,
        function: StandardFunction,
        argument: &ModuleExpression,
        value: &FlowValue,
    ) {
        if value.properties.shareable != PropertyDerivation::Never {
            return;
        }
        let operation = match function {
            StandardFunction::BroadcastPublish => "Broadcast.publish",
            StandardFunction::WatchPublish => "Watch.publish",
            _ => "Watch.starting_with",
        };
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4244",
            self.file_id(module_id),
            argument.range(),
            format!(
                "{operation} cannot share {} with every subscriber because it is exclusive",
                value.blocked_by.as_deref().unwrap_or("the value")
            ),
        ));
    }

    /// `T4260` for a `Process.spec` or `Process.handoff_spec` body: every
    /// incarnation, or every handoff start, runs it again, so its
    /// captures and the providers bound to it must be shareable. Records the needs that
    /// flow out of the call, for an enclosing `using` to check.
    fn check_spec_body(
        &mut self,
        module_id: ModuleId,
        function: StandardFunction,
        argument: &ModuleExpression,
        body: &FlowValue,
    ) {
        if let Some(provider) = &body.nonshareable_provider {
            self.nonshareable_spec_provider(module_id, argument.range(), provider);
        } else if body.properties.shareable == PropertyDerivation::Never
            && let Some(unshared_by) = &body.unshared_by
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4260",
                self.file_id(module_id),
                argument.range(),
                format!(
                    "a Process.{} body must be shareable, because {}, but {unshared_by} is \
                     not shareable",
                    function.name(),
                    match function {
                        StandardFunction::ProcessHandoffSpec => "every start of the spec runs it",
                        _ => "every incarnation runs it again",
                    }
                ),
            ));
        }
        if let Some(fact) = self
            .typed
            .expression_facts
            .get(&(module_id, argument.semantic_id()))
            && let Some(contract) = self.typed.callable_contract_for_type(fact.success)
        {
            let needs = &self.typed.needs_rows[contract.needs.raw() as usize].entries;
            self.spec_needs
                .extend(needs.iter().map(|need| need.lexical_name.clone()));
        }
    }

    fn nonshareable_spec_provider(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        provider: &str,
    ) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4260",
            self.file_id(module_id),
            range,
            format!(
                "Process.spec requires provider `{provider}` to be shareable, because every \
                 incarnation of the process uses it"
            ),
        ));
    }

    /// `T4241` at a read of `name` when its value borrows a binding that has moved:
    /// an event outlived the receiver it would receive from.
    fn check_borrows(
        &mut self,
        module_id: ModuleId,
        name: &crate::semantic::Name,
        value: &FlowValue,
        environment: &FlowEnvironment,
    ) {
        for symbol in &value.borrows {
            let Some(boundary) = left_the_task(*symbol, environment) else {
                continue;
            };
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4241",
                self.file_id(module_id),
                name.range,
                format!(
                    "`{}` receives from `{}`, which was transferred {} and cannot be used by \
                     its previous owner",
                    name.text,
                    self.typed.resolved.symbol(*symbol).name,
                    boundary.description(self.typed)
                ),
            ));
        }
    }

    /// Binds `owner` to `value`: the bindings the value carries move into it, so the
    /// new binding is their only owner.
    fn bind_owner(
        &mut self,
        owner: SymbolId,
        mut value: FlowValue,
        at: TextRange,
        environment: &mut FlowEnvironment,
    ) {
        let carries = std::mem::take(&mut value.carries);
        for symbol in carries {
            self.transfer_binding(
                symbol,
                at,
                TransferBoundary::IntoBinding(owner),
                environment,
            );
        }
        environment.insert(
            owner,
            FlowBinding {
                value,
                ownership: OwnershipState::Available,
                requirement: false,
            },
        );
    }

    fn transfer_carried(
        &mut self,
        value: &FlowValue,
        at: TextRange,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        for symbol in &value.carries {
            self.transfer_binding(*symbol, at, boundary, environment);
        }
    }

    fn transfer_binding(
        &mut self,
        symbol: SymbolId,
        at: TextRange,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        let Some(binding) = environment.get_mut(&symbol) else {
            return;
        };
        if binding.ownership == OwnershipState::Available {
            binding.ownership = OwnershipState::Transferred { at, boundary };
        }
    }

    fn capture_symbols(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
    ) -> Vec<SymbolId> {
        let mut captures = HashSet::new();
        self.collect_capture_symbols(module_id, expression, environment, &mut captures);
        let mut captures = captures.into_iter().collect::<Vec<_>>();
        captures.sort_unstable();
        captures
    }

    fn collect_capture_symbols(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
        captures: &mut HashSet<SymbolId>,
    ) {
        if let ModuleExpression::Name { semantic_id, .. } = expression
            && let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id)
            && environment.contains_key(&symbol)
        {
            captures.insert(symbol);
        }
        for child in expression_children(expression) {
            self.collect_capture_symbols(module_id, child, environment, captures);
        }
    }

    fn merge_transfers(&self, environment: &mut FlowEnvironment, branches: &[FlowEnvironment]) {
        for (symbol, binding) in environment {
            if let Some((at, boundary)) =
                branches.iter().find_map(|branch| match branch.get(symbol) {
                    Some(FlowBinding {
                        ownership: OwnershipState::Transferred { at, boundary },
                        ..
                    }) => Some((*at, *boundary)),
                    _ => None,
                })
            {
                binding.ownership = OwnershipState::Transferred { at, boundary };
            }
        }
    }

    fn add_pattern_bindings(
        &self,
        module_id: ModuleId,
        pattern: &Pattern,
        environment: &mut FlowEnvironment,
    ) {
        if let Pattern::Binding {
            semantic_id, name, ..
        }
        | Pattern::Typed {
            semantic_id,
            name: Some(name),
            ..
        }
        | Pattern::Alias {
            semantic_id, name, ..
        } = pattern
            && let Some(symbol) = self.typed.resolved.source_symbol(
                module_id,
                *semantic_id,
                ProjectSymbolKind::PatternBinding,
                &name.text,
            )
            && let Some(type_id) = self
                .typed
                .pattern_binding_types
                .get(&(module_id, *semantic_id))
        {
            environment.insert(
                symbol,
                FlowBinding {
                    value: FlowValue::from_type(self.typed, *type_id),
                    ownership: OwnershipState::Available,
                    requirement: false,
                },
            );
        }
        for child in pattern.children() {
            self.add_pattern_bindings(module_id, child, environment);
        }
    }

    fn expression_type(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<crate::ids::TypeId> {
        self.typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            .map(|fact| fact.success)
    }

    fn type_value(&self, module_id: ModuleId, expression: &ModuleExpression) -> FlowValue {
        self.typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            .map_or_else(FlowValue::invalid, |fact| {
                FlowValue::from_type(self.typed, fact.success)
            })
    }

    fn record(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        value: FlowValue,
    ) -> FlowValue {
        self.expression_properties
            .insert((module_id, expression.semantic_id()), value.properties);
        value
    }

    /// Whether `callee` holds its arguments as they are: a struct or enum
    /// constructor, declared in the program or by a standard module, such as
    /// `Conn`, `Ask.Return` or `Some`, or the prelude's `Success` or `Failure`,
    /// whose value is its argument.
    fn is_constructor(&self, module_id: ModuleId, callee: &ModuleExpression) -> bool {
        self.expression_binding_symbol(module_id, callee)
            .is_some_and(|symbol| {
                let declared = self.typed.resolved.symbol(symbol);
                match declared.kind {
                    ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor => {
                        true
                    }
                    ProjectSymbolKind::BuiltinValue => match self
                        .typed
                        .resolved
                        .standard_symbol(symbol)
                    {
                        Some(StandardSymbol::Constructor(_)) => true,
                        Some(_) => false,
                        None => matches!(declared.name.as_str(), "Success" | "Failure"),
                    },
                    _ => false,
                }
            })
    }

    fn standard_function(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<StandardFunction> {
        let symbol = self.expression_binding_symbol(module_id, expression)?;
        match self.typed.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Function(function)) => Some(function),
            _ => self.typed.resolved.native_operation(symbol),
        }
    }

    /// The binding a field read like `jobs.receiver` starts from, when it is
    /// one this function tracks.
    fn field_root_binding(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
    ) -> Option<SymbolId> {
        let mut root = expression;
        loop {
            match root {
                ModuleExpression::Select { receiver, .. } => root = receiver,
                ModuleExpression::Group { expression, .. } => root = expression,
                ModuleExpression::Name { semantic_id, .. } if !std::ptr::eq(root, expression) => {
                    return self
                        .resolved_value_symbol(module_id, *semantic_id)
                        .filter(|symbol| environment.contains_key(symbol));
                }
                _ => return None,
            }
        }
    }

    fn expression_binding_symbol(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<SymbolId> {
        match expression {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                self.resolved_value_symbol(module_id, *semantic_id)
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_binding_symbol(module_id, expression)
            }
            _ => None,
        }
    }

    fn resolved_value_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
    ) -> Option<SymbolId> {
        match self
            .typed
            .resolved
            .reference_resolution(module_id, semantic_id, Namespace::Value)?
        {
            ReferenceResolution::Resolved(symbol) => Some(symbol),
            _ => None,
        }
    }

    fn use_after_transfer(
        &mut self,
        module_id: ModuleId,
        symbol: SymbolId,
        range: TextRange,
        boundary: TransferBoundary,
    ) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4241",
            self.file_id(module_id),
            range,
            format!(
                "`{}` was transferred {} and cannot be captured again",
                self.typed.resolved.symbol(symbol).name,
                boundary.description(self.typed)
            ),
        ));
    }

    /// Requires: `loop_id` is the loop's `state_id`, which defines its state binding.
    /// Ensures: a capability bound outside the loop that the body may transfer is reported,
    /// because the next pass would use it again. The state binding is exempt: each pass
    /// rebinds it to the body's value. Returns what the loop's `break` values carry.
    fn evaluate_loop(
        &mut self,
        module_id: ModuleId,
        loop_id: SemanticId,
        initial: &ModuleExpression,
        body: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> BTreeSet<SymbolId> {
        let initial_value = self.evaluate(module_id, initial, environment);
        let state = self
            .typed
            .resolved
            .source_symbols(module_id, loop_id)
            .iter()
            .copied()
            .find(|symbol| {
                self.typed.resolved.symbol(*symbol).kind == ProjectSymbolKind::LocalBinding
            });
        // The initial value's bindings move into the state, which only the pass binds.
        if let Some(state) = state {
            let boundary = TransferBoundary::IntoBinding(state);
            self.transfer_carried(&initial_value, initial.range(), boundary, environment);
        }
        let mut pass = environment.clone();
        if let Some(state) = state {
            self.bind_owner(state, initial_value, initial.range(), &mut pass);
        }
        self.loop_breaks.push(BTreeSet::new());
        self.evaluate(module_id, body, &mut pass);
        let breaks = self.loop_breaks.pop().unwrap_or_default();
        let mut reused = environment
            .iter()
            .filter(|(_, before)| before.ownership == OwnershipState::Available)
            .filter_map(|(symbol, _)| match pass.get(symbol)?.ownership {
                OwnershipState::Transferred { at, boundary } => Some((*symbol, at, boundary)),
                OwnershipState::Available => None,
            })
            .collect::<Vec<_>>();
        reused.sort_by_key(|(_, at, _)| at.start());
        for (symbol, at, boundary) in reused {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4250",
                self.file_id(module_id),
                at,
                format!(
                    "`{}` is transferred {} inside a loop; the next pass would use it again",
                    self.typed.resolved.symbol(symbol).name,
                    boundary.description(self.typed)
                ),
            ));
        }
        pass.retain(|symbol, _| environment.contains_key(symbol));
        *environment = pass;
        breaks
    }

    fn file_id(&self, module_id: ModuleId) -> crate::ids::FileId {
        self.typed.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .file_id()
    }
}

/// Whether `expression` reads a binding by name. A field or pattern read out of a
/// binding leaves the binding its owner, so it takes nothing the binding carries.
/// How `symbol`'s value left this task, if it did. A move into another binding
/// of the task follows that binding, and one taken apart by a pattern stays,
/// so an event reading the receiver is still answered by this task's copy.
fn left_the_task(symbol: SymbolId, environment: &FlowEnvironment) -> Option<TransferBoundary> {
    let mut current = symbol;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        match environment.get(&current)?.ownership {
            OwnershipState::Available
            | OwnershipState::Transferred {
                boundary: TransferBoundary::IntoPattern,
                ..
            } => return None,
            OwnershipState::Transferred {
                boundary: TransferBoundary::IntoBinding(owner),
                ..
            } => current = owner,
            OwnershipState::Transferred { boundary, .. } => return Some(boundary),
        }
    }
    None
}

fn reads_a_binding(expression: &ModuleExpression) -> bool {
    match expression {
        ModuleExpression::Name { .. } => true,
        ModuleExpression::Group { expression, .. } => reads_a_binding(expression),
        _ => false,
    }
}

fn expression_children(expression: &ModuleExpression) -> Vec<&ModuleExpression> {
    match expression {
        ModuleExpression::Literal(_)
        | ModuleExpression::Name { .. }
        | ModuleExpression::Empty { .. } => Vec::new(),
        ModuleExpression::Select { receiver, .. }
        | ModuleExpression::Unary {
            operand: receiver, ..
        }
        | ModuleExpression::Group {
            expression: receiver,
            ..
        } => vec![receiver],
        ModuleExpression::Call {
            callee, arguments, ..
        } => std::iter::once(callee.as_ref())
            .chain(arguments.iter().map(|argument| &argument.value))
            .collect(),
        ModuleExpression::List { elements, .. }
        | ModuleExpression::Tuple { elements, .. }
        | ModuleExpression::Sequence {
            expressions: elements,
            ..
        } => elements.iter().collect(),
        ModuleExpression::Record { fields, .. } => {
            fields.iter().map(|field| &field.value).collect()
        }
        ModuleExpression::Update { base, fields, .. } => std::iter::once(base.as_ref())
            .chain(fields.iter().map(|field| &field.value))
            .collect(),
        ModuleExpression::Interpolation { parts, .. } => {
            crate::semantic::interpolated_expressions(parts).collect()
        }
        ModuleExpression::Binary { left, right, .. } => vec![left, right],
        ModuleExpression::Using {
            expression,
            providers,
            ..
        } => std::iter::once(expression.as_ref())
            .chain(providers.iter().map(|provider| &provider.value))
            .collect(),
        ModuleExpression::UsingRecord {
            expression, record, ..
        } => vec![record, expression],
        ModuleExpression::Block { items, .. } => items
            .iter()
            .map(|item| match item {
                ModuleBlockItem::Binding { value, .. } | ModuleBlockItem::Expression(value) => {
                    value
                }
            })
            .collect(),
        ModuleExpression::Function { body, .. } => vec![body],
        ModuleExpression::Loop { initial, body, .. } => vec![initial, body],
        ModuleExpression::Break { value, .. } => value.iter().map(AsRef::as_ref).collect(),
        ModuleExpression::Case { subjects, arms, .. } => subjects
            .iter()
            .chain(
                arms.iter()
                    .flat_map(|arm| arm.guard.iter().chain(std::iter::once(&arm.body))),
            )
            .collect(),
    }
}
