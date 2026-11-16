//! Every place a symbol's name is written in a project: its declaration and its
//! references. The editor's find-references and rename answer from this.

use crate::ids::{SymbolId, TextRange};
use crate::resolution::{Namespace, ProjectSymbolKind, ReferenceResolution, ResolvedProject};
use crate::semantic::{
    CaseOrigin, ImportBinding, ModuleExpression, SemanticModule, is_generated_name,
};
use crate::typing::TypedProject;
use std::path::{Path, PathBuf};

/// The symbol named at a position, with every place its name is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolOccurrences {
    name: String,
    kind: &'static str,
    renamable: bool,
    occurrences: Vec<Occurrence>,
}

/// One place a symbol's name is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Occurrence {
    module_path: String,
    source_path: PathBuf,
    /// The name itself: for `Text.length`, only `length`.
    range: TextRange,
    declaration: bool,
}

impl SymbolOccurrences {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    /// Whether every place the name is written is the project's own source:
    /// false for standard and built-in symbols and for module aliases.
    pub const fn is_renamable(&self) -> bool {
        self.renamable
    }

    /// Declarations first, then references, each in module then source order.
    pub fn occurrences(&self) -> &[Occurrence] {
        &self.occurrences
    }
}

impl Occurrence {
    pub fn module_path(&self) -> &str {
        &self.module_path
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }

    pub const fn is_declaration(&self) -> bool {
        self.declaration
    }
}

/// The symbol whose reference or declaration name holds `offset` in
/// `module_path`, or `None` when no symbol's name is there.
pub(crate) fn occurrences_at(
    typed: &TypedProject,
    module_path: &str,
    offset: usize,
) -> Option<SymbolOccurrences> {
    let resolved = &typed.resolved;
    let module = resolved
        .semantic
        .modules
        .iter()
        .find(|module| module.path == module_path)?;
    let target = symbol_at(resolved, module, offset)?;
    let symbol = resolved.symbol(target);
    // A struct's type and constructor, or a type and its value, are one
    // declaration with one name.
    let targets: Vec<SymbolId> = match (symbol.module_id, symbol.semantic_id) {
        (Some(module_id), Some(semantic_id)) => resolved
            .symbols
            .iter()
            .filter(|other| {
                other.module_id == Some(module_id)
                    && other.semantic_id == Some(semantic_id)
                    && other.name == symbol.name
            })
            .map(|other| other.id)
            .collect(),
        _ => vec![target],
    };
    let mut occurrences = declarations(resolved, target);
    let source_backed = !occurrences.is_empty();
    let mut references: Vec<Occurrence> = resolved
        .references
        .iter()
        .filter(|reference| {
            reference_target(reference.resolution).is_some_and(|id| targets.contains(&id))
                && !is_generated_name(&reference.text)
        })
        .map(|reference| {
            let module = &resolved.semantic.modules[reference.file_id.raw() as usize];
            occurrence(module, reference.range, false)
        })
        .collect();
    references.extend(imported_occurrences(resolved, target));
    references.sort_by(|left, right| {
        (&left.module_path, left.range.start()).cmp(&(&right.module_path, right.range.start()))
    });
    occurrences.extend(references);
    // A built-in or a module alias declares no name in source.
    let renamable = source_backed
        && !occurrences
            .iter()
            .any(|occurrence| occurrence.module_path.starts_with("std/"));
    Some(SymbolOccurrences {
        name: symbol.name.clone(),
        kind: symbol.kind.name(),
        renamable,
        occurrences,
    })
}

/// The symbol whose reference or declaration name holds `offset` in
/// `module`.
pub(crate) fn symbol_at(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    offset: usize,
) -> Option<SymbolId> {
    referenced_at(resolved, module, offset).or_else(|| declared_at(resolved, module, offset))
}

fn reference_target(resolution: ReferenceResolution) -> Option<SymbolId> {
    match resolution {
        ReferenceResolution::Resolved(symbol) | ReferenceResolution::Inaccessible(symbol) => {
            Some(symbol)
        }
        _ => None,
    }
}

fn contains(range: TextRange, offset: usize) -> bool {
    range.start() as usize <= offset && offset <= range.end() as usize
}

fn referenced_at(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    offset: usize,
) -> Option<SymbolId> {
    resolved
        .references
        .iter()
        .filter(|reference| {
            reference.file_id == module.source.file_id()
                && contains(reference.range, offset)
                && !is_generated_name(&reference.text)
        })
        .min_by_key(|reference| reference.range.end() - reference.range.start())
        .and_then(|reference| {
            reference_target(reference.resolution).or_else(|| {
                (reference.namespace == Namespace::Value)
                    .then(|| struct_in_scope(resolved, module, &reference.text))
                    .flatten()
            })
        })
}

/// The struct named `name` that a constructor call in `module` builds: one
/// the module declares or imports by name.
fn struct_in_scope(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    name: &str,
) -> Option<SymbolId> {
    let declaring = std::iter::once(module.path.as_str()).chain(
        module
            .imports
            .iter()
            .filter(|import| match &import.binding {
                ImportBinding::Members(members) => members.iter().any(|member| member.text == name),
                _ => false,
            })
            .map(|import| import.path.as_str()),
    );
    declaring
        .filter_map(|path| {
            resolved
                .semantic
                .modules
                .iter()
                .find(|candidate| candidate.path == path)
        })
        .find_map(|declaring| {
            resolved.symbols.iter().find(|symbol| {
                symbol.module_id == Some(declaring.id)
                    && symbol.kind == ProjectSymbolKind::Struct
                    && symbol.name == name
            })
        })
        .map(|symbol| symbol.id)
}

fn declared_at(
    resolved: &ResolvedProject,
    module: &SemanticModule,
    offset: usize,
) -> Option<SymbolId> {
    resolved
        .symbols
        .iter()
        .filter(|symbol| symbol.module_id == Some(module.id))
        .find(|symbol| {
            declaration_ranges(resolved, symbol.id)
                .iter()
                .any(|range| contains(*range, offset))
        })
        .map(|symbol| symbol.id)
}

fn declarations(resolved: &ResolvedProject, symbol: SymbolId) -> Vec<Occurrence> {
    let Some(module_id) = resolved.symbol(symbol).module_id else {
        return Vec::new();
    };
    let module = &resolved.semantic.modules[module_id.raw() as usize];
    declaration_ranges(resolved, symbol)
        .into_iter()
        .map(|range| occurrence(module, range, true))
        .collect()
}

/// Where `symbol` names itself: a local's binding, an item's name, each clause
/// head of a clause group, or a field or variant name.
fn declaration_ranges(resolved: &ResolvedProject, symbol: SymbolId) -> Vec<TextRange> {
    let symbol = resolved.symbol(symbol);
    if let Some(range) = symbol.name_range {
        return vec![range];
    }
    let (Some(module_id), Some(semantic_id)) = (symbol.module_id, symbol.semantic_id) else {
        return Vec::new();
    };
    let module = &resolved.semantic.modules[module_id.raw() as usize];
    for declaration in &module.declarations {
        if declaration.semantic_id == semantic_id {
            let mut ranges = vec![declaration.name.range];
            if let Some(ModuleExpression::Case {
                arms,
                origin: CaseOrigin::Clauses { .. },
                ..
            }) = &declaration.body
            {
                let text = module.source.text();
                let length = declaration.name.text.len();
                for arm in arms {
                    let start = arm.range.start() as usize;
                    let head = TextRange::new(start, start + length);
                    if text.get(start..start + length) == Some(declaration.name.text.as_str())
                        && head != declaration.name.range
                    {
                        ranges.push(head);
                    }
                }
            }
            return ranges;
        }
        let members = declaration
            .variants
            .iter()
            .map(|variant| (variant.semantic_id, variant.name.range))
            .chain(declaration.variants.iter().flat_map(|variant| {
                variant
                    .fields
                    .iter()
                    .map(|field| (field.semantic_id, field.name.range))
            }))
            .chain(
                declaration
                    .fields
                    .iter()
                    .map(|field| (field.semantic_id, field.name.range)),
            );
        for (member, range) in members {
            if member == semantic_id {
                return vec![range];
            }
        }
    }
    Vec::new()
}

/// The names an item is written by outside its references: each member import
/// of it (`import lib.{Point}`), and for a struct, each constructor call the
/// type checker resolves through the type in scope (`Point(1, 2)`), which name
/// resolution leaves unresolved.
fn imported_occurrences(resolved: &ResolvedProject, target: SymbolId) -> Vec<Occurrence> {
    let symbol = resolved.symbol(target);
    // Only a module-level item can be imported; a local of the same name in
    // the declaring module is another symbol.
    if matches!(
        symbol.kind,
        ProjectSymbolKind::GenericParameter
            | ProjectSymbolKind::Parameter
            | ProjectSymbolKind::Requirement
            | ProjectSymbolKind::LocalBinding
            | ProjectSymbolKind::PatternBinding
    ) {
        return Vec::new();
    }
    let Some(declaring) = symbol
        .module_id
        .map(|module_id| &resolved.semantic.modules[module_id.raw() as usize])
    else {
        return Vec::new();
    };
    let mut occurrences = Vec::new();
    let struct_constructor = matches!(
        symbol.kind,
        ProjectSymbolKind::Struct | ProjectSymbolKind::StructConstructor
    );
    for module in &resolved.semantic.modules {
        let mut in_scope = module.id == declaring.id;
        for import in module
            .imports
            .iter()
            .filter(|import| import.path == declaring.path)
        {
            if let ImportBinding::Members(members) = &import.binding {
                for member in members.iter().filter(|member| member.text == symbol.name) {
                    in_scope = true;
                    occurrences.push(occurrence(module, member.range, false));
                }
            }
        }
        if struct_constructor && in_scope {
            occurrences.extend(
                resolved
                    .references
                    .iter()
                    .filter(|reference| {
                        reference.file_id == module.source.file_id()
                            && reference.namespace == Namespace::Value
                            && reference.text == symbol.name
                            && reference.resolution == ReferenceResolution::Unresolved
                    })
                    .map(|reference| occurrence(module, reference.range, false)),
            );
        }
    }
    occurrences
}

fn occurrence(module: &SemanticModule, range: TextRange, declaration: bool) -> Occurrence {
    Occurrence {
        module_path: module.path.clone(),
        source_path: module.source.path().to_owned(),
        range,
        declaration,
    }
}
