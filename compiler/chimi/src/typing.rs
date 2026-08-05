use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FailureRowId, ModuleId, NeedsRowId, SemanticId, SymbolId, TypeId};
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, Resolution, ResolvedProgram, ResolvedProject,
};
use crate::semantic::Literal;
use crate::semantic::{
    FunctionSignature, ModuleDeclaration, ModuleDeclarationKind, RequirementRowUse,
    RequirementTermUse, TypeUse,
};
use std::collections::HashMap;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GenericKind {
    Unconstrained,
    Value,
    FailureRow,
    NeedsRow,
    Conflicting,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CanonicalType {
    Named {
        symbol: SymbolId,
        arguments: Vec<TypeId>,
    },
    Generic(SymbolId),
    Function(CallableContract),
    InferenceVariable(u32),
    Poison,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CallableContract {
    pub(crate) inputs: Vec<TypeId>,
    pub(crate) success: TypeId,
    pub(crate) failures: FailureRowId,
    pub(crate) needs: NeedsRowId,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FailureRow {
    pub(crate) variants: Vec<SymbolId>,
    pub(crate) parameters: Vec<SymbolId>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Need {
    pub(crate) requirement: SymbolId,
    pub(crate) lexical_name: String,
    pub(crate) interface: TypeId,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct NeedsRow {
    pub(crate) entries: Vec<Need>,
    pub(crate) parameters: Vec<SymbolId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FieldShape {
    pub(crate) semantic_id: SemanticId,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
}

#[derive(Debug)]
pub(crate) struct TypedProject {
    pub(crate) resolved: Arc<ResolvedProject>,
    pub(crate) types: Vec<CanonicalType>,
    pub(crate) failure_rows: Vec<FailureRow>,
    pub(crate) needs_rows: Vec<NeedsRow>,
    pub(crate) contracts: HashMap<SymbolId, CallableContract>,
    pub(crate) declaration_types: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) type_facts: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) fields: HashMap<SymbolId, Vec<FieldShape>>,
    pub(crate) generic_kinds: HashMap<SymbolId, GenericKind>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

pub(crate) fn check_project(resolved: ResolvedProject) -> TypedProject {
    ProjectTypeChecker::new(resolved).check()
}

impl TypedProject {
    pub(crate) fn contract(
        &self,
        module_path: &str,
        function_name: &str,
    ) -> Option<&CallableContract> {
        let module = self
            .resolved
            .semantic
            .modules
            .iter()
            .find(|module| module.path == module_path)?;
        let symbol = self.resolved.symbols.iter().find(|symbol| {
            symbol.module_id == Some(module.id)
                && symbol.kind == ProjectSymbolKind::Function
                && symbol.name == function_name
        })?;
        self.contracts.get(&symbol.id)
    }

    pub(crate) fn format_function_contract(&self, contract: &CallableContract) -> String {
        let inputs = contract
            .inputs
            .iter()
            .map(|input| self.format_type(*input))
            .collect::<Vec<_>>()
            .join(", ");
        format!("fn({inputs}) -> {}", self.format_contract_result(contract))
    }

    pub(crate) fn generic_kind(
        &self,
        module_path: &str,
        declaration_name: &str,
        generic_name: &str,
    ) -> Option<GenericKind> {
        let module = self
            .resolved
            .semantic
            .modules
            .iter()
            .find(|module| module.path == module_path)?;
        let declaration = module
            .declarations
            .iter()
            .find(|declaration| declaration.name.text == declaration_name)?;
        let symbol = self.resolved.symbols.iter().find(|symbol| {
            symbol.module_id == Some(module.id)
                && symbol.semantic_id == Some(declaration.semantic_id)
                && symbol.kind == ProjectSymbolKind::GenericParameter
                && symbol.name == generic_name
        })?;
        self.generic_kinds.get(&symbol.id).copied()
    }

    fn format_type(&self, id: TypeId) -> String {
        match &self.types[id.raw() as usize] {
            CanonicalType::Named { symbol, arguments } => {
                let mut result = self.resolved.symbol(*symbol).name.clone();
                if !arguments.is_empty() {
                    result.push('<');
                    result.push_str(
                        &arguments
                            .iter()
                            .map(|argument| self.format_type(*argument))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    result.push('>');
                }
                result
            }
            CanonicalType::Generic(symbol) => self.resolved.symbol(*symbol).name.clone(),
            CanonicalType::Function(contract) => self.format_function_contract(contract),
            CanonicalType::InferenceVariable(_) => "_".to_owned(),
            CanonicalType::Poison => "<invalid>".to_owned(),
        }
    }

    fn format_contract_result(&self, contract: &CallableContract) -> String {
        let mut result = self.format_type(contract.success);
        let failures = &self.failure_rows[contract.failures.raw() as usize];
        if !failures.variants.is_empty() || !failures.parameters.is_empty() {
            result.push_str(" errors ");
            result.push_str(&self.format_failure_row(failures));
        }
        let needs = &self.needs_rows[contract.needs.raw() as usize];
        if !needs.entries.is_empty() || !needs.parameters.is_empty() {
            result.push_str(" needs ");
            result.push_str(&self.format_needs_row(needs));
        }
        result
    }

    fn format_failure_row(&self, row: &FailureRow) -> String {
        let mut members = row
            .parameters
            .iter()
            .map(|symbol| self.resolved.symbol(*symbol).name.clone())
            .collect::<Vec<_>>();
        let mut variants_by_enum: HashMap<SymbolId, Vec<SymbolId>> = HashMap::new();
        for variant in &row.variants {
            if let Some(owner) = self.enum_for_variant(*variant) {
                variants_by_enum.entry(owner).or_default().push(*variant);
            }
        }
        let mut enum_groups = variants_by_enum.into_iter().collect::<Vec<_>>();
        enum_groups.sort_by_key(|(owner, _)| *owner);
        for (owner, mut variants) in enum_groups {
            variants.sort_unstable();
            let all_variants = self.enum_variants(owner);
            if variants == all_variants {
                members.push(self.resolved.symbol(owner).name.clone());
            } else {
                members.extend(variants.into_iter().map(|variant| {
                    format!(
                        "{}.{}",
                        self.resolved.symbol(owner).name,
                        self.resolved.symbol(variant).name
                    )
                }));
            }
        }
        members.sort();
        if members.len() == 1 {
            members.pop().expect("one member")
        } else {
            format!("{{ {} }}", members.join(", "))
        }
    }

    fn format_needs_row(&self, row: &NeedsRow) -> String {
        let parameters = row
            .parameters
            .iter()
            .map(|symbol| self.resolved.symbol(*symbol).name.as_str())
            .collect::<Vec<_>>()
            .join(" + ");
        let entries = format!(
            "{{ {} }}",
            row.entries
                .iter()
                .map(|need| format!(
                    "{}: {}",
                    need.lexical_name,
                    self.format_type(need.interface)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        match (parameters.is_empty(), row.entries.is_empty()) {
            (false, false) => format!("{parameters} + {entries}"),
            (false, true) => parameters,
            (true, false) => entries,
            (true, true) => "{}".to_owned(),
        }
    }

    fn enum_for_variant(&self, variant: SymbolId) -> Option<SymbolId> {
        let variant = self.resolved.symbol(variant);
        let module_id = variant.module_id?;
        let variant_semantic_id = variant.semantic_id?;
        let declaration = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| {
                declaration.kind == ModuleDeclarationKind::Enum
                    && declaration
                        .variants
                        .iter()
                        .any(|candidate| candidate.semantic_id == variant_semantic_id)
            })?;
        self.resolved
            .symbols
            .iter()
            .find(|symbol| {
                symbol.module_id == Some(module_id)
                    && symbol.semantic_id == Some(declaration.semantic_id)
                    && symbol.kind == ProjectSymbolKind::Enum
            })
            .map(|symbol| symbol.id)
    }

    fn enum_variants(&self, owner: SymbolId) -> Vec<SymbolId> {
        let owner = self.resolved.symbol(owner);
        let module_id = owner.module_id.expect("enum is source-backed");
        let declaration = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| {
                declaration.semantic_id == owner.semantic_id.expect("enum semantic identity")
            })
            .expect("enum symbol owns a declaration");
        let mut variants = declaration
            .variants
            .iter()
            .filter_map(|variant| {
                self.resolved
                    .symbols
                    .iter()
                    .find(|symbol| {
                        symbol.module_id == Some(module_id)
                            && symbol.semantic_id == Some(variant.semantic_id)
                            && symbol.kind == ProjectSymbolKind::EnumConstructor
                    })
                    .map(|symbol| symbol.id)
            })
            .collect::<Vec<_>>();
        variants.sort_unstable();
        variants
    }
}

struct ProjectTypeChecker {
    resolved: Arc<ResolvedProject>,
    types: Vec<CanonicalType>,
    type_intern: HashMap<CanonicalType, TypeId>,
    failure_rows: Vec<FailureRow>,
    failure_row_intern: HashMap<FailureRow, FailureRowId>,
    needs_rows: Vec<NeedsRow>,
    needs_row_intern: HashMap<NeedsRow, NeedsRowId>,
    contracts: HashMap<SymbolId, CallableContract>,
    declaration_types: HashMap<(ModuleId, SemanticId), TypeId>,
    type_facts: HashMap<(ModuleId, SemanticId), TypeId>,
    fields: HashMap<SymbolId, Vec<FieldShape>>,
    generic_kinds: HashMap<SymbolId, GenericKind>,
    diagnostics: Vec<Diagnostic>,
    next_inference_variable: u32,
}

impl ProjectTypeChecker {
    fn new(resolved: ResolvedProject) -> Self {
        let resolved = Arc::new(resolved);
        let diagnostics = resolved.diagnostics.clone();
        let mut checker = Self {
            resolved,
            types: Vec::new(),
            type_intern: HashMap::new(),
            failure_rows: Vec::new(),
            failure_row_intern: HashMap::new(),
            needs_rows: Vec::new(),
            needs_row_intern: HashMap::new(),
            contracts: HashMap::new(),
            declaration_types: HashMap::new(),
            type_facts: HashMap::new(),
            fields: HashMap::new(),
            generic_kinds: HashMap::new(),
            diagnostics,
            next_inference_variable: 0,
        };
        checker.intern_type(CanonicalType::Poison);
        checker.intern_failure_row(FailureRow {
            variants: Vec::new(),
            parameters: Vec::new(),
        });
        checker.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: Vec::new(),
        });
        checker
    }

    fn check(mut self) -> TypedProject {
        let order = self.resolved.dependency_order.clone();
        for module_id in order {
            let declarations = self.resolved.semantic.modules[module_id.raw() as usize]
                .declarations
                .clone();
            for declaration in declarations {
                self.check_declaration(module_id, &declaration);
            }
        }
        TypedProject {
            resolved: self.resolved,
            types: self.types,
            failure_rows: self.failure_rows,
            needs_rows: self.needs_rows,
            contracts: self.contracts,
            declaration_types: self.declaration_types,
            type_facts: self.type_facts,
            fields: self.fields,
            generic_kinds: self.generic_kinds,
            diagnostics: self.diagnostics,
        }
    }

    fn check_declaration(&mut self, module_id: ModuleId, declaration: &ModuleDeclaration) {
        let generic_symbols = declaration
            .generic_parameters
            .iter()
            .filter_map(|name| {
                self.find_symbol(
                    module_id,
                    Some(declaration.semantic_id),
                    ProjectSymbolKind::GenericParameter,
                    &name.text,
                )
                .map(|symbol| (name.text.clone(), symbol))
            })
            .collect::<HashMap<_, _>>();
        for symbol in generic_symbols.values() {
            self.generic_kinds
                .insert(*symbol, GenericKind::Unconstrained);
        }

        let Some(declaration_symbol) = self.declaration_symbol(module_id, declaration) else {
            return;
        };
        match declaration.kind {
            ModuleDeclarationKind::Struct
            | ModuleDeclarationKind::Enum
            | ModuleDeclarationKind::Interface => {
                let arguments = declaration
                    .generic_parameters
                    .iter()
                    .filter_map(|name| generic_symbols.get(&name.text).copied())
                    .map(|symbol| self.intern_type(CanonicalType::Generic(symbol)))
                    .collect();
                let declaration_type = self.intern_type(CanonicalType::Named {
                    symbol: declaration_symbol,
                    arguments,
                });
                self.declaration_types
                    .insert((module_id, declaration.semantic_id), declaration_type);
                let declaration_fields = declaration
                    .fields
                    .iter()
                    .map(|field| FieldShape {
                        semantic_id: field.semantic_id,
                        name: field.name.text.clone(),
                        type_id: self.lower_value_type(
                            module_id,
                            &field.type_use,
                            &generic_symbols,
                        ),
                    })
                    .collect();
                self.fields.insert(declaration_symbol, declaration_fields);
                for variant in &declaration.variants {
                    let variant_fields = variant
                        .fields
                        .iter()
                        .map(|field| FieldShape {
                            semantic_id: field.semantic_id,
                            name: field.name.text.clone(),
                            type_id: self.lower_value_type(
                                module_id,
                                &field.type_use,
                                &generic_symbols,
                            ),
                        })
                        .collect::<Vec<_>>();
                    let inputs = variant_fields.iter().map(|field| field.type_id).collect();
                    if let Some(constructor) = self.find_symbol(
                        module_id,
                        Some(variant.semantic_id),
                        ProjectSymbolKind::EnumConstructor,
                        &variant.name.text,
                    ) {
                        let contract = CallableContract {
                            inputs,
                            success: declaration_type,
                            failures: FailureRowId::new(0),
                            needs: NeedsRowId::new(0),
                        };
                        self.contracts.insert(constructor, contract.clone());
                        self.fields.insert(constructor, variant_fields);
                        self.intern_type(CanonicalType::Function(contract));
                    }
                }
                if declaration.kind == ModuleDeclarationKind::Struct {
                    let inputs = self.fields[&declaration_symbol]
                        .iter()
                        .map(|field| field.type_id)
                        .collect();
                    if let Some(constructor) = self.find_symbol(
                        module_id,
                        Some(declaration.semantic_id),
                        ProjectSymbolKind::StructConstructor,
                        &declaration.name.text,
                    ) {
                        let contract = CallableContract {
                            inputs,
                            success: declaration_type,
                            failures: FailureRowId::new(0),
                            needs: NeedsRowId::new(0),
                        };
                        self.contracts.insert(constructor, contract.clone());
                        self.intern_type(CanonicalType::Function(contract));
                    }
                }
            }
            ModuleDeclarationKind::Function => {
                let signature = declaration
                    .function
                    .as_ref()
                    .expect("function declaration owns a signature");
                let contract = self.lower_signature(module_id, signature, &generic_symbols);
                self.contracts.insert(declaration_symbol, contract.clone());
                let function_type = self.intern_type(CanonicalType::Function(contract));
                self.declaration_types
                    .insert((module_id, declaration.semantic_id), function_type);
            }
            ModuleDeclarationKind::Binding => {}
        }
    }

    fn lower_signature(
        &mut self,
        module_id: ModuleId,
        signature: &FunctionSignature,
        generics: &HashMap<String, SymbolId>,
    ) -> CallableContract {
        let inputs = signature
            .parameters
            .iter()
            .map(|parameter| match &parameter.type_use {
                Some(type_use) => self.lower_value_type(module_id, type_use, generics),
                None => self.fresh_inference_type(),
            })
            .collect();
        let success = match &signature.success {
            Some(type_use) => self.lower_value_type(module_id, type_use, generics),
            None => self.fresh_inference_type(),
        };
        let needs = self.lower_needs_row(module_id, &signature.requirements, generics);
        CallableContract {
            inputs,
            success,
            failures: FailureRowId::new(0),
            needs,
        }
    }

    fn lower_value_type(
        &mut self,
        module_id: ModuleId,
        type_use: &TypeUse,
        generics: &HashMap<String, SymbolId>,
    ) -> TypeId {
        let result = match type_use {
            TypeUse::Named {
                semantic_id,
                path,
                arguments,
            } => {
                match (
                    path.last(),
                    self.resolved_type_symbol(module_id, *semantic_id),
                ) {
                    (Some(name), Some(symbol)) => {
                        let kind = self.resolved.symbol(symbol).kind;
                        if kind == ProjectSymbolKind::GenericParameter {
                            self.constrain_generic(module_id, symbol, GenericKind::Value, name);
                            self.intern_type(CanonicalType::Generic(symbol))
                        } else {
                            let arguments = arguments
                                .iter()
                                .map(|argument| {
                                    self.lower_value_type(module_id, argument, generics)
                                })
                                .collect();
                            self.intern_type(CanonicalType::Named { symbol, arguments })
                        }
                    }
                    _ => TypeId::new(0),
                }
            }
            TypeUse::Function {
                inputs,
                success,
                failures,
                requirements,
                ..
            } => {
                let inputs = inputs
                    .iter()
                    .map(|input| self.lower_value_type(module_id, input, generics))
                    .collect();
                let success = self.lower_value_type(module_id, success, generics);
                let failures = failures.as_ref().map_or(FailureRowId::new(0), |failures| {
                    self.lower_failure_row(module_id, failures, generics)
                });
                let needs = self.lower_needs_row(module_id, requirements, generics);
                self.intern_type(CanonicalType::Function(CallableContract {
                    inputs,
                    success,
                    failures,
                    needs,
                }))
            }
        };
        self.type_facts
            .insert((module_id, type_use.semantic_id()), result);
        result
    }

    fn lower_failure_row(
        &mut self,
        module_id: ModuleId,
        type_use: &TypeUse,
        _generics: &HashMap<String, SymbolId>,
    ) -> FailureRowId {
        let TypeUse::Named {
            semantic_id, path, ..
        } = type_use
        else {
            return FailureRowId::new(0);
        };
        let Some(name) = path.last() else {
            return FailureRowId::new(0);
        };
        let Some(symbol) = self.resolved_type_symbol(module_id, *semantic_id) else {
            return FailureRowId::new(0);
        };
        if self.resolved.symbol(symbol).kind == ProjectSymbolKind::GenericParameter {
            self.constrain_generic(module_id, symbol, GenericKind::FailureRow, name);
            return self.intern_failure_row(FailureRow {
                variants: Vec::new(),
                parameters: vec![symbol],
            });
        }
        if self.resolved.symbol(symbol).kind != ProjectSymbolKind::Enum {
            self.type_error(
                module_id,
                "T4102",
                name,
                format!(
                    "failure row expects an enum or row parameter, found {}",
                    name.text
                ),
            );
            return FailureRowId::new(0);
        }
        let symbol_module = self
            .resolved
            .symbol(symbol)
            .module_id
            .expect("enum is source-backed");
        let declaration = self.resolved.semantic.modules[symbol_module.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| {
                declaration.semantic_id
                    == self
                        .resolved
                        .symbol(symbol)
                        .semantic_id
                        .expect("enum has semantic identity")
            })
            .expect("enum symbol resolves to its declaration");
        let mut variants = declaration
            .variants
            .iter()
            .filter_map(|variant| {
                self.find_symbol(
                    symbol_module,
                    Some(variant.semantic_id),
                    ProjectSymbolKind::EnumConstructor,
                    &variant.name.text,
                )
            })
            .collect::<Vec<_>>();
        variants.sort_unstable();
        self.intern_failure_row(FailureRow {
            variants,
            parameters: Vec::new(),
        })
    }

    fn lower_needs_row(
        &mut self,
        module_id: ModuleId,
        row: &RequirementRowUse,
        generics: &HashMap<String, SymbolId>,
    ) -> NeedsRowId {
        let mut entries = Vec::new();
        let mut parameters = Vec::new();
        for term in &row.terms {
            match term {
                RequirementTermUse::Variable { name, .. } => {
                    if let Some(symbol) = generics.get(&name.text).copied() {
                        self.constrain_generic(module_id, symbol, GenericKind::NeedsRow, name);
                        parameters.push(symbol);
                    }
                }
                RequirementTermUse::Field {
                    semantic_id,
                    name,
                    interface,
                } => {
                    let Some(requirement) = self.find_symbol(
                        module_id,
                        Some(*semantic_id),
                        ProjectSymbolKind::Requirement,
                        &name.text,
                    ) else {
                        continue;
                    };
                    let interface = self.lower_value_type(module_id, interface, generics);
                    if let Some(existing) = entries
                        .iter()
                        .find(|entry: &&Need| entry.lexical_name == name.text)
                    {
                        if existing.interface != interface {
                            self.type_error(
                                module_id,
                                "T4103",
                                name,
                                format!(
                                    "requirement `{}` names incompatible interface types",
                                    name.text
                                ),
                            );
                        }
                        continue;
                    }
                    entries.push(Need {
                        requirement,
                        lexical_name: name.text.clone(),
                        interface,
                    });
                }
            }
        }
        entries.sort_by(|left, right| left.lexical_name.cmp(&right.lexical_name));
        parameters.sort_unstable();
        parameters.dedup();
        self.intern_needs_row(NeedsRow {
            entries,
            parameters,
        })
    }

    fn constrain_generic(
        &mut self,
        module_id: ModuleId,
        symbol: SymbolId,
        wanted: GenericKind,
        name: &crate::semantic::Name,
    ) {
        let current = self
            .generic_kinds
            .get(&symbol)
            .copied()
            .unwrap_or(GenericKind::Unconstrained);
        let next = match current {
            GenericKind::Unconstrained => wanted,
            same if same == wanted => same,
            GenericKind::Conflicting => GenericKind::Conflicting,
            _ => {
                self.type_error(
                    module_id,
                    "T4101",
                    name,
                    format!("generic `{}` is used as incompatible kinds", name.text),
                );
                GenericKind::Conflicting
            }
        };
        self.generic_kinds.insert(symbol, next);
    }

    fn declaration_symbol(
        &self,
        module_id: ModuleId,
        declaration: &ModuleDeclaration,
    ) -> Option<SymbolId> {
        let kind = match declaration.kind {
            ModuleDeclarationKind::Struct => ProjectSymbolKind::Struct,
            ModuleDeclarationKind::Enum => ProjectSymbolKind::Enum,
            ModuleDeclarationKind::Interface => ProjectSymbolKind::Interface,
            ModuleDeclarationKind::Function => ProjectSymbolKind::Function,
            ModuleDeclarationKind::Binding => ProjectSymbolKind::ModuleBinding,
        };
        self.find_symbol(
            module_id,
            Some(declaration.semantic_id),
            kind,
            &declaration.name.text,
        )
    }

    fn find_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: Option<SemanticId>,
        kind: ProjectSymbolKind,
        name: &str,
    ) -> Option<SymbolId> {
        self.resolved
            .symbols
            .iter()
            .find(|symbol| {
                symbol.module_id == Some(module_id)
                    && symbol.semantic_id == semantic_id
                    && symbol.kind == kind
                    && symbol.name == name
            })
            .map(|symbol| symbol.id)
    }

    fn resolved_type_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
    ) -> Option<SymbolId> {
        self.resolved
            .references
            .iter()
            .find(|reference| {
                reference.file_id.raw() == module_id.raw()
                    && reference.semantic_id == semantic_id
                    && reference.namespace == Namespace::Type
            })
            .and_then(|reference| match reference.resolution {
                ReferenceResolution::Resolved(symbol) => Some(symbol),
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_)
                | ReferenceResolution::DeferredToType => None,
            })
    }

    fn fresh_inference_type(&mut self) -> TypeId {
        let variable = self.next_inference_variable;
        self.next_inference_variable += 1;
        self.intern_type(CanonicalType::InferenceVariable(variable))
    }

    fn intern_type(&mut self, value: CanonicalType) -> TypeId {
        if let Some(id) = self.type_intern.get(&value).copied() {
            return id;
        }
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(value.clone());
        self.type_intern.insert(value, id);
        id
    }

    fn intern_failure_row(&mut self, value: FailureRow) -> FailureRowId {
        if let Some(id) = self.failure_row_intern.get(&value).copied() {
            return id;
        }
        let id = FailureRowId::new(self.failure_rows.len() as u32);
        self.failure_rows.push(value.clone());
        self.failure_row_intern.insert(value, id);
        id
    }

    fn intern_needs_row(&mut self, value: NeedsRow) -> NeedsRowId {
        if let Some(id) = self.needs_row_intern.get(&value).copied() {
            return id;
        }
        let id = NeedsRowId::new(self.needs_rows.len() as u32);
        self.needs_rows.push(value.clone());
        self.needs_row_intern.insert(value, id);
        id
    }

    fn type_error(
        &mut self,
        module_id: ModuleId,
        code: &'static str,
        name: &crate::semantic::Name,
        message: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            code,
            self.resolved.semantic.modules[module_id.raw() as usize]
                .source
                .file_id(),
            name.range,
            message,
        ));
    }
}
