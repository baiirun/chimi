use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{CoreNodeId, FileId, OriginId, SemanticId, SymbolId, TextRange};
use crate::resolution::{Resolution, SymbolKind};
use crate::semantic::Literal;
use crate::typing::{CanonicalType, TypeFact, TypedProgram, TypedProject};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Origin {
    pub(crate) origin_id: OriginId,
    pub(crate) file_id: FileId,
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

    Ok(VerifiedRoot {
        typed: Arc::new(typed),
        entry,
    })
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
