//! `std/process`, `std/supervisor`, and `std/clock`: their callable contracts and the
//! static process rules `T4261`–`T4264`
//! (`docs/specification/process-and-supervisor-static-contracts.md`).
//!
//! The contracts are the static contract's table exactly. The rules run after the
//! body fixed point, over settled facts, as `durations::check` does:
//!
//! - `T4261`: the message type of a `process name` declaration or of a `Process.spec`
//!   body is not transferable.
//! - `T4262`: the second argument of `Process.register` is not a direct reference to
//!   a `process name` declaration.
//! - `T4263`: a literal argument out of range. "Literal" is the contract's word: an
//!   integer literal (with an optional minus), or `Duration.zero` or a `Duration`
//!   constructor applied to an integer literal. Constants and computed values are
//!   checked at run time, by the call that receives them.
//! - `T4264`: one `Supervisor.policy` members list literal registers a `process name`
//!   twice. Elements are followed through `Supervisor.process`, `Process.register`,
//!   and module constants, never through function calls.
//! - `T4240`: a `Process.spec` body whose failure type is not transferable. Ending
//!   with `Failure(E)` returns `E` beyond the body, so a task-local value such as the
//!   inbox would escape its owner.

use super::durations::{ConstantEvaluator, Folded};
use super::{
    CallableContract, CanonicalType, FailureRow, FailureVariantArguments, Need, NeedsRow,
    ProjectTypeChecker, PropertyDerivation, TypedProject,
};
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FailureRowId, ModuleId, NeedsRowId, SymbolId, TextRange, TypeId};
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::{
    CallArgument, Literal, ModuleBlockItem, ModuleDeclarationKind, ModuleExpression, UnaryOperator,
    interpolated_expressions,
};
use crate::standard::{
    DurationUnitRole, StandardConstant, StandardConstructor, StandardFunction, StandardGeneric,
    StandardSymbol, StandardType,
};
use std::collections::{HashMap, HashSet};

impl ProjectTypeChecker {
    /// Registers the contracts of every `std/process`, `std/supervisor`, and
    /// `std/clock` function.
    pub(super) fn index_process_contracts(&mut self) {
        use StandardFunction as F;
        use StandardGeneric as G;
        let int = self.builtin_type("Int");
        let void = self.builtin_type("Void");
        let duration = self.standard_named_type(StandardType::Duration, Vec::new());
        let pure = (FailureRowId::new(0), NeedsRowId::new(0));
        let named =
            |checker: &mut Self, kind, arguments| checker.standard_named_type(kind, arguments);
        let member = named(self, StandardType::SupervisorMember, Vec::new());
        let policy = named(self, StandardType::SupervisorPolicy, Vec::new());
        let restarts = named(self, StandardType::SupervisorRestarts, Vec::new());
        let restart = named(self, StandardType::SupervisorRestart, Vec::new());
        let running = named(self, StandardType::SupervisorRunning, Vec::new());
        let managed_supervisor = named(self, StandardType::SupervisorManagedSupervisor, Vec::new());
        let members = self.builtin_type_with_arguments("List", vec![member]);
        let start_failures = self.standard_failure_row(&[
            StandardConstructor::StartCapacityReached,
            StandardConstructor::StartNameTaken,
            StandardConstructor::StartStopped,
        ]);
        let name_taken = self.standard_failure_row(&[StandardConstructor::StartNameTaken]);

        // Process.spec: fn<M, E, R>(fn(Inbox<M>) -> Void errors E needs R, Int, Duration)
        //   -> Spec<M, E> needs R
        let message = self.standard_generic_type(G::ProcessSpecMessage);
        let failures = self.standard_generic_type(G::ProcessSpecFailures);
        let body_failures = self.parameter_failure_row(G::ProcessSpecFailures);
        let needs = self.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: vec![self.resolved.standard_generic(G::ProcessSpecNeeds)],
        });
        let inbox = named(self, StandardType::ProcessInbox, vec![message]);
        let body = self.intern_type(CanonicalType::Function(CallableContract {
            inputs: vec![inbox],
            success: void,
            failures: body_failures,
            needs,
        }));
        let spec = named(self, StandardType::ProcessSpec, vec![message, failures]);
        let mut contracts = vec![(
            F::ProcessSpec,
            CallableContract {
                inputs: vec![body, int, duration],
                success: spec,
                failures: pure.0,
                needs,
            },
        )];

        let message = self.standard_generic_type(G::ProcessRegisterMessage);
        let failures = self.standard_generic_type(G::ProcessRegisterFailures);
        let spec = named(self, StandardType::ProcessSpec, vec![message, failures]);
        let address = named(self, StandardType::ProcessAddress, vec![message]);
        contracts.push((F::ProcessRegister, pure_contract(vec![spec, address], spec)));

        let message = self.standard_generic_type(G::ProcessReceiveMessage);
        let inbox = named(self, StandardType::ProcessInbox, vec![message]);
        contracts.push((F::ProcessReceive, pure_contract(vec![inbox], message)));

        let message = self.standard_generic_type(G::ProcessSendMessage);
        let address = named(self, StandardType::ProcessAddress, vec![message]);
        let send_variants = [
            StandardConstructor::ProcessSendFull,
            StandardConstructor::ProcessSendUnavailable,
        ]
        .map(|constructor| self.standard_constructor_symbol(constructor));
        let send_failures = self.intern_failure_row(FailureRow {
            variants: send_variants.to_vec(),
            variant_arguments: send_variants
                .into_iter()
                .map(|variant| FailureVariantArguments {
                    variant,
                    arguments: vec![message],
                })
                .collect(),
            parameters: Vec::new(),
            exclusions: Vec::new(),
        });
        contracts.push((
            F::ProcessSend,
            CallableContract {
                inputs: vec![address, message],
                success: void,
                failures: send_failures,
                needs: pure.1,
            },
        ));

        let message = self.standard_generic_type(G::SupervisorProcessMessage);
        let failures = self.standard_generic_type(G::SupervisorProcessFailures);
        let spec = named(self, StandardType::ProcessSpec, vec![message, failures]);
        contracts.push((
            F::SupervisorProcess,
            pure_contract(vec![spec, restart], member),
        ));
        contracts.push((
            F::SupervisorSupervisor,
            pure_contract(vec![policy, restart], member),
        ));
        contracts.push((
            F::SupervisorPolicy,
            pure_contract(vec![members, int, restarts], policy),
        ));
        contracts.push((
            F::SupervisorRestarts,
            pure_contract(vec![int, duration], restarts),
        ));
        contracts.push((
            F::SupervisorStart,
            CallableContract {
                inputs: vec![policy],
                success: running,
                failures: name_taken,
                needs: pure.1,
            },
        ));

        let message = self.standard_generic_type(G::SupervisorStartProcessMessage);
        let failures = self.standard_generic_type(G::SupervisorStartProcessFailures);
        let spec = named(self, StandardType::ProcessSpec, vec![message, failures]);
        let managed = named(self, StandardType::SupervisorManagedProcess, vec![message]);
        contracts.push((
            F::SupervisorStartProcess,
            CallableContract {
                inputs: vec![running, spec, restart],
                success: managed,
                failures: start_failures,
                needs: pure.1,
            },
        ));
        contracts.push((
            F::SupervisorStartSupervisor,
            CallableContract {
                inputs: vec![running, policy, restart],
                success: managed_supervisor,
                failures: start_failures,
                needs: pure.1,
            },
        ));

        let message = self.standard_generic_type(G::SupervisorAddressMessage);
        let managed = named(self, StandardType::SupervisorManagedProcess, vec![message]);
        let address = named(self, StandardType::ProcessAddress, vec![message]);
        contracts.push((F::SupervisorAddress, pure_contract(vec![managed], address)));

        let message = self.standard_generic_type(G::SupervisorRemoveMessage);
        let managed = named(self, StandardType::SupervisorManagedProcess, vec![message]);
        contracts.push((
            F::SupervisorRemoveProcess,
            pure_contract(vec![running, managed], void),
        ));
        contracts.push((
            F::SupervisorRemoveSupervisor,
            pure_contract(vec![running, managed_supervisor], void),
        ));
        contracts.push((F::SupervisorStop, pure_contract(vec![running], void)));
        contracts.push((F::SupervisorWait, pure_contract(vec![running], void)));

        // Clock.sleep: fn(Duration) -> Void needs { clock: Clock }. A standard function
        // has no body, so its own symbol stands in for the requirement binding.
        let clock = named(self, StandardType::Clock, Vec::new());
        let sleep = self.standard_function_symbol(F::ClockSleep);
        let clock_need = self.intern_needs_row(NeedsRow {
            entries: vec![Need {
                requirement: sleep,
                lexical_name: "clock".to_owned(),
                interface: clock,
                explained: false,
            }],
            parameters: Vec::new(),
        });
        contracts.push((
            F::ClockSleep,
            CallableContract {
                inputs: vec![duration],
                success: void,
                failures: pure.0,
                needs: clock_need,
            },
        ));

        for (function, contract) in contracts {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(symbol, contract);
        }
    }

    /// The shapes of the `std/process` and `std/supervisor` failure and restart
    /// variants, as `(field types, the type built)`.
    pub(super) fn process_constructor_shape(
        &mut self,
        constructor: StandardConstructor,
    ) -> (Vec<TypeId>, TypeId) {
        match constructor {
            StandardConstructor::ProcessSendFull | StandardConstructor::ProcessSendUnavailable => {
                let message = self.standard_generic_type(StandardGeneric::ProcessSendMessage);
                (
                    vec![message],
                    self.standard_named_type(StandardType::ProcessSendError, vec![message]),
                )
            }
            StandardConstructor::RestartNever | StandardConstructor::RestartOnDefect => (
                Vec::new(),
                self.standard_named_type(StandardType::SupervisorRestart, Vec::new()),
            ),
            StandardConstructor::StartNameTaken => (
                vec![self.builtin_type("Text")],
                self.standard_named_type(StandardType::SupervisorStartError, Vec::new()),
            ),
            StandardConstructor::StartCapacityReached | StandardConstructor::StartStopped => (
                Vec::new(),
                self.standard_named_type(StandardType::SupervisorStartError, Vec::new()),
            ),
            other => unreachable!("{other:?} is not a process or supervisor constructor"),
        }
    }

    /// The type of a `process name <identifier>: <Message>` declaration.
    pub(super) fn process_name_type(&mut self, message: TypeId) -> TypeId {
        self.standard_named_type(StandardType::ProcessAddress, vec![message])
    }

    fn parameter_failure_row(&mut self, generic: StandardGeneric) -> FailureRowId {
        self.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: vec![self.resolved.standard_generic(generic)],
            exclusions: Vec::new(),
        })
    }
}

fn pure_contract(inputs: Vec<TypeId>, success: TypeId) -> CallableContract {
    CallableContract {
        inputs,
        success,
        failures: FailureRowId::new(0),
        needs: NeedsRowId::new(0),
    }
}

/// `T4261`–`T4264` for every `process name` declaration and every process call in
/// the bodies and constant initializers of `modules`.
///
/// Ensures: at most one diagnostic per offending argument or declaration, each at the
/// argument (or declared name) that is wrong; a non-literal argument draws no `T4263`.
pub(super) fn check(typed: &TypedProject, modules: &HashSet<ModuleId>) -> Vec<Diagnostic> {
    let mut checker = Checker {
        typed,
        evaluator: ConstantEvaluator::new(typed),
        diagnostics: Vec::new(),
    };
    for module in &typed.resolved.semantic.modules {
        if !modules.contains(&module.id) {
            continue;
        }
        for declaration in &module.declarations {
            if declaration.kind == ModuleDeclarationKind::ProcessName
                && let Some(address) = typed
                    .declaration_types
                    .get(&(module.id, declaration.semantic_id))
                && let Some(message) = checker.type_argument(*address, StandardType::ProcessAddress)
            {
                checker.check_transferable(
                    module.id,
                    declaration.name.range,
                    message,
                    &format!("process name `{}`", declaration.name.text),
                );
            }
            if let Some(body) = &declaration.body {
                checker.visit(module.id, body);
            }
        }
    }
    checker.diagnostics
}

struct Checker<'typed> {
    typed: &'typed TypedProject,
    evaluator: ConstantEvaluator<'typed>,
    diagnostics: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn visit(&mut self, module_id: ModuleId, expression: &ModuleExpression) {
        let mut visit = |child: &ModuleExpression| self.visit(module_id, child);
        match expression {
            ModuleExpression::Literal(_)
            | ModuleExpression::Name { .. }
            | ModuleExpression::Empty { .. } => {}
            ModuleExpression::Select { receiver, .. } => visit(receiver),
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                visit(callee);
                for argument in arguments {
                    visit(&argument.value);
                }
                self.check_call(module_id, expression, callee, arguments);
            }
            ModuleExpression::List { elements, .. }
            | ModuleExpression::Tuple { elements, .. }
            | ModuleExpression::Sequence {
                expressions: elements,
                ..
            } => elements.iter().for_each(visit),
            ModuleExpression::Record { fields, .. } => {
                fields.iter().for_each(|field| visit(&field.value));
            }
            ModuleExpression::Update { base, fields, .. } => {
                visit(base);
                fields.iter().for_each(|field| visit(&field.value));
            }
            ModuleExpression::Unary { operand, .. } => visit(operand),
            ModuleExpression::Binary { left, right, .. } => {
                visit(left);
                visit(right);
            }
            ModuleExpression::Group { expression, .. }
            | ModuleExpression::Function {
                body: expression, ..
            } => visit(expression),
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => {
                visit(expression);
                providers.iter().for_each(|provider| visit(&provider.value));
            }
            ModuleExpression::Block { items, .. } => {
                for item in items {
                    match item {
                        ModuleBlockItem::Binding { value, .. }
                        | ModuleBlockItem::Expression(value) => visit(value),
                    }
                }
            }
            ModuleExpression::Case { subjects, arms, .. } => {
                subjects.iter().for_each(&mut visit);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        visit(guard);
                    }
                    visit(&arm.body);
                }
            }
            ModuleExpression::Loop { initial, body, .. } => {
                visit(initial);
                visit(body);
            }
            ModuleExpression::Break { value, .. } => {
                if let Some(value) = value {
                    visit(value);
                }
            }
            ModuleExpression::Interpolation { parts, .. } => {
                interpolated_expressions(parts).for_each(visit);
            }
        }
    }

    fn check_call(
        &mut self,
        module_id: ModuleId,
        call: &ModuleExpression,
        callee: &ModuleExpression,
        arguments: &[CallArgument],
    ) {
        let Some(function) = self.standard_function(module_id, callee) else {
            return;
        };
        let argument = |index: usize| arguments.get(index).map(|argument| &argument.value);
        match function {
            StandardFunction::ProcessSpec => {
                if let (Some(body), Some(fact)) = (
                    argument(0),
                    self.typed
                        .expression_facts
                        .get(&(module_id, call.semantic_id())),
                ) && let Some(message) =
                    self.type_argument(fact.success, StandardType::ProcessSpec)
                {
                    self.check_transferable(module_id, body.range(), message, "this process body");
                }
                if let (Some(body), Some(fact)) = (
                    argument(0),
                    self.typed
                        .expression_facts
                        .get(&(module_id, call.semantic_id())),
                ) && let Some(failure) = self
                    .type_arguments(fact.success, StandardType::ProcessSpec)
                    .get(1)
                    .copied()
                {
                    self.check_failure_stays_local(module_id, body.range(), failure);
                }
                if let Some(capacity) = argument(1)
                    && let Some(value) = integer_literal(capacity)
                    && value < 1
                {
                    self.range_error(
                        module_id,
                        capacity,
                        format!("a mailbox capacity must be at least 1, but this one is {value}"),
                    );
                }
                if let Some(within) = argument(2)
                    && let Some(value) = self.duration_literal(module_id, within)
                    && value < 0
                {
                    self.range_error(
                        module_id,
                        within,
                        format!(
                            "`shutdown_within` must be at least zero, but this one is {}",
                            crate::duration::to_text(value)
                        ),
                    );
                }
            }
            StandardFunction::ProcessRegister => {
                if let Some(name) = argument(1)
                    && !self.reports_its_own_resolution(module_id, name)
                    && self.process_name_reference(module_id, name).is_none()
                {
                    self.diagnostics.push(
                        self.error(
                            module_id,
                            "T4262",
                            name.range(),
                            "`Process.register` needs a direct reference to a `process name` \
                         declaration, so the name is known at build time; a local, a \
                         parameter, a constant, or a computed address cannot be registered"
                                .to_owned(),
                        ),
                    );
                }
            }
            StandardFunction::SupervisorPolicy => {
                let capacity = argument(1)
                    .and_then(|capacity| integer_literal(capacity).map(|value| (capacity, value)));
                if let Some((capacity, value)) = capacity
                    && value < 1
                {
                    self.range_error(
                        module_id,
                        capacity,
                        format!(
                            "a supervisor capacity must be at least 1, but this one is {value}"
                        ),
                    );
                }
                if let Some(ModuleExpression::List { elements, .. }) = argument(0).map(strip_groups)
                {
                    if let (Some(members), Some((_, capacity))) = (argument(0), capacity)
                        && capacity >= 1
                        && elements.len() as i64 > capacity
                    {
                        self.range_error(
                            module_id,
                            members,
                            format!(
                                "this members list has {} members, more than the capacity {capacity}",
                                elements.len()
                            ),
                        );
                    }
                    self.check_duplicate_names(module_id, elements);
                }
            }
            StandardFunction::SupervisorRestarts => {
                if let Some(limit) = argument(0)
                    && let Some(value) = integer_literal(limit)
                    && value < 0
                {
                    self.range_error(
                        module_id,
                        limit,
                        format!("a restart limit must be at least 0, but this one is {value}"),
                    );
                }
                if let Some(window) = argument(1)
                    && let Some(value) = self.duration_literal(module_id, window)
                    && value <= 0
                {
                    self.range_error(
                        module_id,
                        window,
                        format!(
                            "a restart window must be positive, but this one is {}",
                            crate::duration::to_text(value)
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    /// `T4264`: the second element of `elements` that registers an already-seen name.
    fn check_duplicate_names(&mut self, module_id: ModuleId, elements: &[ModuleExpression]) {
        let mut seen = HashMap::<SymbolId, usize>::new();
        for (index, element) in elements.iter().enumerate() {
            let Some(name) = self.registered_name(module_id, element, &mut HashSet::new()) else {
                continue;
            };
            if let Some(first) = seen.get(&name) {
                let label = process_name_label(self.typed, name);
                self.diagnostics.push(self.error(
                    module_id,
                    "T4264",
                    element.range(),
                    format!(
                        "member {} registers process name `{label}`, which member {} of this \
                         list already registers; one supervisor policy cannot start two \
                         processes under one name",
                        index + 1,
                        first + 1
                    ),
                ));
            } else {
                seen.insert(name, index);
            }
        }
    }

    /// The `process name` a member expression registers, followed through
    /// `Supervisor.process`, `Process.register`, and module constants. `None` when
    /// it registers none or the walk cannot see it.
    fn registered_name(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        following: &mut HashSet<SymbolId>,
    ) -> Option<SymbolId> {
        let expression = strip_groups(expression);
        if let ModuleExpression::Call {
            callee, arguments, ..
        } = expression
        {
            return match self.standard_function(module_id, callee)? {
                StandardFunction::SupervisorProcess => {
                    self.registered_name(module_id, &arguments.first()?.value, following)
                }
                StandardFunction::ProcessRegister => {
                    self.process_name_reference(module_id, &arguments.get(1)?.value)
                }
                _ => None,
            };
        }
        let symbol = self.value_symbol(module_id, expression)?;
        let project_symbol = self.typed.resolved.symbol(symbol);
        let (ProjectSymbolKind::Constant, Some(owner), Some(semantic_id)) = (
            project_symbol.kind,
            project_symbol.module_id,
            project_symbol.semantic_id,
        ) else {
            return None;
        };
        if !following.insert(symbol) {
            return None;
        }
        let body = self.typed.resolved.semantic.modules[owner.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)?
            .body
            .as_ref()?;
        self.registered_name(owner, body, following)
    }

    /// `T4261` when `message` is never transferable.
    fn check_transferable(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        message: TypeId,
        subject: &str,
    ) {
        if self.typed.value_properties(message).transferable != PropertyDerivation::Never {
            return;
        }
        let found = self.typed.format_type(message);
        self.diagnostics.push(self.error(
            module_id,
            "T4261",
            range,
            format!(
                "the message type `{found}` of {subject} is not transferable, but every \
                 message moves to another process; it holds a task-local value such as a \
                 `Task` or a `Process.Inbox`"
            ),
        ));
    }

    /// `T4240` when the body's failure type `failure` is never transferable.
    fn check_failure_stays_local(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        failure: TypeId,
    ) {
        if self.typed.value_properties(failure).transferable != PropertyDerivation::Never {
            return;
        }
        let found = self.typed.format_type(failure);
        self.diagnostics.push(self.error(
            module_id,
            "T4240",
            range,
            format!(
                "this process body can end with a failure of type `{found}`, which holds a \
                 task-local value such as a `Process.Inbox`; ending with `Failure` would \
                 return it beyond the body"
            ),
        ));
    }

    fn range_error(&mut self, module_id: ModuleId, argument: &ModuleExpression, message: String) {
        let diagnostic = self.error(module_id, "T4263", argument.range(), message);
        self.diagnostics.push(diagnostic);
    }

    fn error(
        &self,
        module_id: ModuleId,
        code: &'static str,
        range: TextRange,
        message: String,
    ) -> Diagnostic {
        let file_id = self.typed.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .file_id();
        Diagnostic::error(DiagnosticStage::Type, code, file_id, range, message)
    }

    /// The nanoseconds of a literal duration: `Duration.zero`, or a `Duration`
    /// constructor applied to an integer literal.
    fn duration_literal(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<i64> {
        let expression = strip_groups(expression);
        let literal = match expression {
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                matches!(
                    self.evaluator
                        .duration_function(module_id, callee)
                        .and_then(StandardFunction::duration_unit),
                    Some((_, DurationUnitRole::Constructor))
                ) && matches!(arguments.as_slice(), [argument] if integer_literal(&argument.value).is_some())
            }
            _ => self
                .value_symbol(module_id, expression)
                .is_some_and(|symbol| {
                    self.typed.resolved.standard_symbol(symbol)
                        == Some(StandardSymbol::Constant(StandardConstant::DurationZero))
                }),
        };
        if !literal {
            return None;
        }
        match self.evaluator.fold(module_id, expression) {
            Ok(Folded::Duration(value)) => Some(value),
            _ => None,
        }
    }

    /// The `process name` symbol `expression` names directly, after parentheses.
    fn process_name_reference(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<SymbolId> {
        let symbol = self.value_symbol(module_id, strip_groups(expression))?;
        (self.typed.resolved.symbol(symbol).kind == ProjectSymbolKind::ProcessName)
            .then_some(symbol)
    }

    fn standard_function(
        &self,
        module_id: ModuleId,
        callee: &ModuleExpression,
    ) -> Option<StandardFunction> {
        match self
            .typed
            .resolved
            .standard_symbol(self.value_symbol(module_id, callee)?)?
        {
            StandardSymbol::Function(function) => Some(function),
            _ => None,
        }
    }

    /// Whether `expression` is a name that resolution already rejected (unresolved,
    /// inaccessible, or ambiguous), so a further diagnostic would only pile on.
    fn reports_its_own_resolution(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> bool {
        let (ModuleExpression::Name { semantic_id, .. }
        | ModuleExpression::Select { semantic_id, .. }) = strip_groups(expression)
        else {
            return false;
        };
        matches!(
            self.typed
                .resolved
                .reference_resolution(module_id, *semantic_id, Namespace::Value),
            Some(
                ReferenceResolution::Unresolved
                    | ReferenceResolution::Inaccessible(_)
                    | ReferenceResolution::Ambiguous
            )
        )
    }

    fn value_symbol(&self, module_id: ModuleId, expression: &ModuleExpression) -> Option<SymbolId> {
        match strip_groups(expression) {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                match self.typed.resolved.reference_resolution(
                    module_id,
                    *semantic_id,
                    Namespace::Value,
                )? {
                    ReferenceResolution::Resolved(symbol) => Some(symbol),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The first type argument of `id` when it is the standard type `kind`.
    fn type_argument(&self, id: TypeId, kind: StandardType) -> Option<TypeId> {
        self.type_arguments(id, kind).first().copied()
    }

    /// The type arguments of `id` when it is the standard type `kind`, else none.
    fn type_arguments(&self, id: TypeId, kind: StandardType) -> &[TypeId] {
        match &self.typed.types[self.typed.resolve_type_id(id).raw() as usize] {
            CanonicalType::Named { symbol, arguments }
                if self.typed.resolved.standard_symbol(*symbol)
                    == Some(StandardSymbol::Type(kind)) =>
            {
                arguments
            }
            _ => &[],
        }
    }
}

/// `<module path>.<identifier>`, the label traces and `NameTaken` use.
pub(crate) fn process_name_label(typed: &TypedProject, symbol: SymbolId) -> String {
    let symbol = typed.resolved.symbol(symbol);
    let module = symbol
        .module_id
        .map(|module| {
            typed.resolved.semantic.modules[module.raw() as usize]
                .path
                .as_str()
        })
        .unwrap_or_default();
    format!("{module}.{}", symbol.name)
}

/// The value of an integer literal, with an optional minus, after parentheses.
fn integer_literal(expression: &ModuleExpression) -> Option<i64> {
    match strip_groups(expression) {
        ModuleExpression::Literal(Literal::Integer { value, .. }) => Some(*value),
        ModuleExpression::Unary {
            operator: UnaryOperator::Negate,
            operand,
            ..
        } => integer_literal(operand).and_then(i64::checked_neg),
        _ => None,
    }
}

fn strip_groups(mut expression: &ModuleExpression) -> &ModuleExpression {
    while let ModuleExpression::Group {
        expression: inner, ..
    } = expression
    {
        expression = inner;
    }
    expression
}
