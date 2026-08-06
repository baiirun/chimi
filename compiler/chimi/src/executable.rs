use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{CoreNodeId, FileId, ModuleId, OriginId, SemanticId, SymbolId, TextRange, TypeId};
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, Resolution, SymbolKind,
};
use crate::semantic::{
    BinaryOperator, Literal, ModuleBlockItem, ModuleDeclarationKind, ModuleExpression, Pattern,
    PatternLiteral, UnaryOperator,
};
use crate::typing::{CanonicalType, TypeFact, TypedProgram, TypedProject};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Origin {
    pub(crate) origin_id: OriginId,
    pub(crate) file_id: FileId,
    pub(crate) module_id: ModuleId,
    pub(crate) semantic_id: SemanticId,
    pub(crate) range: TextRange,
}

#[derive(Clone, Debug)]
pub(crate) struct LegacyCoreProgram {
    pub(crate) computation: String,
    pub(crate) root_node_id: CoreNodeId,
    pub(crate) origin: Origin,
    pub(crate) constructor_name: String,
    pub(crate) field_name: String,
    pub(crate) value: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreProgram {
    pub(crate) computation: String,
    pub(crate) entry: SymbolId,
    type_count: usize,
    pub(crate) symbols: Vec<CoreSymbol>,
    pub(crate) functions: Vec<CoreFunction>,
    pub(crate) bindings: Vec<CoreBinding>,
    pub(crate) nodes: Vec<CoreNode>,
    pub(crate) origins: Vec<Origin>,
}

impl CoreProgram {
    pub(crate) fn symbol(&self, id: SymbolId) -> &CoreSymbol {
        self.symbols
            .get(id.raw() as usize)
            .filter(|symbol| symbol.id == id)
            .expect("core symbol ID belongs to this program")
    }

    fn assert_well_formed(&self) {
        debug_assert!(
            self.functions
                .iter()
                .any(|function| function.symbol == self.entry)
        );
        debug_assert!(!self.computation.is_empty());
        for (index, symbol) in self.symbols.iter().enumerate() {
            debug_assert_eq!(symbol.id.raw() as usize, index);
        }
        for (index, node) in self.nodes.iter().enumerate() {
            debug_assert_eq!(node.id.raw() as usize, index);
            debug_assert!((node.origin.raw() as usize) < self.origins.len());
            debug_assert!((node.type_id.raw() as usize) < self.type_count);
            self.assert_operation(&node.operation);
        }
        for function in &self.functions {
            debug_assert!((function.symbol.raw() as usize) < self.symbols.len());
            debug_assert!(
                function
                    .parameters
                    .iter()
                    .all(|parameter| (parameter.raw() as usize) < self.symbols.len())
            );
            debug_assert!((function.body.raw() as usize) < self.nodes.len());
        }
        for binding in &self.bindings {
            debug_assert!((binding.symbol.raw() as usize) < self.symbols.len());
            debug_assert!((binding.value.raw() as usize) < self.nodes.len());
        }
        for (index, origin) in self.origins.iter().enumerate() {
            debug_assert_eq!(origin.origin_id.raw() as usize, index);
            let _ = (
                origin.file_id,
                origin.module_id,
                origin.semantic_id,
                origin.range,
            );
        }
    }

    pub(crate) fn stats(
        &self,
    ) -> (
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    ) {
        let mut constants = 0;
        let mut constructions = 0;
        let mut calls = 0;
        let mut sequences = 0;
        let mut dependencies = 0;
        for node in &self.nodes {
            match node.operation {
                CoreOperation::Constant(_) => constants += 1,
                CoreOperation::Construct { .. } => constructions += 1,
                CoreOperation::Call { .. } => calls += 1,
                CoreOperation::Sequence(_) => sequences += 1,
                CoreOperation::DependencyMethod { .. } => dependencies += 1,
                _ => {}
            }
        }
        (
            self.functions.len(),
            self.bindings.len(),
            self.nodes.len(),
            self.origins.len(),
            constants,
            constructions,
            calls,
            sequences,
            dependencies,
        )
    }

    fn assert_operation(&self, operation: &CoreOperation) {
        let node_exists = |node: CoreNodeId| (node.raw() as usize) < self.nodes.len();
        match operation {
            CoreOperation::Constant(literal) => touch_literal(literal),
            CoreOperation::Reference(symbol) => {
                debug_assert!((symbol.raw() as usize) < self.symbols.len());
            }
            CoreOperation::Select { receiver, member } => {
                debug_assert!(node_exists(*receiver));
                debug_assert!(!member.is_empty());
            }
            CoreOperation::DependencyMethod {
                requirement,
                interface,
                method,
                name,
            } => {
                debug_assert!((requirement.raw() as usize) < self.symbols.len());
                debug_assert!((interface.raw() as usize) < self.symbols.len());
                let _ = method;
                debug_assert!(!name.is_empty());
            }
            CoreOperation::Construct {
                constructor,
                arguments,
            } => {
                debug_assert!((constructor.raw() as usize) < self.symbols.len());
                for argument in arguments {
                    let _ = &argument.label;
                    debug_assert!(node_exists(argument.value));
                }
            }
            CoreOperation::Call { callee, arguments } => {
                debug_assert!(node_exists(*callee));
                for argument in arguments {
                    let _ = &argument.label;
                    debug_assert!(node_exists(argument.value));
                }
            }
            CoreOperation::List(elements) => {
                debug_assert!(elements.iter().all(|element| node_exists(*element)));
            }
            CoreOperation::Record(fields) => {
                for field in fields {
                    debug_assert!(!field.name.is_empty());
                    debug_assert!(node_exists(field.value));
                }
            }
            CoreOperation::Unary { operator, operand } => {
                let _ = operator;
                debug_assert!(node_exists(*operand));
            }
            CoreOperation::Binary {
                operator,
                left,
                right,
            } => {
                let _ = operator;
                debug_assert!(node_exists(*left));
                debug_assert!(node_exists(*right));
            }
            CoreOperation::Identity(value) => debug_assert!(node_exists(*value)),
            CoreOperation::Provision {
                expression,
                providers,
            } => {
                debug_assert!(node_exists(*expression));
                for provider in providers {
                    debug_assert!(!provider.name.is_empty());
                    debug_assert!(node_exists(provider.value));
                }
            }
            CoreOperation::Sequence(steps) => {
                for step in steps {
                    match step {
                        CoreStep::Bind { symbol, value } => {
                            debug_assert!((symbol.raw() as usize) < self.symbols.len());
                            debug_assert!(node_exists(*value));
                        }
                        CoreStep::Expression(value) => debug_assert!(node_exists(*value)),
                    }
                }
            }
            CoreOperation::Closure { parameters, body } => {
                debug_assert!(
                    parameters
                        .iter()
                        .all(|parameter| (parameter.raw() as usize) < self.symbols.len())
                );
                debug_assert!(node_exists(*body));
            }
            CoreOperation::Match { subjects, arms } => {
                debug_assert!(subjects.iter().all(|subject| node_exists(*subject)));
                for arm in arms {
                    for pattern in &arm.patterns {
                        touch_pattern(pattern);
                    }
                    if let Some(guard) = arm.guard {
                        debug_assert!(node_exists(guard));
                    }
                    debug_assert!(node_exists(arm.body));
                }
            }
        }
    }
}

fn touch_literal(literal: &CoreLiteral) {
    match literal {
        CoreLiteral::Integer(value) => {
            let _ = value;
        }
        CoreLiteral::Text(value) => {
            let _ = value;
        }
        CoreLiteral::Boolean(value) => {
            let _ = value;
        }
    }
}

fn touch_pattern(pattern: &CorePattern) {
    match pattern {
        CorePattern::Wildcard => {}
        CorePattern::Binding(symbol) => {
            let _ = symbol;
        }
        CorePattern::Literal(literal) => touch_literal(literal),
        CorePattern::Constructor { symbol, fields } => {
            let _ = symbol;
            for field in fields {
                touch_pattern(field);
            }
        }
        CorePattern::Outcome { success, value } => {
            let _ = success;
            touch_pattern(value);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CoreSymbol {
    pub(crate) id: SymbolId,
    pub(crate) name: String,
    pub(crate) kind: CoreSymbolKind,
    pub(crate) constructor: Option<CoreConstructor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreSymbolKind {
    Function,
    Binding,
    Requirement,
    Local,
    Constructor,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreConstructor {
    pub(crate) type_name: String,
    pub(crate) variant_name: Option<String>,
    pub(crate) fields: Vec<String>,
    pub(crate) outcome_success: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreFunction {
    pub(crate) symbol: SymbolId,
    pub(crate) parameters: Vec<SymbolId>,
    pub(crate) body: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreBinding {
    pub(crate) symbol: SymbolId,
    pub(crate) value: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreNode {
    pub(crate) id: CoreNodeId,
    pub(crate) origin: OriginId,
    pub(crate) type_id: TypeId,
    pub(crate) operation: CoreOperation,
}

#[derive(Clone, Debug)]
pub(crate) enum CoreOperation {
    Constant(CoreLiteral),
    Reference(SymbolId),
    Select {
        receiver: CoreNodeId,
        member: String,
    },
    DependencyMethod {
        requirement: SymbolId,
        interface: SymbolId,
        method: SemanticId,
        name: String,
    },
    Construct {
        constructor: SymbolId,
        arguments: Vec<CoreArgument>,
    },
    Call {
        callee: CoreNodeId,
        arguments: Vec<CoreArgument>,
    },
    List(Vec<CoreNodeId>),
    Record(Vec<CoreField>),
    Unary {
        operator: UnaryOperator,
        operand: CoreNodeId,
    },
    Binary {
        operator: BinaryOperator,
        left: CoreNodeId,
        right: CoreNodeId,
    },
    Identity(CoreNodeId),
    Provision {
        expression: CoreNodeId,
        providers: Vec<CoreField>,
    },
    Sequence(Vec<CoreStep>),
    Closure {
        parameters: Vec<SymbolId>,
        body: CoreNodeId,
    },
    Match {
        subjects: Vec<CoreNodeId>,
        arms: Vec<CoreArm>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum CoreLiteral {
    Integer(i64),
    Text(String),
    Boolean(bool),
}

#[derive(Clone, Debug)]
pub(crate) struct CoreArgument {
    pub(crate) label: Option<String>,
    pub(crate) value: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreField {
    pub(crate) name: String,
    pub(crate) value: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) enum CoreStep {
    Bind { symbol: SymbolId, value: CoreNodeId },
    Expression(CoreNodeId),
}

#[derive(Clone, Debug)]
pub(crate) struct CoreArm {
    pub(crate) patterns: Vec<CorePattern>,
    pub(crate) guard: Option<CoreNodeId>,
    pub(crate) body: CoreNodeId,
}

#[derive(Clone, Debug)]
pub(crate) enum CorePattern {
    Wildcard,
    Binding(SymbolId),
    Literal(CoreLiteral),
    Constructor {
        symbol: SymbolId,
        fields: Vec<CorePattern>,
    },
    Outcome {
        success: bool,
        value: Box<CorePattern>,
    },
}

pub(crate) struct LegacyVerifiedRoot {
    typed: Arc<TypedProgram>,
    entry: SymbolId,
}

pub(crate) struct VerifiedRoot {
    pub(crate) typed: Arc<TypedProject>,
    pub(crate) entry: SymbolId,
    reachable_declarations: Vec<SymbolId>,
    origins: Vec<Origin>,
}

impl VerifiedRoot {
    pub(crate) fn origin_id(&self, module_id: ModuleId, semantic_id: SemanticId) -> Option<u32> {
        self.origins
            .iter()
            .find(|origin| origin.module_id == module_id && origin.semantic_id == semantic_id)
            .map(|origin| origin.origin_id.raw())
    }

    pub(crate) fn verification_stats(&self) -> (usize, usize, usize) {
        let reachable_modules = self
            .origins
            .iter()
            .map(|origin| origin.module_id)
            .collect::<HashSet<_>>()
            .len();
        (
            self.reachable_declarations.len(),
            self.origins.len(),
            reachable_modules,
        )
    }
}

pub(crate) fn seal_entry(
    typed: impl Into<Arc<TypedProject>>,
) -> Result<VerifiedRoot, Vec<Diagnostic>> {
    let typed = typed.into();
    if !typed.diagnostics.is_empty() {
        return Err(typed.diagnostics.clone());
    }

    let roots = typed
        .resolved
        .semantic
        .modules
        .iter()
        .flat_map(|module| {
            module
                .declarations
                .iter()
                .filter(|declaration| {
                    declaration.kind == crate::semantic::ModuleDeclarationKind::Function
                        && declaration.public
                        && declaration.name.text == "main"
                })
                .map(move |declaration| (module.id, declaration))
        })
        .collect::<Vec<_>>();
    let Some((root_module, root)) = roots.first().copied() else {
        let (file_id, range) = typed.resolved.semantic.modules.first().map_or(
            (FileId::new(0), TextRange::empty(0)),
            |module| {
                (
                    module.source.file_id(),
                    module.declarations.first().map_or_else(
                        || TextRange::empty(module.source.text().len()),
                        |declaration| declaration.name.range,
                    ),
                )
            },
        );
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5001",
            file_id,
            range,
            "program must export exactly one `pub fn main()`",
        )]);
    };
    if roots.len() != 1 {
        let module = &typed.resolved.semantic.modules[root_module.raw() as usize];
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5003",
            module.source.file_id(),
            root.name.range,
            "program exports more than one `main` function",
        )]);
    }

    let entry = typed
        .resolved
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == crate::resolution::ProjectSymbolKind::Function
                && symbol.module_id == Some(root_module)
                && symbol.semantic_id == Some(root.semantic_id)
        })
        .map(|symbol| symbol.id)
        .expect("resolved entry function has a symbol");
    let contract = typed
        .contracts
        .get(&entry)
        .expect("typed entry function has a callable contract");
    let module = &typed.resolved.semantic.modules[root_module.raw() as usize];
    if !contract.inputs.is_empty() {
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5004",
            module.source.file_id(),
            root.name.range,
            "program entry `main` cannot accept arguments",
        )]);
    }
    if !root.generic_parameters.is_empty()
        || !typed.failure_rows[contract.failures.raw() as usize]
            .parameters
            .is_empty()
        || !typed.needs_rows[contract.needs.raw() as usize]
            .parameters
            .is_empty()
        || matches!(
            resolved_type(&typed, contract.success),
            CanonicalType::InferenceVariable(_) | CanonicalType::Generic(_) | CanonicalType::Poison
        )
    {
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5005",
            module.source.file_id(),
            root.name.range,
            "program entry `main` must have a fully concrete contract",
        )]);
    }

    let reachable = collect_reachable_program(&typed, entry)?;
    Ok(VerifiedRoot {
        typed,
        entry,
        reachable_declarations: reachable.declarations,
        origins: reachable.origins,
    })
}

struct ReachableProgram {
    declarations: Vec<SymbolId>,
    origins: Vec<Origin>,
}

#[derive(Default)]
struct ReachabilityState {
    queue: VecDeque<SymbolId>,
    seen_declarations: HashSet<SymbolId>,
    origins: Vec<(ModuleId, SemanticId, TextRange)>,
    seen_origins: HashSet<(ModuleId, SemanticId)>,
    diagnostics: Vec<Diagnostic>,
}

fn collect_reachable_program(
    typed: &TypedProject,
    entry: SymbolId,
) -> Result<ReachableProgram, Vec<Diagnostic>> {
    let mut references = HashMap::<(FileId, SemanticId), Vec<_>>::new();
    for reference in &typed.resolved.references {
        if reference.namespace == Namespace::Value {
            references
                .entry((reference.file_id, reference.semantic_id))
                .or_default()
                .push(reference);
        }
    }

    let mut state = ReachabilityState {
        queue: VecDeque::from([entry]),
        ..ReachabilityState::default()
    };
    while let Some(symbol_id) = state.queue.pop_front() {
        if !state.seen_declarations.insert(symbol_id) {
            continue;
        }
        let symbol = typed.resolved.symbol(symbol_id);
        let (Some(module_id), Some(semantic_id)) = (symbol.module_id, symbol.semantic_id) else {
            continue;
        };
        let module = &typed.resolved.semantic.modules[module_id.raw() as usize];
        let Some(declaration) = module
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
        else {
            state.diagnostics.push(Diagnostic::error(
                DiagnosticStage::ExecutionSetup,
                "E5006",
                module.source.file_id(),
                TextRange::empty(0),
                "reachable symbol has no semantic declaration",
            ));
            continue;
        };
        if declaration.kind == ModuleDeclarationKind::Function
            && !typed.contracts.contains_key(&symbol_id)
        {
            state.diagnostics.push(Diagnostic::error(
                DiagnosticStage::ExecutionSetup,
                "E5006",
                module.source.file_id(),
                declaration.name.range,
                format!(
                    "reachable function `{}` has no callable contract",
                    declaration.name.text
                ),
            ));
        }
        if let Some(body) = &declaration.body {
            collect_reachable_expression(typed, module_id, body, &references, &mut state);
        }
    }
    if !state.diagnostics.is_empty() {
        return Err(state.diagnostics);
    }

    let mut declarations = state.seen_declarations.into_iter().collect::<Vec<_>>();
    declarations.sort_unstable();
    state
        .origins
        .sort_by_key(|(module_id, semantic_id, _)| (*module_id, *semantic_id));
    let origins = state
        .origins
        .into_iter()
        .enumerate()
        .map(|(index, (module_id, semantic_id, range))| Origin {
            origin_id: OriginId::new(index as u32),
            file_id: typed.resolved.semantic.modules[module_id.raw() as usize]
                .source
                .file_id(),
            module_id,
            semantic_id,
            range,
        })
        .collect();
    Ok(ReachableProgram {
        declarations,
        origins,
    })
}

fn collect_reachable_expression(
    typed: &TypedProject,
    module_id: ModuleId,
    expression: &ModuleExpression,
    references: &HashMap<(FileId, SemanticId), Vec<&crate::resolution::ProjectReference>>,
    state: &mut ReachabilityState,
) {
    let module = &typed.resolved.semantic.modules[module_id.raw() as usize];
    let semantic_id = expression.semantic_id();
    let range = expression.range();
    if range.start() == range.end() || range.end() as usize > module.source.text().len() {
        state.diagnostics.push(Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5008",
            module.source.file_id(),
            range,
            "reachable expression has no valid source range",
        ));
    }
    if state.seen_origins.insert((module_id, semantic_id)) {
        state.origins.push((module_id, semantic_id, range));
    }
    match typed.expression_facts.get(&(module_id, semantic_id)) {
        Some(fact) if type_is_nonpoisoned(typed, fact.success, &mut HashSet::new()) => {}
        _ => state.diagnostics.push(Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5006",
            module.source.file_id(),
            range,
            "reachable expression has no complete type fact",
        )),
    }
    if let Some(expression_references) = references.get(&(module.source.file_id(), semantic_id)) {
        for reference in expression_references {
            match reference.resolution {
                ReferenceResolution::Resolved(symbol) => {
                    if matches!(
                        typed.resolved.symbol(symbol).kind,
                        ProjectSymbolKind::Function | ProjectSymbolKind::ModuleBinding
                    ) {
                        state.queue.push_back(symbol);
                    }
                }
                ReferenceResolution::DeferredToType => {}
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_) => {
                    state.diagnostics.push(Diagnostic::error(
                        DiagnosticStage::ExecutionSetup,
                        "E5007",
                        module.source.file_id(),
                        reference.range,
                        "reachable reference is not executable",
                    ))
                }
            }
        }
    }

    let mut visit = |child: &ModuleExpression| {
        collect_reachable_expression(typed, module_id, child, references, state);
    };
    match expression {
        ModuleExpression::Literal(_)
        | ModuleExpression::Name { .. }
        | ModuleExpression::Empty { .. } => {}
        ModuleExpression::Select { receiver, .. } => {
            let resolved_module_member = references
                .get(&(module.source.file_id(), semantic_id))
                .is_some_and(|references| {
                    references.iter().any(|reference| {
                        matches!(reference.resolution, ReferenceResolution::Resolved(_))
                    })
                })
                && matches!(receiver.as_ref(), ModuleExpression::Name { .. });
            let dependency_receiver =
                referenced_expression_symbol(references, module.source.file_id(), receiver)
                    .is_some_and(|symbol| {
                        typed.resolved.symbol(symbol).kind == ProjectSymbolKind::Requirement
                    });
            if !resolved_module_member && !dependency_receiver {
                visit(receiver);
            }
        }
        ModuleExpression::Group {
            expression: receiver,
            ..
        }
        | ModuleExpression::Unary {
            operand: receiver, ..
        } => visit(receiver),
        ModuleExpression::Call {
            callee, arguments, ..
        } => {
            let callee_symbol =
                referenced_expression_symbol(references, module.source.file_id(), callee);
            let constructor =
                callee_symbol.is_some_and(|symbol| is_core_constructor(typed, symbol));
            if !constructor {
                visit(callee);
            }
            for argument in arguments {
                visit(&argument.value);
            }
        }
        ModuleExpression::List { elements, .. }
        | ModuleExpression::Sequence {
            expressions: elements,
            ..
        } => {
            for element in elements {
                visit(element);
            }
        }
        ModuleExpression::Record { fields, .. } => {
            for field in fields {
                visit(&field.value);
            }
        }
        ModuleExpression::Binary { left, right, .. } => {
            visit(left);
            visit(right);
        }
        ModuleExpression::Using {
            expression,
            providers,
            ..
        } => {
            visit(expression);
            for provider in providers {
                visit(&provider.value);
            }
        }
        ModuleExpression::Block { items, .. } => {
            for item in items {
                match item {
                    ModuleBlockItem::Binding { value, .. } | ModuleBlockItem::Expression(value) => {
                        visit(value)
                    }
                }
            }
        }
        ModuleExpression::Function { body, .. } => visit(body),
        ModuleExpression::Case { subjects, arms, .. } => {
            for subject in subjects {
                visit(subject);
            }
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    visit(guard);
                }
                visit(&arm.body);
            }
        }
    }
}

fn referenced_expression_symbol(
    references: &HashMap<(FileId, SemanticId), Vec<&crate::resolution::ProjectReference>>,
    file_id: FileId,
    expression: &ModuleExpression,
) -> Option<SymbolId> {
    match expression {
        ModuleExpression::Name { semantic_id, .. }
        | ModuleExpression::Select { semantic_id, .. } => references
            .get(&(file_id, *semantic_id))?
            .iter()
            .find_map(|reference| match reference.resolution {
                ReferenceResolution::Resolved(symbol) => Some(symbol),
                _ => None,
            }),
        ModuleExpression::Group { expression, .. } => {
            referenced_expression_symbol(references, file_id, expression)
        }
        _ => None,
    }
}

fn type_is_nonpoisoned(
    typed: &TypedProject,
    type_id: crate::ids::TypeId,
    seen: &mut HashSet<crate::ids::TypeId>,
) -> bool {
    if !seen.insert(type_id) {
        return true;
    }
    match resolved_type(typed, type_id) {
        CanonicalType::Poison | CanonicalType::InferenceVariable(_) => false,
        CanonicalType::Named { arguments, .. } => arguments
            .iter()
            .all(|argument| type_is_nonpoisoned(typed, *argument, seen)),
        CanonicalType::Function(contract) | CanonicalType::PolymorphicFunction { contract, .. } => {
            contract
                .inputs
                .iter()
                .all(|input| type_is_nonpoisoned(typed, *input, seen))
                && type_is_nonpoisoned(typed, contract.success, seen)
        }
        CanonicalType::Record(fields) => fields
            .iter()
            .all(|field| type_is_nonpoisoned(typed, field.type_id, seen)),
        CanonicalType::Generic(_) => true,
    }
}

pub(crate) fn lower(root: VerifiedRoot) -> CoreProgram {
    let computation = root.typed.resolved.symbol(root.entry).name.clone();
    let symbols = lower_core_symbols(&root.typed);
    let origin_ids = root
        .origins
        .iter()
        .map(|origin| ((origin.module_id, origin.semantic_id), origin.origin_id))
        .collect();
    let mut lowerer = CoreLowerer {
        typed: Arc::clone(&root.typed),
        origin_ids,
        nodes: Vec::new(),
    };
    let mut functions = Vec::new();
    let mut bindings = Vec::new();
    for symbol_id in root.reachable_declarations {
        let symbol = lowerer.typed.resolved.symbol(symbol_id);
        let module_id = symbol
            .module_id
            .expect("reachable declaration belongs to a module");
        let semantic_id = symbol
            .semantic_id
            .expect("reachable declaration has semantic identity");
        let declaration = lowerer.typed.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
            .expect("verified reachable symbol owns a declaration");
        let kind = declaration.kind;
        let parameters = declaration
            .function
            .as_ref()
            .map(|signature| {
                signature
                    .parameters
                    .iter()
                    .map(|parameter| {
                        lowerer
                            .local_symbol(
                                module_id,
                                parameter.semantic_id,
                                ProjectSymbolKind::Parameter,
                            )
                            .expect("verified function parameter has a symbol")
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let body = declaration
            .body
            .as_ref()
            .expect("verified executable declaration owns a body")
            .clone();
        let body = lowerer.expression(module_id, &body);
        match kind {
            ModuleDeclarationKind::Function => functions.push(CoreFunction {
                symbol: symbol_id,
                parameters,
                body,
            }),
            ModuleDeclarationKind::Binding => bindings.push(CoreBinding {
                symbol: symbol_id,
                value: body,
            }),
            ModuleDeclarationKind::Struct
            | ModuleDeclarationKind::Enum
            | ModuleDeclarationKind::Interface => {
                unreachable!("only executable declarations enter reachability")
            }
        }
    }
    let core = CoreProgram {
        computation,
        entry: root.entry,
        type_count: root.typed.types.len(),
        symbols,
        functions,
        bindings,
        nodes: lowerer.nodes,
        origins: root.origins,
    };
    core.assert_well_formed();
    core
}

struct CoreLowerer {
    typed: Arc<TypedProject>,
    origin_ids: HashMap<(ModuleId, SemanticId), OriginId>,
    nodes: Vec<CoreNode>,
}

impl CoreLowerer {
    fn expression(&mut self, module_id: ModuleId, expression: &ModuleExpression) -> CoreNodeId {
        let operation = match expression {
            ModuleExpression::Literal(literal) => CoreOperation::Constant(match literal {
                Literal::Integer { value, .. } => CoreLiteral::Integer(*value),
                Literal::Text { value, .. } => CoreLiteral::Text(value.clone()),
                Literal::Boolean { value, .. } => CoreLiteral::Boolean(*value),
            }),
            ModuleExpression::Name { semantic_id, .. } => CoreOperation::Reference(
                self.value_symbol(module_id, *semantic_id)
                    .expect("verified value reference resolves"),
            ),
            ModuleExpression::Select {
                semantic_id,
                receiver,
                member,
                ..
            } => {
                if let Some(symbol) = self.value_symbol(module_id, *semantic_id) {
                    CoreOperation::Reference(symbol)
                } else if let Some((requirement, interface, method)) =
                    self.dependency_method(module_id, receiver, &member.text)
                {
                    CoreOperation::DependencyMethod {
                        requirement,
                        interface,
                        method,
                        name: member.text.clone(),
                    }
                } else {
                    let receiver = self.expression(module_id, receiver);
                    CoreOperation::Select {
                        receiver,
                        member: member.text.clone(),
                    }
                }
            }
            ModuleExpression::Call {
                callee, arguments, ..
            } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| CoreArgument {
                        label: argument.label.as_ref().map(|label| label.text.clone()),
                        value: self.expression(module_id, &argument.value),
                    })
                    .collect::<Vec<_>>();
                let callee_symbol = self.expression_symbol(module_id, callee);
                if let Some(constructor) =
                    callee_symbol.filter(|symbol| is_core_constructor(&self.typed, *symbol))
                {
                    CoreOperation::Construct {
                        constructor,
                        arguments,
                    }
                } else {
                    let callee = self.expression(module_id, callee);
                    CoreOperation::Call { callee, arguments }
                }
            }
            ModuleExpression::List { elements, .. } => CoreOperation::List(
                elements
                    .iter()
                    .map(|element| self.expression(module_id, element))
                    .collect(),
            ),
            ModuleExpression::Record { fields, .. } => CoreOperation::Record(
                fields
                    .iter()
                    .map(|field| CoreField {
                        name: field.name.text.clone(),
                        value: self.expression(module_id, &field.value),
                    })
                    .collect(),
            ),
            ModuleExpression::Unary {
                operator, operand, ..
            } => CoreOperation::Unary {
                operator: *operator,
                operand: self.expression(module_id, operand),
            },
            ModuleExpression::Binary {
                operator,
                left,
                right,
                ..
            } => CoreOperation::Binary {
                operator: *operator,
                left: self.expression(module_id, left),
                right: self.expression(module_id, right),
            },
            ModuleExpression::Group { expression, .. } => {
                CoreOperation::Identity(self.expression(module_id, expression))
            }
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => CoreOperation::Provision {
                expression: self.expression(module_id, expression),
                providers: providers
                    .iter()
                    .map(|provider| CoreField {
                        name: provider.name.text.clone(),
                        value: self.expression(module_id, &provider.value),
                    })
                    .collect(),
            },
            ModuleExpression::Sequence { expressions, .. } => CoreOperation::Sequence(
                expressions
                    .iter()
                    .map(|expression| CoreStep::Expression(self.expression(module_id, expression)))
                    .collect(),
            ),
            ModuleExpression::Block { items, .. } => CoreOperation::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        ModuleBlockItem::Binding { value, .. } => CoreStep::Bind {
                            symbol: self
                                .local_symbol(
                                    module_id,
                                    value.semantic_id(),
                                    ProjectSymbolKind::LocalBinding,
                                )
                                .expect("verified local binding has a symbol"),
                            value: self.expression(module_id, value),
                        },
                        ModuleBlockItem::Expression(expression) => {
                            CoreStep::Expression(self.expression(module_id, expression))
                        }
                    })
                    .collect(),
            ),
            ModuleExpression::Function {
                signature, body, ..
            } => CoreOperation::Closure {
                parameters: signature
                    .parameters
                    .iter()
                    .map(|parameter| {
                        self.local_symbol(
                            module_id,
                            parameter.semantic_id,
                            ProjectSymbolKind::Parameter,
                        )
                        .expect("verified closure parameter has a symbol")
                    })
                    .collect(),
                body: self.expression(module_id, body),
            },
            ModuleExpression::Case { subjects, arms, .. } => CoreOperation::Match {
                subjects: subjects
                    .iter()
                    .map(|subject| self.expression(module_id, subject))
                    .collect(),
                arms: arms
                    .iter()
                    .map(|arm| CoreArm {
                        patterns: arm
                            .patterns
                            .iter()
                            .map(|pattern| self.pattern(module_id, pattern))
                            .collect(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|guard| self.expression(module_id, guard)),
                        body: self.expression(module_id, &arm.body),
                    })
                    .collect(),
            },
            ModuleExpression::Empty { .. } => {
                unreachable!("diagnostic-free verified programs contain no empty expressions")
            }
        };
        self.push_node(module_id, expression, operation)
    }

    fn pattern(&self, module_id: ModuleId, pattern: &Pattern) -> CorePattern {
        if let Some(literal) = &pattern.literal {
            return CorePattern::Literal(match literal {
                PatternLiteral::Integer(value) => CoreLiteral::Integer(*value),
                PatternLiteral::Text(value) => CoreLiteral::Text(value.clone()),
                PatternLiteral::Boolean(value) => CoreLiteral::Boolean(*value),
            });
        }
        let Some(name) = &pattern.name else {
            return CorePattern::Wildcard;
        };
        if name.text == "_" {
            return CorePattern::Wildcard;
        }
        let symbol = self
            .value_symbol(module_id, pattern.semantic_id)
            .or_else(|| {
                self.local_symbol(
                    module_id,
                    pattern.semantic_id,
                    ProjectSymbolKind::PatternBinding,
                )
            })
            .expect("verified pattern resolves");
        let resolved = self.typed.resolved.symbol(symbol);
        if resolved.module_id.is_none()
            && resolved.kind == ProjectSymbolKind::BuiltinValue
            && matches!(resolved.name.as_str(), "Success" | "Failure")
        {
            let [value] = pattern.arguments.as_slice() else {
                unreachable!("verified outcome patterns have one argument")
            };
            CorePattern::Outcome {
                success: resolved.name == "Success",
                value: Box::new(self.pattern(module_id, value)),
            }
        } else if resolved.kind == ProjectSymbolKind::EnumConstructor {
            CorePattern::Constructor {
                symbol,
                fields: pattern
                    .arguments
                    .iter()
                    .map(|field| self.pattern(module_id, field))
                    .collect(),
            }
        } else {
            CorePattern::Binding(symbol)
        }
    }

    fn push_node(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        operation: CoreOperation,
    ) -> CoreNodeId {
        let id = CoreNodeId::new(self.nodes.len() as u32);
        let semantic_id = expression.semantic_id();
        self.nodes.push(CoreNode {
            id,
            origin: self.origin_ids[&(module_id, semantic_id)],
            type_id: self.typed.expression_facts[&(module_id, semantic_id)].success,
            operation,
        });
        id
    }

    fn expression_symbol(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<SymbolId> {
        match expression {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                self.value_symbol(module_id, *semantic_id)
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_symbol(module_id, expression)
            }
            _ => None,
        }
    }

    fn dependency_method(
        &self,
        module_id: ModuleId,
        receiver: &ModuleExpression,
        member: &str,
    ) -> Option<(SymbolId, SymbolId, SemanticId)> {
        let requirement = self.expression_symbol(module_id, receiver)?;
        if self.typed.resolved.symbol(requirement).kind != ProjectSymbolKind::Requirement {
            return None;
        }
        let receiver_type = self
            .typed
            .expression_facts
            .get(&(module_id, receiver.semantic_id()))?
            .success;
        let CanonicalType::Named {
            symbol: interface, ..
        } = resolved_type(&self.typed, receiver_type)
        else {
            return None;
        };
        let method = self
            .typed
            .fields
            .get(interface)?
            .iter()
            .find(|field| field.name == member)?
            .semantic_id;
        Some((requirement, *interface, method))
    }

    fn value_symbol(&self, module_id: ModuleId, semantic_id: SemanticId) -> Option<SymbolId> {
        self.typed
            .resolved
            .reference_resolution(module_id, semantic_id, Namespace::Value)
            .and_then(|resolution| match resolution {
                ReferenceResolution::Resolved(symbol) => Some(symbol),
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_)
                | ReferenceResolution::DeferredToType => None,
            })
    }

    fn local_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        kind: ProjectSymbolKind,
    ) -> Option<SymbolId> {
        self.typed
            .resolved
            .symbols
            .iter()
            .find(|symbol| {
                symbol.module_id == Some(module_id)
                    && symbol.semantic_id == Some(semantic_id)
                    && symbol.kind == kind
            })
            .map(|symbol| symbol.id)
    }
}

fn lower_core_symbols(typed: &TypedProject) -> Vec<CoreSymbol> {
    typed
        .resolved
        .symbols
        .iter()
        .map(|symbol| {
            let kind = match symbol.kind {
                ProjectSymbolKind::Function => CoreSymbolKind::Function,
                ProjectSymbolKind::ModuleBinding => CoreSymbolKind::Binding,
                ProjectSymbolKind::Requirement => CoreSymbolKind::Requirement,
                ProjectSymbolKind::Parameter
                | ProjectSymbolKind::LocalBinding
                | ProjectSymbolKind::PatternBinding => CoreSymbolKind::Local,
                ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor => {
                    CoreSymbolKind::Constructor
                }
                ProjectSymbolKind::BuiltinValue
                    if matches!(symbol.name.as_str(), "Success" | "Failure") =>
                {
                    CoreSymbolKind::Constructor
                }
                _ => CoreSymbolKind::Other,
            };
            CoreSymbol {
                id: symbol.id,
                name: symbol.name.clone(),
                kind,
                constructor: (kind == CoreSymbolKind::Constructor)
                    .then(|| lower_core_constructor(typed, symbol.id)),
            }
        })
        .collect()
}

fn lower_core_constructor(typed: &TypedProject, constructor: SymbolId) -> CoreConstructor {
    let symbol = typed.resolved.symbol(constructor);
    if symbol.kind == ProjectSymbolKind::BuiltinValue {
        return CoreConstructor {
            type_name: "Outcome".to_owned(),
            variant_name: Some(symbol.name.clone()),
            fields: vec!["value".to_owned()],
            outcome_success: Some(symbol.name == "Success"),
        };
    }
    let fields = typed
        .fields
        .get(&constructor)
        .expect("verified nominal constructor has fields")
        .iter()
        .map(|field| field.name.clone())
        .collect();
    if symbol.kind == ProjectSymbolKind::StructConstructor {
        return CoreConstructor {
            type_name: symbol.name.clone(),
            variant_name: None,
            fields,
            outcome_success: None,
        };
    }
    let module_id = symbol
        .module_id
        .expect("enum constructor belongs to a module");
    let semantic_id = symbol
        .semantic_id
        .expect("enum constructor has semantic identity");
    let type_name = typed.resolved.semantic.modules[module_id.raw() as usize]
        .declarations
        .iter()
        .find(|declaration| {
            declaration.kind == ModuleDeclarationKind::Enum
                && declaration
                    .variants
                    .iter()
                    .any(|variant| variant.semantic_id == semantic_id)
        })
        .expect("verified enum constructor belongs to an enum")
        .name
        .text
        .clone();
    CoreConstructor {
        type_name,
        variant_name: Some(symbol.name.clone()),
        fields,
        outcome_success: None,
    }
}

fn is_core_constructor(typed: &TypedProject, symbol: SymbolId) -> bool {
    let symbol = typed.resolved.symbol(symbol);
    matches!(
        symbol.kind,
        ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor
    ) || (symbol.kind == ProjectSymbolKind::BuiltinValue
        && matches!(symbol.name.as_str(), "Success" | "Failure"))
}

fn resolved_type(typed: &TypedProject, mut type_id: crate::ids::TypeId) -> &CanonicalType {
    loop {
        match &typed.types[type_id.raw() as usize] {
            CanonicalType::InferenceVariable(variable) => {
                let Some(bound) = typed.inference_bindings.get(variable).copied() else {
                    return &typed.types[type_id.raw() as usize];
                };
                type_id = bound;
            }
            resolved => return resolved,
        }
    }
}

pub(crate) fn seal_legacy_entry(
    typed: TypedProgram,
) -> Result<LegacyVerifiedRoot, Vec<Diagnostic>> {
    let typed = Arc::new(typed);
    if !typed.diagnostics.is_empty() {
        return Err(typed.diagnostics.clone());
    }

    let entry_function = typed
        .resolved
        .semantic
        .functions
        .iter()
        .find(|function| function.public && function.name.text == "main");
    let Some(entry_function) = entry_function else {
        let range = typed.resolved.semantic.functions.first().map_or_else(
            || TextRange::empty(typed.resolved.semantic.source.text().len()),
            |function| function.name.range,
        );
        return Err(vec![Diagnostic::error(
            DiagnosticStage::ExecutionSetup,
            "E5001",
            typed.resolved.semantic.source.file_id(),
            range,
            "program must export `pub fn main()`",
        )]);
    };

    let Some(TypeFact::Known(construction_type)) = typed.construction_type else {
        panic!("diagnostic-free typed program must have a known entry construction type");
    };
    debug_assert_eq!(construction_type, crate::typing::STRUCT_TYPE);
    debug_assert_eq!(
        typed.construction_semantic_id,
        Some(entry_function.body.semantic_id)
    );
    debug_assert_eq!(
        typed.literal_semantic_id,
        Some(entry_function.body.value.semantic_id())
    );
    let Some(TypeFact::Known(_)) = typed.literal_type else {
        panic!("diagnostic-free typed program must have a known literal type");
    };

    let entry = typed
        .resolved
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == SymbolKind::Function
                && symbol.semantic_id == entry_function.semantic_id
                && symbol.name == "main"
        })
        .map(|symbol| symbol.id)
        .expect("resolved main function has a symbol");
    Ok(LegacyVerifiedRoot { typed, entry })
}

pub(crate) fn lower_legacy(root: LegacyVerifiedRoot) -> LegacyCoreProgram {
    let typed = root.typed;
    let entry_symbol = typed.resolved.symbol(root.entry);
    let entry = typed
        .resolved
        .semantic
        .functions
        .iter()
        .find(|function| function.semantic_id == entry_symbol.semantic_id)
        .expect("verified entry resolves to its function");
    let structure = typed
        .resolved
        .structure()
        .expect("verified construction has a struct declaration");
    let Resolution::Resolved(constructor) = typed
        .resolved
        .constructor_resolution
        .expect("verified construction has a resolution")
    else {
        panic!("verified constructor must resolve");
    };
    let field = typed
        .resolved
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == SymbolKind::Field && symbol.semantic_id == structure.field_semantic_id
        })
        .map(|symbol| symbol.id)
        .expect("verified struct field has a symbol");
    let Literal::Integer { value, .. } = entry.body.value else {
        panic!("verified M1 field value must be an integer");
    };

    LegacyCoreProgram {
        computation: entry_symbol.name.clone(),
        root_node_id: CoreNodeId::new(0),
        origin: Origin {
            origin_id: OriginId::new(entry.body.semantic_id.raw()),
            file_id: typed.resolved.semantic.source.file_id(),
            module_id: ModuleId::new(0),
            semantic_id: entry.body.semantic_id,
            range: entry.body.range,
        },
        constructor_name: typed.resolved.symbol(constructor).name.clone(),
        field_name: typed.resolved.symbol(field).name.clone(),
        value,
    }
}
