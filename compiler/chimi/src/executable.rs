use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{CoreNodeId, FileId, ModuleId, OriginId, SemanticId, SymbolId, TextRange};
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, Resolution, SymbolKind,
};
use crate::semantic::{Literal, ModuleBlockItem, ModuleDeclarationKind, ModuleExpression};
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
pub(crate) struct CoreProgram {
    pub(crate) computation: String,
    pub(crate) root_node_id: CoreNodeId,
    pub(crate) origin: Origin,
    pub(crate) constructor: SymbolId,
    pub(crate) constructor_name: String,
    pub(crate) field: SymbolId,
    pub(crate) field_name: String,
    pub(crate) value: i64,
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

pub(crate) fn seal_entry(typed: TypedProject) -> Result<VerifiedRoot, Vec<Diagnostic>> {
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
        typed: Arc::new(typed),
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
            if !resolved_module_member {
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
            visit(callee);
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

pub(crate) fn lower_legacy(root: LegacyVerifiedRoot) -> CoreProgram {
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

    CoreProgram {
        computation: entry_symbol.name.clone(),
        root_node_id: CoreNodeId::new(0),
        origin: Origin {
            origin_id: OriginId::new(entry.body.semantic_id.raw()),
            file_id: typed.resolved.semantic.source.file_id(),
            module_id: ModuleId::new(0),
            semantic_id: entry.body.semantic_id,
            range: entry.body.range,
        },
        constructor,
        constructor_name: typed.resolved.symbol(constructor).name.clone(),
        field,
        field_name: typed.resolved.symbol(field).name.clone(),
        value,
    }
}
