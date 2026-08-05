use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{SemanticId, TypeId};
use crate::resolution::{Resolution, ResolvedProgram};
use crate::semantic::Literal;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TypeFact {
    Known(TypeId),
    Poisoned,
}

#[derive(Debug)]
pub(crate) struct TypedProgram {
    pub(crate) resolved: Arc<ResolvedProgram>,
    pub(crate) construction_semantic_id: Option<SemanticId>,
    pub(crate) construction_type: Option<TypeFact>,
    pub(crate) literal_semantic_id: Option<SemanticId>,
    pub(crate) literal_type: Option<TypeFact>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

pub(crate) const INT_TYPE: TypeId = TypeId::new(0);
pub(crate) const TEXT_TYPE: TypeId = TypeId::new(1);
pub(crate) const STRUCT_TYPE: TypeId = TypeId::new(2);

pub(crate) fn check(resolved: ResolvedProgram) -> TypedProgram {
    let resolved = Arc::new(resolved);
    let mut diagnostics = resolved.diagnostics.clone();
    let function = resolved.semantic.functions.first();
    let construction_semantic_id = function.map(|function| function.body.semantic_id);
    let literal_semantic_id = function.map(|function| function.body.value.semantic_id());
    let mut literal_fact = None;

    let construction_type = function.map(|function| {
        let Some(Resolution::Resolved(_)) = resolved.constructor_resolution else {
            return TypeFact::Poisoned;
        };
        let Some(structure) = resolved.structure() else {
            return TypeFact::Poisoned;
        };

        if function.body.field_name.text != structure.field_name.text {
            diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4002",
                resolved.semantic.source.file_id(),
                function.body.field_name.range,
                format!(
                    "struct `{}` has no field `{}`",
                    structure.name.text, function.body.field_name.text
                ),
            ));
            return TypeFact::Poisoned;
        }

        let literal_value_type = match &function.body.value {
            Literal::Integer { .. } => INT_TYPE,
            Literal::Text { value, .. } => {
                let _ = value;
                TEXT_TYPE
            }
        };
        literal_fact = Some(TypeFact::Known(literal_value_type));
        if literal_value_type != INT_TYPE {
            diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4001",
                resolved.semantic.source.file_id(),
                function.body.value.range(),
                format!(
                    "field `{}` expects `Int`, found `Text`",
                    structure.field_name.text
                ),
            ));
            TypeFact::Poisoned
        } else {
            TypeFact::Known(STRUCT_TYPE)
        }
    });

    TypedProgram {
        resolved,
        construction_semantic_id,
        construction_type,
        literal_semantic_id,
        literal_type: literal_fact,
        diagnostics,
    }
}
