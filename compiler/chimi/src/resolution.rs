use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{SemanticId, SymbolId};
use crate::semantic::{SemanticProgram, StructDeclaration};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SymbolKind {
    Struct,
    Field,
    Function,
}

#[derive(Clone, Debug)]
pub(crate) struct Symbol {
    pub(crate) id: SymbolId,
    pub(crate) kind: SymbolKind,
    pub(crate) name: String,
    pub(crate) semantic_id: SemanticId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Resolution {
    Resolved(SymbolId),
    Unresolved,
}

#[derive(Debug)]
pub(crate) struct ResolvedProgram {
    pub(crate) semantic: Arc<SemanticProgram>,
    pub(crate) symbols: Vec<Symbol>,
    pub(crate) constructor_resolution: Option<Resolution>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl ResolvedProgram {
    pub(crate) fn symbol(&self, symbol_id: SymbolId) -> &Symbol {
        self.symbols
            .get(symbol_id.raw() as usize)
            .filter(|symbol| symbol.id == symbol_id)
            .expect("resolved symbol ID belongs to this snapshot")
    }

    pub(crate) fn structure(&self) -> Option<&StructDeclaration> {
        self.semantic.structure.as_ref()
    }
}

pub(crate) fn resolve(semantic: SemanticProgram) -> ResolvedProgram {
    let semantic = Arc::new(semantic);
    let mut symbols = Vec::new();
    if let Some(structure) = &semantic.structure {
        symbols.push(Symbol {
            id: SymbolId::new(symbols.len() as u32),
            kind: SymbolKind::Struct,
            name: structure.name.text.clone(),
            semantic_id: structure.semantic_id,
        });
        symbols.push(Symbol {
            id: SymbolId::new(symbols.len() as u32),
            kind: SymbolKind::Field,
            name: structure.field_name.text.clone(),
            semantic_id: structure.field_semantic_id,
        });
    }
    for function in &semantic.functions {
        symbols.push(Symbol {
            id: SymbolId::new(symbols.len() as u32),
            kind: SymbolKind::Function,
            name: function.name.text.clone(),
            semantic_id: function.semantic_id,
        });
    }

    let mut diagnostics = semantic.diagnostics.clone();
    if let Some(structure) = &semantic.structure
        && structure.field_type.text != "Int"
    {
        diagnostics.push(Diagnostic::error(
            DiagnosticStage::Name,
            "N3002",
            semantic.source.file_id(),
            structure.field_type.range,
            format!("unresolved type `{}`", structure.field_type.text),
        ));
    }

    let constructor_resolution = semantic.functions.first().map(|function| {
        let resolved = semantic
            .structure
            .as_ref()
            .is_some_and(|structure| structure.name.text == function.body.constructor.text);
        if resolved {
            Resolution::Resolved(SymbolId::new(0))
        } else {
            diagnostics.push(Diagnostic::error(
                DiagnosticStage::Name,
                "N3001",
                semantic.source.file_id(),
                function.body.constructor.range,
                format!(
                    "unresolved constructor `{}`",
                    function.body.constructor.text
                ),
            ));
            Resolution::Unresolved
        }
    });

    ResolvedProgram {
        semantic,
        symbols,
        constructor_resolution,
        diagnostics,
    }
}
