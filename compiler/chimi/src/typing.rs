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
use crate::standard::{
    StandardConstructor, StandardFunction, StandardGeneric, StandardGenericKind, StandardSymbol,
    StandardType,
};
use std::collections::{HashMap, HashSet};
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

        let literal_value_type = match &function.body.value {
            Literal::Integer { .. } => INT_TYPE,
            // The legacy construction form only lowers integer and text literals.
            Literal::Float { .. } => TEXT_TYPE,
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
    FailureSet,
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
    FailureSetValue(FailureRowId),
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
    pub(crate) variant_arguments: Vec<FailureVariantArguments>,
    pub(crate) parameters: Vec<SymbolId>,
    pub(crate) exclusions: Vec<FailureRowExclusion>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FailureVariantArguments {
    pub(crate) variant: SymbolId,
    pub(crate) arguments: Vec<TypeId>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FailureRowExclusion {
    pub(crate) parameter: SymbolId,
    pub(crate) variants: Vec<SymbolId>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PropertyDerivation {
    Always,
    Never,
    ValueDependent,
}

impl PropertyDerivation {
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::ValueDependent, _) | (_, Self::ValueDependent) => Self::ValueDependent,
            (Self::Always, Self::Always) => Self::Always,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapabilityClass {
    Ordinary,
    Exclusive,
    ParentLocal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ValueProperties {
    pub(crate) transferable: PropertyDerivation,
    pub(crate) shareable: PropertyDerivation,
    pub(crate) capability: CapabilityClass,
}

impl ValueProperties {
    pub(crate) const SHAREABLE: Self = Self {
        transferable: PropertyDerivation::Always,
        shareable: PropertyDerivation::Always,
        capability: CapabilityClass::Ordinary,
    };

    pub(crate) const VALUE_DEPENDENT: Self = Self {
        transferable: PropertyDerivation::ValueDependent,
        shareable: PropertyDerivation::ValueDependent,
        capability: CapabilityClass::Ordinary,
    };

    pub(crate) const EXCLUSIVE: Self = Self {
        transferable: PropertyDerivation::Always,
        shareable: PropertyDerivation::Never,
        capability: CapabilityClass::Exclusive,
    };

    pub(crate) const PARENT_LOCAL: Self = Self {
        transferable: PropertyDerivation::Never,
        shareable: PropertyDerivation::Never,
        capability: CapabilityClass::ParentLocal,
    };

    pub(crate) const INVALID: Self = Self {
        transferable: PropertyDerivation::Never,
        shareable: PropertyDerivation::Never,
        capability: CapabilityClass::Ordinary,
    };

    pub(crate) fn combine(self, other: Self) -> Self {
        Self {
            transferable: self.transferable.combine(other.transferable),
            shareable: self.shareable.combine(other.shareable),
            capability: match (self.capability, other.capability) {
                (CapabilityClass::ParentLocal, _) | (_, CapabilityClass::ParentLocal) => {
                    CapabilityClass::ParentLocal
                }
                (CapabilityClass::Exclusive, _) | (_, CapabilityClass::Exclusive) => {
                    CapabilityClass::Exclusive
                }
                (CapabilityClass::Ordinary, CapabilityClass::Ordinary) => CapabilityClass::Ordinary,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct TypedProject {
    pub(crate) resolved: Arc<ResolvedProject>,
    pub(crate) types: Vec<CanonicalType>,
    pub(crate) failure_rows: Vec<FailureRow>,
    pub(crate) needs_rows: Vec<NeedsRow>,
    pub(crate) contracts: HashMap<SymbolId, CallableContract>,
    pub(crate) declaration_types: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) pattern_binding_types: HashMap<(ModuleId, SemanticId), TypeId>,
    /// A polymorphic callee's function type with this call's substitution
    /// applied, keyed by the callee expression. Monomorphic callees have none.
    pub(crate) callee_instantiations: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) type_facts: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) expression_facts: HashMap<(ModuleId, SemanticId), ExpressionFact>,
    pub(crate) expression_value_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    pub(crate) fields: HashMap<SymbolId, Vec<FieldShape>>,
    pub(crate) generic_kinds: HashMap<SymbolId, GenericKind>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Warnings never block compilation, so they stay out of `diagnostics`.
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) inference_bindings: HashMap<u32, TypeId>,
}

const fn binary_operator_symbol(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Remainder => "%",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
    }
}

pub(crate) fn check_project(resolved: ResolvedProject) -> TypedProject {
    ProjectTypeChecker::new(resolved).check()
}

pub(crate) fn check_project_incremental(
    resolved: ResolvedProject,
    previous: &TypedProject,
    affected_modules: &HashSet<ModuleId>,
) -> TypedProject {
    ProjectTypeChecker::from_previous(resolved, previous, affected_modules)
        .check_modules(affected_modules, affected_modules)
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
            let unresolved = row
                .entries
                .into_iter()
                .filter(|need| !need.explained && !self.native_host_need(need.interface))
                .collect::<Vec<_>>();
            for need in unresolved {
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
            let root = self.resolved.symbol(symbol);
            let (root_module, root_declaration) = (root.module_id, root.semantic_id);
            // A callee's row parameter reaches `main` only when a type error kept
            // it from binding; that error is the report. Execution setup still
            // rejects any parameter left on the contract (E5005).
            let declared = row.parameters.into_iter().filter(|parameter| {
                let parameter = self.resolved.symbol(*parameter);
                parameter.module_id == root_module && parameter.semantic_id == root_declaration
            });
            for parameter in declared.collect::<Vec<_>>() {
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

    fn native_host_need(&self, interface: TypeId) -> bool {
        matches!(
            &self.types[interface.raw() as usize],
            CanonicalType::Named { symbol, arguments }
                if arguments.is_empty()
                    && matches!(
                        self.resolved.standard_symbol(*symbol),
                        Some(StandardSymbol::Type(
                            StandardType::Console | StandardType::FileSystem
                        ))
                    )
        )
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

    pub(crate) fn format_type(&self, id: TypeId) -> String {
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
            CanonicalType::FailureSetValue(failures) => {
                self.format_failure_row(&self.failure_rows[failures.raw() as usize])
            }
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

    pub(crate) fn callable_contract_for_type(&self, id: TypeId) -> Option<&CallableContract> {
        match &self.types[self.resolve_type_id(id).raw() as usize] {
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => Some(contract),
            _ => None,
        }
    }

    pub(crate) fn value_properties(&self, id: TypeId) -> ValueProperties {
        self.value_properties_with(id, &HashMap::new(), &mut HashSet::new())
    }

    fn value_properties_with(
        &self,
        id: TypeId,
        generics: &HashMap<SymbolId, TypeId>,
        visiting: &mut HashSet<TypeId>,
    ) -> ValueProperties {
        let id = self.resolve_type_id(id);
        if !visiting.insert(id) {
            return ValueProperties::SHAREABLE;
        }
        let properties = match &self.types[id.raw() as usize] {
            CanonicalType::Named { symbol, arguments } => {
                self.named_value_properties(*symbol, arguments, generics, visiting)
            }
            CanonicalType::Generic(symbol) => generics
                .get(symbol)
                .map_or(ValueProperties::VALUE_DEPENDENT, |bound| {
                    self.value_properties_with(*bound, generics, visiting)
                }),
            CanonicalType::FailureSetValue(failures) => {
                let row = &self.failure_rows[failures.raw() as usize];
                let parameter_properties = if row.parameters.is_empty() {
                    ValueProperties::SHAREABLE
                } else {
                    ValueProperties::VALUE_DEPENDENT
                };
                row.variants
                    .iter()
                    .fold(parameter_properties, |result, variant| {
                        let fields = self.fields.get(variant).into_iter().flatten();
                        fields.fold(result, |result, field| {
                            result.combine(self.value_properties_with(
                                field.type_id,
                                generics,
                                visiting,
                            ))
                        })
                    })
            }
            CanonicalType::Function(_) | CanonicalType::PolymorphicFunction { .. } => {
                ValueProperties::VALUE_DEPENDENT
            }
            CanonicalType::Record(fields) => {
                fields
                    .iter()
                    .fold(ValueProperties::SHAREABLE, |result, field| {
                        result.combine(self.value_properties_with(
                            field.type_id,
                            generics,
                            visiting,
                        ))
                    })
            }
            CanonicalType::InferenceVariable(_) => ValueProperties::VALUE_DEPENDENT,
            CanonicalType::Poison => ValueProperties::INVALID,
        };
        visiting.remove(&id);
        properties
    }

    fn named_value_properties(
        &self,
        symbol: SymbolId,
        arguments: &[TypeId],
        outer_generics: &HashMap<SymbolId, TypeId>,
        visiting: &mut HashSet<TypeId>,
    ) -> ValueProperties {
        match self.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Type(StandardType::Task)) => {
                return ValueProperties::PARENT_LOCAL;
            }
            Some(StandardSymbol::Type(
                StandardType::ChannelReceiver | StandardType::ChannelCompletion,
            )) => {
                return ValueProperties::EXCLUSIVE;
            }
            Some(StandardSymbol::Type(StandardType::ChannelSender)) => {
                return ValueProperties::SHAREABLE;
            }
            _ => {}
        }

        let symbol_kind = self.resolved.symbol(symbol).kind;
        if symbol_kind == ProjectSymbolKind::Interface
            || matches!(
                self.resolved.standard_symbol(symbol),
                Some(StandardSymbol::Type(
                    StandardType::Console | StandardType::FileSystem
                ))
            )
        {
            return ValueProperties::VALUE_DEPENDENT;
        }

        let mut generics = outer_generics.clone();
        for (parameter, argument) in self
            .type_generic_parameters(symbol)
            .into_iter()
            .zip(arguments)
        {
            generics.insert(parameter, *argument);
        }

        let mut fields = self
            .fields
            .get(&symbol)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if symbol_kind == ProjectSymbolKind::Enum {
            for variant in self.enum_variants(symbol) {
                fields.extend(self.fields.get(&variant).into_iter().flatten());
            }
        }
        if !fields.is_empty() {
            return fields
                .into_iter()
                .fold(ValueProperties::SHAREABLE, |result, field| {
                    result.combine(self.value_properties_with(field.type_id, &generics, visiting))
                });
        }

        if matches!(
            self.resolved.symbol(symbol).name.as_str(),
            "List" | "Map" | "Option"
        ) {
            return arguments
                .iter()
                .fold(ValueProperties::SHAREABLE, |result, argument| {
                    result.combine(self.value_properties_with(*argument, &generics, visiting))
                });
        }
        ValueProperties::SHAREABLE
    }

    fn type_generic_parameters(&self, owner: SymbolId) -> Vec<SymbolId> {
        let owner = self.resolved.symbol(owner);
        let (Some(module_id), Some(semantic_id)) = (owner.module_id, owner.semantic_id) else {
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
                self.resolved.symbols.iter().find(|symbol| {
                    symbol.module_id == Some(module_id)
                        && symbol.semantic_id == Some(semantic_id)
                        && symbol.kind == ProjectSymbolKind::GenericParameter
                        && symbol.name == parameter.text
                })
            })
            .map(|symbol| symbol.id)
            .collect()
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
            .map(|symbol| self.format_failure_parameter(row, *symbol))
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
            let owner_name = row
                .variant_arguments
                .iter()
                .find(|arguments| arguments.variant == variants[0])
                .filter(|arguments| {
                    row.variant_arguments.iter().all(|candidate| {
                        !variants.contains(&candidate.variant)
                            || candidate.arguments == arguments.arguments
                    })
                })
                .map(|arguments| {
                    format!(
                        "{}<{}>",
                        self.resolved.symbol(owner).name,
                        arguments
                            .arguments
                            .iter()
                            .map(|argument| self.format_type(*argument))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .unwrap_or_else(|| self.resolved.symbol(owner).name.clone());
            if variants == all_variants {
                members.push(owner_name);
            } else {
                members.extend(variants.into_iter().map(|variant| {
                    format!("{}.{}", owner_name, self.resolved.symbol(variant).name)
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

    pub(crate) fn format_failure_parameter(&self, row: &FailureRow, symbol: SymbolId) -> String {
        let parameter = self.resolved.symbol(symbol).name.clone();
        let Some(exclusion) = row
            .exclusions
            .iter()
            .find(|exclusion| exclusion.parameter == symbol)
        else {
            return parameter;
        };
        let variants = exclusion
            .variants
            .iter()
            .map(|variant| {
                self.enum_for_variant(*variant).map_or_else(
                    || self.resolved.symbol(*variant).name.clone(),
                    |owner| {
                        format!(
                            "{}.{}",
                            self.resolved.symbol(owner).name,
                            self.resolved.symbol(*variant).name
                        )
                    },
                )
            })
            .collect::<Vec<_>>();
        if variants.len() == 1 {
            format!("{parameter} - {}", variants[0])
        } else {
            format!("{parameter} - {{ {} }}", variants.join(", "))
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
        if let Some(StandardSymbol::Constructor(constructor)) =
            self.resolved.standard_symbol(variant)
        {
            return Some(self.resolved.standard_type(constructor.type_kind()));
        }
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
        if let Some(StandardSymbol::Type(kind)) = self.resolved.standard_symbol(owner) {
            let mut variants = self
                .resolved
                .symbols
                .iter()
                .filter_map(|symbol| match self.resolved.standard_symbol(symbol.id) {
                    Some(StandardSymbol::Constructor(constructor))
                        if constructor.type_kind() == kind =>
                    {
                        Some(symbol.id)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            variants.sort_unstable();
            return variants;
        }
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
    pattern_binding_types: HashMap<(ModuleId, SemanticId), TypeId>,
    callee_instantiations: HashMap<(ModuleId, SemanticId), TypeId>,
    type_facts: HashMap<(ModuleId, SemanticId), TypeId>,
    expression_facts: HashMap<(ModuleId, SemanticId), ExpressionFact>,
    fields: HashMap<SymbolId, Vec<FieldShape>>,
    generic_kinds: HashMap<SymbolId, GenericKind>,
    diagnostics: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    /// Values of `Int` expressions folded from literals, for `T4312`.
    integer_constants: HashMap<(ModuleId, SemanticId), i64>,
    next_inference_variable: u32,
    inference_bindings: HashMap<u32, TypeId>,
    /// The type every `break` of a loop must supply; it is also the loop's success type.
    /// A loop no break targets keeps an unconstrained variable, so it fits any context.
    loop_break_types: HashMap<(ModuleId, SemanticId), TypeId>,
}

#[derive(Clone, Debug)]
struct ValueBinding {
    type_id: TypeId,
    need: Option<Need>,
}

#[derive(Clone, Default)]
struct CallSubstitution {
    values: HashMap<SymbolId, TypeId>,
    failures: HashMap<SymbolId, FailureRowId>,
    needs: HashMap<SymbolId, NeedsRowId>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum MatchTag {
    EnumVariant(SymbolId),
    Boolean(bool),
    Integer(i64),
    Text(String),
    /// A literal pattern typing rejected (`T4313`); it matches like an opaque
    /// literal so the rejection causes no reachability follow-ons.
    Rejected(SemanticId),
}

#[derive(Clone, Debug)]
enum MatchPattern {
    Wildcard,
    Constructor {
        tag: MatchTag,
        fields: Vec<MatchPattern>,
    },
    /// A list pattern. `suffix` is `None` for an exact-length pattern and holds the
    /// patterns after `..` otherwise.
    List {
        prefix: Vec<MatchPattern>,
        suffix: Option<Vec<MatchPattern>>,
    },
}

/// The length classes coverage splits one list column into: each exact length below
/// `open`, plus "at least `open`". `open` exceeds every exact-length row and is at least
/// `prefix + suffix`, so every row with `..` matches the whole open class, which is
/// represented by its first `prefix` and last `suffix` elements.
#[derive(Clone, Copy, Debug)]
struct ListColumn {
    open: usize,
    prefix: usize,
    suffix: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ListClass {
    Exact(usize),
    AtLeastOpen,
}

impl ListColumn {
    /// Returns `None` when neither `candidate` nor any row of `matrix` starts with a
    /// list pattern, so ordinary specialization applies.
    fn of(matrix: &[Vec<MatchPattern>], candidate: &MatchPattern) -> Option<Self> {
        let mut column = None::<Self>;
        for head in matrix
            .iter()
            .filter_map(|row| row.first())
            .chain([candidate])
        {
            let MatchPattern::List { prefix, suffix } = head else {
                continue;
            };
            let current = column.get_or_insert(Self {
                open: 0,
                prefix: 0,
                suffix: 0,
            });
            match suffix {
                None => current.open = current.open.max(prefix.len() + 1),
                Some(suffix) => {
                    current.prefix = current.prefix.max(prefix.len());
                    current.suffix = current.suffix.max(suffix.len());
                }
            }
        }
        column.map(|mut column| {
            column.open = column.open.max(column.prefix + column.suffix);
            column
        })
    }

    fn classes(self) -> impl Iterator<Item = ListClass> {
        (0..self.open)
            .map(ListClass::Exact)
            .chain([ListClass::AtLeastOpen])
    }

    fn arity(self, class: ListClass) -> usize {
        match class {
            ListClass::Exact(length) => length,
            ListClass::AtLeastOpen => self.prefix + self.suffix,
        }
    }

    /// The fields `head` contributes to `class`, or `None` when `head` matches no list
    /// of that class.
    fn specialize(self, head: &MatchPattern, class: ListClass) -> Option<Vec<MatchPattern>> {
        let arity = self.arity(class);
        match head {
            MatchPattern::Wildcard => Some(vec![MatchPattern::Wildcard; arity]),
            MatchPattern::List {
                prefix,
                suffix: None,
            } => (class == ListClass::Exact(prefix.len())).then(|| prefix.clone()),
            MatchPattern::List {
                prefix,
                suffix: Some(suffix),
            } => {
                let fixed = prefix.len() + suffix.len();
                let gap = match class {
                    ListClass::Exact(length) if length < fixed => return None,
                    ListClass::Exact(length) => length - fixed,
                    ListClass::AtLeastOpen => arity - fixed,
                };
                let mut fields = prefix.clone();
                fields.extend(std::iter::repeat_n(MatchPattern::Wildcard, gap));
                fields.extend_from_slice(suffix);
                Some(fields)
            }
            MatchPattern::Constructor { .. } => None,
        }
    }

    /// Rebuilds a witness list from the `arity(class)` entries of `fields`. An open
    /// witness spells out `open` elements, such as `[_, _, ..]` for "at least two".
    fn witness(self, class: ListClass, mut fields: Vec<MatchPattern>) -> MatchPattern {
        match class {
            ListClass::Exact(_) => MatchPattern::List {
                prefix: fields,
                suffix: None,
            },
            ListClass::AtLeastOpen => {
                let suffix = fields.split_off(self.prefix);
                let padding = self.open - self.prefix - self.suffix;
                fields.extend(std::iter::repeat_n(MatchPattern::Wildcard, padding));
                MatchPattern::List {
                    prefix: fields,
                    suffix: Some(suffix),
                }
            }
        }
    }
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
            pattern_binding_types: HashMap::new(),
            callee_instantiations: HashMap::new(),
            type_facts: HashMap::new(),
            expression_facts: HashMap::new(),
            fields: HashMap::new(),
            generic_kinds: HashMap::new(),
            diagnostics,
            warnings: Vec::new(),
            integer_constants: HashMap::new(),
            next_inference_variable: 0,
            inference_bindings: HashMap::new(),
            loop_break_types: HashMap::new(),
        };
        checker.intern_type(CanonicalType::Poison);
        checker.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: Vec::new(),
            exclusions: Vec::new(),
        });
        checker.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: Vec::new(),
        });
        checker.index_standard_contracts();
        checker
    }

    fn index_standard_contracts(&mut self) {
        for generic in StandardGeneric::ALL {
            let kind = match generic.kind() {
                StandardGenericKind::Value => GenericKind::Value,
                StandardGenericKind::FailureRow => GenericKind::FailureRow,
                StandardGenericKind::FailureSet => GenericKind::FailureSet,
                StandardGenericKind::NeedsRow => GenericKind::NeedsRow,
            };
            self.generic_kinds
                .insert(self.resolved.standard_generic(generic), kind);
        }

        let text = self.builtin_type("Text");
        let int = self.builtin_type("Int");
        let bool_type = self.builtin_type("Bool");
        let void = self.builtin_type("Void");
        let task_value = self.standard_generic_type(StandardGeneric::TaskValue);
        let task_failures = self.standard_generic_type(StandardGeneric::TaskFailures);
        let task_needs = self.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: vec![self.resolved.standard_generic(StandardGeneric::TaskNeeds)],
        });
        let task_failure_row = self.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: vec![
                self.resolved
                    .standard_generic(StandardGeneric::TaskFailures),
            ],
            exclusions: Vec::new(),
        });
        let task = self.standard_named_type(StandardType::Task, vec![task_value, task_failures]);
        let task_thunk = self.intern_type(CanonicalType::Function(CallableContract {
            inputs: Vec::new(),
            success: task_value,
            failures: task_failure_row,
            needs: task_needs,
        }));
        let channel_value = self.standard_generic_type(StandardGeneric::ChannelValue);
        let channel_sender =
            self.standard_named_type(StandardType::ChannelSender, vec![channel_value]);
        let channel_receiver =
            self.standard_named_type(StandardType::ChannelReceiver, vec![channel_value]);
        let channel_completion =
            self.standard_named_type(StandardType::ChannelCompletion, vec![channel_value]);
        let channel_bundle = self.intern_type(CanonicalType::Record(vec![
            StructuralField {
                name: "completion".to_owned(),
                type_id: channel_completion,
            },
            StructuralField {
                name: "receiver".to_owned(),
                type_id: channel_receiver,
            },
            StructuralField {
                name: "sender".to_owned(),
                type_id: channel_sender,
            },
        ]));
        let channel_receive =
            self.standard_named_type(StandardType::ChannelReceive, vec![channel_value]);
        let channel_send_variants = [
            StandardConstructor::ChannelSendClosed,
            StandardConstructor::ChannelSendAborted,
        ]
        .map(|constructor| self.standard_constructor_symbol(constructor));
        let channel_send_failures = self.intern_failure_row(FailureRow {
            variants: channel_send_variants.to_vec(),
            variant_arguments: channel_send_variants
                .into_iter()
                .map(|variant| FailureVariantArguments {
                    variant,
                    arguments: vec![channel_value],
                })
                .collect(),
            parameters: Vec::new(),
            exclusions: Vec::new(),
        });
        let channel_receive_failures =
            self.standard_failure_row(&[StandardConstructor::ChannelAborted]);
        let path = self.standard_named_type(StandardType::Path, Vec::new());
        let value = self.standard_named_type(StandardType::JsonValue, Vec::new());
        let number = self.standard_named_type(StandardType::JsonNumber, Vec::new());
        let decode_failure = self.standard_failure_row(&[StandardConstructor::DecodeInvalidJson]);
        let number_failure = self.standard_failure_row(&[
            StandardConstructor::NumberNotInteger,
            StandardConstructor::NumberOutOfRange,
        ]);
        let console_failure = self.standard_failure_row(&[StandardConstructor::ConsoleWriteFailed]);
        let map_contracts = [
            (
                StandardFunction::MapEmpty,
                CallableContract {
                    inputs: Vec::new(),
                    success: self.standard_map_type(
                        StandardGeneric::MapEmptyKey,
                        StandardGeneric::MapEmptyValue,
                    ),
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                },
            ),
            (StandardFunction::MapGet, self.standard_map_get_contract()),
            (StandardFunction::MapPut, self.standard_map_put_contract()),
            (
                StandardFunction::MapUpdate,
                self.standard_map_transform_contract(false),
            ),
            (
                StandardFunction::MapMapValues,
                self.standard_map_transform_contract(true),
            ),
        ];
        for (function, contract) in map_contracts {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(symbol, contract);
        }
        for (function, contract) in [
            (
                StandardFunction::TaskStart,
                CallableContract {
                    inputs: vec![task_thunk],
                    success: task,
                    failures: FailureRowId::new(0),
                    needs: task_needs,
                },
            ),
            (
                StandardFunction::TaskResult,
                CallableContract {
                    inputs: vec![task],
                    success: task_value,
                    failures: task_failure_row,
                    needs: NeedsRowId::new(0),
                },
            ),
            (
                StandardFunction::TaskCancel,
                CallableContract {
                    inputs: vec![task],
                    success: void,
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                },
            ),
            (
                StandardFunction::ChannelBounded,
                CallableContract {
                    inputs: vec![int],
                    success: channel_bundle,
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                },
            ),
            (
                StandardFunction::ChannelSend,
                CallableContract {
                    inputs: vec![channel_sender, channel_value],
                    success: void,
                    failures: channel_send_failures,
                    needs: NeedsRowId::new(0),
                },
            ),
            (
                StandardFunction::ChannelReceive,
                CallableContract {
                    inputs: vec![channel_receiver],
                    success: channel_receive,
                    failures: channel_receive_failures,
                    needs: NeedsRowId::new(0),
                },
            ),
            (
                StandardFunction::ChannelClose,
                CallableContract {
                    inputs: vec![channel_completion],
                    success: void,
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                },
            ),
            (
                StandardFunction::ChannelAbort,
                CallableContract {
                    inputs: vec![channel_receiver],
                    success: void,
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                },
            ),
        ] {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(symbol, contract);
        }

        let pure = (FailureRowId::new(0), NeedsRowId::new(0));
        let json_contracts = [
            (
                StandardFunction::JsonDecode,
                vec![text],
                value,
                decode_failure,
            ),
            (StandardFunction::JsonEncode, vec![value], text, pure.0),
            (
                StandardFunction::JsonBoolean,
                vec![bool_type],
                value,
                pure.0,
            ),
            (StandardFunction::JsonInt, vec![int], value, pure.0),
            (StandardFunction::JsonText, vec![text], value, pure.0),
            (
                StandardFunction::JsonArray,
                vec![self.builtin_type_with_arguments("List", vec![value])],
                value,
                pure.0,
            ),
            (
                StandardFunction::JsonObject,
                vec![self.standard_named_type(StandardType::Map, vec![text, value])],
                value,
                pure.0,
            ),
            (
                StandardFunction::JsonNumberToInt,
                vec![number],
                int,
                number_failure,
            ),
        ];
        for (function, inputs, success, failures) in json_contracts {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(
                symbol,
                CallableContract {
                    inputs,
                    success,
                    failures,
                    needs: pure.1,
                },
            );
        }
        let float = self.builtin_type("Float");
        let int_option = self.standard_named_type(StandardType::Option, vec![int]);
        let int_parse_failure = self.standard_failure_row(&[
            StandardConstructor::IntParseEmpty,
            StandardConstructor::IntParseInvalidDigit,
            StandardConstructor::IntParseOutOfRange,
        ]);
        let float_parse_failure = self.standard_failure_row(&[
            StandardConstructor::FloatParseEmpty,
            StandardConstructor::FloatParseInvalid,
            StandardConstructor::FloatParseOutOfRange,
        ]);
        let float_to_int_failure = self.standard_failure_row(&[
            StandardConstructor::FloatNotInteger,
            StandardConstructor::FloatOutOfRange,
        ]);
        let numeric_contracts = [
            (
                StandardFunction::IntParse,
                vec![text],
                int,
                int_parse_failure,
            ),
            (StandardFunction::IntToText, vec![int], text, pure.0),
            (
                StandardFunction::IntAddChecked,
                vec![int, int],
                int_option,
                pure.0,
            ),
            (
                StandardFunction::IntSubtractChecked,
                vec![int, int],
                int_option,
                pure.0,
            ),
            (
                StandardFunction::IntMultiplyChecked,
                vec![int, int],
                int_option,
                pure.0,
            ),
            (
                StandardFunction::IntDivideChecked,
                vec![int, int],
                int_option,
                pure.0,
            ),
            (
                StandardFunction::IntRemainderChecked,
                vec![int, int],
                int_option,
                pure.0,
            ),
            (StandardFunction::IntModulo, vec![int, int], int, pure.0),
            (StandardFunction::FloatFromInt, vec![int], float, pure.0),
            (StandardFunction::FloatTruncate, vec![float], float, pure.0),
            (StandardFunction::FloatFloor, vec![float], float, pure.0),
            (StandardFunction::FloatCeiling, vec![float], float, pure.0),
            (StandardFunction::FloatRound, vec![float], float, pure.0),
            (
                StandardFunction::FloatToInt,
                vec![float],
                int,
                float_to_int_failure,
            ),
            (StandardFunction::FloatToText, vec![float], text, pure.0),
            (
                StandardFunction::FloatParse,
                vec![text],
                float,
                float_parse_failure,
            ),
        ];
        for (function, inputs, success, failures) in numeric_contracts {
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(
                symbol,
                CallableContract {
                    inputs,
                    success,
                    failures,
                    needs: pure.1,
                },
            );
        }
        let text_append = self.standard_function_symbol(StandardFunction::TextAppend);
        self.contracts.insert(
            text_append,
            CallableContract {
                inputs: vec![text, text],
                success: text,
                failures: pure.0,
                needs: pure.1,
            },
        );
        let text_equal = self.standard_function_symbol(StandardFunction::TextEqual);
        self.contracts.insert(
            text_equal,
            CallableContract {
                inputs: vec![text, text],
                success: bool_type,
                failures: pure.0,
                needs: pure.1,
            },
        );
        let io_path = self.standard_function_symbol(StandardFunction::IoPath);
        self.contracts.insert(
            io_path,
            CallableContract {
                inputs: vec![text],
                success: path,
                failures: pure.0,
                needs: pure.1,
            },
        );
        let list_value = self.standard_generic_type(StandardGeneric::ListFirstValue);
        let list = self.builtin_type_with_arguments("List", vec![list_value]);
        let option = self.standard_named_type(StandardType::Option, vec![list_value]);
        let list_first = self.standard_function_symbol(StandardFunction::ListFirst);
        self.contracts.insert(
            list_first,
            CallableContract {
                inputs: vec![list],
                success: option,
                failures: pure.0,
                needs: pure.1,
            },
        );
        let list_length = self.standard_function_symbol(StandardFunction::ListLength);
        let any_list_value = self.standard_generic_type(StandardGeneric::ListLengthValue);
        let any_list = self.builtin_type_with_arguments("List", vec![any_list_value]);
        self.contracts.insert(
            list_length,
            CallableContract {
                inputs: vec![any_list],
                success: int,
                failures: pure.0,
                needs: pure.1,
            },
        );

        let console = self.resolved.standard_type(StandardType::Console);
        let console_method = self.intern_type(CanonicalType::Function(CallableContract {
            inputs: vec![text],
            success: void,
            failures: console_failure,
            needs: pure.1,
        }));
        self.fields.insert(
            console,
            vec![
                FieldShape {
                    semantic_id: SemanticId::new(0),
                    name: "write_line".to_owned(),
                    type_id: console_method,
                },
                FieldShape {
                    semantic_id: SemanticId::new(1),
                    name: "write_error".to_owned(),
                    type_id: console_method,
                },
            ],
        );

        let file_state = self.standard_generic_type(StandardGeneric::FileState);
        let callback_failures = self.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: vec![
                self.resolved
                    .standard_generic(StandardGeneric::FileFailures),
            ],
            exclusions: Vec::new(),
        });
        let callback_needs = self.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: vec![self.resolved.standard_generic(StandardGeneric::FileNeeds)],
        });
        let callback = self.intern_type(CanonicalType::Function(CallableContract {
            inputs: vec![file_state, text],
            success: file_state,
            failures: callback_failures,
            needs: callback_needs,
        }));
        let file_failures = self.intern_failure_row(FailureRow {
            variants: vec![
                self.standard_constructor_symbol(StandardConstructor::FileClose),
                self.standard_constructor_symbol(StandardConstructor::FileLineTooLong),
                self.standard_constructor_symbol(StandardConstructor::FileOpen),
                self.standard_constructor_symbol(StandardConstructor::FileRead),
                self.standard_constructor_symbol(StandardConstructor::Utf8Invalid),
            ],
            variant_arguments: Vec::new(),
            parameters: vec![
                self.resolved
                    .standard_generic(StandardGeneric::FileFailures),
            ],
            exclusions: Vec::new(),
        });
        let file_contract = CallableContract {
            inputs: vec![path, file_state, callback],
            success: file_state,
            failures: file_failures,
            needs: callback_needs,
        };
        let file_method = self.intern_type(CanonicalType::PolymorphicFunction {
            parameters: vec![
                self.resolved.standard_generic(StandardGeneric::FileState),
                self.resolved
                    .standard_generic(StandardGeneric::FileFailures),
                self.resolved.standard_generic(StandardGeneric::FileNeeds),
            ],
            contract: file_contract,
        });
        self.fields.insert(
            self.resolved.standard_type(StandardType::FileSystem),
            vec![FieldShape {
                semantic_id: SemanticId::new(0),
                name: "process_utf8_lines".to_owned(),
                type_id: file_method,
            }],
        );

        for constructor in StandardConstructor::ALL {
            let symbol = self.standard_constructor_symbol(constructor);
            let (inputs, success) = self.standard_constructor_shape(constructor);
            self.fields.insert(
                symbol,
                constructor
                    .field_names()
                    .iter()
                    .zip(&inputs)
                    .enumerate()
                    .map(|(index, (name, type_id))| FieldShape {
                        semantic_id: SemanticId::new(index as u32),
                        name: (*name).to_owned(),
                        type_id: *type_id,
                    })
                    .collect(),
            );
            self.contracts.insert(
                symbol,
                CallableContract {
                    inputs,
                    success,
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                },
            );
        }
    }

    fn standard_generic_type(&mut self, generic: StandardGeneric) -> TypeId {
        self.intern_type(CanonicalType::Generic(
            self.resolved.standard_generic(generic),
        ))
    }

    fn standard_named_type(&mut self, kind: StandardType, arguments: Vec<TypeId>) -> TypeId {
        self.intern_type(CanonicalType::Named {
            symbol: self.resolved.standard_type(kind),
            arguments,
        })
    }

    fn standard_map_type(&mut self, key: StandardGeneric, value: StandardGeneric) -> TypeId {
        let key = self.standard_generic_type(key);
        let value = self.standard_generic_type(value);
        self.standard_named_type(StandardType::Map, vec![key, value])
    }

    fn standard_map_get_contract(&mut self) -> CallableContract {
        let key = self.standard_generic_type(StandardGeneric::MapGetKey);
        let value = self.standard_generic_type(StandardGeneric::MapGetValue);
        let map = self.standard_named_type(StandardType::Map, vec![key, value]);
        let option = self.standard_named_type(StandardType::Option, vec![value]);
        CallableContract {
            inputs: vec![map, key],
            success: option,
            failures: FailureRowId::new(0),
            needs: NeedsRowId::new(0),
        }
    }

    fn standard_map_put_contract(&mut self) -> CallableContract {
        let key = self.standard_generic_type(StandardGeneric::MapPutKey);
        let value = self.standard_generic_type(StandardGeneric::MapPutValue);
        let map = self.standard_named_type(StandardType::Map, vec![key, value]);
        CallableContract {
            inputs: vec![map, key, value],
            success: map,
            failures: FailureRowId::new(0),
            needs: NeedsRowId::new(0),
        }
    }

    fn standard_map_transform_contract(&mut self, changes_value_type: bool) -> CallableContract {
        let (key_generic, input_generic, output_generic, failures, needs) = if changes_value_type {
            (
                StandardGeneric::MapValuesKey,
                StandardGeneric::MapValuesInput,
                StandardGeneric::MapValuesOutput,
                StandardGeneric::MapValuesFailures,
                StandardGeneric::MapValuesNeeds,
            )
        } else {
            (
                StandardGeneric::MapUpdateKey,
                StandardGeneric::MapUpdateValue,
                StandardGeneric::MapUpdateValue,
                StandardGeneric::MapUpdateFailures,
                StandardGeneric::MapUpdateNeeds,
            )
        };
        let key = self.standard_generic_type(key_generic);
        let input = self.standard_generic_type(input_generic);
        let output = self.standard_generic_type(output_generic);
        let failures = self.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: vec![self.resolved.standard_generic(failures)],
            exclusions: Vec::new(),
        });
        let needs = self.intern_needs_row(NeedsRow {
            entries: Vec::new(),
            parameters: vec![self.resolved.standard_generic(needs)],
        });
        let callback_input = if changes_value_type {
            input
        } else {
            self.standard_named_type(StandardType::Option, vec![input])
        };
        let callback = self.intern_type(CanonicalType::Function(CallableContract {
            inputs: vec![callback_input],
            success: output,
            failures,
            needs,
        }));
        let input_map = self.standard_named_type(StandardType::Map, vec![key, input]);
        let output_map = self.standard_named_type(StandardType::Map, vec![key, output]);
        let inputs = if changes_value_type {
            vec![input_map, callback]
        } else {
            vec![input_map, key, callback]
        };
        CallableContract {
            inputs,
            success: output_map,
            failures,
            needs,
        }
    }

    fn standard_failure_row(&mut self, constructors: &[StandardConstructor]) -> FailureRowId {
        let mut variants = constructors
            .iter()
            .map(|constructor| self.standard_constructor_symbol(*constructor))
            .collect::<Vec<_>>();
        variants.sort_unstable();
        self.intern_failure_row(FailureRow {
            variants,
            variant_arguments: Vec::new(),
            parameters: Vec::new(),
            exclusions: Vec::new(),
        })
    }

    fn standard_constructor_shape(
        &mut self,
        constructor: StandardConstructor,
    ) -> (Vec<TypeId>, TypeId) {
        let text = self.builtin_type("Text");
        let int = self.builtin_type("Int");
        let bool_type = self.builtin_type("Bool");
        let value = self.standard_named_type(StandardType::JsonValue, Vec::new());
        let number = self.standard_named_type(StandardType::JsonNumber, Vec::new());
        match constructor {
            StandardConstructor::ConsoleWriteFailed => (
                vec![text, text],
                self.standard_named_type(StandardType::ConsoleError, Vec::new()),
            ),
            StandardConstructor::FileClose
            | StandardConstructor::FileOpen
            | StandardConstructor::FileRead => (
                vec![text, text],
                self.standard_named_type(StandardType::FileError, Vec::new()),
            ),
            StandardConstructor::FileLineTooLong => (
                vec![text, int],
                self.standard_named_type(StandardType::FileError, Vec::new()),
            ),
            StandardConstructor::Utf8Invalid => (
                vec![text, int],
                self.standard_named_type(StandardType::Utf8Error, Vec::new()),
            ),
            StandardConstructor::OptionSome => {
                let value = self.standard_generic_type(StandardGeneric::OptionValue);
                (
                    vec![value],
                    self.standard_named_type(StandardType::Option, vec![value]),
                )
            }
            StandardConstructor::OptionNone => {
                let value = self.standard_generic_type(StandardGeneric::OptionValue);
                (
                    Vec::new(),
                    self.standard_named_type(StandardType::Option, vec![value]),
                )
            }
            StandardConstructor::JsonNull => (Vec::new(), value),
            StandardConstructor::JsonBoolean => (vec![bool_type], value),
            StandardConstructor::JsonNumber => (vec![number], value),
            StandardConstructor::JsonText => (vec![text], value),
            StandardConstructor::JsonArray => (
                vec![self.builtin_type_with_arguments("List", vec![value])],
                value,
            ),
            StandardConstructor::JsonObject => (
                vec![self.standard_named_type(StandardType::Map, vec![text, value])],
                value,
            ),
            StandardConstructor::DecodeInvalidJson => (
                vec![text, int],
                self.standard_named_type(StandardType::DecodeError, Vec::new()),
            ),
            StandardConstructor::NumberNotInteger | StandardConstructor::NumberOutOfRange => (
                vec![number],
                self.standard_named_type(StandardType::NumberError, Vec::new()),
            ),
            StandardConstructor::ChannelSendClosed | StandardConstructor::ChannelSendAborted => {
                let value = self.standard_generic_type(StandardGeneric::ChannelValue);
                (
                    vec![value],
                    self.standard_named_type(StandardType::ChannelSendError, vec![value]),
                )
            }
            StandardConstructor::ChannelReceived => {
                let value = self.standard_generic_type(StandardGeneric::ChannelValue);
                (
                    vec![value],
                    self.standard_named_type(StandardType::ChannelReceive, vec![value]),
                )
            }
            StandardConstructor::ChannelDrained => {
                let value = self.standard_generic_type(StandardGeneric::ChannelValue);
                (
                    Vec::new(),
                    self.standard_named_type(StandardType::ChannelReceive, vec![value]),
                )
            }
            StandardConstructor::ChannelAborted => (
                Vec::new(),
                self.standard_named_type(StandardType::ChannelError, Vec::new()),
            ),
            StandardConstructor::IntParseEmpty
            | StandardConstructor::IntParseInvalidDigit
            | StandardConstructor::IntParseOutOfRange
            | StandardConstructor::FloatParseEmpty
            | StandardConstructor::FloatParseInvalid
            | StandardConstructor::FloatParseOutOfRange
            | StandardConstructor::FloatNotInteger
            | StandardConstructor::FloatOutOfRange => (
                Vec::new(),
                self.standard_named_type(constructor.type_kind(), Vec::new()),
            ),
        }
    }

    fn standard_function_symbol(&self, function: StandardFunction) -> SymbolId {
        self.resolved
            .symbols
            .iter()
            .find_map(|symbol| {
                (self.resolved.standard_symbol(symbol.id)
                    == Some(StandardSymbol::Function(function)))
                .then_some(symbol.id)
            })
            .expect("standard function is resolved")
    }

    fn standard_constructor_symbol(&self, constructor: StandardConstructor) -> SymbolId {
        self.resolved
            .symbols
            .iter()
            .find_map(|symbol| {
                (self.resolved.standard_symbol(symbol.id)
                    == Some(StandardSymbol::Constructor(constructor)))
                .then_some(symbol.id)
            })
            .expect("standard constructor is resolved")
    }

    fn from_previous(
        resolved: ResolvedProject,
        previous: &TypedProject,
        affected_modules: &HashSet<ModuleId>,
    ) -> Self {
        let resolved = Arc::new(resolved);
        let affected_files = affected_modules
            .iter()
            .map(|module_id| {
                resolved.semantic.modules[module_id.raw() as usize]
                    .source
                    .file_id()
            })
            .collect::<HashSet<_>>();
        let affected_symbols = previous
            .resolved
            .symbols
            .iter()
            .filter(|symbol| {
                symbol
                    .module_id
                    .is_some_and(|id| affected_modules.contains(&id))
            })
            .map(|symbol| symbol.id)
            .collect::<HashSet<_>>();
        let types = previous.types.clone();
        let failure_rows = previous.failure_rows.clone();
        let needs_rows = previous.needs_rows.clone();
        let next_inference_variable = types
            .iter()
            .filter_map(|value| match value {
                CanonicalType::InferenceVariable(variable) => Some(*variable),
                _ => None,
            })
            .chain(previous.inference_bindings.keys().copied())
            .max()
            .map_or(0, |variable| variable + 1);
        Self {
            resolved: resolved.clone(),
            type_intern: types
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, value)| (value, TypeId::new(index as u32)))
                .collect(),
            types,
            failure_row_intern: failure_rows
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, value)| (value, FailureRowId::new(index as u32)))
                .collect(),
            failure_rows,
            needs_row_intern: needs_rows
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, value)| (value, NeedsRowId::new(index as u32)))
                .collect(),
            needs_rows,
            contracts: previous
                .contracts
                .iter()
                .filter(|(symbol, _)| !affected_symbols.contains(symbol))
                .map(|(symbol, contract)| (*symbol, contract.clone()))
                .collect(),
            declaration_types: previous
                .declaration_types
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, *value))
                .collect(),
            pattern_binding_types: previous
                .pattern_binding_types
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, *value))
                .collect(),
            callee_instantiations: previous
                .callee_instantiations
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, *value))
                .collect(),
            type_facts: previous
                .type_facts
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, *value))
                .collect(),
            expression_facts: previous
                .expression_facts
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, *value))
                .collect(),
            fields: previous
                .fields
                .iter()
                .filter(|(symbol, _)| !affected_symbols.contains(symbol))
                .map(|(symbol, fields)| (*symbol, fields.clone()))
                .collect(),
            generic_kinds: previous
                .generic_kinds
                .iter()
                .filter(|(symbol, _)| !affected_symbols.contains(symbol))
                .map(|(symbol, kind)| (*symbol, *kind))
                .collect(),
            diagnostics: resolved
                .diagnostics
                .iter()
                .cloned()
                .chain(
                    previous
                        .diagnostics
                        .iter()
                        .filter(|diagnostic| {
                            diagnostic.stage() == DiagnosticStage::Type
                                && !affected_files.contains(&diagnostic.file_id())
                        })
                        .cloned(),
                )
                .collect(),
            warnings: previous
                .warnings
                .iter()
                .filter(|warning| !affected_files.contains(&warning.file_id()))
                .cloned()
                .collect(),
            integer_constants: HashMap::new(),
            next_inference_variable,
            inference_bindings: previous.inference_bindings.clone(),
            loop_break_types: HashMap::new(),
        }
    }

    fn check(self) -> TypedProject {
        let order = self.resolved.dependency_order.clone();
        let modules = order.iter().copied().collect::<HashSet<_>>();
        self.check_modules(&modules, &modules)
    }

    fn check_modules(
        mut self,
        declaration_modules: &HashSet<ModuleId>,
        body_modules: &HashSet<ModuleId>,
    ) -> TypedProject {
        let order = self.resolved.dependency_order.clone();
        for module_id in order
            .iter()
            .filter(|module_id| declaration_modules.contains(module_id))
        {
            let declarations = self.resolved.semantic.modules[module_id.raw() as usize]
                .declarations
                .clone();
            for declaration in declarations {
                self.check_declaration(*module_id, &declaration);
            }
        }
        let declaration_count = body_modules
            .iter()
            .map(|module_id| {
                self.resolved.semantic.modules[module_id.raw() as usize]
                    .declarations
                    .len()
            })
            .sum::<usize>();
        // Each pass moves every inferred contract fact at least one call edge
        // further, and a fact's shortest path crosses at most every
        // declaration, so a monotone fixed point settles within
        // `declaration_count + 1` passes. Failing to settle means a body
        // contributes a fresh fact on every pass, a checker defect that must
        // surface as a diagnostic rather than a crash or a silent cut-off.
        for pass in 0..=declaration_count {
            let mut last_changed = None;
            for module_id in order
                .iter()
                .filter(|module_id| body_modules.contains(module_id))
            {
                let declarations = self.resolved.semantic.modules[module_id.raw() as usize]
                    .declarations
                    .clone();
                for declaration in declarations {
                    if self.check_declaration_body(*module_id, &declaration) {
                        last_changed = Some((*module_id, declaration.name.range));
                    }
                }
            }
            let Some((module_id, range)) = last_changed else {
                break;
            };
            if pass == declaration_count {
                self.type_error_range(
                    module_id,
                    "T4290",
                    range,
                    "internal checker limit: this declaration's inferred contract did not \
                     settle; please report this program",
                );
            }
        }
        let mut typed = TypedProject {
            resolved: self.resolved,
            types: self.types,
            failure_rows: self.failure_rows,
            needs_rows: self.needs_rows,
            contracts: self.contracts,
            declaration_types: self.declaration_types,
            pattern_binding_types: self.pattern_binding_types,
            callee_instantiations: self.callee_instantiations,
            type_facts: self.type_facts,
            expression_facts: self.expression_facts,
            expression_value_properties: HashMap::new(),
            fields: self.fields,
            generic_kinds: self.generic_kinds,
            diagnostics: self.diagnostics,
            warnings: self.warnings,
            inference_bindings: self.inference_bindings,
        };
        let capability_report = crate::capability::check(&typed);
        typed.expression_value_properties = capability_report.expression_properties;
        typed.diagnostics.extend(capability_report.diagnostics);
        typed.validate_closed_roots();
        typed.diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic_phase_order(diagnostic.stage()),
                diagnostic.file_id().raw(),
            )
        });
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
                inferred.failures = self.close_failure_row(inferred.failures);
                inferred.needs = self.close_needs_row(inferred.needs);
                self.contracts.insert(function_symbol, inferred);
                self.contracts[&function_symbol] != previous
            }
            ModuleDeclarationKind::Binding => {
                let mut bindings = HashMap::new();
                let fact = self.infer_expression(module_id, body, &generic_symbols, &mut bindings);
                if let Some(symbol) = self.declaration_symbol(module_id, declaration) {
                    let key = (module_id, declaration.semantic_id);
                    let changed = self
                        .declaration_types
                        .get(&key)
                        .is_none_or(|previous| !self.types_equivalent(*previous, fact.success));
                    self.declaration_types.insert(key, fact.success);
                    self.contracts.remove(&symbol);
                    changed
                } else {
                    false
                }
            }
            ModuleDeclarationKind::Struct
            | ModuleDeclarationKind::Enum
            | ModuleDeclarationKind::Interface => false,
        }
    }

    /// Settles the inferred rows a body contributes to its declaration's
    /// contract. Every body pass allocates fresh inference variables, so a
    /// variable this pass left unbound (a value whose type nothing pins) can
    /// never be bound by a later pass. Keeping it would add a distinct row
    /// entry per pass and stop the body-contract fixed point from converging;
    /// it becomes `Poison`, whose missing type fact is reported elsewhere.
    fn close_failure_row(&mut self, row: FailureRowId) -> FailureRowId {
        let mut row = self.failure_rows[row.raw() as usize].clone();
        for arguments in &mut row.variant_arguments {
            for argument in &mut arguments.arguments {
                *argument = self.close_inferred_type(*argument);
            }
        }
        self.intern_failure_row(row)
    }

    fn close_needs_row(&mut self, row: NeedsRowId) -> NeedsRowId {
        let row = self.needs_rows[row.raw() as usize].clone();
        let entries = row
            .entries
            .into_iter()
            .map(|need| Need {
                interface: self.close_inferred_type(need.interface),
                ..need
            })
            .collect();
        let closed = self.intern_needs_row(NeedsRow {
            entries,
            parameters: row.parameters,
        });
        self.union_needs_rows([closed])
    }

    fn close_inferred_type(&mut self, type_id: TypeId) -> TypeId {
        let type_id = self.resolve_inference_type(type_id);
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::InferenceVariable(_) => self.intern_type(CanonicalType::Poison),
            CanonicalType::Named { symbol, arguments } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.close_inferred_type(argument))
                    .collect();
                self.intern_type(CanonicalType::Named { symbol, arguments })
            }
            CanonicalType::Record(fields) => {
                let fields = fields
                    .into_iter()
                    .map(|field| StructuralField {
                        name: field.name,
                        type_id: self.close_inferred_type(field.type_id),
                    })
                    .collect();
                self.intern_type(CanonicalType::Record(fields))
            }
            CanonicalType::Function(contract) => {
                let contract = self.close_inferred_contract(contract);
                self.intern_type(CanonicalType::Function(contract))
            }
            CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            } => {
                let contract = self.close_inferred_contract(contract);
                self.intern_type(CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                })
            }
            CanonicalType::FailureSetValue(failures) => {
                let failures = self.close_failure_row(failures);
                self.intern_type(CanonicalType::FailureSetValue(failures))
            }
            CanonicalType::Generic(_) | CanonicalType::Poison => type_id,
        }
    }

    fn close_inferred_contract(&mut self, contract: CallableContract) -> CallableContract {
        CallableContract {
            inputs: contract
                .inputs
                .into_iter()
                .map(|input| self.close_inferred_type(input))
                .collect(),
            success: self.close_inferred_type(contract.success),
            failures: self.close_failure_row(contract.failures),
            needs: self.close_needs_row(contract.needs),
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
                    Literal::Float { .. } => "Float",
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
                } else if let Some(fact) = self.fieldless_constructor_value_fact(symbol) {
                    fact
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
                if let Some(fact) = self
                    .resolved_value_symbol(module_id, *semantic_id)
                    .and_then(|symbol| self.fieldless_constructor_value_fact(symbol))
                {
                    fact
                } else if let Some(symbol) = self.resolved_value_symbol(module_id, *semantic_id)
                    && let Some(type_id) = self.symbol_value_type_or_contract(symbol)
                {
                    ExpressionFact {
                        success: type_id,
                        failures: FailureRowId::new(0),
                        needs: NeedsRowId::new(0),
                    }
                } else {
                    let receiver = self.infer_expression(module_id, receiver, generics, bindings);
                    self.reject_private_field(module_id, expression, receiver.success, member);
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
                let labelled_order =
                    self.reject_argument_labels(module_id, callee, callee_symbol, arguments);
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
                    let callee_id = callee.semantic_id();
                    let callee = match callee_symbol
                        .filter(|_| {
                            matches!(
                                callee.as_ref(),
                                ModuleExpression::Name { .. } | ModuleExpression::Select { .. }
                            )
                        })
                        .and_then(|symbol| self.constructor_callable_fact(symbol))
                    {
                        Some(fact) => self.record_expression_fact(module_id, callee, fact),
                        None => self.infer_expression(module_id, callee, generics, bindings),
                    };
                    let mut argument_facts = arguments
                        .iter()
                        .map(|argument| {
                            self.infer_expression(module_id, &argument.value, generics, bindings)
                        })
                        .collect::<Vec<_>>();
                    if let Some(order) = labelled_order {
                        argument_facts = order
                            .into_iter()
                            .map(|index| argument_facts[index])
                            .collect();
                    }
                    self.infer_invocation(
                        module_id,
                        *range,
                        callee_id,
                        callee_symbol,
                        callee,
                        &argument_facts,
                    )
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
                let float = self.builtin_type("Float");
                let success = if self.resolve_inference_type(fact.success) == float {
                    float
                } else {
                    let int = self.builtin_type("Int");
                    self.unify_at(module_id, *range, fact.success, int, "unary operand");
                    if let Some(value) = self.integer_constant(module_id, operand) {
                        match crate::numeric::integer_negate(value) {
                            Ok(negated) => {
                                self.record_integer_constant(module_id, expression, negated)
                            }
                            Err(_) => self.always_defects_warning(module_id, *range),
                        }
                    }
                    int
                };
                ExpressionFact {
                    success,
                    failures: fact.failures,
                    needs: fact.needs,
                }
            }
            ModuleExpression::Unary {
                operator: UnaryOperator::Not,
                operand,
                ..
            } => {
                let fact = self.infer_expression(module_id, operand, generics, bindings);
                let boolean = self.require_boolean_operand(module_id, "not", operand, fact);
                ExpressionFact {
                    success: boolean,
                    failures: fact.failures,
                    needs: fact.needs,
                }
            }
            ModuleExpression::Binary {
                operator: operator @ (BinaryOperator::And | BinaryOperator::Or),
                left: left_operand,
                right: right_operand,
                ..
            } => {
                let spelling = if *operator == BinaryOperator::And {
                    "and"
                } else {
                    "or"
                };
                let left = self.infer_expression(module_id, left_operand, generics, bindings);
                let right = self.infer_expression(module_id, right_operand, generics, bindings);
                let boolean = self.require_boolean_operand(module_id, spelling, left_operand, left);
                self.require_boolean_operand(module_id, spelling, right_operand, right);
                // Static contracts do not depend on which branch runs, so both rows are exposed
                // even though the right operand may be skipped at run time.
                ExpressionFact {
                    success: boolean,
                    failures: self.union_failure_rows([left.failures, right.failures]),
                    needs: self.union_needs_rows([left.needs, right.needs]),
                }
            }
            ModuleExpression::Binary {
                operator,
                left: left_operand,
                right: right_operand,
                range,
                ..
            } => {
                let left = self.infer_expression(module_id, left_operand, generics, bindings);
                let right = self.infer_expression(module_id, right_operand, generics, bindings);
                {
                    let operand_type =
                        self.numeric_operand_type(module_id, *operator, *range, left, right);
                    if let (Some(arithmetic), Some(left), Some(right)) = (
                        crate::numeric::ArithmeticOperator::from_binary(*operator),
                        self.integer_constant(module_id, left_operand),
                        self.integer_constant(module_id, right_operand),
                    ) {
                        match crate::numeric::integer_binary(arithmetic, left, right) {
                            Ok(value) => self.record_integer_constant(module_id, expression, value),
                            Err(_) => self.always_defects_warning(module_id, *range),
                        }
                    }
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
                            operand_type
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
                let mut contract = self
                    .expression_facts
                    .get(&(module_id, expression.semantic_id()))
                    .and_then(|fact| {
                        let type_id = self.resolve_inference_type(fact.success);
                        match &self.types[type_id.raw() as usize] {
                            CanonicalType::Function(contract) => Some(contract.clone()),
                            _ => None,
                        }
                    })
                    .unwrap_or_else(|| self.lower_signature(module_id, signature, generics));
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
            ModuleExpression::Loop {
                state_id,
                initial,
                body,
                ..
            } => self.infer_loop(module_id, *state_id, initial, body, generics, bindings),
            ModuleExpression::Break {
                semantic_id,
                value,
                range,
            } => self.infer_break(
                module_id,
                *semantic_id,
                value.as_deref(),
                *range,
                generics,
                bindings,
            ),
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
        if arguments.len() != 1 {
            self.type_error_range(
                module_id,
                "T4221",
                range,
                format!(
                    "{} expects exactly one value",
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

        let constructed_failure =
            arguments
                .first()
                .zip(facts.first())
                .and_then(|(argument, fact)| {
                    self.failure_row_for_expression(module_id, &argument.value, fact.success)
                });
        if constructed_failure.is_none() && arguments.len() == 1 {
            self.type_error_range(
                module_id,
                "T4220",
                expression_range(&arguments[0].value),
                "Failure expects a value of an enum error type",
            );
        }
        let constructed_failure = constructed_failure.unwrap_or(FailureRowId::new(0));
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

    fn failure_row_for_expression(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        type_id: TypeId,
    ) -> Option<FailureRowId> {
        let constructed = match expression {
            ModuleExpression::Call { callee, .. } => {
                self.expression_value_symbol(module_id, callee)
            }
            ModuleExpression::Name { .. } | ModuleExpression::Select { .. } => {
                self.expression_value_symbol(module_id, expression)
            }
            ModuleExpression::Group { expression, .. } => {
                return self.failure_row_for_expression(module_id, expression, type_id);
            }
            _ => None,
        };
        if let Some(symbol) = constructed
            && self.is_enum_constructor(symbol)
        {
            let type_id = self.resolve_inference_type(type_id);
            let variant_arguments = match self.types[type_id.raw() as usize].clone() {
                CanonicalType::Named { arguments, .. } if !arguments.is_empty() => {
                    vec![FailureVariantArguments {
                        variant: symbol,
                        arguments,
                    }]
                }
                _ => Vec::new(),
            };
            return Some(self.intern_failure_row(FailureRow {
                variants: vec![symbol],
                variant_arguments,
                parameters: Vec::new(),
                exclusions: Vec::new(),
            }));
        }
        let type_id = self.resolve_inference_type(type_id);
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::Named { symbol, arguments }
                if self.resolved.symbol(symbol).kind == ProjectSymbolKind::Enum
                    || matches!(
                        self.resolved.standard_symbol(symbol),
                        Some(StandardSymbol::Type(kind)) if kind.is_failure_enum()
                    ) =>
            {
                let variants = self.enum_variant_symbols(symbol);
                Some(self.intern_failure_row(FailureRow {
                    variant_arguments: if arguments.is_empty() {
                        Vec::new()
                    } else {
                        variants
                            .iter()
                            .copied()
                            .map(|variant| FailureVariantArguments {
                                variant,
                                arguments: arguments.clone(),
                            })
                            .collect()
                    },
                    variants,
                    parameters: Vec::new(),
                    exclusions: Vec::new(),
                }))
            }
            CanonicalType::Generic(parameter)
                if self.generic_kinds.get(&parameter) == Some(&GenericKind::FailureSet) =>
            {
                Some(self.intern_failure_row(FailureRow {
                    variants: Vec::new(),
                    variant_arguments: Vec::new(),
                    parameters: vec![parameter],
                    exclusions: Vec::new(),
                }))
            }
            CanonicalType::FailureSetValue(failures) => Some(failures),
            _ => None,
        }
    }

    /// Ensures: the state's type is the initial value's type; the body must produce that
    /// type (a mismatch is reported at the body, the forgotten-update error); the loop's
    /// success type is its break type; failures and needs are the union of initial and body.
    fn infer_loop(
        &mut self,
        module_id: ModuleId,
        loop_id: SemanticId,
        initial: &ModuleExpression,
        body: &ModuleExpression,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let initial_fact = self.infer_expression(module_id, initial, generics, bindings);
        let break_type = self.fresh_inference_type();
        self.loop_break_types
            .insert((module_id, loop_id), break_type);
        let mut pass_bindings = bindings.clone();
        let state = self
            .resolved
            .source_symbols(module_id, loop_id)
            .iter()
            .copied()
            .find(|symbol| self.resolved.symbol(*symbol).kind == ProjectSymbolKind::LocalBinding);
        if let Some(state) = state {
            pass_bindings.insert(
                state,
                ValueBinding {
                    type_id: initial_fact.success,
                    need: None,
                },
            );
        }
        let body_fact = self.infer_expression(module_id, body, generics, &mut pass_bindings);
        self.unify_at(
            module_id,
            expression_range(body),
            body_fact.success,
            initial_fact.success,
            "loop body (the next state)",
        );
        ExpressionFact {
            success: break_type,
            failures: self.union_failure_rows([initial_fact.failures, body_fact.failures]),
            needs: self.union_needs_rows([initial_fact.needs, body_fact.needs]),
        }
    }

    /// Ensures: the value (or `Void`) unifies with the target loop's break type; the
    /// break's own success type is a fresh variable because control never continues.
    fn infer_break(
        &mut self,
        module_id: ModuleId,
        break_id: SemanticId,
        value: Option<&ModuleExpression>,
        range: TextRange,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let value_fact = match value {
            Some(value) => self.infer_expression(module_id, value, generics, bindings),
            None => ExpressionFact {
                success: self.builtin_type("Void"),
                failures: FailureRowId::new(0),
                needs: NeedsRowId::new(0),
            },
        };
        let break_type = self
            .resolved
            .break_target(module_id, break_id)
            .and_then(|loop_id| self.loop_break_types.get(&(module_id, loop_id)).copied());
        if let Some(break_type) = break_type {
            self.unify_at(
                module_id,
                range,
                value_fact.success,
                break_type,
                "break value",
            );
        }
        ExpressionFact {
            success: self.fresh_inference_type(),
            ..value_fact
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
        if let Some(witness) = self.pattern_row_witness(&coverage, &missing, &subject_types) {
            let witness = witness
                .iter()
                .map(|pattern| self.render_witness(pattern))
                .collect::<Vec<_>>()
                .join(", ");
            self.type_error_range(
                module_id,
                "T4231",
                range,
                format!("case is not exhaustive; `{witness}` is not matched"),
            );
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
        match pattern {
            Pattern::Wildcard { .. } => MatchPattern::Wildcard,
            Pattern::Literal { literal, range, .. } => {
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
                    PatternLiteral::Float => {
                        self.reject_float_pattern(module_id, *range);
                        (
                            self.builtin_type("Float"),
                            MatchTag::Rejected(pattern.semantic_id()),
                        )
                    }
                };
                if !self.unify(expected, literal_type) {
                    self.type_error_range(
                        module_id,
                        "T4230",
                        *range,
                        "literal pattern does not match the subject type",
                    );
                }
                MatchPattern::Constructor {
                    tag,
                    fields: Vec::new(),
                }
            }
            Pattern::Binding { semantic_id, .. } => {
                if let Some(binding) = self.pattern_binding_symbol(module_id, pattern) {
                    self.pattern_binding_types
                        .insert((module_id, *semantic_id), expected);
                    bindings.insert(
                        binding,
                        ValueBinding {
                            type_id: expected,
                            need: None,
                        },
                    );
                }
                MatchPattern::Wildcard
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
                range,
                ..
            } => {
                let element = self.list_pattern_element_type(expected).unwrap_or_else(|| {
                    self.type_error_range(
                        module_id,
                        "T4230",
                        *range,
                        "list pattern does not match the subject type",
                    );
                    self.intern_type(CanonicalType::Poison)
                });
                let mut normalize_all = |checker: &mut Self, patterns: &[Pattern]| {
                    patterns
                        .iter()
                        .map(|element_pattern| {
                            checker.normalize_pattern(module_id, element_pattern, element, bindings)
                        })
                        .collect::<Vec<_>>()
                };
                let prefix = normalize_all(self, prefix);
                let suffix = normalize_all(self, suffix);
                if let Some(rest) = rest {
                    self.normalize_pattern(module_id, rest, expected, bindings);
                }
                MatchPattern::List {
                    prefix,
                    suffix: rest.as_ref().map(|_| suffix),
                }
            }
            Pattern::Constructor {
                arguments, range, ..
            } => self.normalize_constructor_pattern(
                module_id, pattern, arguments, *range, expected, bindings,
            ),
        }
    }

    /// Unifies `expected` with `List<T>` for a fresh `T` and returns `T`, or `None`
    /// when the subject cannot be a list.
    fn list_pattern_element_type(&mut self, expected: TypeId) -> Option<TypeId> {
        let element = self.fresh_inference_type();
        let list = self.builtin_type_with_arguments("List", vec![element]);
        self.unify(expected, list).then_some(element)
    }

    fn normalize_constructor_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        arguments: &[Pattern],
        range: TextRange,
        expected: TypeId,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> MatchPattern {
        let Some(constructor) = self.resolved_pattern_symbol(module_id, pattern) else {
            return MatchPattern::Wildcard;
        };
        if !self.is_enum_constructor(constructor) {
            self.type_error_range(
                module_id,
                "T4230",
                range,
                "ordinary patterns require a value binding, literal, list, or enum variant",
            );
            return MatchPattern::Wildcard;
        }
        let Some(inputs) = self.instantiate_pattern_constructor(constructor, expected) else {
            self.type_error_range(
                module_id,
                "T4230",
                range,
                format!(
                    "pattern `{}` does not match the subject type",
                    self.resolved.symbol(constructor).name
                ),
            );
            return MatchPattern::Wildcard;
        };
        if arguments.len() != inputs.len() {
            self.type_error_range(
                module_id,
                "T4230",
                range,
                format!(
                    "pattern `{}` expects {} fields, found {}",
                    self.resolved.symbol(constructor).name,
                    inputs.len(),
                    arguments.len()
                ),
            );
        }
        let fields = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                arguments
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
        let expected = self.resolve_inference_type(expected);
        if let CanonicalType::FailureSetValue(failures) =
            self.types[expected.raw() as usize].clone()
        {
            if !self.failure_row_may_contain(failures, constructor) {
                return None;
            }
            if let Some(arguments) = self.failure_rows[failures.raw() as usize]
                .variant_arguments
                .iter()
                .find(|arguments| arguments.variant == constructor)
                .cloned()
            {
                let parameters = self
                    .resolved
                    .standard_callable_parameters(constructor)
                    .iter()
                    .copied()
                    .filter(|parameter| {
                        self.generic_kinds.get(parameter) == Some(&GenericKind::Value)
                    });
                substitution
                    .values
                    .extend(parameters.zip(arguments.arguments));
            }
            return Some(
                contract
                    .inputs
                    .iter()
                    .map(|input| self.substitute_call_type(*input, &mut substitution))
                    .collect(),
            );
        }
        let standard_parameters = self
            .resolved
            .standard_callable_parameters(constructor)
            .to_vec();
        for parameter in &standard_parameters {
            if self.generic_kinds.get(parameter) == Some(&GenericKind::Value) {
                let fresh = self.fresh_inference_type();
                substitution.values.insert(*parameter, fresh);
            }
        }
        if !standard_parameters.is_empty() {
            let success = self.substitute_call_type(contract.success, &mut substitution);
            if !self.unify(expected, success) {
                return None;
            }
            return Some(
                contract
                    .inputs
                    .iter()
                    .map(|input| self.substitute_call_type(*input, &mut substitution))
                    .collect(),
            );
        }
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

    /// Requires an operand of `and`, `or`, or `not` to be `Bool`, reporting `T4304` otherwise.
    ///
    /// Returns the `Bool` type, which is the operator's result type either way.
    /// Chooses the one numeric type that both operands of an arithmetic or comparison
    /// operator share; there are no implicit conversions.
    ///
    /// Ensures: an operand with a known type decides the other; two open operands
    /// default to `Int`; mixed known types are `T4302`, and a non-numeric arithmetic
    /// operand is `T4303`, both returning poison so later stages stay quiet.
    fn numeric_operand_type(
        &mut self,
        module_id: ModuleId,
        operator: BinaryOperator,
        range: TextRange,
        left: ExpressionFact,
        right: ExpressionFact,
    ) -> TypeId {
        let int = self.builtin_type("Int");
        let float = self.builtin_type("Float");
        let poison = TypeId::new(0);
        let left_type = self.resolve_inference_type(left.success);
        let right_type = self.resolve_inference_type(right.success);
        let open = |checker: &Self, id: TypeId| {
            id == poison
                || matches!(
                    checker.types[id.raw() as usize],
                    CanonicalType::InferenceVariable(_)
                )
        };
        let operand_type = if !open(self, left_type) {
            left_type
        } else if !open(self, right_type) {
            right_type
        } else {
            int
        };
        let symbol = binary_operator_symbol(operator);
        let comparison = matches!(
            operator,
            BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
        );
        if operand_type != int && operand_type != float {
            if comparison {
                // Ordering beyond numbers belongs to the equality and ordering work.
                self.unify_at(module_id, range, left.success, int, "binary left operand");
                self.unify_at(module_id, range, right.success, int, "binary right operand");
                return int;
            }
            let found = self.type_display_name(operand_type);
            let help = if operator == BinaryOperator::Add && found == "Text" {
                "; join text with `Text.append`"
            } else {
                ""
            };
            self.type_error_range(
                module_id,
                "T4303",
                range,
                format!("`{symbol}` needs `Int` or `Float` operands, found `{found}`{help}"),
            );
            return poison;
        }
        if !open(self, left_type) && !open(self, right_type) && left_type != right_type {
            let (left_name, right_name) = (
                self.type_display_name(left_type),
                self.type_display_name(right_type),
            );
            let numeric = [left_type, right_type]
                .iter()
                .all(|id| *id == int || *id == float);
            let help = if numeric {
                "; convert one side with `Float.from_int` or `Float.to_int`"
            } else {
                ""
            };
            self.type_error_range(
                module_id,
                "T4302",
                range,
                format!(
                    "`{symbol}` needs operands of one type, found `{left_name}` and `{right_name}`{help}"
                ),
            );
            return poison;
        }
        self.unify(left.success, operand_type);
        self.unify(right.success, operand_type);
        operand_type
    }

    fn type_display_name(&self, id: TypeId) -> String {
        match &self.types[self.resolve_inference_type(id).raw() as usize] {
            CanonicalType::Named { symbol, .. } | CanonicalType::Generic(symbol) => {
                self.resolved.symbol(*symbol).name.clone()
            }
            CanonicalType::Function(_) | CanonicalType::PolymorphicFunction { .. } => {
                "function".to_owned()
            }
            CanonicalType::Record(_) => "record".to_owned(),
            _ => "this type".to_owned(),
        }
    }

    /// The `Int` value of `expression` when it is a literal or a folded constant.
    fn integer_constant(&self, module_id: ModuleId, expression: &ModuleExpression) -> Option<i64> {
        match expression {
            ModuleExpression::Literal(Literal::Integer { value, .. }) => Some(*value),
            ModuleExpression::Group { expression, .. } => {
                self.integer_constant(module_id, expression)
            }
            _ => self
                .integer_constants
                .get(&(module_id, expression.semantic_id()))
                .copied(),
        }
    }

    fn record_integer_constant(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        value: i64,
    ) {
        self.integer_constants
            .insert((module_id, expression.semantic_id()), value);
    }

    /// `T4312`: a warning only. The operation still compiles and defects at run time.
    fn always_defects_warning(&mut self, module_id: ModuleId, range: TextRange) {
        let file_id = self.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .file_id();
        if self
            .warnings
            .iter()
            .any(|warning| warning.file_id() == file_id && warning.range() == range)
        {
            return;
        }
        self.warnings.push(Diagnostic::warning(
            DiagnosticStage::Type,
            "T4312",
            file_id,
            range,
            "this operation always defects: its constant operands overflow `Int` or divide by zero",
        ));
    }

    fn reject_float_pattern(&mut self, module_id: ModuleId, range: TextRange) {
        self.type_error_range(
            module_id,
            "T4313",
            range,
            "`Float` has no literal patterns; compare in a guard, as in `x if x < 1.5`",
        );
    }

    fn require_boolean_operand(
        &mut self,
        module_id: ModuleId,
        operator: &str,
        operand: &ModuleExpression,
        fact: ExpressionFact,
    ) -> TypeId {
        let boolean = self.builtin_type("Bool");
        if !self.unify(fact.success, boolean) {
            self.type_error_range(
                module_id,
                "T4304",
                operand.range(),
                format!(
                    "`{operator}` needs `Bool` operands; Chimi has no truthiness; compare explicitly"
                ),
            );
        }
        boolean
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
                "case guards cannot expose failures or dependencies",
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
        self.pattern_row_witness(matrix, candidate, types).is_some()
    }

    /// Returns a row of value shapes that `candidate` matches and no row of `matrix`
    /// matches, or `None` when `candidate` is useless after `matrix`. The witness has
    /// one entry per candidate column; `_` stands for any value.
    fn pattern_row_witness(
        &mut self,
        matrix: &[Vec<MatchPattern>],
        candidate: &[MatchPattern],
        types: &[TypeId],
    ) -> Option<Vec<MatchPattern>> {
        let Some((head, tail)) = candidate.split_first() else {
            return matrix.is_empty().then(Vec::new);
        };
        let Some((&first_type, tail_types)) = types.split_first() else {
            return Some(candidate.to_vec());
        };
        if let Some(column) = ListColumn::of(matrix, head) {
            return self.list_row_witness(matrix, candidate, column, first_type, tail_types);
        }
        match head {
            MatchPattern::Constructor { tag, fields } => {
                let field_types = self
                    .pattern_constructor_field_types(tag, first_type)
                    .unwrap_or_else(|| vec![TypeId::new(0); fields.len()]);
                let specialized = specialize_pattern_matrix(matrix, tag, field_types.len());
                let mut specialized_candidate = fields.clone();
                specialized_candidate.extend_from_slice(tail);
                let mut specialized_types = field_types;
                specialized_types.extend_from_slice(tail_types);
                let witness = self.pattern_row_witness(
                    &specialized,
                    &specialized_candidate,
                    &specialized_types,
                )?;
                Some(constructor_witness(tag, fields.len(), witness))
            }
            MatchPattern::Wildcard => {
                let domain = self.complete_pattern_domain(first_type);
                if let Some(domain) = &domain {
                    let present = matrix
                        .iter()
                        .filter_map(|row| match row.first() {
                            Some(MatchPattern::Constructor { tag, .. }) => Some(tag),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if domain.iter().all(|(tag, _)| present.contains(&tag)) {
                        return domain.iter().find_map(|(tag, field_types)| {
                            let specialized =
                                specialize_pattern_matrix(matrix, tag, field_types.len());
                            let mut specialized_candidate =
                                vec![MatchPattern::Wildcard; field_types.len()];
                            specialized_candidate.extend_from_slice(tail);
                            let mut specialized_types = field_types.clone();
                            specialized_types.extend_from_slice(tail_types);
                            self.pattern_row_witness(
                                &specialized,
                                &specialized_candidate,
                                &specialized_types,
                            )
                            .map(|witness| constructor_witness(tag, field_types.len(), witness))
                        });
                    }
                }
                let default = default_pattern_matrix(matrix);
                let mut witness = self.pattern_row_witness(&default, tail, tail_types)?;
                let missing = domain.and_then(|domain| {
                    let present = matrix
                        .iter()
                        .filter_map(|row| match row.first() {
                            Some(MatchPattern::Constructor { tag, .. }) => Some(tag.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    domain
                        .into_iter()
                        .find(|(tag, _)| !present.contains(tag))
                        .map(|(tag, field_types)| MatchPattern::Constructor {
                            tag,
                            fields: vec![MatchPattern::Wildcard; field_types.len()],
                        })
                });
                witness.insert(0, missing.unwrap_or(MatchPattern::Wildcard));
                Some(witness)
            }
            MatchPattern::List { .. } => unreachable!("list columns are split by length class"),
        }
    }

    /// Usefulness for a column that holds list patterns: the candidate is useful when it
    /// is useful within one of the length classes it matches.
    fn list_row_witness(
        &mut self,
        matrix: &[Vec<MatchPattern>],
        candidate: &[MatchPattern],
        column: ListColumn,
        list_type: TypeId,
        tail_types: &[TypeId],
    ) -> Option<Vec<MatchPattern>> {
        let element = self.list_element_type(list_type);
        column.classes().find_map(|class| {
            let mut specialized_candidate = column.specialize(&candidate[0], class)?;
            specialized_candidate.extend_from_slice(&candidate[1..]);
            let specialized = matrix
                .iter()
                .filter_map(|row| {
                    let (head, tail) = row.split_first()?;
                    let mut fields = column.specialize(head, class)?;
                    fields.extend_from_slice(tail);
                    Some(fields)
                })
                .collect::<Vec<_>>();
            let arity = column.arity(class);
            let mut specialized_types = vec![element; arity];
            specialized_types.extend_from_slice(tail_types);
            let mut witness =
                self.pattern_row_witness(&specialized, &specialized_candidate, &specialized_types)?;
            let rest = witness.split_off(arity);
            let mut row = vec![column.witness(class, witness)];
            row.extend(rest);
            Some(row)
        })
    }

    fn list_element_type(&self, list_type: TypeId) -> TypeId {
        let list_type = self.resolve_inference_type(list_type);
        match &self.types[list_type.raw() as usize] {
            CanonicalType::Named { arguments, .. } if arguments.len() == 1 => arguments[0],
            _ => TypeId::new(0),
        }
    }

    /// Renders a coverage witness in pattern syntax, such as `[]` or `Some(_)`.
    fn render_witness(&self, pattern: &MatchPattern) -> String {
        let join = |patterns: &[MatchPattern]| {
            patterns
                .iter()
                .map(|pattern| self.render_witness(pattern))
                .collect::<Vec<_>>()
        };
        match pattern {
            MatchPattern::Wildcard => "_".to_owned(),
            MatchPattern::Constructor { tag, fields } => {
                let name = match tag {
                    MatchTag::EnumVariant(symbol) => self.resolved.symbol(*symbol).name.clone(),
                    MatchTag::Boolean(true) => "True".to_owned(),
                    MatchTag::Boolean(false) => "False".to_owned(),
                    MatchTag::Integer(value) => value.to_string(),
                    MatchTag::Text(value) => format!("{value:?}"),
                    MatchTag::Rejected(_) => "_".to_owned(),
                };
                if fields.is_empty() {
                    name
                } else {
                    format!("{name}({})", join(fields).join(", "))
                }
            }
            MatchPattern::List { prefix, suffix } => {
                let mut elements = join(prefix);
                if let Some(suffix) = suffix {
                    elements.push("..".to_owned());
                    elements.extend(join(suffix));
                }
                format!("[{}]", elements.join(", "))
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
            MatchTag::Boolean(_)
            | MatchTag::Integer(_)
            | MatchTag::Text(_)
            | MatchTag::Rejected(_) => Some(Vec::new()),
        }
    }

    fn complete_pattern_domain(&mut self, type_id: TypeId) -> Option<Vec<(MatchTag, Vec<TypeId>)>> {
        let type_id = self.resolve_inference_type(type_id);
        if let CanonicalType::FailureSetValue(failures) = self.types[type_id.raw() as usize].clone()
        {
            let row = self.failure_rows[failures.raw() as usize].clone();
            if !row.parameters.is_empty() {
                return None;
            }
            return Some(
                row.variants
                    .into_iter()
                    .filter_map(|variant| {
                        self.instantiate_pattern_constructor(variant, type_id)
                            .map(|fields| (MatchTag::EnumVariant(variant), fields))
                    })
                    .collect(),
            );
        }
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
        let standard_enum = matches!(
            self.resolved.standard_symbol(symbol),
            Some(StandardSymbol::Type(
                StandardType::Option
                    | StandardType::JsonValue
                    | StandardType::DecodeError
                    | StandardType::NumberError
                    | StandardType::ChannelSendError
                    | StandardType::ChannelReceive
                    | StandardType::ChannelError
            ))
        );
        if symbol_kind != ProjectSymbolKind::Enum && !standard_enum {
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
        let mut candidate_failure_variants = Vec::new();
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
                    candidate_failure_variants.extend(variants.iter().copied());
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
                let pattern_range = pattern.head_range();
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
            candidate_failure_variants.extend(subject_variants);
            candidate_failure_variants.sort_unstable();
            candidate_failure_variants.dedup();
            for variant in candidate_failure_variants {
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
        let arguments = pattern.constructor_arguments();
        if arguments.len() != 1 {
            let range = pattern.head_range();
            self.type_error_range(
                module_id,
                "T4222",
                range,
                "outcome patterns require exactly one nested pattern",
            );
            None
        } else {
            arguments.first()
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
            && self.is_enum_constructor(symbol)
        {
            if !self.failure_row_may_contain(failures, symbol) {
                let range = pattern.head_range();
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
        if symbol.is_some() || matches!(pattern, Pattern::Literal { .. } | Pattern::List { .. }) {
            let range = pattern.head_range();
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
        match pattern {
            Pattern::Wildcard { .. } => {}
            Pattern::Literal { literal, range, .. } => {
                let literal_type = self.builtin_type(match literal {
                    PatternLiteral::Integer(_) => "Int",
                    PatternLiteral::Text(_) => "Text",
                    PatternLiteral::Boolean(_) => "Bool",
                    PatternLiteral::Float => {
                        self.reject_float_pattern(module_id, *range);
                        return;
                    }
                });
                if !self.unify(expected, literal_type) {
                    self.type_error_range(
                        module_id,
                        "T4222",
                        *range,
                        "nested outcome pattern does not match its value type",
                    );
                }
            }
            Pattern::Binding { semantic_id, .. } => {
                if let Some(binding) = self.pattern_binding_symbol(module_id, pattern) {
                    self.pattern_binding_types
                        .insert((module_id, *semantic_id), expected);
                    bindings.insert(
                        binding,
                        ValueBinding {
                            type_id: expected,
                            need: None,
                        },
                    );
                }
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
                range,
                ..
            } => {
                let Some(element) = self.list_pattern_element_type(expected) else {
                    self.type_error_range(
                        module_id,
                        "T4222",
                        *range,
                        "nested outcome pattern does not match its value type",
                    );
                    return;
                };
                for element_pattern in prefix.iter().chain(suffix) {
                    self.bind_value_pattern(module_id, element_pattern, element, bindings);
                }
                if let Some(rest) = rest {
                    self.bind_value_pattern(module_id, rest, expected, bindings);
                }
            }
            Pattern::Constructor {
                path, arguments, ..
            } => {
                let Some(name) = path.last() else {
                    return;
                };
                let Some(constructor) = self.resolved_pattern_symbol(module_id, pattern) else {
                    return;
                };
                if !self.is_enum_constructor(constructor) {
                    return;
                }
                let contract = self.contracts[&constructor].clone();
                let mut substitution = CallSubstitution::default();
                let expected = self.resolve_inference_type(expected);
                let matches = match self.types[expected.raw() as usize].clone() {
                    CanonicalType::FailureSetValue(failures) => {
                        self.failure_row_may_contain(failures, constructor)
                    }
                    _ => self.match_call_type(expected, contract.success, &mut substitution),
                };
                if !matches {
                    self.type_error_range(
                        module_id,
                        "T4222",
                        name.range,
                        format!("pattern `{}` does not match the subject type", name.text),
                    );
                    return;
                }
                if arguments.len() != contract.inputs.len() {
                    self.type_error_range(
                        module_id,
                        "T4222",
                        name.range,
                        format!(
                            "pattern `{}` expects {} fields, found {}",
                            name.text,
                            contract.inputs.len(),
                            arguments.len()
                        ),
                    );
                }
                let inputs = contract
                    .inputs
                    .iter()
                    .map(|input| self.substitute_call_type(*input, &mut substitution))
                    .collect::<Vec<_>>();
                for (argument, input) in arguments.iter().zip(inputs) {
                    self.bind_value_pattern(module_id, argument, input, bindings);
                }
            }
        }
    }

    fn pattern_is_builtin(&self, module_id: ModuleId, pattern: &Pattern, name: &str) -> bool {
        self.resolved_pattern_symbol(module_id, pattern)
            .is_some_and(|symbol| self.is_builtin_value(symbol, name))
    }

    fn pattern_binding_symbol(&self, module_id: ModuleId, pattern: &Pattern) -> Option<SymbolId> {
        let Pattern::Binding {
            semantic_id, name, ..
        } = pattern
        else {
            return None;
        };
        self.find_symbol(
            module_id,
            Some(*semantic_id),
            ProjectSymbolKind::PatternBinding,
            &name.text,
        )
    }

    fn resolved_pattern_symbol(&self, module_id: ModuleId, pattern: &Pattern) -> Option<SymbolId> {
        self.resolved
            .reference_resolution(module_id, pattern.semantic_id(), Namespace::Value)
            .and_then(|resolution| match resolution {
                ReferenceResolution::Resolved(symbol) => Some(symbol),
                ReferenceResolution::Unresolved
                | ReferenceResolution::Ambiguous
                | ReferenceResolution::Inaccessible(_)
                | ReferenceResolution::DeferredToType => None,
            })
    }

    fn failure_row_for_type(&mut self, type_id: TypeId) -> Option<FailureRowId> {
        let type_id = self.resolve_inference_type(type_id);
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::Named { symbol, arguments }
                if self.resolved.symbol(symbol).kind == ProjectSymbolKind::Enum
                    || matches!(
                        self.resolved.standard_symbol(symbol),
                        Some(StandardSymbol::Type(kind)) if kind.is_failure_enum()
                    ) =>
            {
                let variants = self.enum_variant_symbols(symbol);
                Some(self.intern_failure_row(FailureRow {
                    variant_arguments: if arguments.is_empty() {
                        Vec::new()
                    } else {
                        variants
                            .iter()
                            .copied()
                            .map(|variant| FailureVariantArguments {
                                variant,
                                arguments: arguments.clone(),
                            })
                            .collect()
                    },
                    variants,
                    parameters: Vec::new(),
                    exclusions: Vec::new(),
                }))
            }
            CanonicalType::Generic(parameter)
                if self.generic_kinds.get(&parameter) == Some(&GenericKind::FailureSet) =>
            {
                Some(self.intern_failure_row(FailureRow {
                    variants: Vec::new(),
                    variant_arguments: Vec::new(),
                    parameters: vec![parameter],
                    exclusions: Vec::new(),
                }))
            }
            CanonicalType::FailureSetValue(failures) => Some(failures),
            _ => None,
        }
    }

    /// The type a failure-set parameter (`E` in `Task<T, E>`) takes when it
    /// is instantiated with `failures`. An empty row is still a concrete
    /// failure set ("cannot fail"); making it `Poison` would hide the binding
    /// from later calls such as `Task.result`, which then leak `E`.
    fn failure_set_argument_type(&mut self, failures: FailureRowId) -> TypeId {
        self.failure_value_type(failures)
            .unwrap_or_else(|| self.intern_type(CanonicalType::FailureSetValue(failures)))
    }

    fn failure_value_type(&mut self, failures: FailureRowId) -> Option<TypeId> {
        let row = self.failure_rows[failures.raw() as usize].clone();
        if row.variants.is_empty() && row.parameters.is_empty() {
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
            return Some(self.intern_type(CanonicalType::FailureSetValue(failures)));
        };
        if !row.parameters.is_empty() {
            return Some(self.intern_type(CanonicalType::FailureSetValue(failures)));
        }
        let mut owner_variants = self.enum_variant_symbols(*owner);
        owner_variants.sort_unstable();
        if row.variants != owner_variants {
            return Some(self.intern_type(CanonicalType::FailureSetValue(failures)));
        }
        if matches!(
            self.resolved.standard_symbol(*owner),
            Some(StandardSymbol::Type(_))
        ) {
            let arguments = row
                .variant_arguments
                .iter()
                .find(|arguments| arguments.variant == row.variants[0])
                .map(|arguments| arguments.arguments.clone())
                .unwrap_or_default();
            let consistent = row.variant_arguments.iter().all(|candidate| {
                !row.variants.contains(&candidate.variant) || candidate.arguments == arguments
            });
            if consistent {
                return Some(self.intern_type(CanonicalType::Named {
                    symbol: *owner,
                    arguments,
                }));
            }
            return Some(self.intern_type(CanonicalType::FailureSetValue(failures)));
        }
        let owner = self.resolved.symbol(*owner);
        self.declaration_types
            .get(&(owner.module_id?, owner.semantic_id?))
            .copied()
            .or_else(|| Some(self.intern_type(CanonicalType::FailureSetValue(failures))))
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
        row.variant_arguments
            .retain(|arguments| !handled_variants.contains(&arguments.variant));
        if handles_unknown_row {
            row.parameters.clear();
            row.exclusions.clear();
        } else {
            for parameter in &row.parameters {
                if let Some(exclusion) = row
                    .exclusions
                    .iter_mut()
                    .find(|exclusion| exclusion.parameter == *parameter)
                {
                    exclusion.variants.extend_from_slice(handled_variants);
                    exclusion.variants.sort_unstable();
                    exclusion.variants.dedup();
                } else if !handled_variants.is_empty() {
                    row.exclusions.push(FailureRowExclusion {
                        parameter: *parameter,
                        variants: handled_variants.to_vec(),
                    });
                }
            }
            row.exclusions.sort_by_key(|exclusion| exclusion.parameter);
        }
        self.intern_failure_row(row)
    }

    fn failure_row_is_empty(&self, failures: FailureRowId) -> bool {
        let row = &self.failure_rows[failures.raw() as usize];
        row.variants.is_empty() && row.parameters.is_empty()
    }

    fn failure_row_may_contain(&self, failures: FailureRowId, variant: SymbolId) -> bool {
        let row = &self.failure_rows[failures.raw() as usize];
        row.variants.contains(&variant)
            || row.parameters.iter().any(|parameter| {
                row.exclusions
                    .iter()
                    .find(|exclusion| exclusion.parameter == *parameter)
                    .is_none_or(|exclusion| !exclusion.variants.contains(&variant))
            })
    }

    fn needs_row_is_empty(&self, needs: NeedsRowId) -> bool {
        let row = &self.needs_rows[needs.raw() as usize];
        row.entries.is_empty() && row.parameters.is_empty()
    }

    fn enum_owner_for_variant(&self, variant: SymbolId) -> Option<SymbolId> {
        if let Some(StandardSymbol::Constructor(constructor)) =
            self.resolved.standard_symbol(variant)
        {
            return Some(self.resolved.standard_type(constructor.type_kind()));
        }
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
        if let Some(StandardSymbol::Type(kind)) = self.resolved.standard_symbol(owner) {
            let mut variants = self
                .resolved
                .symbols
                .iter()
                .filter_map(|symbol| match self.resolved.standard_symbol(symbol.id) {
                    Some(StandardSymbol::Constructor(constructor))
                        if constructor.type_kind() == kind =>
                    {
                        Some(symbol.id)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            variants.sort_unstable();
            return variants;
        }
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

    fn is_enum_constructor(&self, symbol: SymbolId) -> bool {
        self.resolved.symbol(symbol).kind == ProjectSymbolKind::EnumConstructor
            || matches!(
                self.resolved.standard_symbol(symbol),
                Some(StandardSymbol::Constructor(_))
            )
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
            && actual.parameters.iter().all(|parameter| {
                if !expected.parameters.contains(parameter) {
                    return false;
                }
                let actual_exclusions = actual
                    .exclusions
                    .iter()
                    .find(|exclusion| exclusion.parameter == *parameter)
                    .map_or(&[][..], |exclusion| exclusion.variants.as_slice());
                let expected_exclusions = expected
                    .exclusions
                    .iter()
                    .find(|exclusion| exclusion.parameter == *parameter)
                    .map_or(&[][..], |exclusion| exclusion.variants.as_slice());
                expected_exclusions
                    .iter()
                    .all(|variant| actual_exclusions.contains(variant))
            })
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
                variant_arguments: Vec::new(),
                parameters: vec![symbol],
                exclusions: Vec::new(),
            });
        }
        if matches!(
            self.resolved.standard_symbol(symbol),
            Some(StandardSymbol::Type(kind)) if kind.is_failure_enum()
        ) {
            return self.intern_failure_row(FailureRow {
                variants: self.enum_variant_symbols(symbol),
                variant_arguments: Vec::new(),
                parameters: Vec::new(),
                exclusions: Vec::new(),
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
            variant_arguments: Vec::new(),
            parameters: Vec::new(),
            exclusions: Vec::new(),
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
            .reference_resolution(module_id, semantic_id, Namespace::Value)
            .and_then(|resolution| match resolution {
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
        callee_id: SemanticId,
        callee_symbol: Option<SymbolId>,
        callee: ExpressionFact,
        arguments: &[ExpressionFact],
    ) -> ExpressionFact {
        let callee_type = self.resolve_inference_type(callee.success);
        let (contract, parameters, mut substitution) = match self
            .types
            .get(callee_type.raw() as usize)
            .cloned()
        {
            Some(CanonicalType::Function(contract)) => (contract, Vec::new(), None),
            Some(CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            }) => (contract, parameters, Some(CallSubstitution::default())),
            _ => {
                if callee_type != TypeId::new(0) {
                    self.type_error_range(module_id, "T4201", range, "expression is not callable");
                }
                return ExpressionFact {
                    success: TypeId::new(0),
                    failures: self.union_failure_rows(
                        std::iter::once(callee.failures)
                            .chain(arguments.iter().map(|argument| argument.failures)),
                    ),
                    needs: self.union_needs_rows(
                        std::iter::once(callee.needs)
                            .chain(arguments.iter().map(|argument| argument.needs)),
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
        for (argument, expected) in arguments.iter().zip(contract.inputs.iter().copied()) {
            if let Some(substitution) = &mut substitution {
                if !self.match_call_type(argument.success, expected, substitution) {
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
                    argument.success,
                    expected,
                    "call argument",
                );
            }
        }
        let (success, contract_failures, contract_needs) = if let Some(substitution) =
            &mut substitution
        {
            // A value parameter the arguments leave open (`T` in
            // `wrap<T, E>(e: E) -> Res<T, E>`) is fixed by the call's context,
            // so it becomes an inference variable. Only a declared function
            // returning a function keeps it open, so the result generalizes.
            let returns_function = matches!(
                self.types[self.resolve_inference_type(contract.success).raw() as usize],
                CanonicalType::Function(_) | CanonicalType::PolymorphicFunction { .. }
            );
            if !returns_function
                || callee_symbol.is_some_and(|symbol| {
                    matches!(
                        self.resolved.standard_symbol(symbol),
                        Some(StandardSymbol::Function(_) | StandardSymbol::Constructor(_))
                    )
                })
            {
                for parameter in &parameters {
                    if self.generic_kinds.get(parameter) == Some(&GenericKind::Value)
                        && !substitution.values.contains_key(parameter)
                    {
                        let fresh = self.fresh_inference_type();
                        substitution.values.insert(*parameter, fresh);
                    }
                }
            }
            let unresolved = self.unresolved_call_parameters(&parameters, substitution);
            // A copy, so recording the instantiation cannot bind parameters
            // the call left open.
            let instantiated = self.substitute_call_contract_preserving(
                contract.clone(),
                &mut substitution.clone(),
                &unresolved,
            );
            let instantiated = self.intern_type(CanonicalType::Function(instantiated));
            self.callee_instantiations
                .insert((module_id, callee_id), instantiated);
            let success =
                self.substitute_call_type_preserving(contract.success, substitution, &unresolved);
            (
                self.generalize_callable_type(success, unresolved),
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
                    .chain(arguments.iter().map(|argument| argument.failures))
                    .chain(std::iter::once(contract_failures)),
            ),
            needs: self.union_needs_rows(
                std::iter::once(callee.needs)
                    .chain(arguments.iter().map(|argument| argument.needs))
                    .chain(std::iter::once(contract_needs)),
            ),
        }
    }

    fn unresolved_call_parameters(
        &self,
        parameters: &[SymbolId],
        substitution: &CallSubstitution,
    ) -> Vec<SymbolId> {
        parameters
            .iter()
            .copied()
            .filter(
                |parameter| match self.generic_kinds.get(parameter).copied() {
                    Some(GenericKind::Value) => !substitution.values.contains_key(parameter),
                    Some(GenericKind::FailureRow) => !substitution.failures.contains_key(parameter),
                    Some(GenericKind::FailureSet) => {
                        !substitution.values.contains_key(parameter)
                            && !substitution.failures.contains_key(parameter)
                    }
                    Some(GenericKind::NeedsRow) => !substitution.needs.contains_key(parameter),
                    Some(GenericKind::Unconstrained | GenericKind::Conflicting) | None => {
                        !substitution.values.contains_key(parameter)
                            && !substitution.failures.contains_key(parameter)
                            && !substitution.needs.contains_key(parameter)
                    }
                },
            )
            .collect()
    }

    fn generalize_callable_type(&mut self, success: TypeId, parameters: Vec<SymbolId>) -> TypeId {
        if parameters.is_empty() {
            return success;
        }
        let success = self.resolve_inference_type(success);
        match self.types[success.raw() as usize].clone() {
            CanonicalType::Function(contract) => {
                self.intern_type(CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                })
            }
            _ => success,
        }
    }

    /// Reports labelled call arguments, which Chimi does not have. When every
    /// argument of a constructor call names a distinct declared field, the
    /// diagnostic shows the positional rewrite and the returned order lets
    /// checking continue as if the user had written it.
    fn reject_argument_labels(
        &mut self,
        module_id: ModuleId,
        callee: &ModuleExpression,
        callee_symbol: Option<SymbolId>,
        arguments: &[crate::semantic::CallArgument],
    ) -> Option<Vec<usize>> {
        let first_label = arguments
            .iter()
            .find_map(|argument| argument.label.as_ref())?;
        let fields = callee_symbol
            .filter(|symbol| self.is_constructor_symbol(*symbol))
            .and_then(|symbol| self.fields.get(&symbol).cloned());
        let Some(fields) = fields else {
            self.type_error_range(
                module_id,
                "T4210",
                first_label.range,
                format!(
                    "Chimi arguments are positional; remove the `{}:` label",
                    first_label.text
                ),
            );
            return None;
        };
        let order = (arguments.len() == fields.len())
            .then(|| {
                fields
                    .iter()
                    .map(|field| {
                        let mut matching = arguments.iter().enumerate().filter(|(_, argument)| {
                            argument
                                .label
                                .as_ref()
                                .is_some_and(|label| label.text == field.name)
                        });
                        match (matching.next(), matching.next()) {
                            (Some((index, _)), None) => Some(index),
                            _ => None,
                        }
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .flatten();
        let source = self.resolved.semantic.modules[module_id.raw() as usize]
            .source
            .text();
        let slice = |range: TextRange| &source[range.start() as usize..range.end() as usize];
        let message = match &order {
            Some(order) => format!(
                "Chimi arguments are positional; write `{}({})`",
                slice(callee.range()),
                order
                    .iter()
                    .map(|index| slice(arguments[*index].value.range()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!(
                "Chimi arguments are positional; pass `{}` fields in declaration order ({})",
                slice(callee.range()),
                fields
                    .iter()
                    .map(|field| field.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        self.type_error_range(module_id, "T4210", first_label.range, message);
        order
    }

    /// A constructor named as a callee is its construction function, even when
    /// it has no fields (`None()`).
    fn constructor_callable_fact(&mut self, symbol: SymbolId) -> Option<ExpressionFact> {
        if !self.is_constructor_symbol(symbol) {
            return None;
        }
        let contract = self.contracts.get(&symbol).cloned()?;
        Some(ExpressionFact {
            success: self.callable_type_for_symbol(symbol, contract),
            failures: FailureRowId::new(0),
            needs: NeedsRowId::new(0),
        })
    }

    /// A fieldless constructor named as a value is the value it constructs
    /// (`None`, `Broken`), with fresh type arguments for a generic owner.
    fn fieldless_constructor_value_fact(&mut self, symbol: SymbolId) -> Option<ExpressionFact> {
        if !self.is_constructor_symbol(symbol) {
            return None;
        }
        let contract = self.contracts.get(&symbol).cloned()?;
        if !contract.inputs.is_empty() {
            return None;
        }
        let success = if self.callable_generic_parameters(symbol).is_empty() {
            contract.success
        } else {
            self.substitute_call_type(contract.success, &mut CallSubstitution::default())
        };
        Some(ExpressionFact {
            success,
            failures: FailureRowId::new(0),
            needs: NeedsRowId::new(0),
        })
    }

    fn is_constructor_symbol(&self, symbol: SymbolId) -> bool {
        matches!(
            self.resolved.symbol(symbol).kind,
            ProjectSymbolKind::StructConstructor | ProjectSymbolKind::EnumConstructor
        ) || matches!(
            self.resolved.standard_symbol(symbol),
            Some(StandardSymbol::Constructor(_))
        )
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
        let standard = self.resolved.standard_callable_parameters(symbol);
        if !standard.is_empty() {
            return standard.to_vec();
        }
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
            .source_symbols(
                symbol
                    .module_id
                    .expect("source callable belongs to a module"),
                declaration_semantic_id.expect("source callable has a declaration identity"),
            )
            .iter()
            .copied()
            .filter(|candidate| {
                self.resolved.symbol(*candidate).kind == ProjectSymbolKind::GenericParameter
            })
            .collect::<Vec<_>>();
        parameters.sort_unstable();
        parameters
    }

    /// Publishing a struct exposes its nominal type, not its fields, so a
    /// field of a source struct is readable only in its declaring module.
    /// When that module publishes a function named after the field and the
    /// reader imports it under an alias, the message names the accessor call.
    fn reject_private_field(
        &mut self,
        module_id: ModuleId,
        expression: &ModuleExpression,
        receiver_type: TypeId,
        member: &crate::semantic::Name,
    ) {
        let member_range = member.range;
        let member = member.text.as_str();
        let receiver_type = self.resolve_inference_type(receiver_type);
        let CanonicalType::Named { symbol, .. } = &self.types[receiver_type.raw() as usize] else {
            return;
        };
        let owner = self.resolved.symbol(*symbol);
        let Some(owner_module) = owner.module_id else {
            return;
        };
        if owner.kind != ProjectSymbolKind::Struct
            || owner_module == module_id
            || self.field_type(receiver_type, member).is_none()
        {
            return;
        }
        let type_name = owner.name.clone();
        let declaring = &self.resolved.semantic.modules[owner_module.raw() as usize];
        let has_accessor = declaring.declarations.iter().any(|declaration| {
            declaration.kind == ModuleDeclarationKind::Function
                && declaration.public
                && declaration.name.text == member
        });
        let current = &self.resolved.semantic.modules[module_id.raw() as usize];
        let alias = current
            .imports
            .iter()
            .find_map(|import| match &import.binding {
                crate::semantic::ImportBinding::Alias(alias) if import.path == declaring.path => {
                    Some(alias.text.clone())
                }
                _ => None,
            });
        let message = match alias.filter(|_| has_accessor) {
            Some(alias) => {
                let start = expression_range(expression).start() as usize;
                let receiver_text = current.source.text()[start..member_range.start() as usize]
                    .trim_end()
                    .trim_end_matches('.')
                    .trim_end();
                format!(
                    "field {member} of {type_name} is module-private; \
                     use {alias}.{member}({receiver_text})"
                )
            }
            None => format!("field {member} of {type_name} is module-private"),
        };
        self.type_error_range(
            module_id,
            "E_PRIVATE_STRUCT_FIELD",
            expression_range(expression),
            message,
        );
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
                if self.generic_kinds.get(&parameter) == Some(&GenericKind::FailureSet) {
                    if let Some(bound) = substitution.failures.get(&parameter).copied() {
                        return self
                            .failure_row_for_type(actual)
                            .is_some_and(|actual| self.failure_rows_equivalent(actual, bound));
                    }
                    if let Some(bound) = substitution.values.get(&parameter).copied() {
                        return self.unify(actual, bound);
                    }
                    substitution.values.insert(parameter, actual);
                    if let Some(failures) = self.failure_row_for_type(actual) {
                        substitution.failures.insert(parameter, failures);
                    }
                    return true;
                }
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
            (
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
                CanonicalType::Function(_),
            ) => {
                let actual = self.instantiate_function_value(&parameters, contract);
                self.match_call_type(actual, expected, substitution)
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
                let remaining_variant_arguments = actual
                    .variant_arguments
                    .into_iter()
                    .filter(|arguments| remaining_variants.contains(&arguments.variant))
                    .collect();
                let residual = self.intern_failure_row(FailureRow {
                    variants: remaining_variants,
                    variant_arguments: remaining_variant_arguments,
                    parameters: actual.parameters,
                    exclusions: actual.exclusions,
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
        self.substitute_call_type_preserving(type_id, substitution, &[])
    }

    fn substitute_call_type_preserving(
        &mut self,
        type_id: TypeId,
        substitution: &mut CallSubstitution,
        preserved: &[SymbolId],
    ) -> TypeId {
        let type_id = self.resolve_inference_type(type_id);
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::Generic(parameter) if preserved.contains(&parameter) => type_id,
            CanonicalType::Generic(parameter) => {
                if let Some(type_id) = substitution.values.get(&parameter).copied() {
                    type_id
                } else if self.generic_kinds.get(&parameter) == Some(&GenericKind::FailureSet) {
                    if let Some(failures) = substitution.failures.get(&parameter).copied() {
                        self.failure_set_argument_type(failures)
                    } else {
                        let type_id = self.fresh_inference_type();
                        substitution.values.insert(parameter, type_id);
                        type_id
                    }
                } else {
                    let type_id = self.fresh_inference_type();
                    substitution.values.insert(parameter, type_id);
                    type_id
                }
            }
            CanonicalType::Named { symbol, arguments } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| {
                        self.substitute_call_type_preserving(argument, substitution, preserved)
                    })
                    .collect();
                self.intern_type(CanonicalType::Named { symbol, arguments })
            }
            CanonicalType::Function(contract) => {
                let contract =
                    self.substitute_call_contract_preserving(contract, substitution, preserved);
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
                        type_id: self.substitute_call_type_preserving(
                            field.type_id,
                            substitution,
                            preserved,
                        ),
                    })
                    .collect();
                self.intern_type(CanonicalType::Record(fields))
            }
            CanonicalType::FailureSetValue(failures) => {
                let failures = self.substitute_call_failure_row(failures, substitution);
                self.failure_set_argument_type(failures)
            }
            CanonicalType::InferenceVariable(_) | CanonicalType::Poison => type_id,
        }
    }

    fn substitute_call_contract_preserving(
        &mut self,
        contract: CallableContract,
        substitution: &mut CallSubstitution,
        preserved: &[SymbolId],
    ) -> CallableContract {
        CallableContract {
            inputs: contract
                .inputs
                .into_iter()
                .map(|input| self.substitute_call_type_preserving(input, substitution, preserved))
                .collect(),
            success: self.substitute_call_type_preserving(
                contract.success,
                substitution,
                preserved,
            ),
            failures: self.substitute_call_failure_row(contract.failures, substitution),
            needs: self.substitute_call_needs_row_preserving(
                contract.needs,
                substitution,
                preserved,
            ),
        }
    }

    fn substitute_call_failure_row(
        &mut self,
        row: FailureRowId,
        substitution: &CallSubstitution,
    ) -> FailureRowId {
        let row = self.failure_rows[row.raw() as usize].clone();
        let mut type_substitution = (*substitution).clone();
        let variant_arguments = row
            .variant_arguments
            .into_iter()
            .map(|arguments| FailureVariantArguments {
                variant: arguments.variant,
                arguments: arguments
                    .arguments
                    .into_iter()
                    .map(|argument| self.substitute_call_type(argument, &mut type_substitution))
                    .collect(),
            })
            .collect();
        let mut rows = vec![self.intern_failure_row(FailureRow {
            variants: row.variants,
            variant_arguments,
            parameters: Vec::new(),
            exclusions: Vec::new(),
        })];
        for parameter in row.parameters {
            let replacement = substitution
                .failures
                .get(&parameter)
                .copied()
                .unwrap_or_else(|| {
                    self.intern_failure_row(FailureRow {
                        variants: Vec::new(),
                        variant_arguments: Vec::new(),
                        parameters: vec![parameter],
                        exclusions: Vec::new(),
                    })
                });
            let excluded = row
                .exclusions
                .iter()
                .find(|exclusion| exclusion.parameter == parameter)
                .map_or(&[][..], |exclusion| exclusion.variants.as_slice());
            rows.push(self.remove_handled_failures(replacement, excluded, false));
        }
        self.union_failure_rows(rows)
    }

    fn substitute_call_needs_row(
        &mut self,
        row: NeedsRowId,
        substitution: &mut CallSubstitution,
    ) -> NeedsRowId {
        self.substitute_call_needs_row_preserving(row, substitution, &[])
    }

    fn substitute_call_needs_row_preserving(
        &mut self,
        row: NeedsRowId,
        substitution: &mut CallSubstitution,
        preserved: &[SymbolId],
    ) -> NeedsRowId {
        let row = self.needs_rows[row.raw() as usize].clone();
        let entries = row
            .entries
            .into_iter()
            .map(|need| Need {
                interface: self.substitute_call_type_preserving(
                    need.interface,
                    substitution,
                    preserved,
                ),
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
            (CanonicalType::FailureSetValue(left), CanonicalType::FailureSetValue(right)) => {
                self.failure_rows_equivalent(left, right)
            }
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
            (
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
                CanonicalType::Function(_),
            ) => {
                let left = self.instantiate_function_value(&parameters, contract);
                self.unify(left, right)
            }
            (
                CanonicalType::Function(_),
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
            ) => {
                let right = self.instantiate_function_value(&parameters, contract);
                self.unify(left, right)
            }
            _ => false,
        }
    }

    /// Instantiates a generic function value (`wrap`, `Some`, `Map.get`) where
    /// it meets a function type, so it can flow into a monomorphic or
    /// differently generic function-typed slot. Value parameters become fresh
    /// inference variables. Rows have no inference variables, so a failure or
    /// needs row parameter of the value instantiates to the empty row: leaving
    /// it in place would leak the callee's own row parameter into the caller's
    /// contract.
    fn instantiate_function_value(
        &mut self,
        parameters: &[SymbolId],
        contract: CallableContract,
    ) -> TypeId {
        let mut substitution = CallSubstitution::default();
        for parameter in parameters {
            match self.generic_kinds.get(parameter).copied() {
                Some(GenericKind::Value | GenericKind::FailureSet) => {}
                Some(GenericKind::FailureRow) => {
                    substitution
                        .failures
                        .insert(*parameter, FailureRowId::new(0));
                }
                Some(GenericKind::NeedsRow) => {
                    substitution.needs.insert(*parameter, NeedsRowId::new(0));
                }
                Some(GenericKind::Unconstrained | GenericKind::Conflicting) | None => {
                    substitution
                        .failures
                        .insert(*parameter, FailureRowId::new(0));
                    substitution.needs.insert(*parameter, NeedsRowId::new(0));
                }
            }
        }
        let contract = self.substitute_call_contract_preserving(contract, &mut substitution, &[]);
        self.intern_type(CanonicalType::Function(contract))
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

    fn types_equivalent(&self, left: TypeId, right: TypeId) -> bool {
        let left = self.resolve_inference_type(left);
        let right = self.resolve_inference_type(right);
        if left == right {
            return true;
        }
        match (
            &self.types[left.raw() as usize],
            &self.types[right.raw() as usize],
        ) {
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
                        .iter()
                        .zip(right_arguments)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
            }
            (CanonicalType::Function(left), CanonicalType::Function(right)) => {
                self.contracts_equivalent(left, right)
            }
            (
                CanonicalType::PolymorphicFunction {
                    parameters: left_parameters,
                    contract: left_contract,
                },
                CanonicalType::PolymorphicFunction {
                    parameters: right_parameters,
                    contract: right_contract,
                },
            ) => {
                left_parameters == right_parameters
                    && self.contracts_equivalent(left_contract, right_contract)
            }
            (CanonicalType::FailureSetValue(left), CanonicalType::FailureSetValue(right)) => {
                self.failure_rows_equivalent(*left, *right)
            }
            (CanonicalType::Record(left), CanonicalType::Record(right)) => {
                left.len() == right.len()
                    && left.iter().zip(right).all(|(left, right)| {
                        left.name == right.name
                            && self.types_equivalent(left.type_id, right.type_id)
                    })
            }
            _ => false,
        }
    }

    fn contracts_equivalent(&self, left: &CallableContract, right: &CallableContract) -> bool {
        left.inputs.len() == right.inputs.len()
            && left
                .inputs
                .iter()
                .zip(&right.inputs)
                .all(|(left, right)| self.types_equivalent(*left, *right))
            && self.types_equivalent(left.success, right.success)
            && self.failure_rows_equivalent(left.failures, right.failures)
            && self.needs_rows_structurally_equivalent(left.needs, right.needs)
    }

    fn needs_rows_structurally_equivalent(&self, left: NeedsRowId, right: NeedsRowId) -> bool {
        let left = &self.needs_rows[left.raw() as usize];
        let right = &self.needs_rows[right.raw() as usize];
        left.parameters == right.parameters
            && left.entries.len() == right.entries.len()
            && left.entries.iter().all(|left_need| {
                right.entries.iter().any(|right_need| {
                    left_need.lexical_name == right_need.lexical_name
                        && left_need.explained == right_need.explained
                        && self.types_equivalent(left_need.interface, right_need.interface)
                })
            })
    }

    fn union_failure_rows(&mut self, rows: impl IntoIterator<Item = FailureRowId>) -> FailureRowId {
        let mut variants = Vec::new();
        let mut variant_arguments = Vec::new();
        let mut parameters = Vec::new();
        let mut exclusions = HashMap::<SymbolId, HashSet<SymbolId>>::new();
        for row in rows {
            let row = self.failure_rows[row.raw() as usize].clone();
            variants.extend(&row.variants);
            variant_arguments.extend(row.variant_arguments.into_iter().map(|arguments| {
                FailureVariantArguments {
                    variant: arguments.variant,
                    arguments: arguments
                        .arguments
                        .into_iter()
                        .map(|argument| self.resolve_inference_type(argument))
                        .collect(),
                }
            }));
            for parameter in &row.parameters {
                parameters.push(*parameter);
                let row_exclusions = row
                    .exclusions
                    .iter()
                    .find(|exclusion| exclusion.parameter == *parameter)
                    .map_or_else(HashSet::new, |exclusion| {
                        exclusion.variants.iter().copied().collect()
                    });
                exclusions
                    .entry(*parameter)
                    .and_modify(|current| {
                        current.retain(|variant| row_exclusions.contains(variant));
                    })
                    .or_insert(row_exclusions);
            }
        }
        variants.sort_unstable();
        variants.dedup();
        parameters.sort_unstable();
        parameters.dedup();
        let mut exclusions = exclusions
            .into_iter()
            .filter_map(|(parameter, variants)| {
                if variants.is_empty() {
                    return None;
                }
                let mut variants = variants.into_iter().collect::<Vec<_>>();
                variants.sort_unstable();
                Some(FailureRowExclusion {
                    parameter,
                    variants,
                })
            })
            .collect::<Vec<_>>();
        exclusions.sort_by_key(|exclusion| exclusion.parameter);
        self.intern_failure_row(FailureRow {
            variants,
            variant_arguments,
            parameters,
            exclusions,
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
            GenericKind::Value if wanted == GenericKind::FailureRow => GenericKind::FailureSet,
            GenericKind::FailureRow if wanted == GenericKind::Value => GenericKind::FailureSet,
            GenericKind::FailureSet
                if matches!(wanted, GenericKind::Value | GenericKind::FailureRow) =>
            {
                GenericKind::FailureSet
            }
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
        let semantic_id = semantic_id?;
        self.resolved
            .source_symbol(module_id, semantic_id, kind, name)
    }

    fn resolved_type_symbol(
        &self,
        module_id: ModuleId,
        semantic_id: SemanticId,
    ) -> Option<SymbolId> {
        self.resolved
            .reference_resolution(module_id, semantic_id, Namespace::Type)
            .and_then(|resolution| match resolution {
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

    fn intern_failure_row(&mut self, mut value: FailureRow) -> FailureRowId {
        value.variants.sort_unstable();
        value.variants.dedup();
        value
            .variant_arguments
            .retain(|arguments| value.variants.contains(&arguments.variant));
        value.variant_arguments.sort_by(|left, right| {
            left.variant
                .cmp(&right.variant)
                .then_with(|| left.arguments.cmp(&right.arguments))
        });
        value.variant_arguments.dedup();
        value.parameters.sort_unstable();
        value.parameters.dedup();
        value
            .exclusions
            .retain(|exclusion| value.parameters.contains(&exclusion.parameter));
        for exclusion in &mut value.exclusions {
            exclusion.variants.sort_unstable();
            exclusion.variants.dedup();
        }
        value
            .exclusions
            .sort_by_key(|exclusion| exclusion.parameter);
        let mut merged_exclusions = Vec::<FailureRowExclusion>::new();
        for exclusion in value.exclusions {
            if let Some(previous) = merged_exclusions
                .last_mut()
                .filter(|previous| previous.parameter == exclusion.parameter)
            {
                previous.variants.extend(exclusion.variants);
                previous.variants.sort_unstable();
                previous.variants.dedup();
            } else {
                merged_exclusions.push(exclusion);
            }
        }
        value.exclusions = merged_exclusions;
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

const fn diagnostic_phase_order(stage: DiagnosticStage) -> u8 {
    match stage {
        DiagnosticStage::Syntax | DiagnosticStage::Lowering | DiagnosticStage::Name => 0,
        DiagnosticStage::Type => 1,
        DiagnosticStage::ExecutionSetup => 2,
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
                MatchPattern::Constructor { .. } | MatchPattern::List { .. } => None,
            }
        })
        .collect()
}

/// Rebuilds a constructor witness from the first `arity` entries of `witness`.
fn constructor_witness(
    tag: &MatchTag,
    arity: usize,
    mut witness: Vec<MatchPattern>,
) -> Vec<MatchPattern> {
    let tail = witness.split_off(arity.min(witness.len()));
    let mut row = vec![MatchPattern::Constructor {
        tag: tag.clone(),
        fields: witness,
    }];
    row.extend(tail);
    row
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
    expression.range()
}
