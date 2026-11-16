//! Binds standard modules written in Chimi to the operation registry: each
//! `native fn` to its operation, and each runtime-known declaration to the
//! runtime identity the backends construct and inspect.

use super::{ProjectResolver, ProjectSymbolKind, name_error};
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{ModuleId, SemanticId, SymbolId};
use crate::operations::{self, OperationClass, RUNTIME_KNOWN, RuntimeIdentity};
use crate::semantic::{FunctionSignature, ModuleDeclaration, ModuleDeclarationKind};
use crate::standard::{StandardConstructor, StandardSymbol, StandardType};
use crate::standard_tree;

impl ProjectResolver {
    pub(super) fn is_standard_module(&self, module_index: usize) -> bool {
        let module = &self.semantic.modules[module_index];
        standard_tree::is_standard(&module.path, &module.source)
    }

    /// `N3102`: the `std/` prefix belongs to the compiler's embedded tree.
    pub(super) fn reject_workspace_standard_path(&mut self, module_index: usize) {
        let module = &self.semantic.modules[module_index];
        if module.path.starts_with("std/") && !self.is_standard_module(module_index) {
            self.diagnostics.push(name_error(
                module,
                "N3102",
                crate::ids::TextRange::empty(0),
                format!(
                    "module path {} is reserved: `std/` modules come from the compiler",
                    module.path
                ),
            ));
        }
    }

    /// Finds the runtime-known declarations that loaded standard modules declare,
    /// before built-ins are synthesized for the rest.
    ///
    /// Ensures: an enum type is taken from source only with every registered
    /// variant present at its registered payload arity and no other variant, and
    /// a native type only from a `native type` declaration, and a provided
    /// interface only from an `interface` declaration; otherwise `N3105`
    /// names the mismatch and the built-in identity stays synthesized.
    pub(super) fn find_source_identities(&mut self) {
        for module_index in 0..self.semantic.modules.len() {
            if !self.is_standard_module(module_index) {
                continue;
            }
            let module = self.semantic.modules[module_index].clone();
            for entry in RUNTIME_KNOWN {
                let (kind, declaration_kind) = match entry.identity {
                    RuntimeIdentity::Type(kind) => (kind, ModuleDeclarationKind::Enum),
                    RuntimeIdentity::NativeType(kind) => (kind, ModuleDeclarationKind::NativeType),
                    RuntimeIdentity::Interface(kind) => (kind, ModuleDeclarationKind::Interface),
                    RuntimeIdentity::Constructor(_) => continue,
                };
                let Some(type_name) = entry
                    .id
                    .strip_prefix(module.path.as_str())
                    .and_then(|rest| rest.strip_prefix('.'))
                else {
                    continue;
                };
                let declaration = module.declarations.iter().find(|declaration| {
                    declaration.kind == declaration_kind && declaration.name.text == type_name
                });
                let Some(declaration) = declaration else {
                    let form = match declaration_kind {
                        ModuleDeclarationKind::NativeType => "native type",
                        ModuleDeclarationKind::Interface => "interface",
                        _ => "enum",
                    };
                    self.diagnostics.push(name_error(
                        &module,
                        "N3105",
                        crate::ids::TextRange::empty(0),
                        format!("runtime-known {form} {} is not declared", entry.id),
                    ));
                    continue;
                };
                if declaration_kind != ModuleDeclarationKind::Enum {
                    self.source_identities.insert(
                        (module.id, declaration.semantic_id),
                        StandardSymbol::Type(kind),
                    );
                    continue;
                }
                match source_variants(entry.id, declaration) {
                    Ok(variants) => {
                        self.source_identities.insert(
                            (module.id, declaration.semantic_id),
                            StandardSymbol::Type(kind),
                        );
                        for (semantic_id, constructor) in variants {
                            self.source_identities.insert(
                                (module.id, semantic_id),
                                StandardSymbol::Constructor(constructor),
                            );
                        }
                    }
                    Err(message) => self.diagnostics.push(name_error(
                        &module,
                        "N3105",
                        declaration.name.range,
                        message,
                    )),
                }
            }
        }
    }

    /// Gives a source declaration the runtime identity found for it, so the
    /// backends and every standard contract see one symbol per identity.
    pub(super) fn bind_source_identity(
        &mut self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        symbol: SymbolId,
    ) {
        let Some(identity) = self
            .source_identities
            .get(&(module_id, semantic_id))
            .copied()
        else {
            return;
        };
        if self.symbols[symbol.raw() as usize].kind == ProjectSymbolKind::StructConstructor {
            return;
        }
        self.standard_symbols.insert(symbol, identity);
        match identity {
            StandardSymbol::Type(kind) => {
                self.standard_types.insert(kind, symbol);
                if kind == StandardType::Option {
                    self.builtin_types.insert("Option".to_owned(), symbol);
                }
            }
            StandardSymbol::Constructor(
                constructor @ (StandardConstructor::OptionSome | StandardConstructor::OptionNone),
            ) => {
                self.prelude_values
                    .insert(constructor.name().to_owned(), symbol);
            }
            _ => {}
        }
    }

    /// Reports a `native type` that cannot take a registry identity: outside the
    /// standard tree (`N3101`) or with no registered native type of its
    /// qualified name (`N3104`). A registered one took its identity in
    /// [`Self::find_source_identities`].
    pub(super) fn check_native_type(
        &mut self,
        module_index: usize,
        declaration: &ModuleDeclaration,
    ) {
        let module = &self.semantic.modules[module_index];
        let name = &declaration.name;
        let diagnostic = if !self.is_standard_module(module_index) {
            name_error(
                module,
                "N3101",
                name.range,
                format!(
                    "`native type {}`: only modules of the embedded standard tree declare native items",
                    name.text
                ),
            )
        } else {
            let id = format!("{}.{}", module.path, name.text);
            if matches!(
                operations::runtime_known(&id),
                Some(RuntimeIdentity::NativeType(_))
            ) {
                return;
            }
            name_error(
                module,
                "N3104",
                name.range,
                format!("no registered native type is named {id}"),
            )
        };
        self.diagnostics.push(diagnostic);
    }

    /// Binds a `native fn` to its registry operation, or reports why it cannot be
    /// bound: outside the standard tree (`N3101`), unregistered (`N3104`), a
    /// header the registry cannot check (`T4320`), or a public host operation
    /// (`N3106`).
    pub(super) fn bind_native_function(
        &mut self,
        module_index: usize,
        declaration: &ModuleDeclaration,
        signature: &FunctionSignature,
        symbol: SymbolId,
    ) {
        let module = self.semantic.modules[module_index].clone();
        let name = &declaration.name;
        if !self.is_standard_module(module_index) {
            self.diagnostics.push(name_error(
                &module,
                "N3101",
                name.range,
                format!(
                    "`native fn {}`: only modules of the embedded standard tree declare native items",
                    name.text
                ),
            ));
            return;
        }
        let id = format!("{}.{}", module.path, name.text);
        let Some(operation) = operations::operation(&id) else {
            self.diagnostics.push(name_error(
                &module,
                "N3104",
                name.range,
                format!("no registered operation is named {id}"),
            ));
            return;
        };
        let unannotated = signature.success.is_none()
            || signature
                .parameters
                .iter()
                .any(|parameter| parameter.type_use.is_none());
        if unannotated || signature.parameters.len() != operation.arity {
            let message = if unannotated {
                format!(
                    "`native fn {}` must annotate every parameter and its success type",
                    name.text
                )
            } else {
                format!(
                    "`native fn {}` takes {} parameters, but operation {id} takes {}",
                    name.text,
                    signature.parameters.len(),
                    operation.arity
                )
            };
            self.diagnostics.push(Diagnostic::error(
                DiagnosticStage::Type,
                "T4320",
                module.source.file_id(),
                name.range,
                message,
            ));
            return;
        }
        if operation.class == OperationClass::Host && declaration.public {
            self.diagnostics.push(name_error(
                &module,
                "N3106",
                name.range,
                format!("host operation {id} must be private; expose it through a provider"),
            ));
            return;
        }
        self.native_operations
            .insert(symbol, operation.implementation);
    }
}

/// Each registered variant of runtime-known enum `type_id` with its source
/// identity, or why the declaration does not match the registry.
fn source_variants(
    type_id: &str,
    declaration: &ModuleDeclaration,
) -> Result<Vec<(SemanticId, StandardConstructor)>, String> {
    let mut variants = Vec::new();
    for variant in &declaration.variants {
        let id = format!("{type_id}.{}", variant.name.text);
        let Some(RuntimeIdentity::Constructor(constructor)) = operations::runtime_known(&id) else {
            return Err(format!(
                "variant {id} is not a registered runtime-known declaration"
            ));
        };
        let expected = constructor.field_names().len();
        if variant.fields.len() != expected {
            return Err(format!(
                "variant {id} has {} payload fields, but the runtime constructs it with {expected}",
                variant.fields.len()
            ));
        }
        variants.push((variant.semantic_id, constructor));
    }
    for entry in RUNTIME_KNOWN {
        let registered_variant = entry
            .id
            .strip_prefix(type_id)
            .and_then(|rest| rest.strip_prefix('.'));
        if let Some(variant_name) = registered_variant
            && !declaration
                .variants
                .iter()
                .any(|variant| variant.name.text == variant_name)
        {
            return Err(format!(
                "runtime-known variant {} is not declared",
                entry.id
            ));
        }
    }
    Ok(variants)
}
