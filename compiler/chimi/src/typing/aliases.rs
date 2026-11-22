//! Type aliases (`docs/specification/executable-kernel.md`, "Type aliases").
//!
//! `type Name = Type` gives a type a second name. The alias is the same type as
//! what it names: a use of the name lowers to the aliased type itself, so no
//! check ever sees the alias. Lowering is lazy and cached, so an alias may be
//! used above its declaration and from another module.

use super::ProjectTypeChecker;
use crate::ids::{ModuleId, SymbolId, TypeId};
use crate::resolution::ProjectSymbolKind;
use crate::semantic::{ModuleDeclaration, Name, RequirementTermUse, TypeUse};
use std::collections::{HashMap, HashSet};

impl ProjectTypeChecker {
    /// The type `alias` names. Ensures: an alias that names itself, directly or
    /// through other aliases, lowers to `Poison` while it is being lowered, so a
    /// cycle terminates; its declaration reports it (`T4361`). Diagnostics are
    /// reported by the declaration check, not here, so a use never repeats them.
    pub(super) fn alias_type(&mut self, alias: SymbolId) -> TypeId {
        if let Some(type_id) = self.alias_types.get(&alias) {
            return *type_id;
        }
        let Some((module_id, annotation)) = self.alias_annotation(alias) else {
            return TypeId::new(0);
        };
        if !self.aliases_in_progress.insert(alias) {
            return TypeId::new(0);
        }
        let diagnostics = self.diagnostics.len();
        let warnings = self.warnings.len();
        let map_keys = self.written_map_keys.as_ref().map_or(0, Vec::len);
        let type_id = self.lower_value_type(module_id, &annotation, &HashMap::new());
        self.diagnostics.truncate(diagnostics);
        self.warnings.truncate(warnings);
        if let Some(keys) = self.written_map_keys.as_mut() {
            keys.truncate(map_keys);
        }
        self.aliases_in_progress.remove(&alias);
        self.alias_types.insert(alias, type_id);
        type_id
    }

    /// The type a written use of `alias` denotes. Ensures: type arguments on the
    /// use are `T4362` and ignored, because an alias takes none.
    pub(super) fn alias_use_type(
        &mut self,
        module_id: ModuleId,
        alias: SymbolId,
        name: &Name,
        arguments: &[TypeUse],
    ) -> TypeId {
        if !arguments.is_empty() {
            self.type_error(
                module_id,
                "T4362",
                name,
                format!("type alias `{}` takes no type arguments", name.text),
            );
        }
        self.alias_type(alias)
    }

    /// Checks one alias declaration and reports what its type reports. Ensures:
    /// an alias whose type reaches itself through aliases is `T4361`, because
    /// an alias is not a new type and so cannot be recursive; a struct or enum
    /// in between ends the chain.
    pub(super) fn check_type_alias(&mut self, module_id: ModuleId, declaration: &ModuleDeclaration) {
        let Some(annotation) = &declaration.annotation else {
            return;
        };
        self.lower_value_type(module_id, annotation, &HashMap::new());
        let Some(alias) = self.declaration_symbol(module_id, declaration) else {
            return;
        };
        let mut visited = HashSet::new();
        if let Some(path) = self.alias_cycle(alias, module_id, annotation, &mut visited) {
            let through = path
                .iter()
                .map(|symbol| format!("`{}`", self.resolved.symbol(*symbol).name))
                .collect::<Vec<_>>();
            let message = if through.is_empty() {
                format!(
                    "type alias `{}` names itself; an alias is another name for an existing \
                     type, so a recursive type must be a struct or enum",
                    declaration.name.text
                )
            } else {
                format!(
                    "type alias `{}` names itself through {}; an alias is another name for an \
                     existing type, so a recursive type must be a struct or enum",
                    declaration.name.text,
                    through.join(", ")
                )
            };
            self.type_error(module_id, "T4361", &declaration.name, message);
        }
    }

    /// The aliases between `type_use` and `target`, in order, when `type_use`
    /// reaches `target` through aliases only; `None` when it does not.
    fn alias_cycle(
        &self,
        target: SymbolId,
        module_id: ModuleId,
        type_use: &TypeUse,
        visited: &mut HashSet<SymbolId>,
    ) -> Option<Vec<SymbolId>> {
        let mut children = Vec::new();
        match type_use {
            TypeUse::Named {
                semantic_id,
                arguments,
                ..
            } => {
                if let Some(symbol) = self.resolved_type_symbol(module_id, *semantic_id)
                    && self.resolved.symbol(symbol).kind == ProjectSymbolKind::TypeAlias
                {
                    if symbol == target {
                        return Some(Vec::new());
                    }
                    if visited.insert(symbol)
                        && let Some((alias_module, annotation)) = self.alias_annotation(symbol)
                        && let Some(mut path) =
                            self.alias_cycle(target, alias_module, &annotation, visited)
                    {
                        path.insert(0, symbol);
                        return Some(path);
                    }
                }
                children.extend(arguments.iter());
            }
            TypeUse::Function {
                inputs,
                success,
                failures,
                requirements,
                ..
            } => {
                children.extend(inputs.iter());
                children.push(success);
                children.extend(failures.iter().map(|failures| &**failures));
                children.extend(requirements.terms.iter().filter_map(|term| match term {
                    RequirementTermUse::Field { interface, .. } => Some(interface),
                    RequirementTermUse::Variable { .. } | RequirementTermUse::Group { .. } => None,
                }));
            }
            TypeUse::Tuple { elements, .. } | TypeUse::Union {
                members: elements, ..
            } => children.extend(elements.iter()),
            TypeUse::Record { fields, .. } => {
                children.extend(fields.iter().map(|field| &field.type_use));
            }
        }
        children
            .into_iter()
            .find_map(|child| self.alias_cycle(target, module_id, child, visited))
    }

    /// The module and written type of the alias `alias`.
    fn alias_annotation(&self, alias: SymbolId) -> Option<(ModuleId, TypeUse)> {
        let symbol = self.resolved.symbol(alias);
        let (module_id, semantic_id) = (symbol.module_id?, symbol.semantic_id?);
        let annotation = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)?
            .annotation
            .clone()?;
        Some((module_id, annotation))
    }
}
