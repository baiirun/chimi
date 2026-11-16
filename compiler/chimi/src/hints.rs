//! Inlay hints: the type the compiler inferred for each `let` that does not
//! state one, shown after the binding's name.

use crate::ids::TypeId;
use crate::resolution::ProjectSymbolKind;
use crate::typing::{CanonicalType, TypedProject};

/// One inferred type to show after a binding's name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeHint {
    offset: u32,
    label: String,
}

impl TypeHint {
    /// The byte offset just after the binding's name.
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// The text to show there: `: ` and the type, as `: List<Int>`.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// A hint for each un-annotated `let` binding in `module_path`, in source
/// order. A binding whose type is not fully inferred, as in code with a type
/// error, gets none: a wrong hint misleads more than no hint.
pub(crate) fn let_type_hints(typed: &TypedProject, module_path: &str) -> Vec<TypeHint> {
    let resolved = &typed.resolved;
    let Some(module) = resolved
        .semantic
        .modules
        .iter()
        .find(|module| module.path == module_path)
    else {
        return Vec::new();
    };
    let text = module.source.text();
    let mut hints: Vec<TypeHint> = resolved
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.module_id == Some(module.id) && symbol.kind == ProjectSymbolKind::LocalBinding
        })
        .filter_map(|symbol| {
            let name = symbol.name_range?;
            let after = &text[name.end() as usize..];
            if after.trim_start().starts_with(':') {
                return None;
            }
            let fact = typed
                .expression_facts
                .get(&(module.id, symbol.semantic_id?))?;
            is_settled(typed, fact.success, 0).then(|| TypeHint {
                offset: name.end(),
                label: format!(": {}", typed.format_type(fact.success)),
            })
        })
        .collect();
    hints.sort_by_key(|hint| hint.offset);
    hints
}

/// Whether `id` is fully inferred: no part of it is a type error or an
/// unsolved inference variable.
fn is_settled(typed: &TypedProject, id: TypeId, depth: usize) -> bool {
    const DEPTH_LIMIT: usize = 32;
    if depth > DEPTH_LIMIT {
        return false;
    }
    let settled = |id: &TypeId| is_settled(typed, *id, depth + 1);
    match &typed.types[typed.resolve_type_id(id).raw() as usize] {
        CanonicalType::Poison | CanonicalType::InferenceVariable(_) => false,
        CanonicalType::Named { arguments, .. } => arguments.iter().all(settled),
        CanonicalType::Record(fields) => fields.iter().all(|field| settled(&field.type_id)),
        CanonicalType::Tuple(elements) => elements.iter().all(settled),
        CanonicalType::Generic(_)
        | CanonicalType::FailureSetValue(_)
        | CanonicalType::Function(_)
        | CanonicalType::PolymorphicFunction { .. } => true,
    }
}
