use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FailureRowId, ModuleId, NeedsRowId, SemanticId, SymbolId, TextRange, TypeId};
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, Resolution, ResolvedProgram, ResolvedProject,
};
use crate::semantic::Literal;
use crate::semantic::{
    FunctionSignature, ModuleBlockItem, ModuleDeclaration, ModuleDeclarationKind, ModuleExpression,
    RequirementRowUse, RequirementTermUse, TypeUse,
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
    PolymorphicFunction {
        parameters: Vec<SymbolId>,
        contract: CallableContract,
    },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExpressionFact {
    pub(crate) success: TypeId,
    pub(crate) failures: FailureRowId,
    pub(crate) needs: NeedsRowId,
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
    pub(crate) expression_facts: HashMap<(ModuleId, SemanticId), ExpressionFact>,
    pub(crate) fields: HashMap<SymbolId, Vec<FieldShape>>,
    pub(crate) generic_kinds: HashMap<SymbolId, GenericKind>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) inference_bindings: HashMap<u32, TypeId>,
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
        let id = self.resolve_type_id(id);
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
            CanonicalType::PolymorphicFunction { contract, .. } => {
                self.format_function_contract(contract)
            }
            CanonicalType::InferenceVariable(_) => "_".to_owned(),
            CanonicalType::Poison => "<invalid>".to_owned(),
        }
    }

    fn resolve_type_id(&self, mut id: TypeId) -> TypeId {
        while let CanonicalType::InferenceVariable(variable) = self.types[id.raw() as usize] {
            let Some(bound) = self.inference_bindings.get(&variable).copied() else {
                break;
            };
            if bound == id {
                break;
            }
            id = bound;
        }
        id
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
    expression_facts: HashMap<(ModuleId, SemanticId), ExpressionFact>,
    fields: HashMap<SymbolId, Vec<FieldShape>>,
    generic_kinds: HashMap<SymbolId, GenericKind>,
    diagnostics: Vec<Diagnostic>,
    next_inference_variable: u32,
    inference_bindings: HashMap<u32, TypeId>,
}

#[derive(Clone, Debug)]
struct ValueBinding {
    type_id: TypeId,
    need: Option<Need>,
}

#[derive(Default)]
struct CallSubstitution {
    values: HashMap<SymbolId, TypeId>,
    failures: HashMap<SymbolId, FailureRowId>,
    needs: HashMap<SymbolId, NeedsRowId>,
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
            expression_facts: HashMap::new(),
            fields: HashMap::new(),
            generic_kinds: HashMap::new(),
            diagnostics,
            next_inference_variable: 0,
            inference_bindings: HashMap::new(),
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
        for module_id in &order {
            let declarations = self.resolved.semantic.modules[module_id.raw() as usize]
                .declarations
                .clone();
            for declaration in declarations {
                self.check_declaration(*module_id, &declaration);
            }
        }
        let declaration_count = self
            .resolved
            .semantic
            .modules
            .iter()
            .map(|module| module.declarations.len())
            .sum::<usize>();
        for pass in 0..=declaration_count {
            let mut changed = false;
            for module_id in &order {
                let declarations = self.resolved.semantic.modules[module_id.raw() as usize]
                    .declarations
                    .clone();
                for declaration in declarations {
                    changed |= self.check_declaration_body(*module_id, &declaration);
                }
            }
            if !changed {
                break;
            }
            assert!(
                pass < declaration_count,
                "monotonic body-contract inference must converge within the declaration count"
            );
        }
        TypedProject {
            resolved: self.resolved,
            types: self.types,
            failure_rows: self.failure_rows,
            needs_rows: self.needs_rows,
            contracts: self.contracts,
            declaration_types: self.declaration_types,
            type_facts: self.type_facts,
            expression_facts: self.expression_facts,
            fields: self.fields,
            generic_kinds: self.generic_kinds,
            diagnostics: self.diagnostics,
            inference_bindings: self.inference_bindings,
        }
    }

    fn check_declaration(&mut self, module_id: ModuleId, declaration: &ModuleDeclaration) {
        let generic_symbols = self.generic_symbols(module_id, declaration);
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

    fn check_declaration_body(
        &mut self,
        module_id: ModuleId,
        declaration: &ModuleDeclaration,
    ) -> bool {
        let Some(body) = &declaration.body else {
            return false;
        };
        let generic_symbols = self.generic_symbols(module_id, declaration);
        match declaration.kind {
            ModuleDeclarationKind::Function => {
                let Some(function_symbol) = self.declaration_symbol(module_id, declaration) else {
                    return false;
                };
                let signature = declaration
                    .function
                    .as_ref()
                    .expect("function declaration owns a signature");
                let Some(contract) = self.contracts.get(&function_symbol).cloned() else {
                    return false;
                };
                let previous = contract.clone();
                let mut bindings = self.signature_bindings(module_id, signature, &contract);
                let body_fact =
                    self.infer_expression(module_id, body, &generic_symbols, &mut bindings);
                self.unify_expression_result(module_id, body, body_fact.success, contract.success);
                let mut inferred = contract;
                inferred.success = self.resolve_inference_type(inferred.success);
                inferred.failures =
                    self.union_failure_rows([inferred.failures, body_fact.failures]);
                inferred.needs = self.union_needs_rows([inferred.needs, body_fact.needs]);
                self.contracts.insert(function_symbol, inferred);
                self.contracts[&function_symbol] != previous
            }
            ModuleDeclarationKind::Binding => {
                let mut bindings = HashMap::new();
                let fact = self.infer_expression(module_id, body, &generic_symbols, &mut bindings);
                if let Some(symbol) = self.declaration_symbol(module_id, declaration) {
                    let previous = self
                        .declaration_types
                        .insert((module_id, declaration.semantic_id), fact.success);
                    self.contracts.remove(&symbol);
                    previous != Some(fact.success)
                } else {
                    false
                }
            }
            ModuleDeclarationKind::Struct
            | ModuleDeclarationKind::Enum
            | ModuleDeclarationKind::Interface => false,
        }
    }

    fn generic_symbols(
        &self,
        module_id: ModuleId,
        declaration: &ModuleDeclaration,
    ) -> HashMap<String, SymbolId> {
        declaration
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
            .collect()
    }

    fn signature_bindings(
        &self,
        module_id: ModuleId,
        signature: &FunctionSignature,
        contract: &CallableContract,
    ) -> HashMap<SymbolId, ValueBinding> {
        let mut bindings = HashMap::new();
        for (parameter, type_id) in signature.parameters.iter().zip(&contract.inputs) {
            if let Some(symbol) = self.find_symbol(
                module_id,
                Some(parameter.semantic_id),
                ProjectSymbolKind::Parameter,
                &parameter.name.text,
            ) {
                bindings.insert(
                    symbol,
                    ValueBinding {
                        type_id: *type_id,
                        need: None,
                    },
                );
            }
        }
        for need in &self.needs_rows[contract.needs.raw() as usize].entries {
            bindings.insert(
                need.requirement,
                ValueBinding {
                    type_id: need.interface,
                    need: Some(need.clone()),
                },
            );
        }
        bindings
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

    fn infer_expression(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let fact = match expression {
            ModuleExpression::Literal(literal) => ExpressionFact {
                success: self.builtin_type(match literal {
                    Literal::Integer { value, .. } => {
                        let _ = value;
                        "Int"
                    }
                    Literal::Text { value, .. } => {
                        let _ = value;
                        "Text"
                    }
                }),
                failures: FailureRowId::new(0),
                needs: NeedsRowId::new(0),
            },
            ModuleExpression::Name { semantic_id, .. } => {
                let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id) else {
                    return self.record_expression_fact(
                        module_id,
                        expression,
                        self.poison_expression_fact(),
                    );
                };
                if let Some(binding) = bindings.get(&symbol) {
                    let needs = binding.need.as_ref().map_or(NeedsRowId::new(0), |need| {
                        self.intern_needs_row(NeedsRow {
                            entries: vec![need.clone()],
                            parameters: Vec::new(),
                        })
                    });
                    ExpressionFact {
                        success: binding.type_id,
                        failures: FailureRowId::new(0),
                        needs,
                    }
                } else if let Some(contract) = self.contracts.get(&symbol).cloned() {
                    ExpressionFact {
                        success: self.callable_type_for_symbol(symbol, contract),
                        failures: FailureRowId::new(0),
                        needs: NeedsRowId::new(0),
                    }
                } else if let Some(type_id) = self.symbol_value_type(symbol) {
                    ExpressionFact {
                        success: type_id,
                        failures: FailureRowId::new(0),
                        needs: NeedsRowId::new(0),
                    }
                } else {
                    self.poison_expression_fact()
                }
            }
            ModuleExpression::Select {
                semantic_id,
                receiver,
                member,
                ..
            } => {
                if let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id)
                    && let Some(type_id) = self.symbol_value_type_or_contract(symbol)
                {
                    ExpressionFact {
                        success: type_id,
                        failures: FailureRowId::new(0),
                        needs: NeedsRowId::new(0),
                    }
                } else {
                    let receiver = self.infer_expression(module_id, receiver, generics, bindings);
                    let success = self
                        .field_type(receiver.success, &member.text)
                        .unwrap_or_else(|| {
                            if self.resolve_inference_type(receiver.success) != TypeId::new(0) {
                                self.type_error_range(
                                    module_id,
                                    "T4205",
                                    member.range,
                                    format!("type has no field `{}`", member.text),
                                );
                            }
                            TypeId::new(0)
                        });
                    ExpressionFact {
                        success,
                        failures: receiver.failures,
                        needs: receiver.needs,
                    }
                }
            }
            ModuleExpression::Call {
                callee,
                arguments,
                range,
                ..
            } => {
                let callee = self.infer_expression(module_id, callee, generics, bindings);
                let argument_facts = arguments
                    .iter()
                    .map(|argument| self.infer_expression(module_id, argument, generics, bindings))
                    .collect::<Vec<_>>();
                let callee_type = self.resolve_inference_type(callee.success);
                let (contract, mut substitution) =
                    match self.types.get(callee_type.raw() as usize).cloned() {
                        Some(CanonicalType::Function(contract)) => (contract, None),
                        Some(CanonicalType::PolymorphicFunction { contract, .. }) => {
                            (contract, Some(CallSubstitution::default()))
                        }
                        _ => {
                            if callee_type != TypeId::new(0) {
                                self.type_error_range(
                                    module_id,
                                    "T4201",
                                    *range,
                                    "expression is not callable",
                                );
                            }
                            return self.record_expression_fact(
                                module_id,
                                expression,
                                self.poison_expression_fact(),
                            );
                        }
                    };
                if contract.inputs.len() != argument_facts.len() {
                    self.type_error_range(
                        module_id,
                        "T4202",
                        *range,
                        format!(
                            "call expects {} arguments, found {}",
                            contract.inputs.len(),
                            argument_facts.len()
                        ),
                    );
                }
                for (argument, expected) in argument_facts.iter().zip(&contract.inputs) {
                    if let Some(substitution) = &mut substitution {
                        if !self.match_call_type(argument.success, *expected, substitution) {
                            self.type_error_range(
                                module_id,
                                "T4203",
                                *range,
                                "call argument has an incompatible type",
                            );
                        }
                    } else {
                        self.unify_at(
                            module_id,
                            *range,
                            argument.success,
                            *expected,
                            "call argument",
                        );
                    }
                }
                let (success, contract_failures, contract_needs) =
                    if let Some(substitution) = &mut substitution {
                        (
                            self.substitute_call_type(contract.success, substitution),
                            self.substitute_call_failure_row(contract.failures, substitution),
                            self.substitute_call_needs_row(contract.needs, substitution),
                        )
                    } else {
                        (
                            self.resolve_inference_type(contract.success),
                            contract.failures,
                            contract.needs,
                        )
                    };
                let failures = self.union_failure_rows(
                    std::iter::once(callee.failures)
                        .chain(argument_facts.iter().map(|fact| fact.failures))
                        .chain(std::iter::once(contract_failures)),
                );
                let needs = self.union_needs_rows(
                    std::iter::once(callee.needs)
                        .chain(argument_facts.iter().map(|fact| fact.needs))
                        .chain(std::iter::once(contract_needs)),
                );
                ExpressionFact {
                    success,
                    failures,
                    needs,
                }
            }
            ModuleExpression::Sequence { expressions, .. } => {
                let facts = expressions
                    .iter()
                    .map(|child| self.infer_expression(module_id, child, generics, bindings))
                    .collect::<Vec<_>>();
                ExpressionFact {
                    success: facts.last().map_or(TypeId::new(0), |fact| fact.success),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Block { items, .. } => {
                let mut block_bindings = bindings.clone();
                let mut facts = Vec::new();
                for item in items {
                    match item {
                        ModuleBlockItem::Binding { name, value } => {
                            let value_fact = self.infer_expression(
                                module_id,
                                value,
                                generics,
                                &mut block_bindings,
                            );
                            if let Some(symbol) = self.find_symbol(
                                module_id,
                                Some(value.semantic_id()),
                                ProjectSymbolKind::LocalBinding,
                                &name.text,
                            ) {
                                block_bindings.insert(
                                    symbol,
                                    ValueBinding {
                                        type_id: value_fact.success,
                                        need: None,
                                    },
                                );
                            }
                            facts.push(value_fact);
                        }
                        ModuleBlockItem::Expression(value) => facts.push(self.infer_expression(
                            module_id,
                            value,
                            generics,
                            &mut block_bindings,
                        )),
                    }
                }
                ExpressionFact {
                    success: facts.last().map_or(TypeId::new(0), |fact| fact.success),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Function {
                signature, body, ..
            } => {
                let mut contract = self.lower_signature(module_id, signature, generics);
                let mut function_bindings = bindings.clone();
                function_bindings.extend(self.signature_bindings(module_id, signature, &contract));
                let body_fact =
                    self.infer_expression(module_id, body, generics, &mut function_bindings);
                self.unify_expression_result(module_id, body, body_fact.success, contract.success);
                contract.success = self.resolve_inference_type(contract.success);
                contract.failures =
                    self.union_failure_rows([contract.failures, body_fact.failures]);
                contract.needs = self.union_needs_rows([contract.needs, body_fact.needs]);
                ExpressionFact {
                    success: self.intern_type(CanonicalType::Function(contract)),
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                }
            }
            ModuleExpression::Case { subjects, arms, .. } => {
                let mut facts = subjects
                    .iter()
                    .map(|subject| self.infer_expression(module_id, subject, generics, bindings))
                    .collect::<Vec<_>>();
                let mut success = None;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        facts.push(self.infer_expression(module_id, guard, generics, bindings));
                    }
                    let body = self.infer_expression(module_id, &arm.body, generics, bindings);
                    if let Some(expected) = success {
                        self.unify_expression_result(module_id, &arm.body, body.success, expected);
                    } else {
                        success = Some(body.success);
                    }
                    facts.push(body);
                }
                ExpressionFact {
                    success: success.unwrap_or(TypeId::new(0)),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Empty { .. } => self.poison_expression_fact(),
        };
        self.record_expression_fact(module_id, expression, fact)
    }

    fn record_expression_fact(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        fact: ExpressionFact,
    ) -> ExpressionFact {
        self.expression_facts
            .insert((module_id, expression.semantic_id()), fact);
        fact
    }

    const fn poison_expression_fact(&self) -> ExpressionFact {
        ExpressionFact {
            success: TypeId::new(0),
            failures: FailureRowId::new(0),
            needs: NeedsRowId::new(0),
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

    fn builtin_type(&mut self, name: &str) -> TypeId {
        let symbol = self
            .resolved
            .symbols
            .iter()
            .find(|symbol| {
                symbol.module_id.is_none()
                    && symbol.kind == ProjectSymbolKind::BuiltinType
                    && symbol.name == name
            })
            .map(|symbol| symbol.id)
            .expect("compiler builtin type is indexed");
        self.intern_type(CanonicalType::Named {
            symbol,
            arguments: Vec::new(),
        })
    }

    fn resolved_value_symbol(
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
                    && reference.namespace == Namespace::Value
            })
            .and_then(|reference| match reference.resolution {
                ReferenceResolution::Resolved(symbol) => Some(symbol),
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_)
                | ReferenceResolution::DeferredToType => None,
            })
    }

    fn symbol_value_type(&self, symbol: SymbolId) -> Option<TypeId> {
        let symbol = self.resolved.symbol(symbol);
        let module_id = symbol.module_id?;
        let semantic_id = symbol.semantic_id?;
        self.declaration_types
            .get(&(module_id, semantic_id))
            .copied()
    }

    fn symbol_value_type_or_contract(&mut self, symbol: SymbolId) -> Option<TypeId> {
        if let Some(contract) = self.contracts.get(&symbol).cloned() {
            Some(self.callable_type_for_symbol(symbol, contract))
        } else {
            self.symbol_value_type(symbol)
        }
    }

    fn callable_type_for_symbol(&mut self, symbol: SymbolId, contract: CallableContract) -> TypeId {
        let parameters = self.function_generic_parameters(symbol);
        if parameters.is_empty() {
            self.intern_type(CanonicalType::Function(contract))
        } else {
            self.intern_type(CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            })
        }
    }

    fn function_generic_parameters(&self, symbol: SymbolId) -> Vec<SymbolId> {
        let symbol = self.resolved.symbol(symbol);
        if symbol.kind != ProjectSymbolKind::Function {
            return Vec::new();
        }
        let mut parameters = self
            .resolved
            .symbols
            .iter()
            .filter(|candidate| {
                candidate.module_id == symbol.module_id
                    && candidate.semantic_id == symbol.semantic_id
                    && candidate.kind == ProjectSymbolKind::GenericParameter
            })
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        parameters.sort_unstable();
        parameters
    }

    fn field_type(&self, receiver: TypeId, member: &str) -> Option<TypeId> {
        let receiver = self.resolve_inference_type(receiver);
        let CanonicalType::Named { symbol, .. } = &self.types[receiver.raw() as usize] else {
            return None;
        };
        self.fields
            .get(symbol)?
            .iter()
            .find(|field| field.name == member)
            .map(|field| field.type_id)
    }

    fn match_call_type(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        let actual = self.resolve_inference_type(actual);
        let expected = self.resolve_inference_type(expected);
        if actual == TypeId::new(0) || expected == TypeId::new(0) {
            return true;
        }
        match (
            self.types[actual.raw() as usize].clone(),
            self.types[expected.raw() as usize].clone(),
        ) {
            (_, CanonicalType::Generic(parameter)) => {
                if let Some(bound) = substitution.values.get(&parameter).copied() {
                    self.unify(actual, bound)
                } else {
                    substitution.values.insert(parameter, actual);
                    true
                }
            }
            (
                CanonicalType::Named {
                    symbol: actual_symbol,
                    arguments: actual_arguments,
                },
                CanonicalType::Named {
                    symbol: expected_symbol,
                    arguments: expected_arguments,
                },
            ) => {
                actual_symbol == expected_symbol
                    && actual_arguments.len() == expected_arguments.len()
                    && actual_arguments.into_iter().zip(expected_arguments).all(
                        |(actual, expected)| self.match_call_type(actual, expected, substitution),
                    )
            }
            (CanonicalType::Function(actual), CanonicalType::Function(expected)) => {
                actual.inputs.len() == expected.inputs.len()
                    && actual
                        .inputs
                        .into_iter()
                        .zip(expected.inputs)
                        .all(|(actual, expected)| {
                            self.match_call_type(actual, expected, substitution)
                        })
                    && self.match_call_type(actual.success, expected.success, substitution)
                    && self.match_call_failure_row(actual.failures, expected.failures, substitution)
                    && self.match_call_needs_row(actual.needs, expected.needs, substitution)
            }
            _ => self.unify(actual, expected),
        }
    }

    fn match_call_failure_row(
        &mut self,
        actual: FailureRowId,
        expected: FailureRowId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        let actual = self.failure_rows[actual.raw() as usize].clone();
        let expected = self.failure_rows[expected.raw() as usize].clone();
        let mut remaining_variants = actual.variants;
        for variant in &expected.variants {
            let Some(index) = remaining_variants
                .iter()
                .position(|candidate| candidate == variant)
            else {
                return false;
            };
            remaining_variants.remove(index);
        }
        match expected.parameters.as_slice() {
            [] => remaining_variants.is_empty() && actual.parameters.is_empty(),
            [parameter] => {
                let residual = self.intern_failure_row(FailureRow {
                    variants: remaining_variants,
                    parameters: actual.parameters,
                });
                if let Some(bound) = substitution.failures.get(parameter).copied() {
                    self.failure_rows_equivalent(bound, residual)
                } else {
                    substitution.failures.insert(*parameter, residual);
                    true
                }
            }
            _ => false,
        }
    }

    fn match_call_needs_row(
        &mut self,
        actual: NeedsRowId,
        expected: NeedsRowId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        let actual = self.needs_rows[actual.raw() as usize].clone();
        let expected = self.needs_rows[expected.raw() as usize].clone();
        let mut remaining_entries = actual.entries;
        for expected_need in &expected.entries {
            let Some(index) = remaining_entries
                .iter()
                .position(|need| need.lexical_name == expected_need.lexical_name)
            else {
                return false;
            };
            let actual_need = remaining_entries.remove(index);
            if !self.match_call_type(actual_need.interface, expected_need.interface, substitution) {
                return false;
            }
        }
        match expected.parameters.as_slice() {
            [] => remaining_entries.is_empty() && actual.parameters.is_empty(),
            [parameter] => {
                let residual = self.intern_needs_row(NeedsRow {
                    entries: remaining_entries,
                    parameters: actual.parameters,
                });
                if let Some(bound) = substitution.needs.get(parameter).copied() {
                    self.needs_rows_equivalent(bound, residual)
                } else {
                    substitution.needs.insert(*parameter, residual);
                    true
                }
            }
            _ => false,
        }
    }

    fn substitute_call_type(
        &mut self,
        type_id: TypeId,
        substitution: &mut CallSubstitution,
    ) -> TypeId {
        let type_id = self.resolve_inference_type(type_id);
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::Generic(parameter) => substitution
                .values
                .entry(parameter)
                .or_insert_with(|| self.fresh_inference_type())
                .to_owned(),
            CanonicalType::Named { symbol, arguments } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.substitute_call_type(argument, substitution))
                    .collect();
                self.intern_type(CanonicalType::Named { symbol, arguments })
            }
            CanonicalType::Function(contract) => {
                let contract = self.substitute_call_contract(contract, substitution);
                self.intern_type(CanonicalType::Function(contract))
            }
            CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            } => self.intern_type(CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            }),
            CanonicalType::InferenceVariable(_) | CanonicalType::Poison => type_id,
        }
    }

    fn substitute_call_contract(
        &mut self,
        contract: CallableContract,
        substitution: &mut CallSubstitution,
    ) -> CallableContract {
        CallableContract {
            inputs: contract
                .inputs
                .into_iter()
                .map(|input| self.substitute_call_type(input, substitution))
                .collect(),
            success: self.substitute_call_type(contract.success, substitution),
            failures: self.substitute_call_failure_row(contract.failures, substitution),
            needs: self.substitute_call_needs_row(contract.needs, substitution),
        }
    }

    fn substitute_call_failure_row(
        &mut self,
        row: FailureRowId,
        substitution: &CallSubstitution,
    ) -> FailureRowId {
        let row = self.failure_rows[row.raw() as usize].clone();
        let mut rows = vec![self.intern_failure_row(FailureRow {
            variants: row.variants,
            parameters: Vec::new(),
        })];
        for parameter in row.parameters {
            rows.push(
                substitution
                    .failures
                    .get(&parameter)
                    .copied()
                    .unwrap_or_else(|| {
                        self.intern_failure_row(FailureRow {
                            variants: Vec::new(),
                            parameters: vec![parameter],
                        })
                    }),
            );
        }
        self.union_failure_rows(rows)
    }

    fn substitute_call_needs_row(
        &mut self,
        row: NeedsRowId,
        substitution: &mut CallSubstitution,
    ) -> NeedsRowId {
        let row = self.needs_rows[row.raw() as usize].clone();
        let entries = row
            .entries
            .into_iter()
            .map(|need| Need {
                interface: self.substitute_call_type(need.interface, substitution),
                ..need
            })
            .collect();
        let mut rows = vec![self.intern_needs_row(NeedsRow {
            entries,
            parameters: Vec::new(),
        })];
        for parameter in row.parameters {
            rows.push(
                substitution
                    .needs
                    .get(&parameter)
                    .copied()
                    .unwrap_or_else(|| {
                        self.intern_needs_row(NeedsRow {
                            entries: Vec::new(),
                            parameters: vec![parameter],
                        })
                    }),
            );
        }
        self.union_needs_rows(rows)
    }

    fn resolve_inference_type(&self, mut id: TypeId) -> TypeId {
        while let CanonicalType::InferenceVariable(variable) = &self.types[id.raw() as usize] {
            let Some(bound) = self.inference_bindings.get(variable).copied() else {
                break;
            };
            if bound == id {
                break;
            }
            id = bound;
        }
        id
    }

    fn unify_expression_result(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        actual: TypeId,
        expected: TypeId,
    ) {
        self.unify_at(
            module_id,
            expression_range(expression),
            actual,
            expected,
            "function result",
        );
    }

    fn unify_at(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        actual: TypeId,
        expected: TypeId,
        context: &str,
    ) -> bool {
        if self.unify(actual, expected) {
            return true;
        }
        self.type_error_range(
            module_id,
            "T4203",
            range,
            format!("{context} has an incompatible type"),
        );
        false
    }

    fn unify(&mut self, left: TypeId, right: TypeId) -> bool {
        let left = self.resolve_inference_type(left);
        let right = self.resolve_inference_type(right);
        if left == right || left == TypeId::new(0) || right == TypeId::new(0) {
            return true;
        }
        match (
            self.types[left.raw() as usize].clone(),
            self.types[right.raw() as usize].clone(),
        ) {
            (CanonicalType::InferenceVariable(variable), _) => {
                self.inference_bindings.insert(variable, right);
                true
            }
            (_, CanonicalType::InferenceVariable(variable)) => {
                self.inference_bindings.insert(variable, left);
                true
            }
            (
                CanonicalType::Named {
                    symbol: left_symbol,
                    arguments: left_arguments,
                },
                CanonicalType::Named {
                    symbol: right_symbol,
                    arguments: right_arguments,
                },
            ) => {
                left_symbol == right_symbol
                    && left_arguments.len() == right_arguments.len()
                    && left_arguments
                        .into_iter()
                        .zip(right_arguments)
                        .all(|(left, right)| self.unify(left, right))
            }
            (CanonicalType::Generic(left), CanonicalType::Generic(right)) => left == right,
            (CanonicalType::Function(left), CanonicalType::Function(right)) => {
                left.inputs.len() == right.inputs.len()
                    && left
                        .inputs
                        .into_iter()
                        .zip(right.inputs)
                        .all(|(left, right)| self.unify(left, right))
                    && self.unify(left.success, right.success)
                    && self.failure_rows_equivalent(left.failures, right.failures)
                    && self.needs_rows_equivalent(left.needs, right.needs)
            }
            _ => false,
        }
    }

    fn failure_rows_equivalent(&self, left: FailureRowId, right: FailureRowId) -> bool {
        self.failure_rows[left.raw() as usize] == self.failure_rows[right.raw() as usize]
    }

    fn needs_rows_equivalent(&self, left: NeedsRowId, right: NeedsRowId) -> bool {
        let left = &self.needs_rows[left.raw() as usize];
        let right = &self.needs_rows[right.raw() as usize];
        left.parameters == right.parameters
            && left.entries.len() == right.entries.len()
            && left.entries.iter().all(|left_need| {
                right.entries.iter().any(|right_need| {
                    left_need.lexical_name == right_need.lexical_name
                        && left_need.interface == right_need.interface
                })
            })
    }

    fn union_failure_rows(&mut self, rows: impl IntoIterator<Item = FailureRowId>) -> FailureRowId {
        let mut variants = Vec::new();
        let mut parameters = Vec::new();
        for row in rows {
            let row = &self.failure_rows[row.raw() as usize];
            variants.extend(&row.variants);
            parameters.extend(&row.parameters);
        }
        variants.sort_unstable();
        variants.dedup();
        parameters.sort_unstable();
        parameters.dedup();
        self.intern_failure_row(FailureRow {
            variants,
            parameters,
        })
    }

    fn union_needs_rows(&mut self, rows: impl IntoIterator<Item = NeedsRowId>) -> NeedsRowId {
        let mut entries = Vec::<Need>::new();
        let mut parameters = Vec::new();
        for row in rows {
            let row = &self.needs_rows[row.raw() as usize];
            for need in &row.entries {
                if !entries.iter().any(|existing| {
                    existing.lexical_name == need.lexical_name
                        && existing.interface == need.interface
                }) {
                    entries.push(need.clone());
                }
            }
            parameters.extend(&row.parameters);
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

    fn type_error_range(
        &mut self,
        module_id: ModuleId,
        code: &'static str,
        range: TextRange,
        message: impl Into<String>,
    ) {
        let file_id = self.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .file_id();
        if self.diagnostics.iter().any(|diagnostic| {
            diagnostic.stage() == DiagnosticStage::Type
                && diagnostic.code() == code
                && diagnostic.file_id() == file_id
                && diagnostic.range() == range
        }) {
            return;
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticStage::Type,
            code,
            file_id,
            range,
            message,
        ));
    }
}

fn expression_range(expression: &ModuleExpression) -> TextRange {
    match expression {
        ModuleExpression::Literal(literal) => literal.range(),
        ModuleExpression::Name { name, .. } => name.range,
        ModuleExpression::Select { range, .. } | ModuleExpression::Call { range, .. } => *range,
        ModuleExpression::Sequence { expressions, .. } => expressions
            .first()
            .map_or(TextRange::empty(0), expression_range),
        ModuleExpression::Block { items, .. } => items
            .first()
            .map(|item| match item {
                ModuleBlockItem::Binding { value, .. } | ModuleBlockItem::Expression(value) => {
                    expression_range(value)
                }
            })
            .unwrap_or(TextRange::empty(0)),
        ModuleExpression::Function { body, .. } => expression_range(body),
        ModuleExpression::Case { subjects, arms, .. } => subjects
            .first()
            .map(expression_range)
            .or_else(|| arms.first().map(|arm| expression_range(&arm.body)))
            .unwrap_or(TextRange::empty(0)),
        ModuleExpression::Empty { .. } => TextRange::empty(0),
    }
}
