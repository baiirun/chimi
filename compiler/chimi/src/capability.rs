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
//! exclusive binding, or from a part of one, carries that place (`FlowValue::carries`),
//! and the carried places move when the value is bound by `let` or a loop state, taken
//! apart by patterns, crosses a transfer boundary, or is passed to a function whose
//! body transfers that parameter (`TransferSummaries`).

use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{ModuleId, SemanticId, SymbolId, TextRange};
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::{
    BinaryOperator, FunctionSignature, ModuleBlockItem, ModuleDeclaration, ModuleDeclarationKind,
    ModuleExpression, Pattern,
};
use crate::standard::{StandardFunction, StandardSymbol};
use crate::typing::{CapabilityClass, PropertyDerivation, TypedProject, ValueProperties};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) struct CapabilityReport {
    pub(crate) expression_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// One step from a value to a part of it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PathSegment {
    /// A named field: `channel.receiver`, or a constructor pattern's argument.
    Field(String),
    /// A tuple pattern's element, or a constructor pattern's argument whose field
    /// name is unknown.
    Position(usize),
    /// A list pattern's element, counted from the front.
    Front(usize),
    /// A list pattern's element, counted from the back.
    Back(usize),
    /// A list pattern's rest.
    Rest,
}

impl PathSegment {
    /// Whether the parts these steps reach from one value may be the same part. Only
    /// two different steps of the same kind are known to reach different parts.
    fn may_reach_the_same_part(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Field(a), Self::Field(b)) => a == b,
            (Self::Position(a), Self::Position(b))
            | (Self::Front(a), Self::Front(b))
            | (Self::Back(a), Self::Back(b)) => a == b,
            _ => true,
        }
    }
}

/// Whether the parts at two paths from one binding may share a value: one path
/// leads into the other (the empty path is the whole binding).
fn paths_overlap(left: &[PathSegment], right: &[PathSegment]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| left.may_reach_the_same_part(right))
}

/// A binding, or a part of one reached by field reads or patterns. Every exclusive
/// place has one owner: taking it moves it out of its binding.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Place {
    root: SymbolId,
    path: Vec<PathSegment>,
}

impl Place {
    fn whole(root: SymbolId) -> Self {
        Self {
            root,
            path: Vec::new(),
        }
    }

    fn child(&self, segment: PathSegment) -> Self {
        self.joined(std::slice::from_ref(&segment))
    }

    fn joined(&self, path: &[PathSegment]) -> Self {
        let mut place = self.clone();
        place.path.extend_from_slice(path);
        place
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.root == other.root && paths_overlap(&self.path, &other.path)
    }

    fn name(&self, typed: &TypedProject) -> String {
        let mut name = typed.resolved.symbol(self.root).name.clone();
        for segment in &self.path {
            match segment {
                PathSegment::Field(field) => name.push_str(&format!(".{field}")),
                PathSegment::Position(index) => name.push_str(&format!(".{index}")),
                PathSegment::Front(index) => name.push_str(&format!("[{index}]")),
                PathSegment::Back(index) => name.push_str(&format!("[-{}]", index + 1)),
                PathSegment::Rest => name.push_str("[..]"),
            }
        }
        name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Transfer {
    at: TextRange,
    boundary: TransferBoundary,
}

/// For a function or closure, the parts of each parameter, by position, that its
/// body transfers: a call moves those parts of the argument.
type ParameterTransfers = BTreeMap<usize, BTreeSet<Vec<PathSegment>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferBoundary {
    ClosureCapture,
    TaskStart,
    ChannelSend,
    ProcessSend,
    ProcessRemoval,
    SupervisorRemoval,
    /// A `let` or loop state binding took the value that carried it.
    IntoBinding(SymbolId),
    /// The patterns of a `case`, or of a destructuring `let`, took apart a value
    /// that carried it.
    IntoPattern,
    /// A call of a function (the callee's binding, when it has one) whose body
    /// transfers the parameter.
    IntoCall(Option<SymbolId>),
}

impl TransferBoundary {
    fn description(self, typed: &TypedProject) -> String {
        match self {
            Self::ClosureCapture => "into a one-shot closure".to_owned(),
            Self::TaskStart => "by Task.start".to_owned(),
            Self::ChannelSend => "by Channel.send".to_owned(),
            Self::ProcessSend => "by Process.send".to_owned(),
            Self::ProcessRemoval => "by Supervisor.remove_process".to_owned(),
            Self::SupervisorRemoval => "by Supervisor.remove_supervisor".to_owned(),
            Self::IntoBinding(owner) => format!("into `{}`", typed.resolved.symbol(owner).name),
            Self::IntoPattern => "into a pattern".to_owned(),
            Self::IntoCall(Some(callee)) => {
                format!("by a call of `{}`", typed.resolved.symbol(callee).name)
            }
            Self::IntoCall(None) => "by a call".to_owned(),
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
    /// The exclusive places this value may hold: the place itself when the value is
    /// read out of one (a binding, a field of one, or a pattern binding over a part of
    /// one), and those it was built from by constructors, literals, blocks, `case`
    /// arms, `break`, and calls whose result type can hold an argument's type. Taking
    /// the value moves them.
    carries: BTreeSet<Place>,
    /// When the value is a function, what a call of it transfers.
    parameter_transfers: ParameterTransfers,
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
            parameter_transfers: ParameterTransfers::new(),
        }
    }

    fn shareable() -> Self {
        Self {
            properties: ValueProperties::SHAREABLE,
            blocked_by: None,
            unshared_by: None,
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            parameter_transfers: ParameterTransfers::new(),
        }
    }

    fn invalid() -> Self {
        Self {
            properties: ValueProperties::INVALID,
            blocked_by: None,
            unshared_by: None,
            nonshareable_provider: None,
            carries: BTreeSet::new(),
            parameter_transfers: ParameterTransfers::new(),
        }
    }

    fn combine(mut self, mut other: Self) -> Self {
        self.carries.append(&mut other.carries);
        add_parameter_transfers(
            &mut self.parameter_transfers,
            std::mem::take(&mut other.parameter_transfers),
        );
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
            parameter_transfers: self.parameter_transfers,
        }
    }
}

#[derive(Clone, Debug)]
struct FlowBinding {
    value: FlowValue,
    /// The parts of this binding that moved out, by path (the empty path is the
    /// whole binding). Empty for a binding that is `part_of` another.
    moved: Vec<(Vec<PathSegment>, Transfer)>,
    requirement: bool,
    /// For a pattern binding over a part of a binding, that part: the pattern
    /// binding reads it, and taking it moves the part out of its binding.
    part_of: Option<Place>,
}

impl FlowBinding {
    fn owner(value: FlowValue) -> Self {
        Self {
            value,
            moved: Vec::new(),
            requirement: false,
            part_of: None,
        }
    }
}

type FlowEnvironment = HashMap<SymbolId, FlowBinding>;

/// For each named function, the needs (by lexical name) that flow out of a
/// `Process.spec` call in its body and stay open: a `using` around a call of the
/// function snapshots its providers into a spec.
type SpecNeedSummaries = HashMap<SymbolId, BTreeSet<String>>;

/// For each named function whose body transfers a parameter (to a task, a send, a
/// `using` provider, a removal, a closure, a `let` or pattern, or another such call),
/// the parts it transfers. A call of the function moves them out of its arguments.
type TransferSummaries = HashMap<SymbolId, ParameterTransfers>;

#[derive(Default, Eq, PartialEq)]
struct Summaries {
    spec_needs: SpecNeedSummaries,
    transfers: TransferSummaries,
}

/// Ensures: the report of a pass whose spec-need and transfer summaries are a fixed
/// point, so a function's summary is the same whether its callers come before or
/// after it. Both only grow from pass to pass, so the loop ends.
pub(crate) fn check(typed: &TypedProject) -> CapabilityReport {
    let mut summaries = Summaries::default();
    loop {
        let mut checker = CapabilityChecker {
            typed,
            summaries: &summaries,
            expression_properties: HashMap::new(),
            diagnostics: Vec::new(),
            spec_needs: BTreeSet::new(),
            next_summaries: Summaries::default(),
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
    summaries: &'a Summaries,
    expression_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    diagnostics: Vec<Diagnostic>,
    /// Needs that flowed out of `Process.spec` calls evaluated so far in the current
    /// declaration and that no enclosing `using` has bound yet.
    spec_needs: BTreeSet<String>,
    next_summaries: Summaries,
    /// For each enclosing loop of the function being evaluated, innermost last, what
    /// its `break` values carry: the loop's value carries them.
    loop_breaks: Vec<BTreeSet<Place>>,
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
        let mut parameters = Vec::new();
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
            parameters = self.add_signature_bindings(
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
        if let Some(symbol) = function {
            if !self.spec_needs.is_empty() {
                self.next_summaries
                    .spec_needs
                    .insert(symbol, std::mem::take(&mut self.spec_needs));
            }
            let transfers = parameter_transfers(&parameters, &environment);
            if !transfers.is_empty() {
                self.next_summaries.transfers.insert(symbol, transfers);
            }
        }
    }

    /// Returns the parameter symbols by position (`None` where a parameter has no
    /// symbol), for `parameter_transfers`.
    fn add_signature_bindings(
        &self,
        module_id: ModuleId,
        signature: &FunctionSignature,
        contract: &crate::typing::CallableContract,
        environment: &mut FlowEnvironment,
    ) -> Vec<Option<SymbolId>> {
        let mut parameters = Vec::new();
        for (parameter, type_id) in signature.parameters.iter().zip(&contract.inputs) {
            let symbol = self.typed.resolved.source_symbol(
                module_id,
                parameter.semantic_id,
                ProjectSymbolKind::Parameter,
                &parameter.name.text,
            );
            if let Some(symbol) = symbol {
                environment.insert(
                    symbol,
                    FlowBinding::owner(FlowValue::from_type(self.typed, *type_id)),
                );
            }
            parameters.push(symbol);
        }
        for need in &self.typed.needs_rows[contract.needs.raw() as usize].entries {
            environment.insert(
                need.requirement,
                FlowBinding {
                    requirement: true,
                    ..FlowBinding::owner(FlowValue::from_type(self.typed, need.interface))
                },
            );
        }
        parameters
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
                if environment.contains_key(&symbol) {
                    self.read_place(module_id, expression, &name.text, name.range, environment)
                } else if matches!(
                    self.typed.resolved.symbol(symbol).kind,
                    ProjectSymbolKind::Function
                        | ProjectSymbolKind::StructConstructor
                        | ProjectSymbolKind::EnumConstructor
                        | ProjectSymbolKind::BuiltinValue
                ) {
                    let mut value = FlowValue::shareable();
                    if let Some(transfers) = self.summaries.transfers.get(&symbol) {
                        value.parameter_transfers = transfers.clone();
                    }
                    value
                } else {
                    self.type_value(module_id, expression)
                }
            }
            ModuleExpression::Select { range, .. } => {
                if let Some(place) = self.expression_place(module_id, expression, environment) {
                    let text = place.name(self.typed);
                    self.read_place(module_id, expression, &text, *range, environment)
                } else {
                    self.evaluate_select(module_id, expression, environment)
                }
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                let mut callee_value = self.evaluate(module_id, callee, environment);
                let arguments = arguments
                    .iter()
                    .map(|argument| {
                        let value = self.evaluate(module_id, &argument.value, environment);
                        (&argument.value, value)
                    })
                    .collect::<Vec<_>>();
                // A call of a function whose body transfers a parameter moves the
                // argument: what the callee's value says, and for a named function
                // its summary from the previous pass.
                let callee_symbol = self.expression_binding_symbol(module_id, callee);
                let mut transfers = std::mem::take(&mut callee_value.parameter_transfers);
                if let Some(summary) =
                    callee_symbol.and_then(|symbol| self.summaries.transfers.get(&symbol))
                {
                    add_parameter_transfers(&mut transfers, summary.clone());
                }
                self.transfer_parameters(
                    module_id,
                    &transfers,
                    TransferBoundary::IntoCall(callee_symbol),
                    &arguments,
                    environment,
                );
                match self.standard_function(module_id, callee) {
                    Some(StandardFunction::TaskStart) => {
                        if let Some((argument, thunk)) = arguments.first() {
                            self.check_task_transfer(module_id, argument, thunk, environment);
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
                    Some(StandardFunction::ProcessSpec) => {
                        if let Some((argument, body)) = arguments.first() {
                            self.check_spec_body(module_id, argument, body);
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
                            && let Some(needs) = self.summaries.spec_needs.get(&symbol)
                        {
                            self.spec_needs.extend(needs.iter().cloned());
                        }
                    }
                }
                // The callee is not followed: a result that could hold an exclusive
                // value may hold the closure called and every argument whose type
                // occurs in the result's type.
                let mut result = self.type_value(module_id, expression);
                if result.properties.shareable != PropertyDerivation::Always {
                    result.carries = callee_value.carries;
                    let result_type = self.expression_type(module_id, expression);
                    for (argument, value) in arguments {
                        let embeds = match (result_type, self.expression_type(module_id, argument))
                        {
                            (Some(result_type), Some(argument_type)) => {
                                self.typed.type_may_hold(result_type, argument_type)
                            }
                            _ => true,
                        };
                        if embeds {
                            self.hold_part(module_id, &mut result, argument.range(), value);
                        }
                    }
                }
                result
            }
            ModuleExpression::List { elements, .. } | ModuleExpression::Tuple { elements, .. } => {
                let mut value = FlowValue::shareable();
                for element in elements {
                    let part = self.evaluate(module_id, element, environment);
                    value = self.join_part(module_id, value, element.range(), part);
                }
                value
            }
            ModuleExpression::Interpolation { parts, .. } => {
                for part in crate::semantic::interpolated_expressions(parts) {
                    self.evaluate(module_id, part, environment);
                }
                FlowValue::shareable()
            }
            ModuleExpression::Record { fields, .. } => {
                let mut value = FlowValue::shareable();
                for field in fields {
                    let part = self.evaluate(module_id, &field.value, environment);
                    value = self.join_part(module_id, value, field.value.range(), part);
                }
                value
            }
            ModuleExpression::Update { base, fields, .. } => {
                let mut value = self.evaluate(module_id, base, environment);
                for field in fields {
                    let part = self.evaluate(module_id, &field.value, environment);
                    value = self.join_part(module_id, value, field.value.range(), part);
                }
                value
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
                // A subject read out of a place leaves its parts there: a pattern
                // binding reads its part of the place, and taking it moves the part.
                // A subject built here is taken apart by the patterns, as by a
                // destructuring `let`: they take what it carries.
                let mut subject_places = Vec::new();
                for subject in subjects {
                    let value = self.evaluate(module_id, subject, environment);
                    let place = self.expression_place(module_id, subject, environment);
                    if place.is_none() {
                        self.transfer_carried(
                            &value,
                            subject.range(),
                            TransferBoundary::IntoPattern,
                            environment,
                        );
                    }
                    subject_places.push(place);
                }
                let mut values = Vec::new();
                let mut arm_environments = Vec::new();
                for arm in arms {
                    let mut arm_environment = environment.clone();
                    for (index, pattern) in arm.patterns.iter().enumerate() {
                        let place = subject_places.get(index).cloned().flatten();
                        self.add_pattern_bindings(module_id, pattern, place, &mut arm_environment);
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
        let outer_symbols = environment.keys().copied().collect::<BTreeSet<_>>();
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
        let captures = self.capture_places(module_id, body, environment);
        let outer_loop_breaks = std::mem::take(&mut self.loop_breaks);
        let mut closure = FlowValue::shareable();
        let mut function_environment = environment.clone();
        for (place, read) in captures {
            let name = place.name(self.typed);
            if let Some(transfer) = self.moved_part(environment, &place) {
                self.use_after_transfer(module_id, &name, body.range(), transfer.boundary);
            }
            let mut captured = self.place_value(module_id, read, environment);
            captured.parameter_transfers.clear();
            if captured.properties.transferable == PropertyDerivation::Never {
                captured.blocked_by = Some(format!(
                    "capture `{name}` ({})",
                    captured
                        .blocked_by
                        .as_deref()
                        .unwrap_or("non-transferable value")
                ));
            }
            if captured.properties.shareable == PropertyDerivation::Never {
                captured.unshared_by = Some(format!(
                    "capture `{name}` ({})",
                    captured
                        .unshared_by
                        .as_deref()
                        .unwrap_or("non-shareable value")
                ));
            }
            if captured.properties.capability == CapabilityClass::Exclusive {
                self.transfer_place(
                    &place,
                    expression.range(),
                    TransferBoundary::ClosureCapture,
                    environment,
                );
            }
            closure = closure.combine(captured);
            // Inside the closure the capture is its own, whatever happened to it before.
            if let Some(binding) = function_environment.get_mut(&place.root) {
                binding
                    .moved
                    .retain(|(path, _)| !paths_overlap(path, &place.path));
            }
        }

        let mut parameters = Vec::new();
        if let Some(fact) = self
            .typed
            .expression_facts
            .get(&(module_id, expression.semantic_id()))
            && let Some(contract) = self.typed.callable_contract_for_type(fact.success)
        {
            parameters = self.add_signature_bindings(
                module_id,
                signature,
                contract,
                &mut function_environment,
            );
        }
        self.evaluate(module_id, body, &mut function_environment);
        self.loop_breaks = outer_loop_breaks;
        closure.parameter_transfers = parameter_transfers(&parameters, &function_environment);
        closure
    }

    fn check_task_transfer(
        &mut self,
        module_id: ModuleId,
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
                format!("Task.start requires provider `{provider}` to be shareable across tasks"),
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
                    "Task.start cannot transfer thunk because {} is parent-local",
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

    /// `T4260` for a `Process.spec` body: every incarnation runs it again, so its
    /// captures and the providers bound to it must be shareable. Records the needs that
    /// flow out of the call, for an enclosing `using` to check.
    fn check_spec_body(
        &mut self,
        module_id: ModuleId,
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
                    "a Process.spec body must be shareable, because every incarnation runs it \
                     again, but {unshared_by} is not shareable"
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

    /// Binds `owner` to `value`: the places the value carries move into it, so the
    /// new binding is their only owner.
    fn bind_owner(
        &mut self,
        owner: SymbolId,
        mut value: FlowValue,
        at: TextRange,
        environment: &mut FlowEnvironment,
    ) {
        let carries = std::mem::take(&mut value.carries);
        for place in carries {
            self.transfer_place(
                &place,
                at,
                TransferBoundary::IntoBinding(owner),
                environment,
            );
        }
        environment.insert(owner, FlowBinding::owner(value));
    }

    fn transfer_carried(
        &mut self,
        value: &FlowValue,
        at: TextRange,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        for place in &value.carries {
            self.transfer_place(place, at, boundary, environment);
        }
    }

    /// Moves `place` out of its binding, unless a part of it that overlaps has moved
    /// already (the use that took it again was reported where it read the place).
    fn transfer_place(
        &mut self,
        place: &Place,
        at: TextRange,
        boundary: TransferBoundary,
        environment: &mut FlowEnvironment,
    ) {
        let Some(binding) = environment.get_mut(&place.root) else {
            return;
        };
        if !binding
            .moved
            .iter()
            .any(|(path, _)| paths_overlap(path, &place.path))
        {
            binding
                .moved
                .push((place.path.clone(), Transfer { at, boundary }));
        }
    }

    /// The move that took `place`, or a part of it, or a place it is part of.
    fn moved_part(&self, environment: &FlowEnvironment, place: &Place) -> Option<Transfer> {
        environment
            .get(&place.root)?
            .moved
            .iter()
            .find_map(|(path, transfer)| paths_overlap(path, &place.path).then_some(*transfer))
    }

    /// The place `expression` reads: a binding by name (a pattern binding over a part
    /// of a binding reads that part), or a field of a place. `None` for anything else,
    /// and for a need, which no value takes.
    fn expression_place(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
    ) -> Option<Place> {
        match expression {
            ModuleExpression::Name { semantic_id, .. } => {
                let symbol = self.resolved_value_symbol(module_id, *semantic_id)?;
                let binding = environment.get(&symbol)?;
                if binding.requirement {
                    return None;
                }
                Some(
                    binding
                        .part_of
                        .clone()
                        .unwrap_or_else(|| Place::whole(symbol)),
                )
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_place(module_id, expression, environment)
            }
            ModuleExpression::Select {
                receiver, member, ..
            } => Some(
                self.expression_place(module_id, receiver, environment)?
                    .child(PathSegment::Field(member.text.clone())),
            ),
            _ => None,
        }
    }

    /// Reads the place `expression` names (`T4241` when it, or a part of it, moved)
    /// or the need it names. The value carries the place when it is exclusive.
    fn read_place(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        text: &str,
        range: TextRange,
        environment: &FlowEnvironment,
    ) -> FlowValue {
        let place = self.expression_place(module_id, expression, environment);
        if let Some(place) = &place
            && let Some(transfer) = self.moved_part(environment, place)
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4241",
                self.file_id(module_id),
                range,
                format!(
                    "`{text}` was transferred {} and cannot be used by its previous owner",
                    transfer.boundary.description(self.typed)
                ),
            ));
        }
        let mut value = self.place_value(module_id, expression, environment);
        if let Some(place) = place
            && value.properties.capability == CapabilityClass::Exclusive
        {
            value.carries.insert(place);
        }
        value
    }

    /// The value at a place or need, without checking or taking it: a binding's
    /// value, or a field's, which is the value it was read out of when the field's
    /// type alone does not decide (a function field).
    fn place_value(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &FlowEnvironment,
    ) -> FlowValue {
        let mut value = match expression {
            ModuleExpression::Name { semantic_id, .. } => self
                .resolved_value_symbol(module_id, *semantic_id)
                .and_then(|symbol| environment.get(&symbol))
                .map_or_else(
                    || self.type_value(module_id, expression),
                    |binding| binding.value.clone(),
                ),
            ModuleExpression::Group { expression, .. } => {
                self.place_value(module_id, expression, environment)
            }
            ModuleExpression::Select { receiver, .. } => {
                let receiver = self.place_value(module_id, receiver, environment);
                let value = self.type_value(module_id, expression);
                if value.properties.transferable == PropertyDerivation::ValueDependent {
                    receiver
                } else {
                    value
                }
            }
            _ => self.type_value(module_id, expression),
        };
        value.carries.clear();
        self.record(module_id, expression, value)
    }

    /// A field read out of a value that is not a place carries what that value
    /// carries, as `boxed(r).inner` holds `r`.
    fn evaluate_select(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> FlowValue {
        let ModuleExpression::Select { receiver, .. } = expression else {
            return self.type_value(module_id, expression);
        };
        let receiver = self.evaluate(module_id, receiver, environment);
        let mut value = self.type_value(module_id, expression);
        let carries = if value.properties.shareable == PropertyDerivation::Always {
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

    /// Adds `part` to the value built from `whole`. `T4241` at `at` when the part
    /// carries a place that `whole` already holds: the built value would hold one
    /// exclusive value twice.
    fn join_part(
        &mut self,
        module_id: ModuleId,
        whole: FlowValue,
        at: TextRange,
        part: FlowValue,
    ) -> FlowValue {
        self.check_held_once(module_id, &whole.carries, at, &part.carries);
        whole.combine(part)
    }

    /// `join_part` for a call's result, whose properties are its type's.
    fn hold_part(
        &mut self,
        module_id: ModuleId,
        result: &mut FlowValue,
        at: TextRange,
        part: FlowValue,
    ) {
        self.check_held_once(module_id, &result.carries, at, &part.carries);
        result.carries.extend(part.carries);
    }

    fn check_held_once(
        &mut self,
        module_id: ModuleId,
        held: &BTreeSet<Place>,
        at: TextRange,
        part: &BTreeSet<Place>,
    ) {
        if let Some(place) = part
            .iter()
            .find(|place| held.iter().any(|held| held.overlaps(place)))
        {
            self.taken_twice(module_id, place, at);
        }
    }

    fn taken_twice(&mut self, module_id: ModuleId, place: &Place, at: TextRange) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4241",
            self.file_id(module_id),
            at,
            format!(
                "`{}` is taken twice here; an exclusive value has one owner",
                place.name(self.typed)
            ),
        ));
    }

    /// Moves, out of each argument, the parts of it that the callee transfers: the
    /// parts of the place an argument reads, or what a built argument carries.
    /// `T4241` when one call takes a place twice.
    fn transfer_parameters(
        &mut self,
        module_id: ModuleId,
        transfers: &ParameterTransfers,
        boundary: TransferBoundary,
        arguments: &[(&ModuleExpression, FlowValue)],
        environment: &mut FlowEnvironment,
    ) {
        let mut taken = Vec::<Place>::new();
        for (index, paths) in transfers {
            let Some((argument, value)) = arguments.get(*index) else {
                continue;
            };
            let places = match self.expression_place(module_id, argument, environment) {
                Some(place) => paths.iter().map(|path| place.joined(path)).collect(),
                None => value.carries.iter().cloned().collect::<Vec<_>>(),
            };
            for place in places {
                if taken.iter().any(|taken| taken.overlaps(&place)) {
                    self.taken_twice(module_id, &place, argument.range());
                    continue;
                }
                self.transfer_place(&place, argument.range(), boundary, environment);
                taken.push(place);
            }
        }
    }

    /// Collects the places a closure body reads from `environment`, each with the
    /// expression that reads it. A place inside another captured place is left out.
    fn capture_places<'e>(
        &self,
        module_id: ModuleId,
        expression: &'e ModuleExpression,
        environment: &FlowEnvironment,
    ) -> Vec<(Place, &'e ModuleExpression)> {
        let mut captures = BTreeMap::new();
        self.collect_capture_places(module_id, expression, environment, &mut captures);
        let mut kept = Vec::<(Place, &ModuleExpression)>::new();
        for (place, read) in captures {
            if !kept
                .iter()
                .any(|(outer, _)| outer.root == place.root && place.path.starts_with(&outer.path))
            {
                kept.push((place, read));
            }
        }
        kept
    }

    fn collect_capture_places<'e>(
        &self,
        module_id: ModuleId,
        expression: &'e ModuleExpression,
        environment: &FlowEnvironment,
        captures: &mut BTreeMap<Place, &'e ModuleExpression>,
    ) {
        if let Some(place) = self.expression_place(module_id, expression, environment) {
            captures.entry(place).or_insert(expression);
            return;
        }
        for child in expression_children(expression) {
            self.collect_capture_places(module_id, child, environment, captures);
        }
    }

    fn merge_transfers(&self, environment: &mut FlowEnvironment, branches: &[FlowEnvironment]) {
        for (symbol, binding) in environment {
            for branch in branches {
                let Some(branch_binding) = branch.get(symbol) else {
                    continue;
                };
                for (path, transfer) in &branch_binding.moved {
                    if !binding
                        .moved
                        .iter()
                        .any(|(moved, _)| paths_overlap(moved, path))
                    {
                        binding.moved.push((path.clone(), *transfer));
                    }
                }
            }
        }
    }

    /// Binds the names in `pattern`, which matches the value at `place` when the
    /// subject was read out of one: each pattern binding is then part of that place.
    fn add_pattern_bindings(
        &self,
        module_id: ModuleId,
        pattern: &Pattern,
        place: Option<Place>,
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
                    part_of: place.clone(),
                    ..FlowBinding::owner(FlowValue::from_type(self.typed, *type_id))
                },
            );
        }
        let part = |segment| place.as_ref().map(|place| place.child(segment));
        match pattern {
            Pattern::Alias { pattern, .. } => {
                self.add_pattern_bindings(module_id, pattern, place.clone(), environment);
            }
            Pattern::Constructor { arguments, .. } => {
                let fields = self
                    .resolved_value_symbol(module_id, pattern.semantic_id())
                    .and_then(|constructor| self.typed.fields.get(&constructor));
                for (index, argument) in arguments.iter().enumerate() {
                    let segment = fields
                        .and_then(|fields| fields.get(index))
                        .map_or(PathSegment::Position(index), |field| {
                            PathSegment::Field(field.name.clone())
                        });
                    self.add_pattern_bindings(module_id, argument, part(segment), environment);
                }
            }
            Pattern::Tuple { elements, .. } => {
                for (index, element) in elements.iter().enumerate() {
                    let segment = PathSegment::Position(index);
                    self.add_pattern_bindings(module_id, element, part(segment), environment);
                }
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => {
                for (index, element) in prefix.iter().enumerate() {
                    let segment = PathSegment::Front(index);
                    self.add_pattern_bindings(module_id, element, part(segment), environment);
                }
                if let Some(rest) = rest {
                    let segment = PathSegment::Rest;
                    self.add_pattern_bindings(module_id, rest, part(segment), environment);
                }
                for (index, element) in suffix.iter().rev().enumerate() {
                    let segment = PathSegment::Back(index);
                    self.add_pattern_bindings(module_id, element, part(segment), environment);
                }
            }
            Pattern::Wildcard { .. }
            | Pattern::Binding { .. }
            | Pattern::Literal { .. }
            | Pattern::Typed { .. } => {}
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

    fn standard_function(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<StandardFunction> {
        let symbol = self.expression_binding_symbol(module_id, expression)?;
        match self.typed.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Function(function)) => Some(function),
            _ => None,
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
        name: &str,
        range: TextRange,
        boundary: TransferBoundary,
    ) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            "T4241",
            self.file_id(module_id),
            range,
            format!(
                "`{name}` was transferred {} and cannot be captured again",
                boundary.description(self.typed)
            ),
        ));
    }

    /// Requires: `loop_id` is the loop's `state_id`, which defines its state binding.
    /// Ensures: a capability bound outside the loop, or a part of one, that the body may
    /// transfer is reported, because the next pass would use it again. The state binding is exempt: each pass
    /// rebinds it to the body's value. Returns what the loop's `break` values carry.
    fn evaluate_loop(
        &mut self,
        module_id: ModuleId,
        loop_id: SemanticId,
        initial: &ModuleExpression,
        body: &ModuleExpression,
        environment: &mut FlowEnvironment,
    ) -> BTreeSet<Place> {
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
            .filter_map(|(symbol, before)| Some((*symbol, before, pass.get(symbol)?)))
            .flat_map(|(symbol, before, after)| {
                after
                    .moved
                    .iter()
                    .filter(|(path, _)| !before.moved.iter().any(|(moved, _)| moved == path))
                    .map(move |(path, transfer)| {
                        (
                            Place {
                                root: symbol,
                                path: path.clone(),
                            },
                            *transfer,
                        )
                    })
            })
            .collect::<Vec<_>>();
        reused.sort_by_key(|(_, transfer)| transfer.at.start());
        for (place, transfer) in reused {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4250",
                self.file_id(module_id),
                transfer.at,
                format!(
                    "`{}` is transferred {} inside a loop; the next pass would use it again",
                    place.name(self.typed),
                    transfer.boundary.description(self.typed)
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

/// The parts of each parameter that moved by the end of a body, by position.
fn parameter_transfers(
    parameters: &[Option<SymbolId>],
    environment: &FlowEnvironment,
) -> ParameterTransfers {
    parameters
        .iter()
        .enumerate()
        .filter_map(|(index, symbol)| {
            let binding = environment.get(&(*symbol)?)?;
            let paths = binding
                .moved
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<BTreeSet<_>>();
            (!paths.is_empty()).then_some((index, paths))
        })
        .collect()
}

fn add_parameter_transfers(into: &mut ParameterTransfers, transfers: ParameterTransfers) {
    for (index, mut paths) in transfers {
        into.entry(index).or_default().append(&mut paths);
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
