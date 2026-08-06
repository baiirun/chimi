use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FailureRowId, ModuleId, NeedsRowId, SemanticId, SymbolId, TextRange, TypeId};
use crate::resolution::{
    Namespace, ProjectSymbolKind, ReferenceResolution, Resolution, ResolvedProgram, ResolvedProject,
};
use crate::semantic::Literal;
use crate::semantic::{
    BinaryOperator, FunctionSignature, ModuleBlockItem, ModuleCaseArm, ModuleDeclaration,
    ModuleDeclarationKind, ModuleExpression, Pattern, PatternLiteral, RequirementRowUse,
    RequirementTermUse, TypeUse, UnaryOperator,
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
            Literal::Boolean { value, .. } => {
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
    Record(Vec<StructuralField>),
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
    pub(crate) explained: bool,
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct StructuralField {
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
    fn validate_closed_roots(&mut self) {
        let roots = self
            .resolved
            .semantic
            .modules
            .iter()
            .flat_map(|module| {
                module
                    .declarations
                    .iter()
                    .filter(|declaration| {
                        declaration.kind == ModuleDeclarationKind::Function
                            && declaration.public
                            && declaration.name.text == "main"
                    })
                    .filter_map(|declaration| {
                        self.resolved
                            .symbols
                            .iter()
                            .find(|symbol| {
                                symbol.module_id == Some(module.id)
                                    && symbol.semantic_id == Some(declaration.semantic_id)
                                    && symbol.kind == ProjectSymbolKind::Function
                            })
                            .map(|symbol| {
                                (module.source.file_id(), declaration.name.range, symbol.id)
                            })
                    })
            })
            .collect::<Vec<_>>();

        for (file_id, range, symbol) in roots {
            let Some(contract) = self.contracts.get(&symbol).cloned() else {
                continue;
            };
            let row = self.needs_rows[contract.needs.raw() as usize].clone();
            for need in row.entries.into_iter().filter(|need| !need.explained) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::ExecutionSetup,
                    "E5002",
                    file_id,
                    range,
                    format!(
                        "main needs {}: {}",
                        need.lexical_name,
                        self.format_type(need.interface)
                    ),
                ));
            }
            for parameter in row.parameters {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticStage::ExecutionSetup,
                    "E5002",
                    file_id,
                    range,
                    format!(
                        "main has unresolved dependency row {}",
                        self.resolved.symbol(parameter).name
                    ),
                ));
            }
        }
    }

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
            CanonicalType::Record(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name, self.format_type(field.type_id)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
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

struct InvocationArgument {
    label: Option<crate::semantic::Name>,
    fact: ExpressionFact,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum MatchTag {
    EnumVariant(SymbolId),
    Boolean(bool),
    Integer(i64),
    Text(String),
}

#[derive(Clone, Debug)]
enum MatchPattern {
    Wildcard,
    Constructor {
        tag: MatchTag,
        fields: Vec<MatchPattern>,
    },
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
        let mut typed = TypedProject {
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
        };
        typed.validate_closed_roots();
        typed
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
                    let constructor_fields = self.fields[&declaration_symbol].clone();
                    let inputs = constructor_fields
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
                        self.fields.insert(constructor, constructor_fields);
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
                    Literal::Boolean { value, .. } => {
                        let _ = value;
                        "Bool"
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
                } else if self.is_builtin_value(symbol, "Success")
                    || self.is_builtin_value(symbol, "Failure")
                {
                    self.type_error_range(
                        module_id,
                        "T4221",
                        expression_range(expression),
                        "contextual outcome constructors must be called with one value",
                    );
                    self.poison_expression_fact()
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
                let callee_symbol = self.expression_value_symbol(module_id, callee);
                if callee_symbol.is_some_and(|symbol| {
                    self.is_builtin_value(symbol, "Success")
                        || self.is_builtin_value(symbol, "Failure")
                }) {
                    self.infer_outcome_constructor(
                        module_id,
                        callee_symbol.expect("outcome constructor is resolved"),
                        arguments,
                        *range,
                        generics,
                        bindings,
                    )
                } else {
                    let callee = self.infer_expression(module_id, callee, generics, bindings);
                    let argument_facts = arguments
                        .iter()
                        .map(|argument| InvocationArgument {
                            label: argument.label.clone(),
                            fact: self.infer_expression(
                                module_id,
                                &argument.value,
                                generics,
                                bindings,
                            ),
                        })
                        .collect::<Vec<_>>();
                    self.infer_invocation(module_id, *range, callee_symbol, callee, &argument_facts)
                }
            }
            ModuleExpression::List {
                elements, range, ..
            } => {
                let facts = elements
                    .iter()
                    .map(|element| self.infer_expression(module_id, element, generics, bindings))
                    .collect::<Vec<_>>();
                let element_type = facts
                    .first()
                    .map_or_else(|| self.fresh_inference_type(), |fact| fact.success);
                for fact in facts.iter().skip(1) {
                    self.unify_at(
                        module_id,
                        *range,
                        fact.success,
                        element_type,
                        "list element",
                    );
                }
                ExpressionFact {
                    success: self.builtin_type_with_arguments("List", vec![element_type]),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Record { fields, .. } => {
                let mut facts = Vec::new();
                let mut structural_fields = Vec::new();
                for field in fields {
                    let fact = self.infer_expression(module_id, &field.value, generics, bindings);
                    if structural_fields
                        .iter()
                        .any(|existing: &StructuralField| existing.name == field.name.text)
                    {
                        self.type_error_range(
                            module_id,
                            "T4211",
                            field.name.range,
                            format!("record field `{}` occurs more than once", field.name.text),
                        );
                    } else {
                        structural_fields.push(StructuralField {
                            name: field.name.text.clone(),
                            type_id: fact.success,
                        });
                    }
                    facts.push(fact);
                }
                structural_fields.sort_by(|left, right| left.name.cmp(&right.name));
                ExpressionFact {
                    success: self.intern_type(CanonicalType::Record(structural_fields)),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Unary {
                operator: UnaryOperator::Negate,
                operand,
                range,
                ..
            } => {
                let fact = self.infer_expression(module_id, operand, generics, bindings);
                let int = self.builtin_type("Int");
                self.unify_at(module_id, *range, fact.success, int, "unary operand");
                ExpressionFact {
                    success: int,
                    failures: fact.failures,
                    needs: fact.needs,
                }
            }
            ModuleExpression::Binary {
                operator,
                left,
                right,
                range,
                ..
            } => {
                let transform_symbol = self.expression_value_symbol(module_id, right);
                let left = self.infer_expression(module_id, left, generics, bindings);
                let right = self.infer_expression(module_id, right, generics, bindings);
                if *operator == BinaryOperator::Pipe {
                    self.infer_invocation(
                        module_id,
                        *range,
                        transform_symbol,
                        right,
                        &[InvocationArgument {
                            label: None,
                            fact: left,
                        }],
                    )
                } else {
                    let int = self.builtin_type("Int");
                    self.unify_at(module_id, *range, left.success, int, "binary left operand");
                    self.unify_at(
                        module_id,
                        *range,
                        right.success,
                        int,
                        "binary right operand",
                    );
                    ExpressionFact {
                        success: if matches!(
                            operator,
                            BinaryOperator::Less
                                | BinaryOperator::LessEqual
                                | BinaryOperator::Greater
                                | BinaryOperator::GreaterEqual
                        ) {
                            self.builtin_type("Bool")
                        } else {
                            int
                        },
                        failures: self.union_failure_rows([left.failures, right.failures]),
                        needs: self.union_needs_rows([left.needs, right.needs]),
                    }
                }
            }
            ModuleExpression::Group { expression, .. } => {
                self.infer_expression(module_id, expression, generics, bindings)
            }
            ModuleExpression::Using {
                expression,
                providers,
                ..
            } => self.infer_using(module_id, expression, providers, generics, bindings),
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
            ModuleExpression::Case {
                subjects,
                arms,
                range,
                ..
            } => self.infer_case(module_id, subjects, arms, *range, generics, bindings),
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

    fn infer_outcome_constructor(
        &mut self,
        module_id: ModuleId,
        constructor: SymbolId,
        arguments: &[crate::semantic::CallArgument],
        range: TextRange,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let facts = arguments
            .iter()
            .map(|argument| self.infer_expression(module_id, &argument.value, generics, bindings))
            .collect::<Vec<_>>();
        if arguments.len() != 1 || arguments.iter().any(|argument| argument.label.is_some()) {
            self.type_error_range(
                module_id,
                "T4221",
                range,
                format!(
                    "{} expects exactly one unlabeled value",
                    self.resolved.symbol(constructor).name
                ),
            );
        }
        let argument = facts
            .first()
            .copied()
            .unwrap_or_else(|| self.poison_expression_fact());
        if self.is_builtin_value(constructor, "Success") {
            return ExpressionFact {
                success: argument.success,
                failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
            };
        }

        let variants =
            arguments
                .first()
                .zip(facts.first())
                .map_or_else(Vec::new, |(argument, fact)| {
                    self.failure_variants_for_expression(module_id, &argument.value, fact.success)
                });
        if variants.is_empty() && arguments.len() == 1 {
            self.type_error_range(
                module_id,
                "T4220",
                expression_range(&arguments[0].value),
                "Failure expects a value of an enum error type",
            );
        }
        let constructed_failure = self.intern_failure_row(FailureRow {
            variants,
            parameters: Vec::new(),
        });
        ExpressionFact {
            success: self.fresh_inference_type(),
            failures: self.union_failure_rows(
                facts
                    .iter()
                    .map(|fact| fact.failures)
                    .chain(std::iter::once(constructed_failure)),
            ),
            needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
        }
    }

    fn failure_variants_for_expression(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        type_id: TypeId,
    ) -> Vec<SymbolId> {
        let constructed = match expression {
            ModuleExpression::Call { callee, .. } => {
                self.expression_value_symbol(module_id, callee)
            }
            ModuleExpression::Name { .. } | ModuleExpression::Select { .. } => {
                self.expression_value_symbol(module_id, expression)
            }
            ModuleExpression::Group { expression, .. } => {
                return self.failure_variants_for_expression(module_id, expression, type_id);
            }
            _ => None,
        };
        if let Some(symbol) = constructed
            && self.resolved.symbol(symbol).kind == ProjectSymbolKind::EnumConstructor
        {
            return vec![symbol];
        }
        let type_id = self.resolve_inference_type(type_id);
        let CanonicalType::Named { symbol, .. } = self.types[type_id.raw() as usize] else {
            return Vec::new();
        };
        if self.resolved.symbol(symbol).kind == ProjectSymbolKind::Enum {
            self.enum_variant_symbols(symbol)
        } else {
            Vec::new()
        }
    }

    fn infer_case(
        &mut self,
        module_id: ModuleId,
        subjects: &[ModuleExpression],
        arms: &[ModuleCaseArm],
        range: TextRange,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let subject_facts = subjects
            .iter()
            .map(|subject| self.infer_expression(module_id, subject, generics, bindings))
            .collect::<Vec<_>>();
        let has_outcome_pattern = arms.iter().any(|arm| {
            arm.patterns.first().is_some_and(|pattern| {
                self.pattern_is_builtin(module_id, pattern, "Success")
                    || self.pattern_is_builtin(module_id, pattern, "Failure")
            })
        });
        if subjects.len() == 1 && has_outcome_pattern {
            self.infer_outcome_case(module_id, subject_facts[0], arms, range, generics, bindings)
        } else {
            if subjects.len() == 1
                && !has_outcome_pattern
                && !self.failure_row_is_empty(subject_facts[0].failures)
            {
                self.type_error_range(
                    module_id,
                    "T4222",
                    range,
                    "a case over a fallible expression must use Success(...) and Failure(...) patterns",
                );
            }
            self.infer_ordinary_case(module_id, &subject_facts, arms, range, generics, bindings)
        }
    }

    fn infer_ordinary_case(
        &mut self,
        module_id: ModuleId,
        subjects: &[ExpressionFact],
        arms: &[ModuleCaseArm],
        range: TextRange,
        generics: &HashMap<String, SymbolId>,
        bindings: &HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let mut facts = subjects.to_vec();
        let mut success = None;
        let subject_types = subjects
            .iter()
            .map(|subject| subject.success)
            .collect::<Vec<_>>();
        let mut coverage = Vec::<Vec<MatchPattern>>::new();
        for (arm_index, arm) in arms.iter().enumerate() {
            let mut arm_bindings = bindings.clone();
            if arm.patterns.len() != subjects.len() {
                self.type_error_range(
                    module_id,
                    "T4230",
                    arm.range,
                    format!(
                        "case arm has {} patterns for {} subjects",
                        arm.patterns.len(),
                        subjects.len()
                    ),
                );
            }
            let normalized = arm
                .patterns
                .iter()
                .zip(subjects)
                .map(|(pattern, subject)| {
                    self.normalize_pattern(module_id, pattern, subject.success, &mut arm_bindings)
                })
                .collect::<Vec<_>>();
            let useful = self.pattern_row_is_useful(&coverage, &normalized, &subject_types);
            if !useful {
                self.type_error_range(module_id, "T4232", arm.range, "case arm is unreachable");
            }
            let unguarded = arm.guard.is_none();
            let catch_all = normalized
                .iter()
                .all(|pattern| matches!(pattern, MatchPattern::Wildcard));
            if unguarded && catch_all && arm_index + 1 < arms.len() {
                self.type_error_range(
                    module_id,
                    "T4233",
                    arm.range,
                    "an unguarded catch-all must be the final case arm",
                );
            }
            if unguarded && useful && normalized.len() == subjects.len() {
                coverage.push(normalized);
            }
            if let Some(guard) = &arm.guard {
                facts.push(self.infer_case_guard(module_id, guard, generics, &mut arm_bindings));
            }
            let body = self.infer_expression(module_id, &arm.body, generics, &mut arm_bindings);
            if let Some(expected) = success {
                self.unify_expression_result(module_id, &arm.body, body.success, expected);
            } else {
                success = Some(body.success);
            }
            facts.push(body);
        }
        let missing = vec![MatchPattern::Wildcard; subjects.len()];
        if self.pattern_row_is_useful(&coverage, &missing, &subject_types) {
            self.type_error_range(module_id, "T4231", range, "case is not exhaustive");
        }
        ExpressionFact {
            success: success.unwrap_or(TypeId::new(0)),
            failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
            needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
        }
    }

    fn normalize_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        expected: TypeId,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> MatchPattern {
        if let Some(literal) = &pattern.literal {
            let (literal_type, tag) = match literal {
                PatternLiteral::Integer(value) => {
                    (self.builtin_type("Int"), MatchTag::Integer(*value))
                }
                PatternLiteral::Text(value) => {
                    (self.builtin_type("Text"), MatchTag::Text(value.clone()))
                }
                PatternLiteral::Boolean(value) => {
                    (self.builtin_type("Bool"), MatchTag::Boolean(*value))
                }
            };
            if !self.unify(expected, literal_type) {
                self.type_error_range(
                    module_id,
                    "T4230",
                    pattern.range,
                    "literal pattern does not match the subject type",
                );
            }
            return MatchPattern::Constructor {
                tag,
                fields: Vec::new(),
            };
        }
        if let Some(binding) = self.pattern_binding_symbol(module_id, pattern) {
            bindings.insert(
                binding,
                ValueBinding {
                    type_id: expected,
                    need: None,
                },
            );
            return MatchPattern::Wildcard;
        }
        let Some(constructor) = self.resolved_pattern_symbol(module_id, pattern) else {
            return MatchPattern::Wildcard;
        };
        if self.resolved.symbol(constructor).kind != ProjectSymbolKind::EnumConstructor {
            self.type_error_range(
                module_id,
                "T4230",
                pattern.range,
                "ordinary patterns require a value binding, literal, or enum variant",
            );
            return MatchPattern::Wildcard;
        }
        let Some(inputs) = self.instantiate_pattern_constructor(constructor, expected) else {
            self.type_error_range(
                module_id,
                "T4230",
                pattern.range,
                format!(
                    "pattern `{}` does not match the subject type",
                    self.resolved.symbol(constructor).name
                ),
            );
            return MatchPattern::Wildcard;
        };
        if pattern.arguments.len() != inputs.len() {
            self.type_error_range(
                module_id,
                "T4230",
                pattern.range,
                format!(
                    "pattern `{}` expects {} fields, found {}",
                    self.resolved.symbol(constructor).name,
                    inputs.len(),
                    pattern.arguments.len()
                ),
            );
        }
        let fields = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                pattern
                    .arguments
                    .get(index)
                    .map_or(MatchPattern::Wildcard, |argument| {
                        self.normalize_pattern(module_id, argument, *input, bindings)
                    })
            })
            .collect();
        MatchPattern::Constructor {
            tag: MatchTag::EnumVariant(constructor),
            fields,
        }
    }

    fn instantiate_pattern_constructor(
        &mut self,
        constructor: SymbolId,
        expected: TypeId,
    ) -> Option<Vec<TypeId>> {
        let contract = self.contracts.get(&constructor)?.clone();
        let mut substitution = CallSubstitution::default();
        if !self.match_call_type(expected, contract.success, &mut substitution) {
            return None;
        }
        Some(
            contract
                .inputs
                .iter()
                .map(|input| self.substitute_call_type(*input, &mut substitution))
                .collect(),
        )
    }

    fn infer_case_guard(
        &mut self,
        module_id: ModuleId,
        guard: &ModuleExpression,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let fact = self.infer_expression(module_id, guard, generics, bindings);
        let range = expression_range(guard);
        let boolean = self.builtin_type("Bool");
        if !self.unify(fact.success, boolean) {
            self.type_error_range(module_id, "T4234", range, "case guard must produce Bool");
        } else if !self.failure_row_is_empty(fact.failures) || !self.needs_row_is_empty(fact.needs)
        {
            self.type_error_range(
                module_id,
                "T4234",
                range,
                "case guards must be pure and cannot expose failures or dependencies",
            );
        }
        fact
    }

    fn pattern_row_is_useful(
        &mut self,
        matrix: &[Vec<MatchPattern>],
        candidate: &[MatchPattern],
        types: &[TypeId],
    ) -> bool {
        if candidate.is_empty() {
            return matrix.is_empty();
        }
        let Some(first_type) = types.first().copied() else {
            return true;
        };
        let tail_types = &types[1..];
        match &candidate[0] {
            MatchPattern::Constructor { tag, fields } => {
                let field_types = self
                    .pattern_constructor_field_types(tag, first_type)
                    .unwrap_or_else(|| vec![TypeId::new(0); fields.len()]);
                let specialized = specialize_pattern_matrix(matrix, tag, field_types.len());
                let mut specialized_candidate = fields.clone();
                specialized_candidate.extend_from_slice(&candidate[1..]);
                let mut specialized_types = field_types;
                specialized_types.extend_from_slice(tail_types);
                self.pattern_row_is_useful(&specialized, &specialized_candidate, &specialized_types)
            }
            MatchPattern::Wildcard => {
                if let Some(domain) = self.complete_pattern_domain(first_type) {
                    let present = matrix
                        .iter()
                        .filter_map(|row| match row.first() {
                            Some(MatchPattern::Constructor { tag, .. }) => Some(tag),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if domain.iter().all(|(tag, _)| present.contains(&tag)) {
                        return domain.into_iter().any(|(tag, field_types)| {
                            let specialized =
                                specialize_pattern_matrix(matrix, &tag, field_types.len());
                            let mut specialized_candidate =
                                vec![MatchPattern::Wildcard; field_types.len()];
                            specialized_candidate.extend_from_slice(&candidate[1..]);
                            let mut specialized_types = field_types;
                            specialized_types.extend_from_slice(tail_types);
                            self.pattern_row_is_useful(
                                &specialized,
                                &specialized_candidate,
                                &specialized_types,
                            )
                        });
                    }
                }
                let default = default_pattern_matrix(matrix);
                self.pattern_row_is_useful(&default, &candidate[1..], tail_types)
            }
        }
    }

    fn pattern_constructor_field_types(
        &mut self,
        tag: &MatchTag,
        expected: TypeId,
    ) -> Option<Vec<TypeId>> {
        match tag {
            MatchTag::EnumVariant(constructor) => {
                self.instantiate_pattern_constructor(*constructor, expected)
            }
            MatchTag::Boolean(_) | MatchTag::Integer(_) | MatchTag::Text(_) => Some(Vec::new()),
        }
    }

    fn complete_pattern_domain(&mut self, type_id: TypeId) -> Option<Vec<(MatchTag, Vec<TypeId>)>> {
        let type_id = self.resolve_inference_type(type_id);
        let CanonicalType::Named { symbol, .. } = self.types[type_id.raw() as usize].clone() else {
            return None;
        };
        let symbol_kind = self.resolved.symbol(symbol).kind;
        if symbol_kind == ProjectSymbolKind::BuiltinType
            && self.resolved.symbol(symbol).name == "Bool"
        {
            return Some(vec![
                (MatchTag::Boolean(false), Vec::new()),
                (MatchTag::Boolean(true), Vec::new()),
            ]);
        }
        if symbol_kind != ProjectSymbolKind::Enum {
            return None;
        }
        Some(
            self.enum_variant_symbols(symbol)
                .into_iter()
                .filter_map(|variant| {
                    self.instantiate_pattern_constructor(variant, type_id)
                        .map(|fields| (MatchTag::EnumVariant(variant), fields))
                })
                .collect(),
        )
    }

    fn infer_outcome_case(
        &mut self,
        module_id: ModuleId,
        subject: ExpressionFact,
        arms: &[ModuleCaseArm],
        range: TextRange,
        generics: &HashMap<String, SymbolId>,
        bindings: &HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let mut arm_facts = Vec::new();
        let mut success = None;
        let mut success_coverage = Vec::<Vec<MatchPattern>>::new();
        let failure_type = self.failure_value_type(subject.failures);
        let mut failure_coverage = Vec::<Vec<MatchPattern>>::new();
        let mut fallback_handled_variants = Vec::new();
        let mut handles_unknown_row = false;
        let mut saw_failure = false;
        for arm in arms {
            let Some(pattern) = arm.patterns.first() else {
                continue;
            };
            let mut arm_bindings = bindings.clone();
            if self.pattern_is_builtin(module_id, pattern, "Success") {
                if let Some(inner) = self.single_outcome_pattern_argument(module_id, pattern) {
                    let normalized = self.normalize_pattern(
                        module_id,
                        inner,
                        subject.success,
                        &mut arm_bindings,
                    );
                    let row = vec![normalized];
                    let useful =
                        self.pattern_row_is_useful(&success_coverage, &row, &[subject.success]);
                    if !useful {
                        self.type_error_range(
                            module_id,
                            "T4232",
                            arm.range,
                            "outcome case arm is unreachable",
                        );
                    } else if arm.guard.is_none() {
                        success_coverage.push(row);
                    }
                }
            } else if self.pattern_is_builtin(module_id, pattern, "Failure") {
                saw_failure = true;
                if let Some(inner) = self.single_outcome_pattern_argument(module_id, pattern) {
                    let (mut variants, catches_unknown) = self.handled_failure_pattern(
                        module_id,
                        inner,
                        subject.failures,
                        &mut arm_bindings,
                    );
                    if variants.is_empty() && !catches_unknown {
                        // The owning diagnostic already explains why this pattern cannot
                        // participate in failure-channel coverage.
                    } else if let Some(failure_type) = failure_type {
                        let normalized = self.normalize_pattern(
                            module_id,
                            inner,
                            failure_type,
                            &mut arm_bindings,
                        );
                        let row = vec![normalized];
                        let useful =
                            self.pattern_row_is_useful(&failure_coverage, &row, &[failure_type]);
                        if !useful && (!variants.is_empty() || catches_unknown) {
                            self.type_error_range(
                                module_id,
                                "T4232",
                                arm.range,
                                "outcome case arm is unreachable",
                            );
                        } else if arm.guard.is_none() && useful {
                            handles_unknown_row |= catches_unknown;
                            failure_coverage.push(row);
                        }
                    } else if arm.guard.is_none() {
                        fallback_handled_variants.append(&mut variants);
                        handles_unknown_row |= catches_unknown;
                    }
                }
            } else {
                let pattern_range = pattern.name.as_ref().map_or(range, |name| name.range);
                self.type_error_range(
                    module_id,
                    "T4222",
                    pattern_range,
                    "a fallible case arm must begin with Success(...) or Failure(...)",
                );
            }
            if let Some(guard) = &arm.guard {
                arm_facts.push(self.infer_case_guard(
                    module_id,
                    guard,
                    generics,
                    &mut arm_bindings,
                ));
            }
            let body = self.infer_expression(module_id, &arm.body, generics, &mut arm_bindings);
            if let Some(expected) = success {
                self.unify_expression_result(module_id, &arm.body, body.success, expected);
            } else {
                success = Some(body.success);
            }
            arm_facts.push(body);
        }
        let missing_success = vec![MatchPattern::Wildcard];
        if self.pattern_row_is_useful(&success_coverage, &missing_success, &[subject.success]) {
            self.type_error_range(
                module_id,
                "T4223",
                range,
                "outcome case does not fully handle Success",
            );
        }
        if !saw_failure && !self.failure_row_is_empty(subject.failures) {
            self.type_error_range(
                module_id,
                "T4223",
                range,
                "outcome case does not handle Failure",
            );
        }
        let mut handled_variants = fallback_handled_variants;
        if let Some(failure_type) = failure_type {
            let subject_variants = self.failure_rows[subject.failures.raw() as usize]
                .variants
                .clone();
            for variant in subject_variants {
                let field_count = self
                    .instantiate_pattern_constructor(variant, failure_type)
                    .map_or(0, |fields| fields.len());
                let candidate = vec![MatchPattern::Constructor {
                    tag: MatchTag::EnumVariant(variant),
                    fields: vec![MatchPattern::Wildcard; field_count],
                }];
                if !self.pattern_row_is_useful(&failure_coverage, &candidate, &[failure_type]) {
                    handled_variants.push(variant);
                }
            }
        }
        handled_variants.sort_unstable();
        handled_variants.dedup();
        let remaining =
            self.remove_handled_failures(subject.failures, &handled_variants, handles_unknown_row);
        ExpressionFact {
            success: success.unwrap_or(subject.success),
            failures: self.union_failure_rows(
                std::iter::once(remaining).chain(arm_facts.iter().map(|fact| fact.failures)),
            ),
            needs: self.union_needs_rows(
                std::iter::once(subject.needs).chain(arm_facts.iter().map(|fact| fact.needs)),
            ),
        }
    }

    fn single_outcome_pattern_argument<'pattern>(
        &mut self,
        module_id: ModuleId,
        pattern: &'pattern Pattern,
    ) -> Option<&'pattern Pattern> {
        if pattern.arguments.len() != 1 {
            let range = pattern
                .name
                .as_ref()
                .map_or(TextRange::empty(0), |name| name.range);
            self.type_error_range(
                module_id,
                "T4222",
                range,
                "outcome patterns require exactly one nested pattern",
            );
            None
        } else {
            pattern.arguments.first()
        }
    }

    fn handled_failure_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        failures: FailureRowId,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> (Vec<SymbolId>, bool) {
        let symbol = self.resolved_pattern_symbol(module_id, pattern);
        if let Some(symbol) = symbol
            && self.resolved.symbol(symbol).kind == ProjectSymbolKind::EnumConstructor
        {
            if !self.failure_rows[failures.raw() as usize]
                .variants
                .contains(&symbol)
            {
                let range = pattern
                    .name
                    .as_ref()
                    .map_or(TextRange::empty(0), |name| name.range);
                self.type_error_range(
                    module_id,
                    "T4224",
                    range,
                    "failure pattern is not present in the subject's failure channel",
                );
                return (Vec::new(), false);
            }
            let success = self.contracts[&symbol].success;
            self.bind_value_pattern(module_id, pattern, success, bindings);
            return (vec![symbol], false);
        }

        if self.pattern_binding_symbol(module_id, pattern).is_some() {
            if let Some(error_type) = self.failure_value_type(failures) {
                self.bind_value_pattern(module_id, pattern, error_type, bindings);
            } else {
                self.bind_value_pattern(module_id, pattern, TypeId::new(0), bindings);
            }
            return (
                self.failure_rows[failures.raw() as usize].variants.clone(),
                true,
            );
        }
        if symbol.is_some() || pattern.name.is_none() {
            let range = pattern
                .name
                .as_ref()
                .map_or(TextRange::empty(0), |name| name.range);
            self.type_error_range(
                module_id,
                "T4222",
                range,
                "Failure(...) expects an error variant or binding pattern",
            );
        }
        (Vec::new(), false)
    }

    fn bind_value_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        expected: TypeId,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) {
        if let Some(literal) = &pattern.literal {
            let literal_type = self.builtin_type(match literal {
                PatternLiteral::Integer(_) => "Int",
                PatternLiteral::Text(_) => "Text",
                PatternLiteral::Boolean(_) => "Bool",
            });
            if !self.unify(expected, literal_type) {
                self.type_error_range(
                    module_id,
                    "T4222",
                    pattern.range,
                    "nested outcome pattern does not match its value type",
                );
            }
            return;
        }
        let Some(name) = &pattern.name else {
            return;
        };
        if let Some(binding) = self.pattern_binding_symbol(module_id, pattern) {
            bindings.insert(
                binding,
                ValueBinding {
                    type_id: expected,
                    need: None,
                },
            );
            return;
        }
        let Some(constructor) = self.resolved_pattern_symbol(module_id, pattern) else {
            return;
        };
        if self.resolved.symbol(constructor).kind != ProjectSymbolKind::EnumConstructor {
            return;
        }
        let contract = self.contracts[&constructor].clone();
        let mut substitution = CallSubstitution::default();
        if !self.match_call_type(expected, contract.success, &mut substitution) {
            self.type_error_range(
                module_id,
                "T4222",
                name.range,
                format!("pattern `{}` does not match the subject type", name.text),
            );
            return;
        }
        if pattern.arguments.len() != contract.inputs.len() {
            self.type_error_range(
                module_id,
                "T4222",
                name.range,
                format!(
                    "pattern `{}` expects {} fields, found {}",
                    name.text,
                    contract.inputs.len(),
                    pattern.arguments.len()
                ),
            );
        }
        let inputs = contract
            .inputs
            .iter()
            .map(|input| self.substitute_call_type(*input, &mut substitution))
            .collect::<Vec<_>>();
        for (argument, input) in pattern.arguments.iter().zip(inputs) {
            self.bind_value_pattern(module_id, argument, input, bindings);
        }
    }

    fn pattern_is_builtin(&self, module_id: ModuleId, pattern: &Pattern, name: &str) -> bool {
        self.resolved_pattern_symbol(module_id, pattern)
            .is_some_and(|symbol| self.is_builtin_value(symbol, name))
    }

    fn pattern_binding_symbol(&self, module_id: ModuleId, pattern: &Pattern) -> Option<SymbolId> {
        self.find_symbol(
            module_id,
            Some(pattern.semantic_id),
            ProjectSymbolKind::PatternBinding,
            &pattern.name.as_ref()?.text,
        )
    }

    fn resolved_pattern_symbol(&self, module_id: ModuleId, pattern: &Pattern) -> Option<SymbolId> {
        self.resolved
            .references
            .iter()
            .find(|reference| {
                reference.file_id.raw() == module_id.raw()
                    && reference.semantic_id == pattern.semantic_id
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

    fn failure_value_type(&self, failures: FailureRowId) -> Option<TypeId> {
        let row = &self.failure_rows[failures.raw() as usize];
        if !row.parameters.is_empty() {
            return None;
        }
        let mut owners = row
            .variants
            .iter()
            .filter_map(|variant| self.enum_owner_for_variant(*variant))
            .collect::<Vec<_>>();
        owners.sort_unstable();
        owners.dedup();
        let [owner] = owners.as_slice() else {
            return None;
        };
        let owner = self.resolved.symbol(*owner);
        self.declaration_types
            .get(&(owner.module_id?, owner.semantic_id?))
            .copied()
    }

    fn remove_handled_failures(
        &mut self,
        failures: FailureRowId,
        handled_variants: &[SymbolId],
        handles_unknown_row: bool,
    ) -> FailureRowId {
        let mut row = self.failure_rows[failures.raw() as usize].clone();
        row.variants
            .retain(|variant| !handled_variants.contains(variant));
        if handles_unknown_row {
            row.parameters.clear();
        }
        self.intern_failure_row(row)
    }

    fn failure_row_is_empty(&self, failures: FailureRowId) -> bool {
        let row = &self.failure_rows[failures.raw() as usize];
        row.variants.is_empty() && row.parameters.is_empty()
    }

    fn needs_row_is_empty(&self, needs: NeedsRowId) -> bool {
        let row = &self.needs_rows[needs.raw() as usize];
        row.entries.is_empty() && row.parameters.is_empty()
    }

    fn enum_owner_for_variant(&self, variant: SymbolId) -> Option<SymbolId> {
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
        self.find_symbol(
            module_id,
            Some(declaration.semantic_id),
            ProjectSymbolKind::Enum,
            &declaration.name.text,
        )
    }

    fn enum_variant_symbols(&self, owner: SymbolId) -> Vec<SymbolId> {
        let owner = self.resolved.symbol(owner);
        let module_id = owner.module_id.expect("enum is source-backed");
        let semantic_id = owner.semantic_id.expect("enum has semantic identity");
        let declaration = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
            .expect("enum symbol owns its declaration");
        let mut variants = declaration
            .variants
            .iter()
            .filter_map(|variant| {
                self.find_symbol(
                    module_id,
                    Some(variant.semantic_id),
                    ProjectSymbolKind::EnumConstructor,
                    &variant.name.text,
                )
            })
            .collect::<Vec<_>>();
        variants.sort_unstable();
        variants
    }

    fn is_builtin_value(&self, symbol: SymbolId, name: &str) -> bool {
        let symbol = self.resolved.symbol(symbol);
        symbol.module_id.is_none()
            && symbol.kind == ProjectSymbolKind::BuiltinValue
            && symbol.name == name
    }

    fn infer_using(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        providers: &[crate::semantic::RecordValueField],
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let expression_fact = self.infer_expression(module_id, expression, generics, bindings);
        let mut provider_facts = Vec::with_capacity(providers.len());
        let mut seen_names = Vec::<String>::new();
        let mut duplicate_names = Vec::<String>::new();
        for provider in providers {
            let fact = self.infer_expression(module_id, &provider.value, generics, bindings);
            if seen_names.iter().any(|name| name == &provider.name.text) {
                self.type_error_range(
                    module_id,
                    "T4214",
                    provider.name.range,
                    format!("provider `{}` occurs more than once", provider.name.text),
                );
                duplicate_names.push(provider.name.text.clone());
            } else {
                seen_names.push(provider.name.text.clone());
            }
            provider_facts.push((provider, fact));
        }

        let (success, latent_needs) = self.callable_success_and_needs(expression_fact.success);
        let available_needs = self.union_needs_rows([expression_fact.needs, latent_needs]);
        let mut supplied_names = Vec::new();
        let mut explained_names = Vec::new();
        for (provider, fact) in &provider_facts {
            if duplicate_names
                .iter()
                .any(|name| name == &provider.name.text)
            {
                continue;
            }
            let matching_need = self.needs_rows[available_needs.raw() as usize]
                .entries
                .iter()
                .find(|need| need.lexical_name == provider.name.text)
                .cloned();
            let Some(need) = matching_need else {
                self.type_error_range(
                    module_id,
                    "T4212",
                    provider.name.range,
                    format!("expression does not need provider `{}`", provider.name.text),
                );
                continue;
            };
            if let Some(reason) = self.provider_conformance_error(fact.success, need.interface) {
                self.type_error_range(
                    module_id,
                    "T4213",
                    provider.name.range,
                    format!("provider `{}` {reason}", provider.name.text),
                );
                explained_names.push(provider.name.text.clone());
            } else {
                supplied_names.push(provider.name.text.clone());
            }
        }

        let mut remaining_expression_needs =
            self.remove_supplied_needs(expression_fact.needs, &supplied_names);
        remaining_expression_needs =
            self.mark_explained_needs(remaining_expression_needs, &explained_names);
        let mut remaining_latent_needs = self.remove_supplied_needs(latent_needs, &supplied_names);
        remaining_latent_needs =
            self.mark_explained_needs(remaining_latent_needs, &explained_names);
        let success = self.with_callable_needs(success, remaining_latent_needs);
        ExpressionFact {
            success,
            failures: self.union_failure_rows(
                std::iter::once(expression_fact.failures)
                    .chain(provider_facts.iter().map(|(_, fact)| fact.failures)),
            ),
            needs: self.union_needs_rows(
                std::iter::once(remaining_expression_needs)
                    .chain(provider_facts.iter().map(|(_, fact)| fact.needs)),
            ),
        }
    }

    fn callable_success_and_needs(&self, success: TypeId) -> (TypeId, NeedsRowId) {
        let success = self.resolve_inference_type(success);
        match &self.types[success.raw() as usize] {
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => (success, contract.needs),
            _ => (success, NeedsRowId::new(0)),
        }
    }

    fn with_callable_needs(&mut self, success: TypeId, needs: NeedsRowId) -> TypeId {
        let success = self.resolve_inference_type(success);
        match self.types[success.raw() as usize].clone() {
            CanonicalType::Function(mut contract) => {
                contract.needs = needs;
                self.intern_type(CanonicalType::Function(contract))
            }
            CanonicalType::PolymorphicFunction {
                parameters,
                mut contract,
            } => {
                contract.needs = needs;
                self.intern_type(CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                })
            }
            _ => success,
        }
    }

    fn remove_supplied_needs(&mut self, row: NeedsRowId, supplied: &[String]) -> NeedsRowId {
        let mut row = self.needs_rows[row.raw() as usize].clone();
        row.entries
            .retain(|need| !supplied.iter().any(|name| name == &need.lexical_name));
        self.intern_needs_row(row)
    }

    fn mark_explained_needs(&mut self, row: NeedsRowId, explained: &[String]) -> NeedsRowId {
        let mut row = self.needs_rows[row.raw() as usize].clone();
        for need in &mut row.entries {
            if explained.iter().any(|name| name == &need.lexical_name) {
                need.explained = true;
            }
        }
        self.intern_needs_row(row)
    }

    fn provider_conformance_error(
        &mut self,
        provider: TypeId,
        interface: TypeId,
    ) -> Option<String> {
        let provider = self.resolve_inference_type(provider);
        let interface = self.resolve_inference_type(interface);
        let Some(expected_fields) = self.type_fields(interface) else {
            return Some("targets a non-structural requirement".to_owned());
        };
        let Some(actual_fields) = self.type_fields(provider) else {
            return Some("is not a structural value".to_owned());
        };
        for expected in expected_fields {
            let Some(actual) = actual_fields
                .iter()
                .find(|actual| actual.name == expected.name)
            else {
                return Some(format!("is missing member `{}`", expected.name));
            };
            if let Some(reason) =
                self.provider_member_conformance_error(actual.type_id, expected.type_id)
            {
                return Some(format!(
                    "has incompatible member `{}`: {reason}",
                    expected.name
                ));
            }
        }
        None
    }

    fn type_fields(&mut self, type_id: TypeId) -> Option<Vec<StructuralField>> {
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::Record(fields) => Some(fields),
            CanonicalType::Named { symbol, arguments } => {
                let fields = self.fields.get(&symbol)?.clone();
                let parameters = self.named_type_generic_parameters(symbol);
                let mut substitution = CallSubstitution::default();
                substitution
                    .values
                    .extend(parameters.into_iter().zip(arguments));
                Some(
                    fields
                        .into_iter()
                        .map(|field| StructuralField {
                            name: field.name,
                            type_id: self.substitute_call_type(field.type_id, &mut substitution),
                        })
                        .collect(),
                )
            }
            _ => None,
        }
    }

    fn named_type_generic_parameters(&self, symbol: SymbolId) -> Vec<SymbolId> {
        let symbol = self.resolved.symbol(symbol);
        let (Some(module_id), Some(semantic_id)) = (symbol.module_id, symbol.semantic_id) else {
            return Vec::new();
        };
        let Some(declaration) = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
        else {
            return Vec::new();
        };
        declaration
            .generic_parameters
            .iter()
            .filter_map(|parameter| {
                self.find_symbol(
                    module_id,
                    Some(semantic_id),
                    ProjectSymbolKind::GenericParameter,
                    &parameter.text,
                )
            })
            .collect()
    }

    fn provider_member_conformance_error(
        &mut self,
        actual: TypeId,
        expected: TypeId,
    ) -> Option<&'static str> {
        let actual = self.resolve_inference_type(actual);
        let expected = self.resolve_inference_type(expected);
        match (
            self.types[actual.raw() as usize].clone(),
            self.types[expected.raw() as usize].clone(),
        ) {
            (CanonicalType::Function(actual), CanonicalType::Function(expected)) => {
                if actual.inputs.len() != expected.inputs.len() {
                    return Some("input count differs");
                }
                if !actual
                    .inputs
                    .iter()
                    .zip(&expected.inputs)
                    .all(|(actual, expected)| self.unify(*actual, *expected))
                {
                    return Some("input types differ");
                }
                if !self.unify(actual.success, expected.success) {
                    return Some("success type differs");
                }
                if !self.failure_row_is_subset(actual.failures, expected.failures) {
                    return Some("it exposes failures outside the interface contract");
                }
                if !self.needs_row_is_subset(actual.needs, expected.needs) {
                    return Some("it needs dependencies outside the interface contract");
                }
                None
            }
            _ if self.unify(actual, expected) => None,
            _ => Some("value type differs"),
        }
    }

    fn failure_row_is_subset(&self, actual: FailureRowId, expected: FailureRowId) -> bool {
        let actual = &self.failure_rows[actual.raw() as usize];
        let expected = &self.failure_rows[expected.raw() as usize];
        actual
            .variants
            .iter()
            .all(|variant| expected.variants.contains(variant))
            && actual
                .parameters
                .iter()
                .all(|parameter| expected.parameters.contains(parameter))
    }

    fn needs_row_is_subset(&self, actual: NeedsRowId, expected: NeedsRowId) -> bool {
        let actual = &self.needs_rows[actual.raw() as usize];
        let expected = &self.needs_rows[expected.raw() as usize];
        actual.entries.iter().all(|actual| {
            expected.entries.iter().any(|expected| {
                actual.lexical_name == expected.lexical_name
                    && actual.interface == expected.interface
            })
        }) && actual
            .parameters
            .iter()
            .all(|parameter| expected.parameters.contains(parameter))
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
                        explained: false,
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
        self.builtin_type_with_arguments(name, Vec::new())
    }

    fn builtin_type_with_arguments(&mut self, name: &str, arguments: Vec<TypeId>) -> TypeId {
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
        self.intern_type(CanonicalType::Named { symbol, arguments })
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

    fn expression_value_symbol(
        &self,
        module_id: ModuleId,
        expression: &ModuleExpression,
    ) -> Option<SymbolId> {
        match expression {
            ModuleExpression::Name { semantic_id, .. }
            | ModuleExpression::Select { semantic_id, .. } => {
                self.resolved_value_symbol(module_id, *semantic_id)
            }
            ModuleExpression::Group { expression, .. } => {
                self.expression_value_symbol(module_id, expression)
            }
            _ => None,
        }
    }

    fn infer_invocation(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        callee_symbol: Option<SymbolId>,
        callee: ExpressionFact,
        arguments: &[InvocationArgument],
    ) -> ExpressionFact {
        let callee_type = self.resolve_inference_type(callee.success);
        let (contract, mut substitution) = match self.types.get(callee_type.raw() as usize).cloned()
        {
            Some(CanonicalType::Function(contract)) => (contract, None),
            Some(CanonicalType::PolymorphicFunction { contract, .. }) => {
                (contract, Some(CallSubstitution::default()))
            }
            _ => {
                if callee_type != TypeId::new(0) {
                    self.type_error_range(module_id, "T4201", range, "expression is not callable");
                }
                return ExpressionFact {
                    success: TypeId::new(0),
                    failures: self.union_failure_rows(
                        std::iter::once(callee.failures)
                            .chain(arguments.iter().map(|argument| argument.fact.failures)),
                    ),
                    needs: self.union_needs_rows(
                        std::iter::once(callee.needs)
                            .chain(arguments.iter().map(|argument| argument.fact.needs)),
                    ),
                };
            }
        };
        if contract.inputs.len() != arguments.len() {
            self.type_error_range(
                module_id,
                "T4202",
                range,
                format!(
                    "call expects {} arguments, found {}",
                    contract.inputs.len(),
                    arguments.len()
                ),
            );
        }
        let argument_order = self.invocation_argument_order(
            module_id,
            range,
            callee_symbol,
            arguments,
            contract.inputs.len(),
        );
        for (input_index, argument_index) in argument_order.into_iter().enumerate() {
            let Some(expected) = contract.inputs.get(input_index).copied() else {
                break;
            };
            let Some(argument) = arguments.get(argument_index) else {
                continue;
            };
            if let Some(substitution) = &mut substitution {
                if !self.match_call_type(argument.fact.success, expected, substitution) {
                    self.type_error_range(
                        module_id,
                        "T4203",
                        range,
                        "call argument has an incompatible type",
                    );
                }
            } else {
                self.unify_at(
                    module_id,
                    range,
                    argument.fact.success,
                    expected,
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
        ExpressionFact {
            success,
            failures: self.union_failure_rows(
                std::iter::once(callee.failures)
                    .chain(arguments.iter().map(|argument| argument.fact.failures))
                    .chain(std::iter::once(contract_failures)),
            ),
            needs: self.union_needs_rows(
                std::iter::once(callee.needs)
                    .chain(arguments.iter().map(|argument| argument.fact.needs))
                    .chain(std::iter::once(contract_needs)),
            ),
        }
    }

    fn invocation_argument_order(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        callee_symbol: Option<SymbolId>,
        arguments: &[InvocationArgument],
        input_count: usize,
    ) -> Vec<usize> {
        let constructor_fields = callee_symbol.and_then(|symbol| {
            matches!(
                self.resolved.symbol(symbol).kind,
                ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor
            )
            .then(|| self.fields.get(&symbol).cloned())
            .flatten()
        });
        let Some(fields) = constructor_fields else {
            if arguments.iter().any(|argument| argument.label.is_some()) {
                self.type_error_range(
                    module_id,
                    "T4210",
                    range,
                    "named arguments are reserved for nominal constructors",
                );
            }
            return (0..arguments.len().min(input_count)).collect();
        };
        let mut order = Vec::with_capacity(fields.len());
        for field in &fields {
            let matches = arguments
                .iter()
                .enumerate()
                .filter(|(_, argument)| {
                    argument
                        .label
                        .as_ref()
                        .is_some_and(|label| label.text == field.name)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                order.push(matches[0]);
            } else {
                self.type_error_range(
                    module_id,
                    "T4210",
                    range,
                    format!("constructor requires exactly one `{}` field", field.name),
                );
            }
        }
        if arguments.iter().any(|argument| {
            argument
                .label
                .as_ref()
                .is_none_or(|label| !fields.iter().any(|field| field.name == label.text))
        }) {
            self.type_error_range(
                module_id,
                "T4210",
                range,
                "constructor arguments must name declared fields",
            );
        }
        order
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
        let parameters = self.callable_generic_parameters(symbol);
        if parameters.is_empty() {
            self.intern_type(CanonicalType::Function(contract))
        } else {
            self.intern_type(CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            })
        }
    }

    fn callable_generic_parameters(&self, symbol: SymbolId) -> Vec<SymbolId> {
        let symbol = self.resolved.symbol(symbol);
        if !matches!(
            symbol.kind,
            ProjectSymbolKind::Function
                | ProjectSymbolKind::StructConstructor
                | ProjectSymbolKind::EnumConstructor
        ) {
            return Vec::new();
        }
        let declaration_semantic_id = if symbol.kind == ProjectSymbolKind::EnumConstructor {
            let Some(module_id) = symbol.module_id else {
                return Vec::new();
            };
            self.resolved.semantic.modules[module_id.raw() as usize]
                .declarations
                .iter()
                .find(|declaration| {
                    declaration
                        .variants
                        .iter()
                        .any(|variant| Some(variant.semantic_id) == symbol.semantic_id)
                })
                .map(|declaration| declaration.semantic_id)
        } else {
            symbol.semantic_id
        };
        let mut parameters = self
            .resolved
            .symbols
            .iter()
            .filter(|candidate| {
                candidate.module_id == symbol.module_id
                    && candidate.semantic_id == declaration_semantic_id
                    && candidate.kind == ProjectSymbolKind::GenericParameter
            })
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        parameters.sort_unstable();
        parameters
    }

    fn field_type(&self, receiver: TypeId, member: &str) -> Option<TypeId> {
        let receiver = self.resolve_inference_type(receiver);
        match &self.types[receiver.raw() as usize] {
            CanonicalType::Named { symbol, .. } => self
                .fields
                .get(symbol)?
                .iter()
                .find(|field| field.name == member)
                .map(|field| field.type_id),
            CanonicalType::Record(fields) => fields
                .iter()
                .find(|field| field.name == member)
                .map(|field| field.type_id),
            _ => None,
        }
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
            (CanonicalType::Record(actual), CanonicalType::Record(expected)) => {
                actual.len() == expected.len()
                    && actual.into_iter().zip(expected).all(|(actual, expected)| {
                        actual.name == expected.name
                            && self.match_call_type(actual.type_id, expected.type_id, substitution)
                    })
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
            CanonicalType::Record(fields) => {
                let fields = fields
                    .into_iter()
                    .map(|field| StructuralField {
                        name: field.name,
                        type_id: self.substitute_call_type(field.type_id, substitution),
                    })
                    .collect();
                self.intern_type(CanonicalType::Record(fields))
            }
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
            (CanonicalType::Record(left), CanonicalType::Record(right)) => {
                left.len() == right.len()
                    && left.into_iter().zip(right).all(|(left, right)| {
                        left.name == right.name && self.unify(left.type_id, right.type_id)
                    })
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
                if let Some(existing) = entries.iter_mut().find(|existing| {
                    existing.lexical_name == need.lexical_name
                        && existing.interface == need.interface
                }) {
                    // A need is explained only when every contributing path has
                    // already emitted a more specific diagnostic.
                    existing.explained &= need.explained;
                } else {
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

fn specialize_pattern_matrix(
    matrix: &[Vec<MatchPattern>],
    tag: &MatchTag,
    field_count: usize,
) -> Vec<Vec<MatchPattern>> {
    matrix
        .iter()
        .filter_map(|row| {
            let (head, tail) = row.split_first()?;
            match head {
                MatchPattern::Wildcard => {
                    let mut specialized = vec![MatchPattern::Wildcard; field_count];
                    specialized.extend_from_slice(tail);
                    Some(specialized)
                }
                MatchPattern::Constructor {
                    tag: row_tag,
                    fields,
                } if row_tag == tag => {
                    let mut specialized = fields.clone();
                    specialized.extend_from_slice(tail);
                    Some(specialized)
                }
                MatchPattern::Constructor { .. } => None,
            }
        })
        .collect()
}

fn default_pattern_matrix(matrix: &[Vec<MatchPattern>]) -> Vec<Vec<MatchPattern>> {
    matrix
        .iter()
        .filter_map(|row| {
            let (head, tail) = row.split_first()?;
            matches!(head, MatchPattern::Wildcard).then(|| tail.to_vec())
        })
        .collect()
}

fn expression_range(expression: &ModuleExpression) -> TextRange {
    match expression {
        ModuleExpression::Literal(literal) => literal.range(),
        ModuleExpression::Name { name, .. } => name.range,
        ModuleExpression::Select { range, .. }
        | ModuleExpression::Call { range, .. }
        | ModuleExpression::List { range, .. }
        | ModuleExpression::Record { range, .. }
        | ModuleExpression::Unary { range, .. }
        | ModuleExpression::Binary { range, .. } => *range,
        ModuleExpression::Group { expression, .. } => expression_range(expression),
        ModuleExpression::Using { range, .. } => *range,
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
        ModuleExpression::Case { range, .. } => *range,
        ModuleExpression::Empty { .. } => TextRange::empty(0),
    }
}
