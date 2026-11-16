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
use crate::semantic::{BindingKind, CaseOrigin};
use crate::standard::{
    DurationUnitRole, StandardConstant, StandardConstructor, StandardFunction, StandardGeneric,
    StandardGenericKind, StandardSymbol, StandardType,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod clock;
mod constants;
pub(crate) mod durations;
pub(crate) mod failure_sets;
pub(crate) mod process;

mod properties;

pub(crate) use properties::ValueProperty;
use properties::{PropertyLack, TypeView};

/// A property a generic parameter must have, inferred from its uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GenericRequirement {
    pub(crate) property: ValueProperty,
    /// The operator use that created the requirement, even when it reached this
    /// parameter through calls. `None` for a standard function's parameter.
    pub(crate) origin: Option<RequirementOrigin>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequirementOrigin {
    pub(crate) module_id: ModuleId,
    pub(crate) range: TextRange,
    /// What the use at `range` is: an operator such as `==`, or the standard
    /// function, such as `Order.compare`, whose call created the requirement.
    pub(crate) operation: String,
}

/// A property a type must have, checked once the body that needs it has been
/// inferred, so that later unification can still solve the type.
#[derive(Clone, Debug)]
struct PropertyCheck {
    module_id: ModuleId,
    type_id: TypeId,
    property: ValueProperty,
    site: PropertySite,
}

#[derive(Clone, Debug)]
enum PropertySite {
    /// An operand of `==`, `!=`, `<`, `<=`, `>`, or `>=`.
    Operator {
        range: TextRange,
        operator: &'static str,
    },
    /// A call instantiating `parameter`, which carries `requirement`.
    Instantiation {
        range: TextRange,
        callee: String,
        parameter: SymbolId,
        requirement: GenericRequirement,
    },
}

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
            Literal::Boolean { .. } | Literal::Void { .. } => TEXT_TYPE,
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
    /// `(T1, T2, ...)`: two or more elements, structural like a record.
    Tuple(Vec<TypeId>),
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

/// Formats canonical types for contracts, inspection, and diagnostics. It
/// borrows the type tables, so the checker can format while it still owns them.
pub(crate) struct TypeFormatter<'a> {
    resolved: &'a ResolvedProject,
    types: &'a [CanonicalType],
    failure_rows: &'a [FailureRow],
    needs_rows: &'a [NeedsRow],
    generic_requirements: &'a HashMap<SymbolId, GenericRequirement>,
    inference_bindings: &'a HashMap<u32, TypeId>,
}

impl TypeFormatter<'_> {
    pub(crate) fn format_function_contract(&self, contract: &CallableContract) -> String {
        let inputs = contract
            .inputs
            .iter()
            .map(|input| self.format_type(*input))
            .collect::<Vec<_>>()
            .join(", ");
        let mut generics = Vec::new();
        for type_id in contract.inputs.iter().chain([&contract.success]) {
            self.collect_generics(*type_id, &mut generics);
        }
        let requirements = generics
            .into_iter()
            .filter_map(|generic| {
                self.generic_requirements.get(&generic).map(|requirement| {
                    format!(
                        "{}: {}",
                        self.resolved.symbol(generic).name,
                        requirement.property.name()
                    )
                })
            })
            .collect::<Vec<_>>();
        let requirements = if requirements.is_empty() {
            String::new()
        } else {
            format!("<{}>", requirements.join(", "))
        };
        format!(
            "fn{requirements}({inputs}) -> {}",
            self.format_contract_result(contract)
        )
    }

    /// The generic parameters `id` mentions, in first-mention order.
    fn collect_generics(&self, id: TypeId, generics: &mut Vec<SymbolId>) {
        match &self.types[self.resolve_type_id(id).raw() as usize] {
            CanonicalType::Generic(symbol) => {
                if !generics.contains(symbol) {
                    generics.push(*symbol);
                }
            }
            CanonicalType::Named { arguments, .. } => {
                for argument in arguments {
                    self.collect_generics(*argument, generics);
                }
            }
            CanonicalType::Record(fields) => {
                for field in fields {
                    self.collect_generics(field.type_id, generics);
                }
            }
            CanonicalType::Tuple(elements) => {
                for element in elements {
                    self.collect_generics(*element, generics);
                }
            }
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => {
                for input in contract.inputs.iter().chain([&contract.success]) {
                    self.collect_generics(*input, generics);
                }
            }
            CanonicalType::FailureSetValue(_)
            | CanonicalType::InferenceVariable(_)
            | CanonicalType::Poison => {}
        }
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
            CanonicalType::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(|element| self.format_type(*element))
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
        if members.is_empty() {
            // The empty failure set, as in `Task<Int, Never>`.
            "Never".to_owned()
        } else if members.len() == 1 {
            members.pop().expect("one member")
        } else {
            members.join(" | ")
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

#[derive(Debug)]
pub(crate) struct TypedProject {
    pub(crate) resolved: Arc<ResolvedProject>,
    pub(crate) types: Vec<CanonicalType>,
    pub(crate) failure_rows: Vec<FailureRow>,
    pub(crate) needs_rows: Vec<NeedsRow>,
    pub(crate) contracts: HashMap<SymbolId, CallableContract>,
    pub(crate) declaration_types: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) pattern_binding_types: HashMap<(ModuleId, SemanticId), TypeId>,
    /// Each failure-type pattern (`error: FileError` over a failure-set value) by its
    /// semantic id: the variants it covers with their field counts. Core lowering
    /// expands the pattern into one variant test per entry.
    pub(crate) failure_type_patterns: HashMap<(ModuleId, SemanticId), Vec<(SymbolId, usize)>>,
    /// The result shape of each record update, by the update's semantic id.
    pub(crate) record_updates: HashMap<(ModuleId, SemanticId), RecordUpdateShape>,
    /// A polymorphic callee's function type with this call's substitution
    /// applied, keyed by the callee expression. Monomorphic callees have none.
    pub(crate) callee_instantiations: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) type_facts: HashMap<(ModuleId, SemanticId), TypeId>,
    pub(crate) expression_facts: HashMap<(ModuleId, SemanticId), ExpressionFact>,
    pub(crate) expression_value_properties: HashMap<(ModuleId, SemanticId), ValueProperties>,
    pub(crate) fields: HashMap<SymbolId, Vec<FieldShape>>,
    pub(crate) generic_kinds: HashMap<SymbolId, GenericKind>,
    pub(crate) generic_requirements: HashMap<SymbolId, GenericRequirement>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Warnings never block compilation, so they stay out of `diagnostics`.
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) inference_bindings: HashMap<u32, TypeId>,
}

/// The `T4303` help for an arithmetic operator applied to a `Duration`: Chimi has no
/// operator overloading, so `std/duration` provides the operations as functions.
fn duration_operator_help(operator: BinaryOperator) -> String {
    let function = match operator {
        BinaryOperator::Add => "add",
        BinaryOperator::Subtract => "subtract",
        BinaryOperator::Multiply => "multiply",
        BinaryOperator::Divide => "divide",
        _ => return "; `Duration` has no remainder operation".to_owned(),
    };
    format!("; `Duration` arithmetic is a function: use `Duration.{function}`")
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
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
        BinaryOperator::Range => "..",
        BinaryOperator::RangeInclusive => "..=",
    }
}

/// The property a comparison operator needs of its operands, if `operator` is one.
const fn comparison_property(operator: BinaryOperator) -> Option<ValueProperty> {
    match operator {
        BinaryOperator::Equal | BinaryOperator::NotEqual => Some(ValueProperty::Equatable),
        BinaryOperator::Less
        | BinaryOperator::LessEqual
        | BinaryOperator::Greater
        | BinaryOperator::GreaterEqual => Some(ValueProperty::Ordered),
        _ => None,
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
    fn formatter(&self) -> TypeFormatter<'_> {
        TypeFormatter {
            resolved: &self.resolved,
            types: &self.types,
            failure_rows: &self.failure_rows,
            needs_rows: &self.needs_rows,
            generic_requirements: &self.generic_requirements,
            inference_bindings: &self.inference_bindings,
        }
    }

    pub(crate) fn format_function_contract(&self, contract: &CallableContract) -> String {
        self.formatter().format_function_contract(contract)
    }

    pub(crate) fn format_type(&self, id: TypeId) -> String {
        self.formatter().format_type(id)
    }

    pub(crate) fn resolve_type_id(&self, id: TypeId) -> TypeId {
        self.formatter().resolve_type_id(id)
    }

    fn format_failure_row(&self, row: &FailureRow) -> String {
        self.formatter().format_failure_row(row)
    }

    pub(crate) fn format_failure_parameter(&self, row: &FailureRow, symbol: SymbolId) -> String {
        self.formatter().format_failure_parameter(row, symbol)
    }

    fn format_needs_row(&self, row: &NeedsRow) -> String {
        self.formatter().format_needs_row(row)
    }

    fn enum_variants(&self, owner: SymbolId) -> Vec<SymbolId> {
        self.formatter().enum_variants(owner)
    }

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
                .filter(|need| !need.explained && !self.runner_closes(need))
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

    /// Whether the runner's provider plan closes this need on `main`: the
    /// native `Console` and `FileSystem` under any name, and the execution
    /// clock provider for `clock: Clock` (std/clock), whose need name is fixed.
    fn runner_closes(&self, need: &Need) -> bool {
        let CanonicalType::Named { symbol, arguments } = &self.types[need.interface.raw() as usize]
        else {
            return false;
        };
        if !arguments.is_empty() {
            return false;
        }
        match self.resolved.standard_symbol(*symbol) {
            Some(StandardSymbol::Type(StandardType::Console | StandardType::FileSystem)) => true,
            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == "clock",
            _ => false,
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

    /// Whether `id` is equatable or ordered. A generic parameter has the
    /// property when it carries it as an inferred requirement; otherwise the
    /// answer depends on the instantiation.
    pub(crate) fn derived_property(
        &self,
        id: TypeId,
        property: ValueProperty,
    ) -> PropertyDerivation {
        let derivation = TypeView {
            resolved: &self.resolved,
            types: &self.types,
            fields: &self.fields,
            failure_rows: &self.failure_rows,
            inference_bindings: &self.inference_bindings,
        }
        .derive(id, property);
        if derivation.lack.is_some() {
            return PropertyDerivation::Never;
        }
        let required = derivation.generics.iter().all(|generic| {
            self.generic_requirements
                .get(generic)
                .is_some_and(|requirement| requirement.property >= property)
        });
        if derivation.unknown || !required {
            PropertyDerivation::ValueDependent
        } else {
            PropertyDerivation::Always
        }
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

    /// Whether a value of type `container` may hold a value of type `part`: `part`
    /// occurs in `container` through fields, variant fields, elements, and type
    /// arguments. A function, interface, open generic, or unresolved type may hold
    /// anything, since a closure or provider can capture any value. Conservative past
    /// a nesting depth, so a recursive type cannot loop.
    pub(crate) fn type_may_hold(&self, container: TypeId, part: TypeId) -> bool {
        self.type_may_hold_with(container, part, &HashMap::new(), 0)
    }

    fn type_may_hold_with(
        &self,
        container: TypeId,
        part: TypeId,
        generics: &HashMap<SymbolId, TypeId>,
        depth: usize,
    ) -> bool {
        const DEPTH_LIMIT: usize = 16;
        if depth > DEPTH_LIMIT {
            return true;
        }
        let container = self.resolve_type_id(container);
        if let CanonicalType::Generic(symbol) = &self.types[container.raw() as usize]
            && let Some(bound) = generics.get(symbol)
        {
            return self.type_may_hold_with(*bound, part, generics, depth + 1);
        }
        if self.same_type_with(container, part, generics, depth) {
            return true;
        }
        let holds = |id: TypeId, generics: &HashMap<SymbolId, TypeId>| {
            self.type_may_hold_with(id, part, generics, depth + 1)
        };
        match &self.types[container.raw() as usize] {
            CanonicalType::Named { symbol, arguments } => {
                let symbol_kind = self.resolved.symbol(*symbol).kind;
                if symbol_kind == ProjectSymbolKind::Interface
                    || matches!(
                        self.resolved.standard_symbol(*symbol),
                        Some(StandardSymbol::Type(
                            StandardType::Console | StandardType::FileSystem | StandardType::Clock
                        ))
                    )
                {
                    return true;
                }
                let mut inner = generics.clone();
                for (parameter, argument) in self
                    .type_generic_parameters(*symbol)
                    .into_iter()
                    .zip(arguments)
                {
                    inner.insert(parameter, *argument);
                }
                let mut fields = self
                    .fields
                    .get(symbol)
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                if symbol_kind == ProjectSymbolKind::Enum {
                    for variant in self.enum_variants(*symbol) {
                        fields.extend(self.fields.get(&variant).into_iter().flatten());
                    }
                }
                fields.iter().any(|field| holds(field.type_id, &inner))
                    || arguments.iter().any(|argument| holds(*argument, generics))
            }
            CanonicalType::Record(fields) => {
                fields.iter().any(|field| holds(field.type_id, generics))
            }
            CanonicalType::Tuple(elements) => {
                elements.iter().any(|element| holds(*element, generics))
            }
            CanonicalType::Poison => false,
            CanonicalType::Generic(_)
            | CanonicalType::FailureSetValue(_)
            | CanonicalType::Function(_)
            | CanonicalType::PolymorphicFunction { .. }
            | CanonicalType::InferenceVariable(_) => true,
        }
    }

    /// Whether `left`, read with `generics` bound, is the type `right`.
    fn same_type_with(
        &self,
        left: TypeId,
        right: TypeId,
        generics: &HashMap<SymbolId, TypeId>,
        depth: usize,
    ) -> bool {
        const DEPTH_LIMIT: usize = 16;
        if depth > DEPTH_LIMIT {
            return false;
        }
        let left = self.resolve_type_id(left);
        let right = self.resolve_type_id(right);
        if left == right {
            return true;
        }
        let same =
            |left: &TypeId, right: &TypeId| self.same_type_with(*left, *right, generics, depth + 1);
        match (
            &self.types[left.raw() as usize],
            &self.types[right.raw() as usize],
        ) {
            (CanonicalType::Generic(symbol), _) if generics.contains_key(symbol) => {
                same(&generics[symbol], &right)
            }
            (
                CanonicalType::Named { symbol, arguments },
                CanonicalType::Named {
                    symbol: other,
                    arguments: other_arguments,
                },
            ) => {
                symbol == other
                    && arguments.len() == other_arguments.len()
                    && arguments
                        .iter()
                        .zip(other_arguments)
                        .all(|(a, b)| same(a, b))
            }
            (CanonicalType::Tuple(elements), CanonicalType::Tuple(other)) => {
                elements.len() == other.len() && elements.iter().zip(other).all(|(a, b)| same(a, b))
            }
            (CanonicalType::Record(fields), CanonicalType::Record(other)) => {
                fields.len() == other.len()
                    && fields
                        .iter()
                        .zip(other)
                        .all(|(a, b)| a.name == b.name && same(&a.type_id, &b.type_id))
            }
            (CanonicalType::Generic(symbol), CanonicalType::Generic(other)) => symbol == other,
            _ => false,
        }
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
            CanonicalType::Tuple(elements) => {
                elements
                    .iter()
                    .fold(ValueProperties::SHAREABLE, |result, element| {
                        result.combine(self.value_properties_with(*element, generics, visiting))
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
            // The static contract's "Types" table.
            Some(StandardSymbol::Type(StandardType::ProcessInbox)) => {
                return ValueProperties::PARENT_LOCAL;
            }
            Some(StandardSymbol::Type(
                StandardType::SupervisorManagedProcess | StandardType::SupervisorManagedSupervisor,
            )) => {
                return ValueProperties::EXCLUSIVE;
            }
            Some(StandardSymbol::Type(
                StandardType::ProcessAddress
                | StandardType::ProcessSpec
                | StandardType::ProcessMonitor
                | StandardType::ProcessDown
                | StandardType::SupervisorMember
                | StandardType::SupervisorPolicy
                | StandardType::SupervisorRestarts
                | StandardType::SupervisorRestart
                | StandardType::SupervisorRunning
                | StandardType::SupervisorStartError,
            )) => {
                return ValueProperties::SHAREABLE;
            }
            // `Process.SendError<M>` and `Channel.SendError<T>` hand the refused value
            // back, and `Channel.Receive<T>` holds the received value, so each is what
            // its argument is.
            Some(StandardSymbol::Type(
                StandardType::ProcessSendError
                | StandardType::ChannelSendError
                | StandardType::ChannelReceive,
            )) => {
                return arguments
                    .iter()
                    .fold(ValueProperties::SHAREABLE, |result, argument| {
                        result.combine(self.value_properties_with(
                            *argument,
                            outer_generics,
                            visiting,
                        ))
                    });
            }
            _ => {}
        }

        let symbol_kind = self.resolved.symbol(symbol).kind;
        if symbol_kind == ProjectSymbolKind::Interface
            || matches!(
                self.resolved.standard_symbol(symbol),
                Some(StandardSymbol::Type(
                    StandardType::Console | StandardType::FileSystem | StandardType::Clock
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
    failure_type_patterns: HashMap<(ModuleId, SemanticId), Vec<(SymbolId, usize)>>,
    record_updates: HashMap<(ModuleId, SemanticId), RecordUpdateShape>,
    callee_instantiations: HashMap<(ModuleId, SemanticId), TypeId>,
    type_facts: HashMap<(ModuleId, SemanticId), TypeId>,
    expression_facts: HashMap<(ModuleId, SemanticId), ExpressionFact>,
    fields: HashMap<SymbolId, Vec<FieldShape>>,
    generic_kinds: HashMap<SymbolId, GenericKind>,
    generic_requirements: HashMap<SymbolId, GenericRequirement>,
    /// Checks the body being inferred has queued; settled at its end.
    property_checks: Vec<PropertyCheck>,
    /// The innermost expression being inferred, where a generic function value
    /// that meets a function type is instantiated.
    expression_site: Option<(ModuleId, TextRange)>,
    /// Written `Map` key and `Set` item types seen while declarations are
    /// lowered, checked once every field is known; `None` once they have been
    /// checked.
    written_map_keys: Option<Vec<(ModuleId, TextRange, TypeId, &'static str)>>,
    /// The callable each generic function value's parameter list comes from,
    /// to name it in a diagnostic about that instantiation.
    polymorphic_owners: HashMap<Vec<SymbolId>, SymbolId>,
    diagnostics: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    /// Values of `Int` expressions folded from literals, for `T4312`.
    integer_constants: HashMap<(ModuleId, SemanticId), i64>,
    next_inference_variable: u32,
    inference_bindings: HashMap<u32, TypeId>,
    /// The type every `break` of a loop must supply; it is also the loop's success type.
    /// A loop no break targets keeps an unconstrained variable, so it fits any context.
    loop_break_types: HashMap<(ModuleId, SemanticId), TypeId>,
    /// Loops some `break` leaves, with the first such `break`; any other loop has
    /// type `Never`.
    broken_loops: HashMap<(ModuleId, SemanticId), TextRange>,
    /// Each `case` arm body's success type, so `T4308` can find the arm that
    /// produces a value.
    arm_types: HashMap<(ModuleId, SemanticId), TypeId>,
    /// Constants whose latest body pass left an inference variable unbound (`T4324`).
    undetermined_constants: HashSet<(ModuleId, SemanticId)>,
    /// How many function input types the current `unify` or `match_call_type`
    /// is inside. A function input is contravariant, so there `Never` matches
    /// only `Never`: a `fn(Never)` must not pass for a `fn(Int)`.
    function_input_depth: u32,
    /// Set just before lowering an interface member's declared type, the one
    /// place a generic function type `fn<A>(..)` may appear; any nested type
    /// lowers with it cleared.
    generic_function_type_allowed: bool,
    /// Inference variables some check has already used as the narrower side.
    /// Such a variable is bounded above, so widening its failure set by union
    /// would leave that check stale; it may only stay as wide as it is.
    bounded_inference_variables: HashSet<u32>,
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
    /// Row parameters bound so far only by the row of a function argument. Such a
    /// binding is a lower bound: a later function argument widens it to the union,
    /// and any exact use fixes it.
    widenable_failures: HashSet<SymbolId>,
    widenable_needs: HashSet<SymbolId>,
}

/// What a record update builds: the struct constructor, or `None` for a record or
/// structural interface value, and every field of the base's type in its order,
/// with its type and the index of the update entry that replaces it, if any.
#[derive(Clone, Debug)]
pub(crate) struct RecordUpdateShape {
    pub(crate) constructor: Option<SymbolId>,
    pub(crate) fields: Vec<(String, TypeId, Option<usize>)>,
}

/// What pattern checking needs besides the expected type.
#[derive(Clone, Copy)]
struct PatternContext<'a> {
    generics: &'a HashMap<String, SymbolId>,
    /// The pattern is (part of) a function clause's parameter pattern, so type
    /// conflicts are between clauses (`T4281`) and generic positions are opaque.
    clause_head: bool,
    /// The pattern sits inside a constructor, list, or `Success` pattern, where
    /// a failure-type pattern cannot select variants: only a whole subject or a
    /// `Failure` payload is expanded into variant tests.
    nested: bool,
}

/// What an ordinary `case` was desugared from, when it was not written as one.
enum CaseSite {
    /// The clauses of `function`; `range` is the first clause's name, where coverage
    /// diagnostics point. `separated`: another group of the same name exists, so
    /// coverage is not checked.
    Clauses {
        function: String,
        range: TextRange,
        separated: bool,
    },
    /// A destructuring `let` or `loop` state, whose one pattern must be irrefutable.
    Binding { kind: BindingKind },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum MatchTag {
    /// An enum variant, or the one constructor of a struct.
    EnumVariant(SymbolId),
    Boolean(bool),
    Integer(i64),
    Text(String),
    /// The one shape of a tuple of this arity.
    Tuple(usize),
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
            failure_type_patterns: HashMap::new(),
            record_updates: HashMap::new(),
            callee_instantiations: HashMap::new(),
            type_facts: HashMap::new(),
            expression_facts: HashMap::new(),
            fields: HashMap::new(),
            generic_kinds: HashMap::new(),
            generic_requirements: HashMap::new(),
            property_checks: Vec::new(),
            expression_site: None,
            written_map_keys: Some(Vec::new()),
            polymorphic_owners: HashMap::new(),
            diagnostics,
            warnings: Vec::new(),
            integer_constants: HashMap::new(),
            next_inference_variable: 0,
            inference_bindings: HashMap::new(),
            loop_break_types: HashMap::new(),
            broken_loops: HashMap::new(),
            arm_types: HashMap::new(),
            undetermined_constants: HashSet::new(),
            function_input_depth: 0,
            generic_function_type_allowed: false,
            bounded_inference_variables: HashSet::new(),
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

        self.index_process_contracts();
        self.index_clock_contracts();
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
        let console_failure = self.standard_failure_row(&[StandardConstructor::ConsoleWriteFailed]);
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
        let float = self.builtin_type("Float");
        let range = self.builtin_type("Range");
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
            (StandardFunction::RangeStart, vec![range], int, pure.0),
            (StandardFunction::RangeEnd, vec![range], int, pure.0),
            (StandardFunction::RangeLength, vec![range], int, pure.0),
            (
                StandardFunction::RangeContains,
                vec![range, int],
                self.builtin_type("Bool"),
                pure.0,
            ),
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
        let ordering = self.standard_named_type(StandardType::Ordering, Vec::new());
        for (function, generic, returns_ordering) in [
            (
                StandardFunction::OrderCompare,
                StandardGeneric::OrderCompareValue,
                true,
            ),
            (
                StandardFunction::OrderMin,
                StandardGeneric::OrderMinValue,
                false,
            ),
            (
                StandardFunction::OrderMax,
                StandardGeneric::OrderMaxValue,
                false,
            ),
        ] {
            let value = self.standard_generic_type(generic);
            let symbol = self.standard_function_symbol(function);
            self.contracts.insert(
                symbol,
                CallableContract {
                    inputs: vec![value, value],
                    success: if returns_ordering { ordering } else { value },
                    failures: pure.0,
                    needs: pure.1,
                },
            );
            self.generic_requirements.insert(
                self.resolved.standard_generic(generic),
                GenericRequirement {
                    property: ValueProperty::Ordered,
                    origin: None,
                },
            );
        }
        let duration = self.standard_named_type(StandardType::Duration, Vec::new());
        let duration_parse_failure = self.standard_failure_row(&[
            StandardConstructor::DurationParseEmpty,
            StandardConstructor::DurationParseInvalid,
            StandardConstructor::DurationParseOutOfRange,
        ]);
        let mut duration_contracts = vec![
            (
                StandardFunction::DurationAdd,
                vec![duration, duration],
                duration,
                pure.0,
            ),
            (
                StandardFunction::DurationSubtract,
                vec![duration, duration],
                duration,
                pure.0,
            ),
            (
                StandardFunction::DurationMultiply,
                vec![duration, int],
                duration,
                pure.0,
            ),
            (
                StandardFunction::DurationDivide,
                vec![duration, int],
                duration,
                pure.0,
            ),
            (
                StandardFunction::DurationNegate,
                vec![duration],
                duration,
                pure.0,
            ),
            (
                StandardFunction::DurationAbsolute,
                vec![duration],
                duration,
                pure.0,
            ),
            (
                StandardFunction::DurationToText,
                vec![duration],
                text,
                pure.0,
            ),
            (
                StandardFunction::DurationParse,
                vec![text],
                duration,
                duration_parse_failure,
            ),
        ];
        for function in StandardFunction::ALL {
            match function.duration_unit() {
                Some((_, DurationUnitRole::Constructor)) => {
                    duration_contracts.push((function, vec![int], duration, pure.0));
                }
                Some((_, DurationUnitRole::Conversion)) => {
                    duration_contracts.push((function, vec![duration], int, pure.0));
                }
                None => {}
            }
        }
        for (function, inputs, success, failures) in
            numeric_contracts.into_iter().chain(duration_contracts)
        {
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
            if self
                .resolved
                .loaded_standard_type(constructor.type_kind())
                .is_none()
            {
                continue;
            }
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

    /// `std/decimal.Decimal`, the one library type the operator rules name, when
    /// the program loads `std/decimal`; no value has the type otherwise.
    fn decimal_type(&mut self) -> Option<TypeId> {
        let symbol = self.resolved.loaded_standard_type(StandardType::Decimal)?;
        Some(self.intern_type(CanonicalType::Named {
            symbol,
            arguments: Vec::new(),
        }))
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

    /// Requires: `std/json` is loaded, so `Value`, `Number`, and `Map` resolve.
    fn json_value_constructor_shape(
        &mut self,
        constructor: StandardConstructor,
    ) -> (Vec<TypeId>, TypeId) {
        let value = self.standard_named_type(StandardType::JsonValue, Vec::new());
        let inputs = match constructor {
            StandardConstructor::JsonBoolean => vec![self.builtin_type("Bool")],
            StandardConstructor::JsonNumber => {
                vec![self.standard_named_type(StandardType::JsonNumber, Vec::new())]
            }
            StandardConstructor::JsonText => vec![self.builtin_type("Text")],
            StandardConstructor::JsonArray => {
                vec![self.builtin_type_with_arguments("List", vec![value])]
            }
            StandardConstructor::JsonObject => {
                let text = self.builtin_type("Text");
                vec![self.standard_named_type(StandardType::Map, vec![text, value])]
            }
            StandardConstructor::JsonNull => Vec::new(),
            other => unreachable!("{other:?} is not a `Json.Value` constructor"),
        };
        (inputs, value)
    }

    fn standard_constructor_shape(
        &mut self,
        constructor: StandardConstructor,
    ) -> (Vec<TypeId>, TypeId) {
        let text = self.builtin_type("Text");
        let int = self.builtin_type("Int");
        match constructor {
            StandardConstructor::ProcessSendFull
            | StandardConstructor::ProcessSendUnavailable
            | StandardConstructor::RestartNever
            | StandardConstructor::RestartOnDefect
            | StandardConstructor::StartCapacityReached
            | StandardConstructor::StartNameTaken
            | StandardConstructor::StartStopped
            | StandardConstructor::DownReturned
            | StandardConstructor::DownFailed
            | StandardConstructor::DownDefected
            | StandardConstructor::DownStopped
            | StandardConstructor::DownUnavailable => self.process_constructor_shape(constructor),
            StandardConstructor::TimeoutTimedOut => self.timeout_constructor_shape(),
            StandardConstructor::DurationParseEmpty
            | StandardConstructor::DurationParseInvalid
            | StandardConstructor::DurationParseOutOfRange => (
                Vec::new(),
                self.standard_named_type(StandardType::DurationParseError, Vec::new()),
            ),
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
            StandardConstructor::JsonNull
            | StandardConstructor::JsonBoolean
            | StandardConstructor::JsonNumber
            | StandardConstructor::JsonText
            | StandardConstructor::JsonArray
            | StandardConstructor::JsonObject => self.json_value_constructor_shape(constructor),
            StandardConstructor::DecodeInvalidJson => (
                vec![text, int],
                self.standard_named_type(StandardType::DecodeError, Vec::new()),
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
            StandardConstructor::JsonNotInteger
            | StandardConstructor::JsonOutOfRange
            | StandardConstructor::IntParseEmpty
            | StandardConstructor::IntParseInvalidDigit
            | StandardConstructor::IntParseOutOfRange
            | StandardConstructor::FloatParseEmpty
            | StandardConstructor::FloatParseInvalid
            | StandardConstructor::FloatParseOutOfRange
            | StandardConstructor::FloatNotInteger
            | StandardConstructor::FloatOutOfRange
            | StandardConstructor::OrderingLess
            | StandardConstructor::OrderingEqual
            | StandardConstructor::OrderingGreater => (
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
            failure_type_patterns: previous
                .failure_type_patterns
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, value.clone()))
                .collect(),
            record_updates: previous
                .record_updates
                .iter()
                .filter(|((module_id, _), _)| !affected_modules.contains(module_id))
                .map(|(key, value)| (*key, value.clone()))
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
            generic_requirements: previous
                .generic_requirements
                .iter()
                .filter(|(symbol, _)| !affected_symbols.contains(symbol))
                .map(|(symbol, requirement)| (*symbol, requirement.clone()))
                .collect(),
            property_checks: Vec::new(),
            expression_site: None,
            written_map_keys: Some(Vec::new()),
            polymorphic_owners: HashMap::new(),
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
            broken_loops: HashMap::new(),
            arm_types: HashMap::new(),
            undetermined_constants: HashSet::new(),
            function_input_depth: 0,
            generic_function_type_allowed: false,
            bounded_inference_variables: HashSet::new(),
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
        for (module_id, range, key, role) in self.written_map_keys.take().unwrap_or_default() {
            self.check_written_map_key(module_id, range, key, role);
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
        // Body diagnostics come from the settled pass only. An earlier pass can
        // see a callee declared later whose inferred contract is still open
        // (`konst(3)(True)` before `fn konst`), so its errors may not hold.
        let (diagnostics_before_bodies, warnings_before_bodies) =
            (self.diagnostics.len(), self.warnings.len());
        for pass in 0..=declaration_count {
            self.diagnostics.truncate(diagnostics_before_bodies);
            self.warnings.truncate(warnings_before_bodies);
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
            failure_type_patterns: self.failure_type_patterns,
            record_updates: self.record_updates,
            callee_instantiations: self.callee_instantiations,
            type_facts: self.type_facts,
            expression_facts: self.expression_facts,
            expression_value_properties: HashMap::new(),
            fields: self.fields,
            generic_kinds: self.generic_kinds,
            generic_requirements: self.generic_requirements,
            diagnostics: self.diagnostics,
            warnings: self.warnings,
            inference_bindings: self.inference_bindings,
        };
        let constant_diagnostics =
            constants::check(&typed, body_modules, &self.undetermined_constants);
        typed.diagnostics.extend(constant_diagnostics);
        let (duration_errors, duration_warnings) = durations::check(&typed, body_modules);
        typed.diagnostics.extend(duration_errors);
        let process_errors = process::check(&typed, body_modules);
        typed.diagnostics.extend(process_errors);
        typed.warnings.extend(duration_warnings);
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
            | ModuleDeclarationKind::Interface
            | ModuleDeclarationKind::NativeType => {
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
                    .map(|field| {
                        self.generic_function_type_allowed =
                            declaration.kind == ModuleDeclarationKind::Interface;
                        FieldShape {
                            semantic_id: field.semantic_id,
                            name: field.name.text.clone(),
                            type_id: self.lower_value_type(
                                module_id,
                                &field.type_use,
                                &generic_symbols,
                            ),
                        }
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
                    ) && self.resolved.standard_symbol(constructor).is_none()
                    {
                        // A runtime-known variant keeps its registry contract, which
                        // every standard signature shares.
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
                self.add_stated_requirements(module_id, declaration, &generic_symbols);
            }
            ModuleDeclarationKind::Constant => {
                if let Some(annotation) = &declaration.annotation {
                    let declared = self.lower_value_type(module_id, annotation, &generic_symbols);
                    self.declaration_types
                        .insert((module_id, declaration.semantic_id), declared);
                }
            }
            ModuleDeclarationKind::ProcessName => {
                if let Some(annotation) = &declaration.annotation {
                    let message = self.lower_value_type(module_id, annotation, &generic_symbols);
                    let address = self.process_name_type(message);
                    self.declaration_types
                        .insert((module_id, declaration.semantic_id), address);
                }
            }
        }
    }

    /// Gives a standard function the generic requirements the registry states
    /// for it (`operations::STATED_REQUIREMENTS`), as an intrinsic's are: with
    /// no origin, so a lack is reported at the caller's call.
    fn add_stated_requirements(
        &mut self,
        module_id: ModuleId,
        declaration: &ModuleDeclaration,
        generic_symbols: &HashMap<String, SymbolId>,
    ) {
        if !self.resolved.is_standard_module(module_id) {
            return;
        }
        let module_path = &self.resolved.semantic.modules[module_id.raw() as usize].path;
        let id = format!("{module_path}.{}", declaration.name.text);
        for stated in crate::operations::stated_requirements(&id) {
            let generic = generic_symbols
                .get(stated.generic)
                .copied()
                .unwrap_or_else(|| panic!("{id} declares no generic {}", stated.generic));
            self.generic_requirements.insert(
                generic,
                GenericRequirement {
                    property: stated.property,
                    origin: None,
                },
            );
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
                self.property_checks.clear();
                let body_fact =
                    self.infer_expression(module_id, body, &generic_symbols, &mut bindings);
                if signature.success.is_some()
                    && self.is_never(contract.success)
                    && !self.is_never(body_fact.success)
                {
                    self.type_error_range(
                        module_id,
                        "T4308",
                        self.value_producing_range(module_id, body),
                        "this function is declared `-> Never`, but this expression produces a value",
                    );
                } else {
                    self.unify_expression_result(
                        module_id,
                        body,
                        body_fact.success,
                        contract.success,
                    );
                }
                let requirements_changed = self.settle_property_checks(&generic_symbols);
                let mut inferred = contract;
                inferred.success = self.resolve_inference_type(inferred.success);
                inferred.failures =
                    self.union_failure_rows([inferred.failures, body_fact.failures]);
                inferred.needs = self.union_needs_rows([inferred.needs, body_fact.needs]);
                inferred.failures = self.close_failure_row(inferred.failures);
                inferred.needs = self.close_needs_row(inferred.needs);
                self.contracts.insert(function_symbol, inferred);
                self.contracts[&function_symbol] != previous || requirements_changed
            }
            ModuleDeclarationKind::Constant => {
                let mut bindings = HashMap::new();
                self.property_checks.clear();
                let fact = self.infer_expression(module_id, body, &generic_symbols, &mut bindings);
                let Some(symbol) = self.declaration_symbol(module_id, declaration) else {
                    return false;
                };
                let key = (module_id, declaration.semantic_id);
                let success = if let Some(annotation) = &declaration.annotation {
                    let declared = self.lower_value_type(module_id, annotation, &generic_symbols);
                    self.unify_expression_result(module_id, body, fact.success, declared);
                    declared
                } else {
                    fact.success
                };
                self.settle_property_checks(&generic_symbols);
                // A constant is not generic: only its own initializer and annotation may
                // determine its type, so a use elsewhere must not bind what is left open.
                let success = self.resolve_inference_type(success);
                if self.type_is_determined(success) {
                    self.undetermined_constants.remove(&key);
                } else {
                    self.undetermined_constants.insert(key);
                }
                let success = self.close_inferred_type(success);
                let changed = self
                    .declaration_types
                    .get(&key)
                    .is_none_or(|previous| !self.types_equivalent(*previous, success));
                self.declaration_types.insert(key, success);
                self.contracts.remove(&symbol);
                changed
            }
            ModuleDeclarationKind::Struct
            | ModuleDeclarationKind::Enum
            | ModuleDeclarationKind::Interface
            | ModuleDeclarationKind::NativeType
            | ModuleDeclarationKind::ProcessName => false,
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
            CanonicalType::Tuple(elements) => {
                let elements = elements
                    .into_iter()
                    .map(|element| self.close_inferred_type(element))
                    .collect();
                self.intern_type(CanonicalType::Tuple(elements))
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

    /// `contract` with each unbound inference variable replaced by a fresh one,
    /// the same fresh variable for every occurrence of one variable (`opened`).
    fn instantiate_open_contract(
        &mut self,
        contract: CallableContract,
        opened: &mut HashMap<TypeId, TypeId>,
    ) -> CallableContract {
        let failures = {
            let mut row = self.failure_rows[contract.failures.raw() as usize].clone();
            for arguments in &mut row.variant_arguments {
                for argument in &mut arguments.arguments {
                    *argument = self.instantiate_open_type(*argument, opened);
                }
            }
            self.intern_failure_row(row)
        };
        let needs = {
            let row = self.needs_rows[contract.needs.raw() as usize].clone();
            let entries = row
                .entries
                .into_iter()
                .map(|need| Need {
                    interface: self.instantiate_open_type(need.interface, opened),
                    ..need
                })
                .collect();
            self.intern_needs_row(NeedsRow {
                entries,
                parameters: row.parameters,
            })
        };
        CallableContract {
            inputs: contract
                .inputs
                .into_iter()
                .map(|input| self.instantiate_open_type(input, opened))
                .collect(),
            success: self.instantiate_open_type(contract.success, opened),
            failures,
            needs,
        }
    }

    fn instantiate_open_type(
        &mut self,
        type_id: TypeId,
        opened: &mut HashMap<TypeId, TypeId>,
    ) -> TypeId {
        let type_id = self.resolve_inference_type(type_id);
        match self.types[type_id.raw() as usize].clone() {
            CanonicalType::InferenceVariable(_) => {
                if let Some(fresh) = opened.get(&type_id) {
                    *fresh
                } else {
                    let fresh = self.fresh_inference_type();
                    opened.insert(type_id, fresh);
                    fresh
                }
            }
            CanonicalType::Named { symbol, arguments } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.instantiate_open_type(argument, opened))
                    .collect();
                self.intern_type(CanonicalType::Named { symbol, arguments })
            }
            CanonicalType::Record(fields) => {
                let fields = fields
                    .into_iter()
                    .map(|field| StructuralField {
                        name: field.name,
                        type_id: self.instantiate_open_type(field.type_id, opened),
                    })
                    .collect();
                self.intern_type(CanonicalType::Record(fields))
            }
            CanonicalType::Tuple(elements) => {
                let elements = elements
                    .into_iter()
                    .map(|element| self.instantiate_open_type(element, opened))
                    .collect();
                self.intern_type(CanonicalType::Tuple(elements))
            }
            CanonicalType::Function(contract) => {
                let contract = self.instantiate_open_contract(contract, opened);
                self.intern_type(CanonicalType::Function(contract))
            }
            CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            } => {
                let contract = self.instantiate_open_contract(contract, opened);
                self.intern_type(CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                })
            }
            CanonicalType::FailureSetValue(_)
            | CanonicalType::Generic(_)
            | CanonicalType::Poison => type_id,
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
        let failures = signature
            .failures
            .as_ref()
            .map_or(FailureRowId::new(0), |failures| {
                self.lower_failure_row(module_id, failures, generics)
            });
        let needs = self.lower_needs_row(module_id, &signature.requirements, generics);
        CallableContract {
            inputs,
            success,
            failures,
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
        let outer = self
            .expression_site
            .replace((module_id, expression.range()));
        let fact = self.infer_expression_at_site(module_id, expression, generics, bindings);
        self.expression_site = outer;
        fact
    }

    fn infer_expression_at_site(
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
                    Literal::Void { .. } => "Void",
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
                } else if let Some(type_id) = self
                    .symbol_value_type(symbol)
                    .or_else(|| self.standard_constant_type(symbol))
                {
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
                        .instantiated_field_type(receiver.success, &member.text)
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
                    let expected_callee = if labelled_order.is_none() {
                        callee.success
                    } else {
                        TypeId::new(0)
                    };
                    let mut argument_facts = self.infer_call_arguments(
                        module_id,
                        expected_callee,
                        arguments,
                        generics,
                        bindings,
                    );
                    let mut argument_ranges = arguments
                        .iter()
                        .map(|argument| expression_range(&argument.value))
                        .collect::<Vec<_>>();
                    if let Some(order) = labelled_order {
                        argument_facts = order.iter().map(|index| argument_facts[*index]).collect();
                        argument_ranges = order
                            .into_iter()
                            .map(|index| argument_ranges[index])
                            .collect();
                    }
                    self.infer_invocation(
                        module_id,
                        *range,
                        callee_id,
                        callee_symbol,
                        callee,
                        &argument_facts,
                        &argument_ranges,
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
                let mut element_type = facts
                    .first()
                    .map_or_else(|| self.fresh_inference_type(), |fact| fact.success);
                for fact in facts.iter().skip(1) {
                    element_type = self.join_branch_type(
                        module_id,
                        *range,
                        fact.success,
                        element_type,
                        "list element",
                    );
                    if self.is_never(element_type) {
                        element_type = fact.success;
                    }
                }
                ExpressionFact {
                    success: self.builtin_type_with_arguments("List", vec![element_type]),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Tuple { elements, .. } => {
                let facts = elements
                    .iter()
                    .map(|element| self.infer_expression(module_id, element, generics, bindings))
                    .collect::<Vec<_>>();
                ExpressionFact {
                    success: self.intern_type(CanonicalType::Tuple(
                        facts.iter().map(|fact| fact.success).collect(),
                    )),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Interpolation { parts, .. } => {
                let facts = crate::semantic::interpolated_expressions(parts)
                    .map(|part| {
                        let fact = self.infer_expression(module_id, part, generics, bindings);
                        self.check_interpolated_type(module_id, part, fact.success);
                        fact
                    })
                    .collect::<Vec<_>>();
                ExpressionFact {
                    success: self.builtin_type("Text"),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
                }
            }
            ModuleExpression::Update {
                semantic_id,
                base,
                fields,
                ..
            } => self.infer_update(module_id, *semantic_id, base, fields, generics, bindings),
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
                let int = self.builtin_type("Int");
                let operand_type = self.resolve_inference_type(fact.success);
                let decimal = self.decimal_type();
                let known_other = operand_type != int
                    && operand_type != float
                    && Some(operand_type) != decimal
                    && operand_type != TypeId::new(0)
                    && !matches!(
                        self.types[operand_type.raw() as usize],
                        CanonicalType::InferenceVariable(_)
                    );
                let success = if operand_type == float || Some(operand_type) == decimal {
                    operand_type
                } else if known_other {
                    let duration = self.standard_named_type(StandardType::Duration, Vec::new());
                    let help = if operand_type == duration {
                        "; negate a `Duration` with `Duration.negate`"
                    } else {
                        ""
                    };
                    let found = self.type_display_name(operand_type);
                    self.type_error_range(
                        module_id,
                        "T4303",
                        *range,
                        format!(
                            "unary `-` needs an `Int`, `Float`, or `Decimal` operand, found `{found}`{help}"
                        ),
                    );
                    TypeId::new(0)
                } else {
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
                operator: operator @ (BinaryOperator::Range | BinaryOperator::RangeInclusive),
                left: left_operand,
                right: right_operand,
                range,
                ..
            } => {
                let int = self.builtin_type("Int");
                let mut facts = Vec::new();
                for operand in [left_operand, right_operand] {
                    let fact = self.infer_expression(module_id, operand, generics, bindings);
                    if !self.unify(fact.success, int) {
                        let found = self.format_checked_type(fact.success);
                        self.type_error_range(
                            module_id,
                            "T4311",
                            expression_range(operand),
                            format!(
                                "a range bound must be `Int`, but this is `{found}`; `{}` builds integer ranges only",
                                binary_operator_symbol(*operator)
                            ),
                        );
                    }
                    facts.push(fact);
                }
                // A constant `start..=end` normalizes to `start..end + 1`; at the
                // largest `Int` that `+ 1` always defects (`T4312`).
                if *operator == BinaryOperator::RangeInclusive
                    && self.integer_constant(module_id, left_operand).is_some()
                    && self.integer_constant(module_id, right_operand) == Some(i64::MAX)
                {
                    self.always_defects_warning(module_id, *range);
                }
                ExpressionFact {
                    success: self.builtin_type("Range"),
                    failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
                    needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
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
                if let Some(property) = comparison_property(*operator) {
                    // A comparison operand that is itself an ungrouped
                    // comparison only comes from a chain `a < b < c`, which
                    // the parser reported as `S1006`; the `Bool` the chain
                    // pairs with `c` is not a second error. A grouped
                    // `(a < b) == c` lowers to `Group` and is still checked.
                    let chained = matches!(
                        left_operand.as_ref(),
                        ModuleExpression::Binary { operator: inner, .. }
                            if comparison_property(*inner).is_some()
                    );
                    if !chained {
                        self.infer_comparison(module_id, *operator, property, *range, left, right);
                    }
                    ExpressionFact {
                        success: self.builtin_type("Bool"),
                        failures: self.union_failure_rows([left.failures, right.failures]),
                        needs: self.union_needs_rows([left.needs, right.needs]),
                    }
                } else {
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
                        success: operand_type,
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
                        ModuleBlockItem::Binding {
                            name,
                            type_use,
                            value,
                        } => {
                            let value_fact = self.infer_expression(
                                module_id,
                                value,
                                generics,
                                &mut block_bindings,
                            );
                            // An annotation states the binding's type; the value must have it.
                            let binding_type = match type_use {
                                Some(type_use) => {
                                    let annotated =
                                        self.lower_value_type(module_id, type_use, generics);
                                    self.unify_at(
                                        module_id,
                                        expression_range(value),
                                        value_fact.success,
                                        annotated,
                                        "the value of this annotated `let`",
                                    );
                                    annotated
                                }
                                None => value_fact.success,
                            };
                            if let Some(symbol) = self.find_symbol(
                                module_id,
                                Some(value.semantic_id()),
                                ProjectSymbolKind::LocalBinding,
                                &name.text,
                            ) {
                                block_bindings.insert(
                                    symbol,
                                    ValueBinding {
                                        type_id: binding_type,
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
                origin,
                ..
            } => self.infer_case(
                module_id, subjects, arms, *range, origin, generics, bindings,
            ),
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

        if let Some(fact) = facts.first() {
            self.promote_raised_generic(fact.success, generics);
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
            success: self.builtin_type("Never"),
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
        if !self.unify(body_fact.success, initial_fact.success) {
            let (site, found) =
                self.mismatched_tail(module_id, body, body_fact.success, initial_fact.success);
            let message =
                self.mismatch_message("the loop body's next state", found, initial_fact.success);
            self.type_error_range(module_id, "T4203", site, message);
        }
        // A loop that no `break` leaves never produces a value.
        let success = if self.broken_loops.contains_key(&(module_id, loop_id)) {
            break_type
        } else {
            self.builtin_type("Never")
        };
        ExpressionFact {
            success,
            failures: self.union_failure_rows([initial_fact.failures, body_fact.failures]),
            needs: self.union_needs_rows([initial_fact.needs, body_fact.needs]),
        }
    }

    /// Ensures: the value (or `Void`) unifies with the target loop's break type; the
    /// break's own success type is `Never` because control never continues.
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
        let target = self.resolved.break_target(module_id, break_id);
        if let Some(loop_id) = target {
            self.broken_loops
                .entry((module_id, loop_id))
                .or_insert(range);
        }
        let break_type =
            target.and_then(|loop_id| self.loop_break_types.get(&(module_id, loop_id)).copied());
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
            success: self.builtin_type("Never"),
            ..value_fact
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_case(
        &mut self,
        module_id: ModuleId,
        subjects: &[ModuleExpression],
        arms: &[ModuleCaseArm],
        range: TextRange,
        origin: &CaseOrigin,
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let mut subject_facts = subjects
            .iter()
            .map(|subject| self.infer_expression(module_id, subject, generics, bindings))
            .collect::<Vec<_>>();
        // `let _: T = value` is an annotated `let` that binds no name, so a mismatch
        // is reported like `let name: T = value`, at the value. The subject then
        // takes the annotated type, so the pattern adds no second diagnostic.
        if let (
            CaseOrigin::Binding {
                kind: BindingKind::Let,
            },
            [arm],
            [subject],
            [fact],
        ) = (origin, arms, subjects, subject_facts.as_mut_slice())
            && let [
                Pattern::Typed {
                    name: None,
                    type_use,
                    ..
                },
            ] = arm.patterns.as_slice()
        {
            let annotated = self.lower_value_type(module_id, type_use, generics);
            if !self.unify_at(
                module_id,
                expression_range(subject),
                fact.success,
                annotated,
                "the value of this annotated `let`",
            ) {
                fact.success = annotated;
            }
        }
        let site = match origin {
            CaseOrigin::Clauses {
                function,
                separated,
            } => Some(CaseSite::Clauses {
                function: function.text.clone(),
                range: function.range,
                separated: *separated,
            }),
            CaseOrigin::Binding { kind } => Some(CaseSite::Binding { kind: *kind }),
            CaseOrigin::Source => None,
        };
        if let Some(site) = site {
            return self.infer_ordinary_case(
                module_id,
                &subject_facts,
                arms,
                range,
                Some(&site),
                generics,
                bindings,
            );
        }
        let outcome_columns = (0..subjects.len())
            .map(|index| {
                arms.iter().any(|arm| {
                    arm.patterns.get(index).is_some_and(|pattern| {
                        self.pattern_is_builtin(module_id, pattern, "Success")
                            || self.pattern_is_builtin(module_id, pattern, "Failure")
                    })
                })
            })
            .collect::<Vec<_>>();
        if outcome_columns.contains(&true) {
            self.infer_outcome_case(
                module_id,
                &subject_facts,
                &outcome_columns,
                arms,
                range,
                generics,
                bindings,
            )
        } else {
            if subjects.len() == 1 && !self.failure_row_is_empty(subject_facts[0].failures) {
                let failures = self.formatter().format_failure_row(
                    &self.failure_rows[subject_facts[0].failures.raw() as usize],
                );
                self.type_error_range(
                    module_id,
                    "T4222",
                    range,
                    format!(
                        "this case's subject may fail with `{failures}`, so its arms must be \
                         Success(...) and Failure(...) patterns; to match the success value \
                         and let the failure propagate, bind it first (`let next = ...`) \
                         and write `case next`"
                    ),
                );
            }
            self.infer_ordinary_case(
                module_id,
                &subject_facts,
                arms,
                range,
                None,
                generics,
                bindings,
            )
        }
    }

    /// Types an ordinary `case`, or a clause group's body or a destructuring binding when
    /// `site` is set. The rules are the same; those sites get their own diagnostics, and
    /// a `Success` or `Failure` pattern there is `T4282` because they match values.
    #[allow(clippy::too_many_arguments)]
    fn infer_ordinary_case(
        &mut self,
        module_id: ModuleId,
        subjects: &[ExpressionFact],
        arms: &[ModuleCaseArm],
        range: TextRange,
        site: Option<&CaseSite>,
        generics: &HashMap<String, SymbolId>,
        bindings: &HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let clauses = matches!(site, Some(CaseSite::Clauses { .. }));
        let context = PatternContext {
            generics,
            clause_head: clauses,
            nested: false,
        };
        let mut facts = subjects.to_vec();
        let mut success = None;
        let subject_types = subjects
            .iter()
            .map(|subject| subject.success)
            .collect::<Vec<_>>();
        let mut coverage = Vec::<Vec<MatchPattern>>::new();
        if clauses {
            self.seed_generic_clause_parameters(module_id, subjects, arms, generics);
        }
        // After each arm that added coverage: its index and the coverage length.
        let mut coverage_ends = Vec::<(usize, usize)>::new();
        // An arm whose pattern lost a `..` to L2021 does not mean what was written, so
        // it is neither checked for reachability nor counted as covering anything,
        // and with such an arm the case is not checked for exhaustiveness either.
        let mut rejected_arm = false;
        for (arm_index, arm) in arms.iter().enumerate() {
            let mut arm_bindings = bindings.clone();
            // A clause parameter written as an outcome (`T4282`) is rejected the same way.
            let outcome_parameters = arm
                .patterns
                .iter()
                .map(|pattern| {
                    site.is_some_and(|site| self.reject_outcome_parameter(module_id, pattern, site))
                })
                .collect::<Vec<_>>();
            // So is a clause pattern that inspects a generic value (`T4230` below).
            let inspects_generic = clauses
                && arm
                    .patterns
                    .iter()
                    .zip(subjects)
                    .any(|(pattern, subject)| self.inspects_generic(pattern, subject.success));
            // And so is an arm naming a constructor that did not resolve: name
            // resolution reported it, and what it would cover is unknown.
            let rejected = arm.patterns.iter().any(Pattern::has_rejected_rest)
                || outcome_parameters.contains(&true)
                || inspects_generic
                || arm
                    .patterns
                    .iter()
                    .any(|pattern| self.has_unresolved_constructor(module_id, pattern));
            rejected_arm |= rejected;
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
                .zip(&outcome_parameters)
                .map(|((pattern, subject), outcome)| {
                    if *outcome {
                        return MatchPattern::Wildcard;
                    }
                    self.normalize_pattern(
                        module_id,
                        pattern,
                        subject.success,
                        context,
                        &mut arm_bindings,
                    )
                })
                .collect::<Vec<_>>();
            let rows = self.expand_failure_type_rows(module_id, &arm.patterns, normalized);
            let normalized = rows[0].clone();
            let useful_rows = rows
                .iter()
                .filter(|row| {
                    rejected || self.pattern_row_is_useful(&coverage, row, &subject_types)
                })
                .cloned()
                .collect::<Vec<_>>();
            let useful = !useful_rows.is_empty();
            if !useful {
                let message = if clauses {
                    "this clause can never run; earlier clauses match every input it matches"
                } else {
                    "case arm is unreachable"
                };
                let mut diagnostic = Diagnostic::error(
                    DiagnosticStage::Type,
                    "T4232",
                    self.resolved.semantic.modules[module_id.raw() as usize]
                        .source
                        .file_id(),
                    arm.range,
                    message,
                );
                // The earliest arm that, with the arms before it, covers this one.
                let covering = coverage_ends.iter().find(|(_, end)| {
                    rows.iter().all(|row| {
                        !self.pattern_row_is_useful(&coverage[..*end], row, &subject_types)
                    })
                });
                if let Some((covering, _)) = covering {
                    let label = if clauses {
                        "this clause, with the clauses before it, matches every input of the unreachable clause"
                    } else {
                        "this arm, with the arms before it, matches every value of the unreachable arm"
                    };
                    diagnostic = diagnostic.with_label(
                        self.resolved.semantic.modules[module_id.raw() as usize]
                            .source
                            .file_id(),
                        arms[*covering].range,
                        label,
                    );
                }
                self.push_type_diagnostic(diagnostic);
            }
            let unguarded = arm.guard.is_none();
            let catch_all = normalized
                .iter()
                .all(|pattern| matches!(pattern, MatchPattern::Wildcard));
            if unguarded && catch_all && !rejected && arm_index + 1 < arms.len() {
                let message = if clauses {
                    "a clause whose patterns all match anything must be the last clause"
                } else {
                    "an unguarded catch-all must be the final case arm"
                };
                self.type_error_range(module_id, "T4233", arm.range, message);
            }
            if unguarded && useful && !rejected && normalized.len() == subjects.len() {
                coverage.extend(useful_rows);
                coverage_ends.push((arm_index, coverage.len()));
            }
            if let Some(guard) = &arm.guard {
                facts.push(self.infer_case_guard(module_id, guard, generics, &mut arm_bindings));
            }
            let body = self.infer_expression(module_id, &arm.body, generics, &mut arm_bindings);
            self.join_arm_type(module_id, &arm.body, body.success, &mut success);
            facts.push(body);
        }
        let missing = vec![MatchPattern::Wildcard; subjects.len()];
        let separated = matches!(
            site,
            Some(CaseSite::Clauses {
                separated: true,
                ..
            })
        );
        let witness = (!rejected_arm && !separated)
            .then(|| self.pattern_row_witness(&coverage, &missing, &subject_types))
            .flatten();
        if let Some(witness) = witness {
            let witness = witness
                .iter()
                .map(|pattern| self.render_witness(pattern))
                .collect::<Vec<_>>()
                .join(", ");
            match site {
                // A function literal is a single clause and cannot gain another.
                Some(CaseSite::Clauses { function, range, .. }) if function == "fn" => self.type_error_range(
                    module_id,
                    "T4231",
                    *range,
                    format!(
                        "this function literal's parameter patterns do not cover every input; `fn({witness})` is not matched. A function literal cannot have more clauses: bind the parameter and match it with `case` in the body"
                    ),
                ),
                Some(CaseSite::Clauses { function, range, .. }) => self.type_error_range(
                    module_id,
                    "T4231",
                    *range,
                    format!(
                        "the clauses of `{function}` do not cover every input; `{function}({witness})` is not matched. Add a clause for it, return a Failure for inputs it cannot handle, or match with `case`"
                    ),
                ),
                Some(CaseSite::Binding { kind }) => self.type_error_range(
                    module_id,
                    "T4231",
                    arms.first().map_or(range, |arm| arm.range),
                    format!(
                        "a `{}` pattern must match every value, but `{witness}` is not matched; match it with `case` instead",
                        kind.keyword()
                    ),
                ),
                None => self.type_error_range(
                    module_id,
                    "T4231",
                    range,
                    format!("case is not exhaustive; `{witness}` is not matched"),
                ),
            }
        }
        ExpressionFact {
            success: success.unwrap_or(TypeId::new(0)),
            failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
            needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
        }
    }

    /// Whether `pattern` tests a value of generic type `expected`, which no
    /// clause can do: only a binding, `_`, or a typed binding may match it.
    fn inspects_generic(&self, pattern: &Pattern, expected: TypeId) -> bool {
        !matches!(
            pattern,
            Pattern::Wildcard { .. } | Pattern::Binding { .. } | Pattern::Typed { .. }
        ) && matches!(
            self.types[self.resolve_inference_type(expected).raw() as usize],
            CanonicalType::Generic(_)
        )
    }

    /// Joins one arm's success type into the `case` result. `Never` is the bottom
    /// type: a `Never` arm adds nothing, and a later arm replaces a `Never` result.
    /// Otherwise the join widens as `join_branch_type` does.
    fn join_arm_type(
        &mut self,
        module_id: ModuleId,
        body: &ModuleExpression,
        arm: TypeId,
        success: &mut Option<TypeId>,
    ) {
        self.arm_types.insert((module_id, body.semantic_id()), arm);
        *success = Some(match *success {
            Some(joined) => {
                let result = self.join_branch_type(
                    module_id,
                    expression_range(body),
                    arm,
                    joined,
                    "function result",
                );
                if self.is_never(joined) { arm } else { result }
            }
            None => arm,
        });
    }

    /// The innermost expression that lets `body` produce a value, for `T4308`: a
    /// block's last item, the first `case` arm whose type is not `Never`, or the
    /// first `break` that leaves a loop.
    fn value_producing_range(&self, module_id: ModuleId, body: &ModuleExpression) -> TextRange {
        match body {
            ModuleExpression::Block { items, range, .. } => match items.last() {
                Some(ModuleBlockItem::Expression(value)) => {
                    self.value_producing_range(module_id, value)
                }
                Some(ModuleBlockItem::Binding { value, .. }) => expression_range(value),
                None => *range,
            },
            ModuleExpression::Case { arms, range, .. } => arms
                .iter()
                .find(|arm| {
                    self.arm_types
                        .get(&(module_id, arm.body.semantic_id()))
                        .is_some_and(|arm_type| !self.is_never(*arm_type))
                })
                .map_or(*range, |arm| {
                    self.value_producing_range(module_id, &arm.body)
                }),
            ModuleExpression::Loop {
                state_id, range, ..
            } => self
                .broken_loops
                .get(&(module_id, *state_id))
                .copied()
                .unwrap_or(*range),
            other => expression_range(other),
        }
    }

    /// Whether `pattern` or a pattern inside it names a constructor that name
    /// resolution could not resolve.
    fn has_unresolved_constructor(&self, module_id: ModuleId, pattern: &Pattern) -> bool {
        matches!(pattern, Pattern::Constructor { .. })
            && matches!(
                self.resolved.reference_resolution(
                    module_id,
                    pattern.semantic_id(),
                    Namespace::Value
                ),
                Some(ReferenceResolution::Unresolved | ReferenceResolution::Ambiguous)
            )
            || pattern
                .children()
                .into_iter()
                .any(|child| self.has_unresolved_constructor(module_id, child))
    }

    /// Where `body` produced a value of the wrong type, and that type: a block's
    /// last expression, or the first `case` arm that is neither `Never` nor
    /// `expected`, searched through nested blocks and cases.
    fn mismatched_tail(
        &self,
        module_id: ModuleId,
        body: &ModuleExpression,
        found: TypeId,
        expected: TypeId,
    ) -> (TextRange, TypeId) {
        match body {
            ModuleExpression::Block { items, .. } => match items.last() {
                Some(ModuleBlockItem::Expression(value)) => {
                    self.mismatched_tail(module_id, value, found, expected)
                }
                _ => (expression_range(body), found),
            },
            ModuleExpression::Case { arms, .. } => {
                let expected_resolved = self.resolve_inference_type(expected);
                arms.iter()
                    .find_map(|arm| {
                        let arm_type = *self.arm_types.get(&(module_id, arm.body.semantic_id()))?;
                        (!self.is_never(arm_type)
                            && self.resolve_inference_type(arm_type) != expected_resolved)
                            .then(|| self.mismatched_tail(module_id, &arm.body, arm_type, expected))
                    })
                    .unwrap_or((expression_range(body), found))
            }
            _ => (expression_range(body), found),
        }
    }

    fn is_never(&self, type_id: TypeId) -> bool {
        let type_id = self.resolve_inference_type(type_id);
        matches!(
            &self.types[type_id.raw() as usize],
            CanonicalType::Named { symbol, arguments }
                if arguments.is_empty() && self.is_builtin_type(*symbol, "Never")
        )
    }

    fn is_builtin_type(&self, symbol: SymbolId, name: &str) -> bool {
        let symbol = self.resolved.symbol(symbol);
        symbol.module_id.is_none()
            && symbol.kind == ProjectSymbolKind::BuiltinType
            && symbol.name == name
    }

    fn normalize_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        expected: TypeId,
        context: PatternContext<'_>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> MatchPattern {
        // A failure-set parameter's value is an open failure set: its variant
        // patterns are checked against the set, as for a `Failure(...)` payload.
        let expected = self.open_failure_set_view(expected);
        if context.clause_head
            && self.inspects_generic(pattern, expected)
            && let CanonicalType::Generic(parameter) =
                self.types[self.resolve_inference_type(expected).raw() as usize].clone()
        {
            let name = self.resolved.symbol(parameter).name.clone();
            self.type_error_range(
                module_id,
                "T4230",
                pattern.range(),
                format!(
                    "a pattern against the generic parameter `{name}` may only be a binding, `_`, or `name: {name}`; no clause can inspect a generic value"
                ),
            );
            return MatchPattern::Wildcard;
        }
        match pattern {
            Pattern::Wildcard { .. } => MatchPattern::Wildcard,
            Pattern::Typed {
                semantic_id,
                type_use,
                range,
                ..
            } => {
                let resolved = self.resolve_inference_type(expected);
                if let CanonicalType::FailureSetValue(failures) =
                    self.types[resolved.raw() as usize].clone()
                {
                    if context.nested {
                        self.type_error_range(
                            module_id,
                            "T4230",
                            *range,
                            "a failure-type pattern can select variants only as a whole case subject or a `Failure(...)` payload; bind the value here and match it in its own `case`",
                        );
                        return MatchPattern::Wildcard;
                    }
                    self.failure_type_pattern(
                        module_id, pattern, type_use, *range, failures, context, bindings,
                    );
                    return MatchPattern::Wildcard;
                }
                let annotated = self.lower_value_type(module_id, type_use, context.generics);
                if !self.unify(expected, annotated) {
                    self.typed_binding_mismatch(module_id, *range, expected, annotated, context);
                }
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
            Pattern::Alias {
                semantic_id,
                pattern: inner,
                ..
            } => {
                let normalized =
                    self.normalize_pattern(module_id, inner, expected, context, bindings);
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
                normalized
            }
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
                    // `Void` has one value, so its pattern covers like a wildcard.
                    PatternLiteral::Void => {
                        let void = self.builtin_type("Void");
                        if !self.unify(expected, void) {
                            self.type_error_range(
                                module_id,
                                "T4230",
                                *range,
                                "a `Void` pattern does not match the subject type",
                            );
                        }
                        return MatchPattern::Wildcard;
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
                    if context.clause_head {
                        self.clause_type_conflict(module_id, *range, expected, literal_type);
                    } else {
                        self.type_error_range(
                            module_id,
                            "T4230",
                            *range,
                            "literal pattern does not match the subject type",
                        );
                    }
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
                            checker.normalize_pattern(
                                module_id,
                                element_pattern,
                                element,
                                PatternContext {
                                    nested: true,
                                    ..context
                                },
                                bindings,
                            )
                        })
                        .collect::<Vec<_>>()
                };
                let prefix = normalize_all(self, prefix);
                let suffix = normalize_all(self, suffix);
                if let Some(rest) = rest {
                    self.normalize_pattern(module_id, rest, expected, context, bindings);
                }
                MatchPattern::List {
                    prefix,
                    suffix: rest.as_ref().map(|_| suffix),
                }
            }
            Pattern::Tuple {
                elements, range, ..
            } => {
                let types =
                    self.tuple_pattern_element_types(module_id, expected, elements.len(), *range);
                let fitting = types.is_some();
                let types = types.unwrap_or_else(|| vec![TypeId::new(0); elements.len()]);
                let fields = elements
                    .iter()
                    .zip(types)
                    .map(|(element, element_type)| {
                        self.normalize_pattern(module_id, element, element_type, context, bindings)
                    })
                    .collect();
                // A pattern rejected with `T4230` covers like a wildcard, so the
                // rejection causes no coverage follow-ons.
                if fitting {
                    MatchPattern::Constructor {
                        tag: MatchTag::Tuple(elements.len()),
                        fields,
                    }
                } else {
                    MatchPattern::Wildcard
                }
            }
            Pattern::Constructor {
                arguments, range, ..
            } => self.normalize_constructor_pattern(
                module_id, pattern, arguments, *range, expected, context, bindings,
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

    #[allow(clippy::too_many_arguments)]
    fn normalize_constructor_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        arguments: &[Pattern],
        range: TextRange,
        expected: TypeId,
        context: PatternContext<'_>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> MatchPattern {
        let Some(constructor) = self.resolved_pattern_symbol(module_id, pattern) else {
            return MatchPattern::Wildcard;
        };
        if !self.is_constructor_symbol(constructor) {
            self.type_error_range(
                module_id,
                "T4230",
                range,
                "ordinary patterns require a value binding, literal, list, enum variant, or struct",
            );
            return MatchPattern::Wildcard;
        }
        let Some(inputs) = self.instantiate_pattern_constructor(constructor, expected) else {
            if context.clause_head {
                let constructed = self
                    .contracts
                    .get(&constructor)
                    .map(|contract| contract.success);
                if let Some(constructed) = constructed {
                    self.clause_type_conflict(module_id, range, expected, constructed);
                    return MatchPattern::Wildcard;
                }
            }
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
                        self.normalize_pattern(
                            module_id,
                            argument,
                            *input,
                            PatternContext {
                                nested: true,
                                ..context
                            },
                            bindings,
                        )
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
                    })
                    .collect::<Vec<_>>();
                if !parameters.is_empty() {
                    substitution
                        .values
                        .extend(parameters.into_iter().zip(arguments.arguments));
                } else if let Some(owner) = self.enum_owner_for_variant(constructor) {
                    // A user enum: match the raised instance against the declared
                    // result, which binds the enum's own type parameters.
                    let raised = self.intern_type(CanonicalType::Named {
                        symbol: owner,
                        arguments: arguments.arguments,
                    });
                    self.match_call_type(raised, contract.success, &mut substitution);
                }
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

    /// Chooses the one numeric type that both operands of an arithmetic operator
    /// share; there are no implicit conversions.
    ///
    /// Ensures: an operand with a known type decides the other; two open operands
    /// default to `Int`; a non-numeric operand on either side is `T4303`, which
    /// wins over `T4302` for mixed known types; both return poison so later
    /// stages stay quiet.
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
        if left_type == poison || right_type == poison {
            return poison;
        }
        let known = |checker: &Self, id: TypeId| {
            !matches!(
                checker.types[id.raw() as usize],
                CanonicalType::InferenceVariable(_)
            )
        };
        let symbol = binary_operator_symbol(operator);
        let decimal = self.decimal_type();
        if let Some(found) = [left_type, right_type]
            .into_iter()
            .find(|id| known(self, *id) && *id != int && *id != float && Some(*id) != decimal)
        {
            let duration = self.standard_named_type(StandardType::Duration, Vec::new());
            let is_duration = found == duration;
            let found = self.type_display_name(found);
            let help = if operator == BinaryOperator::Add && found == "Text" {
                "; join text with `Text.append`".to_owned()
            } else if is_duration {
                duration_operator_help(operator)
            } else {
                String::new()
            };
            self.type_error_range(
                module_id,
                "T4303",
                range,
                format!(
                    "`{symbol}` needs `Int`, `Float`, or `Decimal` operands, found `{found}`{help}"
                ),
            );
            return poison;
        }
        if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder)
            && decimal.is_some_and(|decimal| left_type == decimal || right_type == decimal)
        {
            self.type_error_range(
                module_id,
                "T4306",
                range,
                format!(
                    "`{symbol}` is not defined on `Decimal`: division needs a result scale and a \
                     rounding mode; use `Decimal.divide(dividend, divisor, scale, rounding)`"
                ),
            );
            return poison;
        }
        if known(self, left_type) && known(self, right_type) && left_type != right_type {
            let (left_name, right_name) = (
                self.type_display_name(left_type),
                self.type_display_name(right_type),
            );
            let help = if [left_type, right_type]
                .iter()
                .all(|id| *id == int || *id == float)
            {
                "; convert one side with `Float.from_int` or `Float.to_int`"
            } else if left_type == int || right_type == int {
                "; build an exact `Decimal` from an `Int` with `Decimal.of(value, 0)`"
            } else if left_type == float || right_type == float {
                "; `Decimal` has no conversion from `Float`, which is not exact: build the \
                 `Decimal` from its digits with `Decimal.of` or `Decimal.parse`"
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
        let operand_type = if known(self, left_type) {
            left_type
        } else if known(self, right_type) {
            right_type
        } else {
            int
        };
        self.unify(left.success, operand_type);
        self.unify(right.success, operand_type);
        operand_type
    }

    /// Types `==`, `!=`, `<`, `<=`, `>`, and `>=`: both operands share one type,
    /// which must have `property`. The property check waits for the end of the
    /// body, so later uses can still solve an open operand type.
    fn infer_comparison(
        &mut self,
        module_id: ModuleId,
        operator: BinaryOperator,
        property: ValueProperty,
        range: TextRange,
        left: ExpressionFact,
        right: ExpressionFact,
    ) {
        let poison = TypeId::new(0);
        let symbol = binary_operator_symbol(operator);
        let (left_type, right_type) = (
            self.resolve_inference_type(left.success),
            self.resolve_inference_type(right.success),
        );
        if left_type == poison || right_type == poison {
            return;
        }
        // A member value widens to its failure set, so the comparison is at
        // the set type.
        let widened = self
            .failure_set_holding(left_type, right_type)
            .or_else(|| self.failure_set_holding(right_type, left_type));
        if let Some(set) = widened {
            self.property_checks.push(PropertyCheck {
                module_id,
                type_id: set,
                property,
                site: PropertySite::Operator {
                    range,
                    operator: symbol,
                },
            });
            return;
        }
        if !self.unify(left.success, right.success) && !self.unify(right.success, left.success) {
            let (left_name, right_name) = (
                self.type_display_name(left_type),
                self.type_display_name(right_type),
            );
            let int = self.builtin_type("Int");
            let float = self.builtin_type("Float");
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
            return;
        }
        self.property_checks.push(PropertyCheck {
            module_id,
            type_id: left.success,
            property,
            site: PropertySite::Operator {
                range,
                operator: symbol,
            },
        });
    }

    /// `set` when it is a closed failure set that holds every variant of the
    /// enum `member`, so a `member` value widens to it.
    fn failure_set_holding(&self, set: TypeId, member: TypeId) -> Option<TypeId> {
        let CanonicalType::FailureSetValue(failures) = &self.types[set.raw() as usize] else {
            return None;
        };
        let CanonicalType::Named { symbol, arguments } = &self.types[member.raw() as usize] else {
            return None;
        };
        let is_enum = self.resolved.symbol(*symbol).kind == ProjectSymbolKind::Enum
            || matches!(
                self.resolved.standard_symbol(*symbol),
                Some(StandardSymbol::Type(kind)) if kind.is_failure_enum()
            );
        if !is_enum {
            return None;
        }
        let row = &self.failure_rows[failures.raw() as usize];
        let variants = self.enum_variant_symbols(*symbol);
        (row.parameters.is_empty()
            && arguments.is_empty()
            && !variants.is_empty()
            && variants
                .iter()
                .all(|variant| row.variants.contains(variant)))
        .then_some(set)
    }

    fn type_view(&self) -> TypeView<'_> {
        TypeView {
            resolved: &self.resolved,
            types: &self.types,
            fields: &self.fields,
            failure_rows: &self.failure_rows,
            inference_bindings: &self.inference_bindings,
        }
    }

    /// Settles the property checks the body just inferred queued. A check that
    /// depends on one of the declaration's own generic parameters becomes an
    /// inferred requirement on it; any other lack is reported.
    ///
    /// Returns whether a requirement was added, which changes the contract.
    fn settle_property_checks(&mut self, generics: &HashMap<String, SymbolId>) -> bool {
        let own = generics.values().copied().collect::<HashSet<_>>();
        let mut changed = false;
        // Instantiations already reported, as (module, range, type, property).
        // A call whose range encloses one of them with the same lacking type
        // only receives that type from it, as `Map.put` does from `Map.empty`
        // in `Map.empty() |> Map.put(f, 1)`, so it is not a second error.
        // Arguments are inferred before their call queues its checks, so the
        // inner call comes first.
        let mut reported = Vec::<(ModuleId, TextRange, String, ValueProperty)>::new();
        for check in std::mem::take(&mut self.property_checks) {
            let derivation = self.type_view().derive(check.type_id, check.property);
            if let Some(lack) = derivation.lack {
                if let PropertySite::Instantiation { range, .. } = &check.site {
                    let found = self.format_checked_type(check.type_id);
                    let follows = reported
                        .iter()
                        .any(|(module_id, inner, earlier, property)| {
                            *module_id == check.module_id
                                && *property == check.property
                                && *earlier == found
                                && range.start() <= inner.start()
                                && inner.end() <= range.end()
                        });
                    reported.push((check.module_id, *range, found, check.property));
                    if follows {
                        continue;
                    }
                }
                self.report_property_lack(&check, &lack);
                continue;
            }
            let origin = match &check.site {
                PropertySite::Operator { range, operator } => Some(RequirementOrigin {
                    module_id: check.module_id,
                    range: *range,
                    operation: (*operator).to_owned(),
                }),
                // A requirement with no origin starts in a standard function,
                // so this call is the use that creates it in the caller.
                PropertySite::Instantiation {
                    requirement,
                    range,
                    callee,
                    ..
                } => requirement.origin.clone().or_else(|| {
                    Some(RequirementOrigin {
                        module_id: check.module_id,
                        range: *range,
                        operation: callee.clone(),
                    })
                }),
            };
            for generic in derivation
                .generics
                .into_iter()
                .filter(|id| own.contains(id))
            {
                let stronger = self
                    .generic_requirements
                    .get(&generic)
                    .is_none_or(|existing| existing.property < check.property);
                if stronger {
                    self.generic_requirements.insert(
                        generic,
                        GenericRequirement {
                            property: check.property,
                            origin: origin.clone(),
                        },
                    );
                    changed = true;
                }
            }
        }
        changed
    }

    fn report_property_lack(&mut self, check: &PropertyCheck, lack: &PropertyLack) {
        let found = self.format_checked_type(check.type_id);
        let property = check.property.name();
        let code = match check.property {
            ValueProperty::Equatable => "T4300",
            ValueProperty::Ordered => "T4301",
        };
        let file_id = self.resolved.semantic.modules[check.module_id.raw() as usize]
            .source
            .file_id();
        let diagnostic = match &check.site {
            PropertySite::Operator { range, operator } => Diagnostic::error(
                DiagnosticStage::Type,
                code,
                file_id,
                *range,
                format!(
                    "`{operator}` needs an {property} type, but `{found}` is not {property}: {}",
                    lack.describe()
                ),
            ),
            PropertySite::Instantiation {
                range,
                callee,
                parameter,
                requirement,
            } => {
                let parameter_name = self.resolved.symbol(*parameter).name.clone();
                match &requirement.origin {
                    None => Diagnostic::error(
                        DiagnosticStage::Type,
                        code,
                        file_id,
                        *range,
                        format!(
                            "`{callee}` needs `{parameter_name}` to be {property}, but `{found}` is not {property}: {}",
                            lack.describe()
                        ),
                    ),
                    Some(origin) => {
                        let mut diagnostic = Diagnostic::error(
                            DiagnosticStage::Type,
                            "T4305",
                            file_id,
                            *range,
                            format!(
                                "this call instantiates `{parameter_name}` of `{callee}` with `{found}`, \
                                 which is not {property}: {}",
                                lack.describe()
                            ),
                        );
                        let origin_file = self.resolved.semantic.modules
                            [origin.module_id.raw() as usize]
                            .source
                            .file_id();
                        diagnostic = diagnostic.with_label(
                            origin_file,
                            origin.range,
                            format!(
                                "`{parameter_name}` must be {property} because `{}` uses `{}` here",
                                callee, origin.operation
                            ),
                        );
                        if let Some((module_id, range)) = self.generic_parameter_range(*parameter) {
                            let parameter_file = self.resolved.semantic.modules
                                [module_id.raw() as usize]
                                .source
                                .file_id();
                            diagnostic = diagnostic.with_label(
                                parameter_file,
                                range,
                                format!("`{parameter_name}` is declared here"),
                            );
                        }
                        diagnostic
                    }
                }
            }
        };
        self.push_type_diagnostic(diagnostic);
    }

    /// The declaration that owns generic `parameter`, to name a call target
    /// that has no name of its own, such as the function `mk()` returns.
    fn generic_owner_name(&self, parameter: SymbolId) -> String {
        let symbol = self.resolved.symbol(parameter);
        symbol
            .module_id
            .zip(symbol.semantic_id)
            .and_then(|(module_id, semantic_id)| {
                self.resolved.semantic.modules[module_id.raw() as usize]
                    .declarations
                    .iter()
                    .find(|declaration| declaration.semantic_id == semantic_id)
            })
            .map_or_else(
                || "this function".to_owned(),
                |declaration| declaration.name.text.clone(),
            )
    }

    /// Where a source generic parameter is declared.
    fn generic_parameter_range(&self, parameter: SymbolId) -> Option<(ModuleId, TextRange)> {
        let symbol = self.resolved.symbol(parameter);
        let module_id = symbol.module_id?;
        let semantic_id = symbol.semantic_id?;
        self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)?
            .generic_parameters
            .iter()
            .find(|name| name.text == symbol.name)
            .map(|name| (module_id, name.range))
    }

    fn format_checked_type(&self, type_id: TypeId) -> String {
        let id = self.resolve_inference_type(type_id);
        match &self.types[id.raw() as usize] {
            CanonicalType::Named { symbol, arguments } if !arguments.is_empty() => format!(
                "{}<{}>",
                self.resolved.symbol(*symbol).name,
                arguments
                    .iter()
                    .map(|argument| self.format_checked_type(*argument))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            CanonicalType::Record(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|field| format!(
                        "{}: {}",
                        field.name,
                        self.format_checked_type(field.type_id)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            CanonicalType::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(|element| self.format_checked_type(*element))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => format!(
                "fn({}) -> {}",
                contract
                    .inputs
                    .iter()
                    .map(|input| self.format_checked_type(*input))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.format_checked_type(contract.success)
            ),
            _ => self.type_display_name(id),
        }
    }

    fn type_display_name(&self, id: TypeId) -> String {
        match &self.types[self.resolve_inference_type(id).raw() as usize] {
            CanonicalType::Named { symbol, .. } | CanonicalType::Generic(symbol) => {
                self.resolved.symbol(*symbol).name.clone()
            }
            CanonicalType::Function(_) | CanonicalType::PolymorphicFunction { .. } => {
                self.formatter().format_type(id)
            }
            CanonicalType::Record(_) => "record".to_owned(),
            CanonicalType::Tuple(_) => "tuple".to_owned(),
            CanonicalType::FailureSetValue(failures) => self.format_failure_set(*failures),
            _ => "this type".to_owned(),
        }
    }

    /// A failure set as source writes it: whole enums by name, a part of an
    /// enum by its variants, and row parameters, joined with ` | `.
    fn format_failure_set(&self, failures: FailureRowId) -> String {
        let row = &self.failure_rows[failures.raw() as usize];
        let mut members = row
            .parameters
            .iter()
            .map(|parameter| self.resolved.symbol(*parameter).name.clone())
            .collect::<Vec<_>>();
        let mut owners = Vec::<SymbolId>::new();
        for variant in &row.variants {
            match self.enum_owner_for_variant(*variant) {
                Some(owner) if !owners.contains(&owner) => owners.push(owner),
                Some(_) => {}
                None => members.push(self.resolved.symbol(*variant).name.clone()),
            }
        }
        for owner in owners {
            let name = self.resolved.symbol(owner).name.clone();
            let variants = self.enum_variant_symbols(owner);
            if variants
                .iter()
                .all(|variant| row.variants.contains(variant))
            {
                members.push(name);
            } else {
                members.extend(
                    variants
                        .iter()
                        .filter(|variant| row.variants.contains(variant))
                        .map(|variant| format!("{name}.{}", self.resolved.symbol(*variant).name)),
                );
            }
        }
        members.sort();
        if members.is_empty() {
            "Never".to_owned()
        } else {
            members.join(" | ")
        }
    }

    fn formatter(&self) -> TypeFormatter<'_> {
        TypeFormatter {
            resolved: &self.resolved,
            types: &self.types,
            failure_rows: &self.failure_rows,
            needs_rows: &self.needs_rows,
            generic_requirements: &self.generic_requirements,
            inference_bindings: &self.inference_bindings,
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

    /// Requires an operand of `and`, `or`, or `not` to be `Bool`, reporting `T4304` otherwise.
    ///
    /// Returns the `Bool` type, which is the operator's result type either way.
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

    /// Checks `name: Type` against a failure-set value. `Type` must name a closed
    /// enum; the pattern covers that enum's variants present in `failures` and binds
    /// the failure set restricted to them. A closed row without any is `T4224`; an
    /// open row may hold any of them. Returns the covered variants and records them
    /// for Core lowering.
    #[allow(clippy::too_many_arguments)]
    fn failure_type_pattern(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        type_use: &TypeUse,
        range: TextRange,
        failures: FailureRowId,
        context: PatternContext<'_>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> Vec<SymbolId> {
        let annotated = self.lower_value_type(module_id, type_use, context.generics);
        let annotated = self.resolve_inference_type(annotated);
        // The annotation names failure-set members: a closed enum, one variant,
        // or a written set of them. A generic member cannot be selected.
        let selected = match self.types[annotated.raw() as usize] {
            CanonicalType::Poison => return Vec::new(),
            _ => self
                .failure_set_of(annotated)
                .map(|row| self.failure_rows[row.raw() as usize].clone())
                .filter(|row| row.parameters.is_empty() && !row.variants.is_empty()),
        };
        let Some(selected) = selected else {
            let name = self.type_display_name(annotated);
            self.type_error_range(
                module_id,
                "T4230",
                range,
                format!(
                    "a failure-type pattern selects the variants of a closed enum, and `{name}` is not one"
                ),
            );
            return Vec::new();
        };
        let variants = selected
            .variants
            .into_iter()
            .filter(|variant| self.failure_row_may_contain(failures, *variant))
            .collect::<Vec<_>>();
        if variants.is_empty() {
            let name = self.type_display_name(annotated);
            self.type_error_range(
                module_id,
                "T4224",
                range,
                format!("no `{name}` failure is present in the subject's failure channel"),
            );
            return Vec::new();
        }
        let covered = variants
            .iter()
            .map(|variant| {
                let fields = self
                    .contracts
                    .get(variant)
                    .map_or(0, |contract| contract.inputs.len());
                (*variant, fields)
            })
            .collect();
        self.failure_type_patterns
            .insert((module_id, pattern.semantic_id()), covered);
        let bound = self
            .restricted_failure_type(failures, &variants)
            .unwrap_or(TypeId::new(0));
        self.bind_pattern_name(module_id, pattern, bound, bindings);
        variants
    }

    /// The type of a value from `failures` known to be one of `variants`: the enum
    /// itself when they are all of its variants, otherwise a narrowed failure set.
    fn restricted_failure_type(
        &mut self,
        failures: FailureRowId,
        variants: &[SymbolId],
    ) -> Option<TypeId> {
        let row = self.failure_rows[failures.raw() as usize].clone();
        let mut variants = variants.to_vec();
        variants.sort_unstable();
        variants.dedup();
        let restricted = self.intern_failure_row(FailureRow {
            variant_arguments: row
                .variant_arguments
                .into_iter()
                .filter(|arguments| variants.contains(&arguments.variant))
                .collect(),
            variants,
            parameters: Vec::new(),
            exclusions: Vec::new(),
        });
        self.failure_value_type(restricted)
    }

    fn bind_pattern_name(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        type_id: TypeId,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) {
        if let Some(binding) = self.pattern_binding_symbol(module_id, pattern) {
            self.pattern_binding_types
                .insert((module_id, pattern.semantic_id()), type_id);
            bindings.insert(
                binding,
                ValueBinding {
                    type_id,
                    need: None,
                },
            );
        }
    }

    /// The coverage rows a `Failure(...)` payload pattern contributes: one row per
    /// variant for a failure-type pattern (through any alias), otherwise its
    /// ordinary normalization.
    fn failure_coverage_rows(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        failure_type: TypeId,
        context: PatternContext<'_>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> Vec<Vec<MatchPattern>> {
        match pattern {
            Pattern::Alias { pattern: inner, .. } => {
                self.failure_coverage_rows(module_id, inner, failure_type, context, bindings)
            }
            Pattern::Typed { .. } => self
                .failure_type_patterns
                .get(&(module_id, pattern.semantic_id()))
                .map(|covered| {
                    covered
                        .iter()
                        .map(|(variant, fields)| {
                            vec![MatchPattern::Constructor {
                                tag: MatchTag::EnumVariant(*variant),
                                fields: vec![MatchPattern::Wildcard; *fields],
                            }]
                        })
                        .collect()
                })
                .unwrap_or_else(|| vec![vec![MatchPattern::Wildcard]]),
            _ => vec![vec![self.normalize_pattern(
                module_id,
                pattern,
                failure_type,
                context,
                bindings,
            )]],
        }
    }

    /// Expands each failure-type pattern of an ordinary arm (`file_error: FileError`
    /// over a failure-set subject) into one row per variant it covers, so coverage
    /// sees the variants. Other arms keep their single row.
    fn expand_failure_type_rows(
        &self,
        module_id: ModuleId,
        patterns: &[Pattern],
        normalized: Vec<MatchPattern>,
    ) -> Vec<Vec<MatchPattern>> {
        let mut rows = vec![Vec::new()];
        for (index, column) in normalized.into_iter().enumerate() {
            let mut pattern = patterns.get(index);
            while let Some(Pattern::Alias { pattern: inner, .. }) = pattern {
                pattern = Some(inner);
            }
            let alternatives = pattern
                .and_then(|pattern| {
                    self.failure_type_patterns
                        .get(&(module_id, pattern.semantic_id()))
                })
                .map_or_else(
                    || vec![column.clone()],
                    |covered| {
                        covered
                            .iter()
                            .map(|(variant, fields)| MatchPattern::Constructor {
                                tag: MatchTag::EnumVariant(*variant),
                                fields: vec![MatchPattern::Wildcard; *fields],
                            })
                            .collect()
                    },
                );
            rows = rows
                .into_iter()
                .flat_map(|row| {
                    alternatives.iter().map(move |alternative| {
                        let mut row = row.clone();
                        row.push(alternative.clone());
                        row
                    })
                })
                .collect();
        }
        rows
    }

    /// Types `{ ..base, field: value }`. The base must be a struct visible here, a
    /// record, or a structural interface value (`T4284` otherwise); each named field
    /// must exist (`T4205`), appear once (`T4211`), and keep its declared type
    /// (`T4203`). The result has the base's type, and its failures and needs are the
    /// union of the parts.
    fn infer_update(
        &mut self,
        module_id: ModuleId,
        semantic_id: SemanticId,
        base: &ModuleExpression,
        fields: &[crate::semantic::RecordValueField],
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let base_fact = self.infer_expression(module_id, base, generics, bindings);
        let mut facts = vec![base_fact];
        let base_type = self.resolve_inference_type(base_fact.success);
        let mut shape = self.update_shape(module_id, base, base_type);
        let mut seen = Vec::<&str>::new();
        for (index, field) in fields.iter().enumerate() {
            let fact = self.infer_expression(module_id, &field.value, generics, bindings);
            facts.push(fact);
            let Some(shape) = shape.as_mut() else {
                continue;
            };
            if seen.contains(&field.name.text.as_str()) {
                self.type_error_range(
                    module_id,
                    "T4211",
                    field.name.range,
                    format!("update field `{}` occurs more than once", field.name.text),
                );
                continue;
            }
            seen.push(&field.name.text);
            let Some(slot) = shape
                .fields
                .iter_mut()
                .find(|(name, _, _)| *name == field.name.text)
            else {
                let base_name = self.type_display_name(base_type);
                self.type_error_range(
                    module_id,
                    "T4205",
                    field.name.range,
                    format!(
                        "`{base_name}` has no field `{}`; an update cannot add fields",
                        field.name.text
                    ),
                );
                continue;
            };
            slot.2 = Some(index);
            let declared = slot.1;
            if !self.value_fits(fact.success, declared) {
                let expected = self.type_display_name(declared);
                let found = self.type_display_name(fact.success);
                self.type_error_range(
                    module_id,
                    "T4203",
                    expression_range(&field.value),
                    format!(
                        "field `{}` has type `{expected}`, not `{found}`; an update keeps the base's type",
                        field.name.text
                    ),
                );
            }
        }
        if let Some(shape) = shape {
            self.record_updates.insert((module_id, semantic_id), shape);
        }
        ExpressionFact {
            success: base_fact.success,
            failures: self.union_failure_rows(facts.iter().map(|fact| fact.failures)),
            needs: self.union_needs_rows(facts.iter().map(|fact| fact.needs)),
        }
    }

    /// The fields an update of a `base_type` value rebuilds, reporting `T4284` for a
    /// base that is not a struct, record, or interface, and
    /// `E_PRIVATE_STRUCT_CONSTRUCTOR` for a struct of another module.
    fn update_shape(
        &mut self,
        module_id: ModuleId,
        base: &ModuleExpression,
        base_type: TypeId,
    ) -> Option<RecordUpdateShape> {
        let constructor = match self.types[base_type.raw() as usize].clone() {
            CanonicalType::Record(_) => None,
            CanonicalType::Poison => return None,
            CanonicalType::Named { symbol, .. } => {
                let declaration = self.resolved.symbol(symbol).clone();
                match declaration.kind {
                    ProjectSymbolKind::Interface => None,
                    ProjectSymbolKind::Struct => {
                        if declaration.module_id != Some(module_id) {
                            self.diagnostics.push(Diagnostic::error(
                                DiagnosticStage::Type,
                                "E_PRIVATE_STRUCT_CONSTRUCTOR",
                                self.resolved.semantic.modules[module_id.raw() as usize]
                                    .source
                                    .file_id(),
                                expression_range(base),
                                format!(
                                    "`{}` has module-private fields, so only its own module can update it; use the module's public functions",
                                    declaration.name
                                ),
                            ));
                            return None;
                        }
                        Some(self.find_symbol(
                            declaration.module_id?,
                            declaration.semantic_id,
                            ProjectSymbolKind::StructConstructor,
                            &declaration.name,
                        )?)
                    }
                    _ => return self.reject_update_base(module_id, base, base_type),
                }
            }
            _ => return self.reject_update_base(module_id, base, base_type),
        };
        let fields = self
            .type_fields(base_type)?
            .into_iter()
            .map(|field| (field.name, field.type_id, None))
            .collect();
        Some(RecordUpdateShape {
            constructor,
            fields,
        })
    }

    fn reject_update_base(
        &mut self,
        module_id: ModuleId,
        base: &ModuleExpression,
        base_type: TypeId,
    ) -> Option<RecordUpdateShape> {
        let message = if matches!(
            self.types[self.resolve_inference_type(base_type).raw() as usize],
            CanonicalType::InferenceVariable(_)
        ) {
            "only a struct or record value can be updated, and this value's type is not known yet; annotate the parameter or `let` binding it comes from with its struct type".to_owned()
        } else {
            let name = self.type_display_name(base_type);
            format!("only a struct or record value can be updated, and this is `{name}`")
        };
        self.type_error_range(module_id, "T4284", expression_range(base), message);
        None
    }

    /// Gives a clause parameter the generic type that any clause names for it
    /// (`_: T`), before any clause's patterns are checked, so a literal or
    /// constructor against it is `T4230` whichever clause comes first.
    fn seed_generic_clause_parameters(
        &mut self,
        module_id: ModuleId,
        subjects: &[ExpressionFact],
        arms: &[ModuleCaseArm],
        generics: &HashMap<String, SymbolId>,
    ) {
        for arm in arms {
            for (pattern, subject) in arm.patterns.iter().zip(subjects) {
                let mut pattern = pattern;
                while let Pattern::Alias { pattern: inner, .. } = pattern {
                    pattern = inner;
                }
                let Pattern::Typed { type_use, .. } = pattern else {
                    continue;
                };
                let subject_type = self.resolve_inference_type(subject.success);
                if !matches!(
                    self.types[subject_type.raw() as usize],
                    CanonicalType::InferenceVariable(_)
                ) {
                    continue;
                }
                let annotated = self.lower_value_type(module_id, type_use, generics);
                let annotated = self.resolve_inference_type(annotated);
                if matches!(
                    self.types[annotated.raw() as usize],
                    CanonicalType::Generic(_)
                ) {
                    self.unify(subject_type, annotated);
                }
            }
        }
    }

    /// `T4282`: a clause parameter never holds a live outcome, because callers
    /// evaluate arguments first and a failing argument has already left the caller.
    fn reject_outcome_parameter(
        &mut self,
        module_id: ModuleId,
        pattern: &Pattern,
        site: &CaseSite,
    ) -> bool {
        let outcome = self.pattern_is_builtin(module_id, pattern, "Success")
            || self.pattern_is_builtin(module_id, pattern, "Failure");
        if outcome {
            let message = match site {
                CaseSite::Clauses { .. } => "a parameter is a value, never an outcome: the caller evaluates the argument first, and a failing argument has already left through the caller's failure channel. Match the operation with `case` at the call site, or pass a thunk to a recovery function".to_owned(),
                CaseSite::Binding { kind } => format!(
                    "a `{}` pattern matches a value, never an outcome: a failure has already left through the enclosing failure channel. Match the operation with `case`",
                    kind.keyword()
                ),
            };
            self.type_error_range(module_id, "T4282", pattern.range(), message);
        }
        outcome
    }

    /// `T4281`: two clauses need different types for one parameter position.
    fn clause_type_conflict(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        expected: TypeId,
        found: TypeId,
    ) {
        let expected = self.type_display_name(expected);
        let found = self.type_display_name(found);
        self.type_error_range(
            module_id,
            "T4281",
            range,
            format!(
                "this clause needs `{found}` where another clause needs `{expected}`; clauses select by value, and Chimi does not overload functions by type. Use an enum to model the alternatives"
            ),
        );
    }

    /// A typed binding names a type other than the value's: `T4281` across clause
    /// heads, `T4283` elsewhere.
    fn typed_binding_mismatch(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        expected: TypeId,
        annotated: TypeId,
        context: PatternContext<'_>,
    ) {
        if context.clause_head {
            self.clause_type_conflict(module_id, range, expected, annotated);
            return;
        }
        let expected = self.type_display_name(expected);
        let annotated = self.type_display_name(annotated);
        self.type_error_range(
            module_id,
            "T4283",
            range,
            format!(
                "the value has type `{expected}`, not `{annotated}`; a typed binding does not test types at run time, and only failure values can be selected by type. Remove the annotation, or match variants"
            ),
        );
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
                // `Int` has no finite domain, but when the column tests integers a
                // concrete one it misses reads better than `_`.
                let missing = missing.or_else(|| unmatched_integer(matrix));
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
                    MatchTag::Tuple(_) => String::new(),
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
            MatchTag::Tuple(arity) => self.tuple_element_types(expected, *arity).ok(),
            MatchTag::Boolean(_)
            | MatchTag::Integer(_)
            | MatchTag::Text(_)
            | MatchTag::Rejected(_) => Some(Vec::new()),
        }
    }

    /// The element types of `expected` as a tuple of `arity` elements. An unsolved
    /// `expected` becomes such a tuple. `Err` holds the arity `expected` has instead,
    /// or `None` when it is not a tuple at all.
    fn tuple_element_types(
        &mut self,
        expected: TypeId,
        arity: usize,
    ) -> Result<Vec<TypeId>, Option<usize>> {
        let resolved = self.resolve_inference_type(expected);
        match self.types[resolved.raw() as usize].clone() {
            CanonicalType::Tuple(elements) if elements.len() == arity => Ok(elements),
            CanonicalType::Tuple(elements) => Err(Some(elements.len())),
            CanonicalType::Poison => Ok((0..arity).map(|_| TypeId::new(0)).collect()),
            _ => {
                let elements = (0..arity)
                    .map(|_| self.fresh_inference_type())
                    .collect::<Vec<_>>();
                let tuple = self.intern_type(CanonicalType::Tuple(elements.clone()));
                if self.unify(expected, tuple) {
                    Ok(elements)
                } else {
                    Err(None)
                }
            }
        }
    }

    /// The element types for a tuple pattern of `arity` elements against `expected`;
    /// `None`, after reporting `T4230`, when the value is not a tuple of that arity.
    fn tuple_pattern_element_types(
        &mut self,
        module_id: ModuleId,
        expected: TypeId,
        arity: usize,
        range: TextRange,
    ) -> Option<Vec<TypeId>> {
        match self.tuple_element_types(expected, arity) {
            Ok(elements) => Some(elements),
            Err(actual) => {
                let message = match actual {
                    Some(actual) => format!(
                        "this tuple pattern has {arity} elements, but the value is a tuple of {actual}"
                    ),
                    None => format!(
                        "a tuple pattern does not match a value of type `{}`",
                        self.format_checked_type(expected)
                    ),
                };
                self.type_error_range(module_id, "T4230", range, message);
                None
            }
        }
    }

    /// The constructors a value of `type_id` may be built with, each with its
    /// field types, or `None` for an open domain. A constructor with a field no
    /// value inhabits, as `Err` of `Result<T, E>` with the empty failure set `E`,
    /// builds no value and is left out, so a `case` need not cover it.
    fn complete_pattern_domain(&mut self, type_id: TypeId) -> Option<Vec<(MatchTag, Vec<TypeId>)>> {
        let mut domain = self.declared_pattern_domain(type_id)?;
        domain.retain(|(_, fields)| !fields.iter().any(|field| self.is_uninhabited(*field)));
        Some(domain)
    }

    /// `Never`, or the empty failure set: a type with no values.
    fn is_uninhabited(&self, type_id: TypeId) -> bool {
        let type_id = self.resolve_inference_type(type_id);
        self.is_never(type_id)
            || matches!(
                self.types[type_id.raw() as usize],
                CanonicalType::FailureSetValue(failures) if self.failure_row_is_empty(failures)
            )
    }

    fn declared_pattern_domain(&mut self, type_id: TypeId) -> Option<Vec<(MatchTag, Vec<TypeId>)>> {
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
        if let CanonicalType::Tuple(elements) = self.types[type_id.raw() as usize].clone() {
            // A tuple has one shape, like a struct.
            return Some(vec![(MatchTag::Tuple(elements.len()), elements)]);
        }
        let CanonicalType::Named { symbol, .. } = self.types[type_id.raw() as usize].clone() else {
            return None;
        };
        let symbol_kind = self.resolved.symbol(symbol).kind;
        if symbol_kind == ProjectSymbolKind::Struct {
            // A struct has one shape: its constructor is its whole domain.
            let declaration = self.resolved.symbol(symbol);
            let constructor = self.find_symbol(
                declaration.module_id?,
                declaration.semantic_id,
                ProjectSymbolKind::StructConstructor,
                &declaration.name.clone(),
            )?;
            let fields = self.instantiate_pattern_constructor(constructor, type_id)?;
            return Some(vec![(MatchTag::EnumVariant(constructor), fields)]);
        }
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
                    | StandardType::Ordering
                    | StandardType::JsonValue
                    | StandardType::DecodeError
                    | StandardType::JsonConversionError
                    | StandardType::ChannelSendError
                    | StandardType::ProcessSendError
                    | StandardType::ProcessDown
                    | StandardType::SupervisorRestart
                    | StandardType::SupervisorStartError
                    | StandardType::ChannelReceive
                    | StandardType::ChannelError
                    | StandardType::IntParseError
                    | StandardType::FloatParseError
                    | StandardType::FloatConversionError
                    | StandardType::DurationParseError
                    | StandardType::ConsoleError
                    | StandardType::FileError
                    | StandardType::Utf8Error
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

    /// Types a `case` that matches outcomes. `outcome_columns[i]` marks a subject
    /// that some arm matches with `Success(...)` or `Failure(...)`; every arm must
    /// then match that subject that way (`T4222`). Coverage is the single-subject
    /// rule applied per combination of Success and Failure across the outcome
    /// subjects: the all-Success combination must be exhaustive (`T4223`), and a
    /// failure no arm matches bubbles. Since the first failing subject is the one
    /// that bubbles, a failure variant leaves a subject's row only when every
    /// combination in which that subject is the first to fail covers it fully.
    #[allow(clippy::too_many_arguments)]
    fn infer_outcome_case(
        &mut self,
        module_id: ModuleId,
        subjects: &[ExpressionFact],
        outcome_columns: &[bool],
        arms: &[ModuleCaseArm],
        range: TextRange,
        generics: &HashMap<String, SymbolId>,
        bindings: &HashMap<SymbolId, ValueBinding>,
    ) -> ExpressionFact {
        let context = PatternContext {
            generics,
            clause_head: false,
            nested: false,
        };
        let single = subjects.len() == 1;
        let failure_types = subjects
            .iter()
            .zip(outcome_columns)
            .map(|(subject, outcome)| {
                if *outcome {
                    self.failure_value_type(subject.failures)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        // The column types of one Success (`true`) / Failure (`false`) combination
        // over the outcome columns, in order.
        let column_types = |signature: &[bool]| {
            let mut kinds = signature.iter();
            subjects
                .iter()
                .zip(outcome_columns)
                .zip(&failure_types)
                .map(|((subject, outcome), failure_type)| {
                    match outcome.then(|| *kinds.next().expect("one kind per outcome column")) {
                        Some(false) => failure_type.unwrap_or(TypeId::new(0)),
                        Some(true) | None => subject.success,
                    }
                })
                .collect::<Vec<_>>()
        };
        let mut arm_facts = Vec::new();
        let mut success = None;
        let mut coverage = HashMap::<Vec<bool>, Vec<Vec<MatchPattern>>>::new();
        let mut candidate_failure_variants = vec![Vec::<SymbolId>::new(); subjects.len()];
        let mut catches_unknown_columns = vec![false; subjects.len()];
        let mut saw_failure = vec![false; subjects.len()];
        for arm in arms {
            let mut arm_bindings = bindings.clone();
            let mut covers = arm.patterns.len() == subjects.len();
            if !covers {
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
            let mut signature = Vec::new();
            let mut alternatives = Vec::<Vec<MatchPattern>>::new();
            let mut catches_unknown = vec![false; subjects.len()];
            for (index, (pattern, subject)) in arm.patterns.iter().zip(subjects).enumerate() {
                if !outcome_columns[index] {
                    let normalized = self.normalize_pattern(
                        module_id,
                        pattern,
                        subject.success,
                        context,
                        &mut arm_bindings,
                    );
                    let rows = self.expand_failure_type_rows(
                        module_id,
                        std::slice::from_ref(pattern),
                        vec![normalized],
                    );
                    alternatives.push(rows.into_iter().flatten().collect());
                    continue;
                }
                if self.pattern_is_builtin(module_id, pattern, "Success") {
                    signature.push(true);
                    let Some(inner) = self.single_outcome_pattern_argument(module_id, pattern)
                    else {
                        covers = false;
                        continue;
                    };
                    let normalized = self.normalize_pattern(
                        module_id,
                        inner,
                        subject.success,
                        PatternContext {
                            nested: true,
                            ..context
                        },
                        &mut arm_bindings,
                    );
                    alternatives.push(vec![normalized]);
                } else if self.pattern_is_builtin(module_id, pattern, "Failure") {
                    signature.push(false);
                    saw_failure[index] = true;
                    let Some(inner) = self.single_outcome_pattern_argument(module_id, pattern)
                    else {
                        covers = false;
                        continue;
                    };
                    let (variants, unknown) = self.handled_failure_pattern(
                        module_id,
                        inner,
                        subject.failures,
                        context,
                        &mut arm_bindings,
                    );
                    candidate_failure_variants[index].extend(variants.iter().copied());
                    match failure_types[index] {
                        None if unknown => {
                            self.type_error_range(
                                module_id,
                                "T4232",
                                arm.range,
                                "outcome case arm is unreachable: the subject cannot fail",
                            );
                            covers = false;
                        }
                        // The owning diagnostic already explains why this pattern
                        // cannot participate in failure-channel coverage.
                        _ if variants.is_empty() && !unknown => covers = false,
                        None => covers = false,
                        Some(failure_type) => {
                            // Coverage normalization binds again; the bindings
                            // `handled_failure_pattern` made, with narrowed alias and
                            // failure-type bindings, are the ones the arm keeps.
                            let mut scratch = arm_bindings.clone();
                            let rows = self.failure_coverage_rows(
                                module_id,
                                inner,
                                failure_type,
                                context,
                                &mut scratch,
                            );
                            alternatives.push(rows.into_iter().flatten().collect());
                            catches_unknown[index] = unknown;
                        }
                    }
                } else {
                    signature.push(true);
                    covers = false;
                    let message = if single {
                        "a fallible case arm must begin with Success(...) or Failure(...)"
                    } else {
                        "another arm matches this subject as an outcome, so every arm must \
                         match it with Success(...) or Failure(...)"
                    };
                    self.type_error_range(module_id, "T4222", pattern.head_range(), message);
                }
            }
            if covers {
                let rows = alternatives.iter().fold(vec![Vec::new()], |rows, column| {
                    rows.into_iter()
                        .flat_map(|row| {
                            column.iter().map(move |alternative| {
                                let mut row = row.clone();
                                row.push(alternative.clone());
                                row
                            })
                        })
                        .collect()
                });
                let types = column_types(&signature);
                let matrix = coverage.get(&signature).cloned().unwrap_or_default();
                let useful_rows = rows
                    .into_iter()
                    .filter(|row| self.pattern_row_is_useful(&matrix, row, &types))
                    .collect::<Vec<_>>();
                if useful_rows.is_empty() {
                    self.type_error_range(
                        module_id,
                        "T4232",
                        arm.range,
                        "outcome case arm is unreachable",
                    );
                } else if arm.guard.is_none() {
                    for (column, unknown) in catches_unknown_columns.iter_mut().zip(catches_unknown)
                    {
                        *column |= unknown;
                    }
                    coverage.entry(signature).or_default().extend(useful_rows);
                }
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
            self.join_arm_type(module_id, &arm.body, body.success, &mut success);
            arm_facts.push(body);
        }
        let outcome_indices = outcome_columns
            .iter()
            .enumerate()
            .filter_map(|(index, outcome)| outcome.then_some(index))
            .collect::<Vec<_>>();
        let all_success = vec![true; outcome_indices.len()];
        let missing = vec![MatchPattern::Wildcard; subjects.len()];
        let witness = self.pattern_row_witness(
            coverage.get(&all_success).map_or(&[][..], Vec::as_slice),
            &missing,
            &column_types(&all_success),
        );
        if let Some(witness) = witness {
            let message = if single {
                "outcome case does not fully handle Success".to_owned()
            } else {
                let witness = witness
                    .iter()
                    .zip(outcome_columns)
                    .map(|(pattern, outcome)| {
                        let rendered = self.render_witness(pattern);
                        if *outcome {
                            format!("Success({rendered})")
                        } else {
                            rendered
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("outcome case does not fully handle Success; `{witness}` is not matched")
            };
            self.type_error_range(module_id, "T4223", range, message);
        }
        for &index in &outcome_indices {
            if !saw_failure[index] && !self.failure_row_is_empty(subjects[index].failures) {
                let message = if single {
                    "outcome case does not handle Failure".to_owned()
                } else {
                    format!(
                        "outcome case does not handle Failure of subject {}",
                        index + 1
                    )
                };
                self.type_error_range(module_id, "T4223", range, message);
            }
        }
        let mut remaining_rows = Vec::new();
        for (position, &index) in outcome_indices.iter().enumerate() {
            let subject = subjects[index];
            let Some(failure_type) = failure_types[index] else {
                remaining_rows.push(subject.failures);
                continue;
            };
            // Combinations where this subject is the first to fail: earlier outcome
            // subjects succeeded, later ones may do either, except that a subject
            // that cannot fail always succeeds.
            let later = outcome_indices.len() - position - 1;
            let mut signatures = (0..1_usize << later)
                .map(|bits| {
                    (0..outcome_indices.len())
                        .map(|other| match other.cmp(&position) {
                            std::cmp::Ordering::Less => true,
                            std::cmp::Ordering::Equal => false,
                            std::cmp::Ordering::Greater => {
                                failure_types[outcome_indices[other]].is_none()
                                    || bits >> (other - position - 1) & 1 == 0
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            signatures.sort_unstable();
            signatures.dedup();
            let fully_covered = |this: &mut Self, column: MatchPattern| {
                signatures.iter().all(|signature| {
                    let mut row = missing.clone();
                    row[index] = column.clone();
                    !this.pattern_row_is_useful(
                        coverage.get(signature).map_or(&[][..], Vec::as_slice),
                        &row,
                        &column_types(signature),
                    )
                })
            };
            let mut candidates = std::mem::take(&mut candidate_failure_variants[index]);
            candidates.extend(
                self.failure_rows[subject.failures.raw() as usize]
                    .variants
                    .iter()
                    .copied(),
            );
            candidates.sort_unstable();
            candidates.dedup();
            let mut handled_variants = Vec::new();
            for variant in candidates {
                let field_count = self
                    .instantiate_pattern_constructor(variant, failure_type)
                    .map_or(0, |fields| fields.len());
                let candidate = MatchPattern::Constructor {
                    tag: MatchTag::EnumVariant(variant),
                    fields: vec![MatchPattern::Wildcard; field_count],
                };
                if fully_covered(self, candidate) {
                    handled_variants.push(variant);
                }
            }
            let handles_unknown_row =
                catches_unknown_columns[index] && fully_covered(self, MatchPattern::Wildcard);
            remaining_rows.push(self.remove_handled_failures(
                subject.failures,
                &handled_variants,
                handles_unknown_row,
            ));
        }
        let plain_subjects = subjects
            .iter()
            .zip(outcome_columns)
            .filter(|(_, outcome)| !**outcome)
            .map(|(subject, _)| subject.failures);
        ExpressionFact {
            success: success.unwrap_or(subjects[outcome_indices[0]].success),
            failures: self.union_failure_rows(
                remaining_rows
                    .into_iter()
                    .chain(plain_subjects)
                    .chain(arm_facts.iter().map(|fact| fact.failures)),
            ),
            needs: self.union_needs_rows(
                subjects
                    .iter()
                    .map(|subject| subject.needs)
                    .chain(arm_facts.iter().map(|fact| fact.needs)),
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
        context: PatternContext<'_>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> (Vec<SymbolId>, bool) {
        match pattern {
            Pattern::Wildcard { .. } => {
                return (
                    self.failure_rows[failures.raw() as usize].variants.clone(),
                    true,
                );
            }
            Pattern::Typed {
                type_use, range, ..
            } => {
                let variants = self.failure_type_pattern(
                    module_id, pattern, type_use, *range, failures, context, bindings,
                );
                return (variants, false);
            }
            Pattern::Alias { pattern: inner, .. } => {
                let (variants, catches_unknown) =
                    self.handled_failure_pattern(module_id, inner, failures, context, bindings);
                let bound = if catches_unknown {
                    self.failure_value_type(failures)
                } else {
                    self.restricted_failure_type(failures, &variants)
                };
                self.bind_pattern_name(
                    module_id,
                    pattern,
                    bound.unwrap_or(TypeId::new(0)),
                    bindings,
                );
                return (variants, catches_unknown);
            }
            _ => {}
        }
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
            // Bind against the row, not the constructor's declared type, so the
            // payload gets the type arguments the variant was raised with.
            let row_value = self.intern_type(CanonicalType::FailureSetValue(failures));
            self.bind_value_pattern(module_id, pattern, row_value, context, bindings);
            return (vec![symbol], false);
        }

        if self.pattern_binding_symbol(module_id, pattern).is_some() {
            if let Some(error_type) = self.failure_value_type(failures) {
                self.bind_value_pattern(module_id, pattern, error_type, context, bindings);
            } else {
                self.bind_value_pattern(module_id, pattern, TypeId::new(0), context, bindings);
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
        context: PatternContext<'_>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) {
        match pattern {
            Pattern::Wildcard { .. } => {}
            Pattern::Typed {
                semantic_id,
                type_use,
                range,
                ..
            } => {
                let annotated = self.lower_value_type(module_id, type_use, context.generics);
                if !self.unify(expected, annotated) {
                    self.typed_binding_mismatch(module_id, *range, expected, annotated, context);
                }
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
            Pattern::Alias {
                semantic_id,
                pattern: inner,
                ..
            } => {
                self.bind_value_pattern(module_id, inner, expected, context, bindings);
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
            Pattern::Literal { literal, range, .. } => {
                let literal_type = self.builtin_type(match literal {
                    PatternLiteral::Integer(_) => "Int",
                    PatternLiteral::Text(_) => "Text",
                    PatternLiteral::Boolean(_) => "Bool",
                    PatternLiteral::Void => "Void",
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
                    self.bind_value_pattern(module_id, element_pattern, element, context, bindings);
                }
                if let Some(rest) = rest {
                    self.bind_value_pattern(module_id, rest, expected, context, bindings);
                }
            }
            Pattern::Tuple {
                elements, range, ..
            } => {
                let types = self
                    .tuple_pattern_element_types(module_id, expected, elements.len(), *range)
                    .unwrap_or_else(|| vec![TypeId::new(0); elements.len()]);
                for (element, element_type) in elements.iter().zip(types) {
                    self.bind_value_pattern(module_id, element, element_type, context, bindings);
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
                if !self.is_constructor_symbol(constructor) {
                    return;
                }
                let contract = self.contracts[&constructor].clone();
                let mut substitution = CallSubstitution::default();
                let expected = self.resolve_inference_type(expected);
                let instantiated = match self.types[expected.raw() as usize].clone() {
                    CanonicalType::FailureSetValue(_) => {
                        self.instantiate_pattern_constructor(constructor, expected)
                    }
                    _ => self
                        .match_call_type(expected, contract.success, &mut substitution)
                        .then(|| {
                            contract
                                .inputs
                                .iter()
                                .map(|input| self.substitute_call_type(*input, &mut substitution))
                                .collect::<Vec<_>>()
                        }),
                };
                let Some(inputs) = instantiated else {
                    self.type_error_range(
                        module_id,
                        "T4222",
                        name.range,
                        format!("pattern `{}` does not match the subject type", name.text),
                    );
                    return;
                };
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
                for (argument, input) in arguments.iter().zip(inputs) {
                    self.bind_value_pattern(module_id, argument, input, context, bindings);
                }
            }
        }
    }

    fn pattern_is_builtin(&self, module_id: ModuleId, pattern: &Pattern, name: &str) -> bool {
        self.resolved_pattern_symbol(module_id, pattern)
            .is_some_and(|symbol| self.is_builtin_value(symbol, name))
    }

    fn pattern_binding_symbol(&self, module_id: ModuleId, pattern: &Pattern) -> Option<SymbolId> {
        let (semantic_id, name) = match pattern {
            Pattern::Binding {
                semantic_id, name, ..
            }
            | Pattern::Typed {
                semantic_id,
                name: Some(name),
                ..
            }
            | Pattern::Alias {
                semantic_id, name, ..
            } => (semantic_id, name),
            _ => return None,
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
            // `Never` has no values: the empty failure set.
            _ if self.is_never(type_id) => Some(FailureRowId::new(0)),
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
        // One unrestricted failure-set parameter projects to the parameter
        // itself, so `Failure(error)` over `errors E` binds a value of type `E`
        // when the signature also uses `E` as a value type.
        if let ([], [parameter], []) = (
            row.variants.as_slice(),
            row.parameters.as_slice(),
            row.exclusions.as_slice(),
        ) && self.generic_kinds.get(parameter) == Some(&GenericKind::FailureSet)
        {
            return Some(self.intern_type(CanonicalType::Generic(*parameter)));
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
        // A generic user enum keeps the type arguments its variants were raised
        // with, so `Failure(error)` over `Bounce<Order>` failures binds
        // `Bounce<Order>`, not the declaration's `Bounce<T>`. A variant that does
        // not mention a parameter (`Bounce.Lost`) records it as poison, which
        // agrees with any argument.
        if let Some(arguments) = self.merged_variant_arguments(&row) {
            return Some(self.intern_type(CanonicalType::Named {
                symbol: *owner,
                arguments,
            }));
        }
        let owner = self.resolved.symbol(*owner);
        self.declaration_types
            .get(&(owner.module_id?, owner.semantic_id?))
            .copied()
            .or_else(|| Some(self.intern_type(CanonicalType::FailureSetValue(failures))))
    }

    /// The one argument list every raised variant of `row` agrees on, position by
    /// position, with poison agreeing with anything. `None` when two variants
    /// disagree or no position is known.
    fn merged_variant_arguments(&mut self, row: &FailureRow) -> Option<Vec<TypeId>> {
        let poison = TypeId::new(0);
        let mut merged: Option<Vec<TypeId>> = None;
        for candidate in &row.variant_arguments {
            if !row.variants.contains(&candidate.variant) {
                continue;
            }
            let arguments = candidate
                .arguments
                .iter()
                .map(|argument| self.resolve_inference_type(*argument))
                .collect::<Vec<_>>();
            let Some(current) = merged.as_mut() else {
                merged = Some(arguments);
                continue;
            };
            if current.len() != arguments.len() {
                return None;
            }
            for (slot, argument) in current.iter_mut().zip(arguments) {
                if *slot == poison {
                    *slot = argument;
                } else if argument != poison && argument != *slot {
                    return None;
                }
            }
        }
        merged.filter(|arguments| arguments.iter().any(|argument| *argument != poison))
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

    /// Whether `actual` may be stored in a struct or record field, or passed as
    /// a call argument, of type `expected`: the `using` provider rule, so a
    /// function value may expose fewer failures and needs than `expected`
    /// declares.
    fn value_fits(&mut self, actual: TypeId, expected: TypeId) -> bool {
        self.provider_member_conformance_error(actual, expected)
            .is_none()
    }

    fn provider_member_conformance_error(
        &mut self,
        actual: TypeId,
        expected: TypeId,
    ) -> Option<&'static str> {
        let actual = self.resolve_inference_type(actual);
        let expected = self.resolve_inference_type(expected);
        if actual == expected {
            return None;
        }
        match (
            self.types[actual.raw() as usize].clone(),
            self.types[expected.raw() as usize].clone(),
        ) {
            // A generic slot, such as a generic interface member, is used at
            // any instantiation, so whatever fills it must be as generic.
            (
                _,
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
            ) => self.generic_member_conformance_error(actual, &parameters, contract),
            (CanonicalType::Function(actual), CanonicalType::Function(expected)) => {
                if actual.inputs.len() != expected.inputs.len() {
                    return Some("input count differs");
                }
                if !actual
                    .inputs
                    .iter()
                    .zip(&expected.inputs)
                    .all(|(actual, expected)| {
                        self.function_input_depth += 1;
                        let unified = self.unify(*actual, *expected);
                        self.function_input_depth -= 1;
                        unified
                    })
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

    /// Whether provider member `actual` fits a generic interface member
    /// `fn<parameters>(contract)`. The member's parameters stay rigid: the
    /// provider must work for every choice of them, so a generic provider is
    /// instantiated afresh and matched against the member's contract. The check
    /// leaves no trace when it fails, and it fails when it would bind a type
    /// that existed before it (such as an unannotated lambda's parameter) to
    /// one of the member's parameters, since that parameter would then escape
    /// the member's scope.
    fn generic_member_conformance_error(
        &mut self,
        actual: TypeId,
        parameters: &[SymbolId],
        contract: CallableContract,
    ) -> Option<&'static str> {
        let bindings = self.inference_bindings.clone();
        let bounded = self.bounded_inference_variables.clone();
        let property_checks = self.property_checks.len();
        let existing_variables = self.next_inference_variable;
        let reason = self.generic_member_mismatch(actual, parameters, contract);
        let reason = reason.or_else(|| {
            let escapes = (0..existing_variables).any(|variable| {
                let bound = self.inference_bindings.get(&variable).copied();
                bound != bindings.get(&variable).copied()
                    && bound.is_some_and(|bound| self.type_mentions_generics(bound, parameters))
            });
            escapes.then_some(
                "it is not generic; supply a generic function such as one declared `fn name<A>(..)`",
            )
        });
        if reason.is_some() {
            self.inference_bindings = bindings;
            self.bounded_inference_variables = bounded;
            self.property_checks.truncate(property_checks);
        }
        reason
    }

    fn generic_member_mismatch(
        &mut self,
        actual: TypeId,
        parameters: &[SymbolId],
        contract: CallableContract,
    ) -> Option<&'static str> {
        let expected = self.intern_type(CanonicalType::Function(contract));
        let actual = self.resolve_inference_type(actual);
        let CanonicalType::PolymorphicFunction {
            parameters: own,
            contract: own_contract,
        } = self.types[actual.raw() as usize].clone()
        else {
            return self.provider_member_conformance_error(actual, expected);
        };
        let (instance, substitution) =
            self.instantiate_function_value_with(&own, own_contract, Some(expected));
        if let Some(reason) = self.provider_member_conformance_error(instance, expected) {
            return Some(reason);
        }
        // The member promises nothing about its parameters, so a provider that
        // compares or orders a value of one of them does not fit.
        let constrains_member_parameter = own.iter().any(|parameter| {
            self.generic_requirements.contains_key(parameter)
                && substitution
                    .values
                    .get(parameter)
                    .is_some_and(|argument| self.type_mentions_generics(*argument, parameters))
        });
        constrains_member_parameter.then_some(
            "it compares or orders a value of a member type parameter, which the interface does not promise",
        )
    }

    /// Whether `type_id`, its rows included, mentions any of `generics`.
    fn type_mentions_generics(&self, type_id: TypeId, generics: &[SymbolId]) -> bool {
        let mentions = |type_id: TypeId| self.type_mentions_generics(type_id, generics);
        let contract_mentions = |contract: &CallableContract| {
            contract.inputs.iter().copied().any(mentions)
                || mentions(contract.success)
                || self.failure_row_mentions_generics(contract.failures, generics)
                || self.needs_row_mentions_generics(contract.needs, generics)
        };
        match &self.types[self.resolve_inference_type(type_id).raw() as usize] {
            CanonicalType::Generic(symbol) => generics.contains(symbol),
            CanonicalType::Named { arguments, .. } => arguments.iter().copied().any(mentions),
            CanonicalType::FailureSetValue(row) => self.failure_row_mentions_generics(*row, generics),
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => contract_mentions(contract),
            CanonicalType::Record(fields) => fields.iter().any(|field| mentions(field.type_id)),
            CanonicalType::Tuple(elements) => elements.iter().copied().any(mentions),
            CanonicalType::InferenceVariable(_) | CanonicalType::Poison => false,
        }
    }

    fn failure_row_mentions_generics(&self, row: FailureRowId, generics: &[SymbolId]) -> bool {
        let row = &self.failure_rows[row.raw() as usize];
        row.parameters.iter().any(|parameter| generics.contains(parameter))
            || row.exclusions.iter().any(|exclusion| generics.contains(&exclusion.parameter))
            || row.variant_arguments.iter().any(|arguments| {
                arguments
                    .arguments
                    .iter()
                    .any(|argument| self.type_mentions_generics(*argument, generics))
            })
    }

    fn needs_row_mentions_generics(&self, row: NeedsRowId, generics: &[SymbolId]) -> bool {
        let row = &self.needs_rows[row.raw() as usize];
        row.parameters.iter().any(|parameter| generics.contains(parameter))
            || row
                .entries
                .iter()
                .any(|need| self.type_mentions_generics(need.interface, generics))
    }

    fn failure_row_is_subset(&self, actual: FailureRowId, expected: FailureRowId) -> bool {
        let actual = &self.failure_rows[actual.raw() as usize];
        let expected = &self.failure_rows[expected.raw() as usize];
        // A variant of a generic enum is the same member only at the same type
        // arguments: `W<Text>.Bad` is not in a set that holds `W<Int>.Bad`.
        let arguments = |row: &FailureRow, variant: SymbolId| {
            row.variant_arguments
                .iter()
                .find(|arguments| arguments.variant == variant)
                .map(|arguments| {
                    arguments
                        .arguments
                        .iter()
                        .map(|argument| self.resolve_inference_type(*argument))
                        .collect::<Vec<_>>()
                })
        };
        actual
            .variants
            .iter()
            .all(|variant| {
                expected.variants.contains(variant)
                    && arguments(actual, *variant) == arguments(expected, *variant)
            })
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
        let generic_function_type_allowed = std::mem::take(&mut self.generic_function_type_allowed);
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
                    (Some(_), Some(symbol)) if self.is_failure_variant(symbol) => {
                        self.variant_failure_set_type(symbol)
                    }
                    (Some(name), Some(symbol)) => {
                        let kind = self.resolved.symbol(symbol).kind;
                        if kind == ProjectSymbolKind::GenericParameter {
                            self.constrain_generic(module_id, symbol, GenericKind::Value, name);
                            self.intern_type(CanonicalType::Generic(symbol))
                        } else {
                            let lowered = arguments
                                .iter()
                                .map(|argument| {
                                    self.lower_value_type(module_id, argument, generics)
                                })
                                .collect::<Vec<_>>();
                            // `E` in `Task<T, E>` is a failure set: a generic
                            // there also denotes the task's failure row.
                            if matches!(
                                self.resolved.standard_symbol(symbol),
                                Some(StandardSymbol::Type(
                                    StandardType::Task | StandardType::ProcessSpec
                                ))
                            ) && let Some(TypeUse::Named {
                                semantic_id: failures_id,
                                path: failures_path,
                                ..
                            }) = arguments.get(1)
                                && let (Some(name), Some(failures)) = (
                                    failures_path.last(),
                                    self.resolved_type_symbol(module_id, *failures_id),
                                )
                                && self.resolved.symbol(failures).kind
                                    == ProjectSymbolKind::GenericParameter
                            {
                                self.constrain_generic(
                                    module_id,
                                    failures,
                                    GenericKind::FailureRow,
                                    name,
                                );
                            }
                            let role = match self.resolved.standard_symbol(symbol) {
                                Some(StandardSymbol::Type(StandardType::Map)) => Some("`Map` keys"),
                                Some(StandardSymbol::Type(StandardType::Set)) => {
                                    Some("`Set` items")
                                }
                                _ => None,
                            };
                            if let (Some(role), Some(key)) = (role, lowered.first()) {
                                if let Some(pending) = &mut self.written_map_keys {
                                    pending.push((module_id, name.range, *key, role));
                                } else {
                                    self.check_written_map_key(module_id, name.range, *key, role);
                                }
                            }
                            self.intern_type(CanonicalType::Named {
                                symbol,
                                arguments: lowered,
                            })
                        }
                    }
                    _ => TypeId::new(0),
                }
            }
            TypeUse::Function {
                semantic_id,
                generic_parameters,
                inputs,
                success,
                failures,
                requirements,
            } => {
                if let Some(first) = generic_parameters.first()
                    && !generic_function_type_allowed
                {
                    self.type_error(
                        module_id,
                        "T4360",
                        first,
                        "a generic function type is allowed only as the whole type of an \
                         interface member; declare a generic function or interface instead",
                    );
                }
                let parameters = generic_parameters
                    .iter()
                    .filter_map(|name| {
                        self.find_symbol(
                            module_id,
                            Some(*semantic_id),
                            ProjectSymbolKind::GenericParameter,
                            &name.text,
                        )
                    })
                    .collect::<Vec<_>>();
                for parameter in &parameters {
                    self.generic_kinds
                        .entry(*parameter)
                        .or_insert(GenericKind::Unconstrained);
                }
                // Rows look their parameters up by name, so the member's own
                // generics join the ones in scope.
                let mut scoped;
                let generics = if parameters.is_empty() {
                    generics
                } else {
                    scoped = generics.clone();
                    for parameter in &parameters {
                        scoped.insert(self.resolved.symbol(*parameter).name.clone(), *parameter);
                    }
                    &scoped
                };
                let inputs = inputs
                    .iter()
                    .map(|input| self.lower_value_type(module_id, input, generics))
                    .collect();
                let success = self.lower_value_type(module_id, success, generics);
                let failures = failures.as_ref().map_or(FailureRowId::new(0), |failures| {
                    self.lower_failure_row(module_id, failures, generics)
                });
                let needs = self.lower_needs_row(module_id, requirements, generics);
                let contract = CallableContract {
                    inputs,
                    success,
                    failures,
                    needs,
                };
                if parameters.is_empty() || !generic_function_type_allowed {
                    self.intern_type(CanonicalType::Function(contract))
                } else {
                    self.intern_type(CanonicalType::PolymorphicFunction {
                        parameters,
                        contract,
                    })
                }
            }
            TypeUse::Union { members, .. } => self.lower_failure_set_type(module_id, members),
            TypeUse::Tuple { elements, .. } => {
                let elements = elements
                    .iter()
                    .map(|element| self.lower_value_type(module_id, element, generics))
                    .collect();
                self.intern_type(CanonicalType::Tuple(elements))
            }
            TypeUse::Record { fields, .. } => {
                let mut structural_fields = Vec::<StructuralField>::new();
                for field in fields {
                    let type_id = self.lower_value_type(module_id, &field.type_use, generics);
                    if structural_fields
                        .iter()
                        .any(|existing| existing.name == field.name.text)
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
                            type_id,
                        });
                    }
                }
                structural_fields.sort_by(|left, right| left.name.cmp(&right.name));
                self.intern_type(CanonicalType::Record(structural_fields))
            }
        };
        self.type_facts
            .insert((module_id, type_use.semantic_id()), result);
        result
    }

    /// Rejects a written `Map<K, V>` whose `K` is not equatable, at the `Map`
    /// name. A generic `K` is left to the code that builds the map.
    /// `role` names what `key` is, as "`Map` keys".
    fn check_written_map_key(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        key: TypeId,
        role: &str,
    ) {
        let Some(lack) = self.type_view().derive(key, ValueProperty::Equatable).lack else {
            return;
        };
        let found = self.format_checked_type(key);
        self.type_error_range(
            module_id,
            "T4300",
            range,
            format!(
                "{role} must be equatable, but `{found}` is not equatable: {}",
                lack.describe()
            ),
        );
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

    /// Infers call arguments in source order, except that a function literal
    /// argument is inferred after the others: its unannotated parameters first
    /// take the callee's parameter types (instantiated by the other arguments),
    /// so its body can use them (`apply(money, fn(x) { x.cents })`). The call
    /// itself is still matched by `infer_invocation`.
    fn infer_call_arguments(
        &mut self,
        module_id: ModuleId,
        callee_type: TypeId,
        arguments: &[crate::semantic::CallArgument],
        generics: &HashMap<String, SymbolId>,
        bindings: &mut HashMap<SymbolId, ValueBinding>,
    ) -> Vec<ExpressionFact> {
        let callee_type = self.resolve_inference_type(callee_type);
        let (inputs, mut substitution) = match self.types[callee_type.raw() as usize].clone() {
            CanonicalType::Function(contract) => (contract.inputs, None),
            CanonicalType::PolymorphicFunction { contract, .. } => {
                (contract.inputs, Some(CallSubstitution::default()))
            }
            _ => (Vec::new(), None),
        };
        let is_function_literal = |argument: &crate::semantic::CallArgument| {
            matches!(argument.value, ModuleExpression::Function { .. })
        };
        let mut facts = vec![None; arguments.len()];
        for (index, argument) in arguments.iter().enumerate() {
            if is_function_literal(argument) && inputs.get(index).is_some() {
                continue;
            }
            let fact = self.infer_expression(module_id, &argument.value, generics, bindings);
            if let (Some(substitution), Some(expected)) = (&mut substitution, inputs.get(index)) {
                // Errors are reported when `infer_invocation` matches the call.
                self.match_call_type(fact.success, *expected, substitution);
            }
            facts[index] = Some(fact);
        }
        // Literals whose parameter types the other arguments already fix go first,
        // and each literal's inferred type binds the callee's parameters for the
        // literals after it (`later(fn(m) { m.cents }, fn() { Money(4) })`).
        let mut pending = (0..arguments.len())
            .filter(|index| facts[*index].is_none())
            .collect::<Vec<_>>();
        while !pending.is_empty() {
            let position = match &substitution {
                Some(substitution) => pending
                    .iter()
                    .position(|index| {
                        self.literal_parameters_are_known(inputs[*index], substitution)
                    })
                    .unwrap_or(0),
                None => 0,
            };
            let index = pending.remove(position);
            let argument = &arguments[index];
            let expected = match &mut substitution {
                Some(substitution) => self.literal_slot(inputs[index], substitution),
                None => inputs[index],
            };
            self.seed_function_literal_parameters(module_id, &argument.value, expected, generics);
            let fact = self.infer_expression(module_id, &argument.value, generics, bindings);
            if let Some(substitution) = &mut substitution {
                // Errors are reported when `infer_invocation` matches the call.
                self.match_call_type(fact.success, inputs[index], substitution);
            }
            facts[index] = Some(fact);
        }
        facts
            .into_iter()
            .map(|fact| fact.expect("every argument is inferred"))
            .collect()
    }

    /// Whether every parameter type of the function slot `slot` is fixed by
    /// `substitution`: no callee generic in them is unbound or bound only to an
    /// open inference variable, and no callee row parameter in them is unbound.
    fn literal_parameters_are_known(&self, slot: TypeId, substitution: &CallSubstitution) -> bool {
        let slot = self.resolve_inference_type(slot);
        let CanonicalType::Function(contract) = &self.types[slot.raw() as usize] else {
            return true;
        };
        contract
            .inputs
            .iter()
            .all(|input| self.type_is_known(*input, substitution, &mut HashSet::new()))
    }

    fn type_is_known(
        &self,
        type_id: TypeId,
        substitution: &CallSubstitution,
        visiting: &mut HashSet<TypeId>,
    ) -> bool {
        let type_id = self.resolve_inference_type(type_id);
        if !visiting.insert(type_id) {
            return true;
        }
        match &self.types[type_id.raw() as usize] {
            CanonicalType::Generic(parameter) => {
                let bound = substitution
                    .values
                    .get(parameter)
                    .map(|bound| self.resolve_inference_type(*bound));
                match bound {
                    Some(bound) => !matches!(
                        self.types[bound.raw() as usize],
                        CanonicalType::InferenceVariable(_)
                    ),
                    None => substitution.failures.contains_key(parameter),
                }
            }
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => {
                self.failure_rows[contract.failures.raw() as usize]
                    .parameters
                    .iter()
                    .all(|parameter| substitution.failures.contains_key(parameter))
                    && self.needs_rows[contract.needs.raw() as usize]
                        .parameters
                        .iter()
                        .all(|parameter| substitution.needs.contains_key(parameter))
                    && contract
                        .inputs
                        .iter()
                        .chain([&contract.success])
                        .all(|input| self.type_is_known(*input, substitution, visiting))
            }
            CanonicalType::Named { arguments, .. } => arguments
                .iter()
                .all(|argument| self.type_is_known(*argument, substitution, visiting)),
            CanonicalType::Record(fields) => fields
                .iter()
                .all(|field| self.type_is_known(field.type_id, substitution, visiting)),
            CanonicalType::Tuple(elements) => elements
                .iter()
                .all(|element| self.type_is_known(*element, substitution, visiting)),
            CanonicalType::FailureSetValue(failures) => self.failure_rows[failures.raw() as usize]
                .parameters
                .iter()
                .all(|parameter| substitution.failures.contains_key(parameter)),
            CanonicalType::InferenceVariable(_) => false,
            CanonicalType::Poison => true,
        }
    }

    /// The callee input `input` as a function literal's parameters should see it.
    /// A callee row parameter nothing has bound yet is read as empty: the literal
    /// is monomorphic, so its parameter types must not name the callee's own row
    /// parameters, and the empty row is a valid instantiation the call match can
    /// still confirm. Fresh inference variables for unbound value generics are
    /// kept in `substitution`, so later arguments share them.
    fn literal_slot(&mut self, input: TypeId, substitution: &mut CallSubstitution) -> TypeId {
        let mut closed = substitution.clone();
        for parameter in self.type_row_parameters(input) {
            if !substitution.failures.contains_key(&parameter)
                && !substitution.needs.contains_key(&parameter)
            {
                closed.failures.insert(parameter, FailureRowId::new(0));
                closed.needs.insert(parameter, NeedsRowId::new(0));
            }
        }
        let slot = self.substitute_call_type(input, &mut closed);
        substitution.values = closed.values;
        slot
    }

    /// Gives each unannotated parameter of a function literal the matching
    /// input of `expected`, when that is a function type of the same arity.
    fn seed_function_literal_parameters(
        &mut self,
        module_id: ModuleId,
        literal: &ModuleExpression,
        expected: TypeId,
        generics: &HashMap<String, SymbolId>,
    ) {
        let ModuleExpression::Function { signature, .. } = literal else {
            return;
        };
        let expected = self.resolve_inference_type(expected);
        let CanonicalType::Function(expected) = self.types[expected.raw() as usize].clone() else {
            return;
        };
        if expected.inputs.len() != signature.parameters.len() {
            return;
        }
        let key = (module_id, literal.semantic_id());
        // A later fixed-point pass reuses the literal's contract from the
        // previous one, as the function-literal case does.
        let previous = self.expression_facts.get(&key).and_then(|fact| {
            let type_id = self.resolve_inference_type(fact.success);
            match &self.types[type_id.raw() as usize] {
                CanonicalType::Function(contract) => Some(contract.clone()),
                _ => None,
            }
        });
        let contract =
            previous.unwrap_or_else(|| self.lower_signature(module_id, signature, generics));
        for (input, expected) in contract.inputs.iter().zip(expected.inputs) {
            let input = self.resolve_inference_type(*input);
            if matches!(
                self.types[input.raw() as usize],
                CanonicalType::InferenceVariable(_)
            ) {
                self.unify(input, expected);
            }
        }
        let success = self.intern_type(CanonicalType::Function(contract));
        self.expression_facts.insert(
            key,
            ExpressionFact {
                success,
                failures: FailureRowId::new(0),
                needs: NeedsRowId::new(0),
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_invocation(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        callee_id: SemanticId,
        callee_symbol: Option<SymbolId>,
        callee: ExpressionFact,
        arguments: &[ExpressionFact],
        argument_ranges: &[TextRange],
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
            // A value whose type is still open, such as an unannotated parameter,
            // becomes a function from this call: one input per argument, an
            // open result, and no failures or needs.
            Some(CanonicalType::InferenceVariable(_)) => {
                let contract = CallableContract {
                    inputs: arguments
                        .iter()
                        .map(|_| self.fresh_inference_type())
                        .collect(),
                    success: self.fresh_inference_type(),
                    failures: FailureRowId::new(0),
                    needs: NeedsRowId::new(0),
                };
                let function = self.intern_type(CanonicalType::Function(contract.clone()));
                self.unify(callee_type, function);
                (contract, Vec::new(), None)
            }
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
        // A generic function value is matched after the other arguments, so the
        // callee's row parameters it meets are already bound (`apply(run,
        // failing)` binds `F` from `failing` before instantiating `run`).
        let mut ordered = arguments
            .iter()
            .zip(argument_ranges.iter().copied())
            .zip(contract.inputs.iter().copied())
            .collect::<Vec<_>>();
        ordered.sort_by_key(|((argument, _), _)| {
            let argument = self.resolve_inference_type(argument.success);
            matches!(
                self.types[argument.raw() as usize],
                CanonicalType::PolymorphicFunction { .. }
            )
        });
        let constructs_struct = callee_symbol.is_some_and(|symbol| {
            self.resolved.symbol(symbol).kind == ProjectSymbolKind::StructConstructor
        });
        for ((argument, argument_range), expected) in ordered {
            if let Some(substitution) = &mut substitution {
                if !self.match_call_argument(argument.success, expected, substitution) {
                    // Show the parameter with what the call has bound so far, without
                    // binding anything new for the arguments still to match.
                    let expected = self.substitute_call_type(expected, &mut substitution.clone());
                    if let Some(message) =
                        self.failure_set_shortfall("call argument", argument.success, expected)
                    {
                        self.type_error_range(module_id, "T4346", argument_range, message);
                    } else {
                        let message =
                            self.mismatch_message("call argument", argument.success, expected);
                        self.type_error_range(module_id, "T4203", argument_range, message);
                    }
                }
            } else if constructs_struct || self.both_functions(argument.success, expected) {
                if !self.value_fits(argument.success, expected) {
                    if let Some(message) =
                        self.failure_set_shortfall("call argument", argument.success, expected)
                    {
                        self.type_error_range(module_id, "T4346", argument_range, message);
                    } else {
                        let message =
                            self.mismatch_message("call argument", argument.success, expected);
                        self.type_error_range(module_id, "T4203", argument_range, message);
                    }
                }
            } else {
                self.unify_at(
                    module_id,
                    argument_range,
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
            for parameter in &parameters {
                let (Some(requirement), Some(argument)) = (
                    self.instantiated_requirement(*parameter),
                    substitution.values.get(parameter).copied(),
                ) else {
                    continue;
                };
                self.property_checks.push(PropertyCheck {
                    module_id,
                    type_id: argument,
                    property: requirement.property,
                    site: PropertySite::Instantiation {
                        range,
                        callee: callee_symbol.map_or_else(
                            || self.generic_owner_name(*parameter),
                            |symbol| self.callable_display_name(symbol),
                        ),
                        parameter: *parameter,
                        requirement,
                    },
                });
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

    /// `Map.put` for a standard function, the declared name otherwise.
    /// The requirement a use of generic `parameter` checks. An operator use
    /// inside a standard module is not the caller's to fix: the use of the
    /// standard function is, as for an intrinsic, so the diagnostic and every
    /// requirement it propagates stay in the caller's source.
    fn instantiated_requirement(&self, parameter: SymbolId) -> Option<GenericRequirement> {
        let mut requirement = self.generic_requirements.get(&parameter).cloned()?;
        if requirement
            .origin
            .as_ref()
            .is_some_and(|origin| self.resolved.is_standard_module(origin.module_id))
        {
            requirement.origin = None;
        }
        Some(requirement)
    }

    fn callable_display_name(&self, symbol: SymbolId) -> String {
        let declared = self.resolved.symbol(symbol);
        let name = declared.name.clone();
        if let Some(module_id) = declared.module_id
            && self.resolved.is_standard_module(module_id)
        {
            let path = &self.resolved.semantic.modules[module_id.raw() as usize].path;
            return format!("{}.{name}", crate::standard_tree::conventional_alias(path));
        }
        match self.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Function(function)) => match function.module() {
                Some(module) => format!("{}.{name}", module.qualifier()),
                None => name,
            },
            _ => name,
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
                .or_else(|| self.standard_constant_type(symbol))
        }
    }

    /// The type of a standard module constant such as `Duration.zero`.
    fn standard_constant_type(&mut self, symbol: SymbolId) -> Option<TypeId> {
        match self.resolved.standard_symbol(symbol)? {
            StandardSymbol::Constant(StandardConstant::DurationZero) => {
                Some(self.standard_named_type(StandardType::Duration, Vec::new()))
            }
            _ => None,
        }
    }

    fn callable_type_for_symbol(&mut self, symbol: SymbolId, contract: CallableContract) -> TypeId {
        // A type the function's body leaves open is generic in its inferred
        // contract: each reference gets its own copy, so one caller cannot fix
        // it for another (executable-kernel, "Generics").
        let contract = if self.resolved.symbol(symbol).kind == ProjectSymbolKind::Function {
            self.instantiate_open_contract(contract, &mut HashMap::new())
        } else {
            contract
        };
        let parameters = self.callable_generic_parameters(symbol);
        if parameters.is_empty() {
            self.intern_type(CanonicalType::Function(contract))
        } else {
            self.polymorphic_owners
                .entry(parameters.clone())
                .or_insert(symbol);
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
        let struct_symbol = *symbol;
        let declaring = &self.resolved.semantic.modules[owner_module.raw() as usize];
        // An accessor is a published function named after the field whose one
        // parameter is the struct.
        let has_accessor = declaring.declarations.iter().any(|declaration| {
            declaration.kind == ModuleDeclarationKind::Function
                && declaration.public
                && declaration.name.text == member
                && self
                    .declaration_symbol(owner_module, declaration)
                    .and_then(|function| self.contracts.get(&function))
                    .is_some_and(|contract| match contract.inputs.as_slice() {
                        [input] => matches!(
                            &self.types[self.resolve_inference_type(*input).raw() as usize],
                            CanonicalType::Named { symbol, .. } if *symbol == struct_symbol
                        ),
                        _ => false,
                    })
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

    /// The type of `member` read from a value of type `receiver`, with the
    /// declaration's generic parameters replaced by the receiver's arguments
    /// (`value: T` read from a `Box<Int>` is `Int`).
    fn instantiated_field_type(&mut self, receiver: TypeId, member: &str) -> Option<TypeId> {
        let declared = self.field_type(receiver, member)?;
        let receiver = self.resolve_inference_type(receiver);
        let CanonicalType::Named { symbol, arguments } =
            self.types[receiver.raw() as usize].clone()
        else {
            return Some(declared);
        };
        let owner = self.resolved.symbol(symbol);
        let (Some(module_id), Some(semantic_id)) = (owner.module_id, owner.semantic_id) else {
            return Some(declared);
        };
        let Some(declaration) = self.resolved.semantic.modules[module_id.raw() as usize]
            .declarations
            .iter()
            .find(|declaration| declaration.semantic_id == semantic_id)
            .cloned()
        else {
            return Some(declared);
        };
        if declaration.generic_parameters.len() != arguments.len() {
            return Some(declared);
        }
        let generics = self.generic_symbols(module_id, &declaration);
        let mut substitution = CallSubstitution::default();
        for (name, argument) in declaration.generic_parameters.iter().zip(arguments) {
            if let Some(parameter) = generics.get(&name.text) {
                substitution.values.insert(*parameter, argument);
            }
        }
        Some(self.substitute_call_type(declared, &mut substitution))
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
                    substitution.widenable_failures.remove(&parameter);
                    if let Some(bound) = substitution.failures.get(&parameter).copied() {
                        // A type still being inferred, as the input of `fn(_) {
                        // True }`, becomes the bound set.
                        if matches!(
                            self.types[actual.raw() as usize],
                            CanonicalType::InferenceVariable(_)
                        ) {
                            let bound = self.failure_set_type(bound);
                            return self.unify(actual, bound);
                        }
                        // Under a function input, a value of the slot's type is
                        // handed to `actual`, so `actual` must take every member
                        // of the bound set: a predicate over `A | B` serves an
                        // operation that fails with `A` (`T4346`).
                        let input = !self.function_input_depth.is_multiple_of(2);
                        return self.failure_row_for_type(actual).is_some_and(|actual| {
                            if input {
                                self.failure_row_is_subset(bound, actual)
                            } else {
                                self.failure_rows_equivalent(actual, bound)
                            }
                        });
                    }
                    if let Some(bound) = substitution.values.get(&parameter).copied() {
                        return self.unify(actual, bound);
                    }
                    // A failure set takes only a failure-set value, or its row
                    // would lose the failures it stands for.
                    let Some(failures) = self.failure_row_for_type(actual) else {
                        return matches!(
                            self.types[actual.raw() as usize],
                            CanonicalType::InferenceVariable(_) | CanonicalType::Poison
                        );
                    };
                    substitution.values.insert(parameter, actual);
                    substitution.failures.insert(parameter, failures);
                    return true;
                }
                if let Some(bound) = substitution.values.get(&parameter).copied() {
                    let unified = self.unify(actual, bound);
                    // A generic function bound first and a fixed one second
                    // share the fixed type: keeping the generic one would let
                    // the call use the fixed function at any type.
                    if unified
                        && self.is_polymorphic_function(bound)
                        && !self.is_polymorphic_function(actual)
                    {
                        substitution.values.insert(parameter, actual);
                    }
                    unified
                } else {
                    substitution.values.insert(parameter, actual);
                    true
                }
            }
            // A `Never` value fits any slot, as in `unify`; a function literal that
            // only fails reaches here when its type feeds the call substitution.
            _ if self.function_input_depth == 0 && self.is_never(actual) => true,
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
                    && actual_arguments
                        .into_iter()
                        .zip(expected_arguments)
                        .enumerate()
                        .all(|(index, (actual, expected))| {
                            self.match_call_type_argument(
                                actual_symbol,
                                index,
                                actual,
                                expected,
                                substitution,
                            )
                        })
            }
            (CanonicalType::Function(actual), CanonicalType::Function(expected)) => {
                actual.inputs.len() == expected.inputs.len()
                    && actual
                        .inputs
                        .into_iter()
                        .zip(expected.inputs)
                        .all(|(actual, expected)| {
                            self.function_input_depth += 1;
                            let matched = self.match_call_type(actual, expected, substitution);
                            self.function_input_depth -= 1;
                            matched
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
            (CanonicalType::Tuple(actual), CanonicalType::Tuple(expected)) => {
                actual.len() == expected.len()
                    && actual.into_iter().zip(expected).all(|(actual, expected)| {
                        self.match_call_type(actual, expected, substitution)
                    })
            }
            (
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
                CanonicalType::Function(_),
            ) => {
                // The slot as instantiated so far. A row parameter of the callee
                // still open in it must not bind the value's rows, or the
                // callee's own parameter would leak into the caller. A bound one
                // is not open even when its binding names the same symbol, as in
                // a self-recursive call that passes its own `F` along.
                let open = self
                    .type_row_parameters(expected)
                    .into_iter()
                    .filter(|parameter| {
                        !substitution.failures.contains_key(parameter)
                            && !substitution.needs.contains_key(parameter)
                    })
                    .collect::<Vec<_>>();
                let slot = self.substitute_call_type(expected, &mut substitution.clone());
                let slot = self
                    .type_row_parameters(slot)
                    .iter()
                    .all(|parameter| !open.contains(parameter))
                    .then_some(slot);
                let actual = self.instantiate_function_value(&parameters, contract, slot);
                self.match_call_type(actual, expected, substitution)
            }
            // An open argument type (an unannotated lambda parameter) takes the
            // instantiated parameter type, never the callee's own generics.
            (CanonicalType::InferenceVariable(_), _) => {
                let expected = self.substitute_call_type(expected, substitution);
                self.unify(actual, expected)
            }
            _ => self.unify(actual, expected),
        }
    }

    /// Matches the argument at `index` of two instances of the data type
    /// `owner` by its variance: a covariant argument as a value, one used only
    /// as a function input with the direction flipped, and any other only when
    /// both sides agree, so a `Handler<A>` holding a `fn(A)` does not pass for a
    /// `Handler<A | B>`.
    fn match_call_type_argument(
        &mut self,
        owner: SymbolId,
        index: usize,
        actual: TypeId,
        expected: TypeId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        if self.type_parameter_is_covariant(owner, index) {
            return self.match_call_type(actual, expected, substitution);
        }
        if self.type_parameter_is_contravariant(owner, index) {
            self.function_input_depth += 1;
            let matched = self.match_call_type(actual, expected, substitution);
            self.function_input_depth -= 1;
            return matched;
        }
        if !self.match_call_type(actual, expected, substitution) {
            return false;
        }
        let expected = self.substitute_call_type(expected, &mut substitution.clone());
        self.unify(actual, expected) && self.unify(expected, actual)
    }

    fn is_polymorphic_function(&self, type_id: TypeId) -> bool {
        matches!(
            self.types[self.resolve_inference_type(type_id).raw() as usize],
            CanonicalType::PolymorphicFunction { .. }
        )
    }

    fn both_functions(&mut self, actual: TypeId, expected: TypeId) -> bool {
        let actual = self.resolve_inference_type(actual);
        let expected = self.resolve_inference_type(expected);
        matches!(
            (
                &self.types[actual.raw() as usize],
                &self.types[expected.raw() as usize],
            ),
            (CanonicalType::Function(_), CanonicalType::Function(_))
        )
    }

    /// Matches a call argument against its parameter type. A function value may
    /// expose fewer failures and needs than the parameter's function type declares,
    /// the rule a `using` provider follows; every nested position matches exactly.
    fn match_call_argument(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        let resolved_actual = self.resolve_inference_type(actual);
        let resolved_expected = self.resolve_inference_type(expected);
        let (CanonicalType::Function(actual_contract), CanonicalType::Function(expected_contract)) = (
            self.types[resolved_actual.raw() as usize].clone(),
            self.types[resolved_expected.raw() as usize].clone(),
        ) else {
            return self.match_call_type(actual, expected, substitution);
        };
        actual_contract.inputs.len() == expected_contract.inputs.len()
            && actual_contract
                .inputs
                .into_iter()
                .zip(expected_contract.inputs)
                .all(|(actual, expected)| {
                    self.function_input_depth += 1;
                    let matched = self.match_call_type(actual, expected, substitution);
                    self.function_input_depth -= 1;
                    matched
                })
            && self.match_call_type(
                actual_contract.success,
                expected_contract.success,
                substitution,
            )
            && self.match_call_failure_row_within(
                actual_contract.failures,
                expected_contract.failures,
                substitution,
            )
            && self.match_call_needs_row_within(
                actual_contract.needs,
                expected_contract.needs,
                substitution,
            )
    }

    /// Whether failure row `actual` fits within `expected`. The failures outside
    /// `expected`'s variants bind its row parameter as a widenable lower bound.
    fn match_call_failure_row_within(
        &mut self,
        actual: FailureRowId,
        expected: FailureRowId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        let actual_row = self.failure_rows[actual.raw() as usize].clone();
        let expected_row = self.failure_rows[expected.raw() as usize].clone();
        let [parameter] = expected_row.parameters.as_slice() else {
            return expected_row.parameters.is_empty()
                && self.failure_row_is_subset(actual, expected);
        };
        let variants = actual_row
            .variants
            .into_iter()
            .filter(|variant| !expected_row.variants.contains(variant))
            .collect::<Vec<_>>();
        let variant_arguments = actual_row
            .variant_arguments
            .into_iter()
            .filter(|arguments| variants.contains(&arguments.variant))
            .collect();
        let residual = self.intern_failure_row(FailureRow {
            variants,
            variant_arguments,
            parameters: actual_row.parameters,
            exclusions: actual_row.exclusions,
        });
        match substitution.failures.get(parameter).copied() {
            None => {
                substitution.failures.insert(*parameter, residual);
                substitution.widenable_failures.insert(*parameter);
                true
            }
            Some(bound) if substitution.widenable_failures.contains(parameter) => {
                let widened = self.union_failure_rows([bound, residual]);
                substitution.failures.insert(*parameter, widened);
                true
            }
            Some(bound) => self.failure_row_is_subset(residual, bound),
        }
    }

    /// Whether needs row `actual` fits within `expected`, binding `expected`'s row
    /// parameter as `match_call_failure_row_within` does.
    fn match_call_needs_row_within(
        &mut self,
        actual: NeedsRowId,
        expected: NeedsRowId,
        substitution: &mut CallSubstitution,
    ) -> bool {
        let actual_row = self.needs_rows[actual.raw() as usize].clone();
        let expected_row = self.needs_rows[expected.raw() as usize].clone();
        let mut remaining = Vec::new();
        for need in actual_row.entries {
            match expected_row
                .entries
                .iter()
                .find(|expected| expected.lexical_name == need.lexical_name)
            {
                Some(expected) => {
                    if !self.match_call_type(need.interface, expected.interface, substitution) {
                        return false;
                    }
                }
                None => remaining.push(need),
            }
        }
        let [parameter] = expected_row.parameters.as_slice() else {
            return expected_row.parameters.is_empty()
                && remaining.is_empty()
                && actual_row.parameters.is_empty();
        };
        let residual = self.intern_needs_row(NeedsRow {
            entries: remaining,
            parameters: actual_row.parameters,
        });
        match substitution.needs.get(parameter).copied() {
            None => {
                substitution.needs.insert(*parameter, residual);
                substitution.widenable_needs.insert(*parameter);
                true
            }
            Some(bound) if substitution.widenable_needs.contains(parameter) => {
                let widened = self.union_needs_rows([bound, residual]);
                substitution.needs.insert(*parameter, widened);
                true
            }
            Some(bound) => self.needs_row_is_subset(residual, bound),
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
                    if substitution.widenable_failures.remove(parameter) {
                        substitution.failures.insert(*parameter, residual);
                        self.failure_row_is_subset(bound, residual)
                    } else {
                        self.failure_rows_equivalent(bound, residual)
                    }
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
                    if substitution.widenable_needs.remove(parameter) {
                        substitution.needs.insert(*parameter, residual);
                        self.needs_row_is_subset(bound, residual)
                    } else {
                        self.needs_rows_equivalent(bound, residual)
                    }
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
            // Under the value's own binder only the generics this substitution
            // already binds are replaced, such as `K` in a member `get: fn<A>(K,
            // A) -> A` of `Store<Text>`. Its own parameters, and generics the
            // substitution leaves open, stay as they are.
            CanonicalType::PolymorphicFunction {
                parameters,
                contract,
            } => {
                let mut inner = substitution.clone();
                for parameter in &parameters {
                    inner.values.remove(parameter);
                    inner.failures.remove(parameter);
                    inner.needs.remove(parameter);
                }
                let mut mentioned = Vec::new();
                let formatter = self.formatter();
                for type_id in contract.inputs.iter().chain([&contract.success]) {
                    formatter.collect_generics(*type_id, &mut mentioned);
                }
                // Every generic the contract mentions that `inner` leaves open
                // is kept, the value's own parameters among them.
                let mut inner_preserved = preserved.to_vec();
                inner_preserved.extend(
                    mentioned
                        .into_iter()
                        .filter(|generic| {
                            !inner.values.contains_key(generic)
                                && !inner.failures.contains_key(generic)
                        }),
                );
                let contract =
                    self.substitute_call_contract_preserving(contract, &mut inner, &inner_preserved);
                self.intern_type(CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                })
            }
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
            CanonicalType::Tuple(elements) => {
                let elements = elements
                    .into_iter()
                    .map(|element| {
                        self.substitute_call_type_preserving(element, substitution, preserved)
                    })
                    .collect();
                self.intern_type(CanonicalType::Tuple(elements))
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

    /// The type shared by `joined` (the alternatives so far) and `actual`
    /// (the next case arm or list element). When `actual` is wider, such as
    /// a whole enum after a narrowed failure set, the join widens to it.
    fn join_branch_type(
        &mut self,
        module_id: ModuleId,
        range: TextRange,
        actual: TypeId,
        joined: TypeId,
        context: &str,
    ) -> TypeId {
        if let Some(join) = self.join_alternative_types(actual, joined) {
            return join;
        }
        if self.are_distinct_enums(actual, joined) {
            let joined_name = self.type_display_name(joined);
            let actual_name = self.type_display_name(actual);
            self.type_error_range(
                module_id,
                "T4341",
                range,
                format!(
                    "{context} has type `{actual_name}`, but earlier ones have `{joined_name}`; \
                     two different enums do not mix silently. To keep both, give a value the \
                     failure-set type, as in `let value: {joined_name} | {actual_name} = ...`"
                ),
            );
            return joined;
        }
        self.type_error_range(
            module_id,
            "T4203",
            range,
            format!("{context} has an incompatible type"),
        );
        joined
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
        if let Some(message) = self.failure_set_shortfall(context, actual, expected) {
            self.type_error_range(module_id, "T4346", range, message);
            return false;
        }
        let message = self.mismatch_message(context, actual, expected);
        self.type_error_range(module_id, "T4203", range, message);
        false
    }

    /// `"{subject} has type `A`, expected `E`"`. Two function types that agree on
    /// their inputs and result differ only in a row, and the message says which.
    fn mismatch_message(&self, subject: &str, actual: TypeId, expected: TypeId) -> String {
        let formatter = self.formatter();
        let found = formatter.format_type(actual);
        let wanted = formatter.format_type(expected);
        let mut message = format!("{subject} has type `{found}`, expected `{wanted}`");
        let contract =
            |id: TypeId| match &self.types[self.resolve_inference_type(id).raw() as usize] {
                CanonicalType::Function(contract)
                | CanonicalType::PolymorphicFunction { contract, .. } => Some(contract.clone()),
                _ => None,
            };
        if let (Some(actual), Some(expected)) = (contract(actual), contract(expected)) {
            let shape = |contract: &CallableContract| {
                (
                    contract
                        .inputs
                        .iter()
                        .map(|input| formatter.format_type(*input))
                        .collect::<Vec<_>>(),
                    formatter.format_type(contract.success),
                )
            };
            if shape(&actual) == shape(&expected) {
                let failures = |contract: &CallableContract| {
                    formatter
                        .format_failure_row(&self.failure_rows[contract.failures.raw() as usize])
                };
                let needs = |contract: &CallableContract| {
                    formatter.format_needs_row(&self.needs_rows[contract.needs.raw() as usize])
                };
                if failures(&actual) != failures(&expected) {
                    message.push_str(
                        "; they differ only in what they may fail with (the failure row)",
                    );
                } else if needs(&actual) != needs(&expected) {
                    message.push_str(
                        "; they differ only in the capabilities they need (the needs row)",
                    );
                }
            }
        }
        message
    }

    /// The last inference variable on `id`'s binding chain, if `id` is one.
    fn last_inference_variable(&self, mut id: TypeId) -> Option<u32> {
        let mut last = None;
        while let CanonicalType::InferenceVariable(variable) = &self.types[id.raw() as usize] {
            last = Some(*variable);
            match self.inference_bindings.get(variable).copied() {
                Some(bound) if bound != id => id = bound,
                _ => break,
            }
        }
        last
    }

    /// Unifies `actual` with `expected`. The check is directional for
    /// narrowed failure sets: a value known to be one of an enum's variants
    /// is accepted where the whole enum is expected, but a whole-enum value
    /// is not accepted where only some variants are, unless the narrowed
    /// side is an inference variable, which then widens to the enum.
    fn unify(&mut self, left: TypeId, right: TypeId) -> bool {
        let right_variable = self.last_inference_variable(right);
        // Whichever side must fit the other is now bounded above: the actual
        // side outside function inputs, the expected side inside an odd number.
        let narrower = if self.function_input_depth.is_multiple_of(2) {
            self.last_inference_variable(left)
        } else {
            right_variable
        };
        if let Some(variable) = narrower {
            self.bounded_inference_variables.insert(variable);
        }
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
                self.bind_inference_variable(variable, right)
            }
            (_, CanonicalType::InferenceVariable(variable)) => {
                self.bind_inference_variable(variable, left)
            }
            // `Never` has no values, so a `Never` value fits any expected type.
            // The rule is one-way: nothing else fits where `Never` is expected.
            _ if self.function_input_depth == 0 && self.is_never(left) => true,
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
                if left_symbol != right_symbol || left_arguments.len() != right_arguments.len() {
                    return false;
                }
                // Two instances meeting in inference join a covariant failure-set
                // argument by union ("Joins"), so a list of two captures with
                // different rows holds `Result<T, A | B>`.
                let mut joined = Vec::with_capacity(right_arguments.len());
                let mut widened = false;
                for (index, (left, right)) in
                    left_arguments.into_iter().zip(right_arguments).enumerate()
                {
                    // An argument used only as a function input fits the other
                    // way round, as the input of a function type does.
                    if self.type_parameter_is_contravariant(left_symbol, index) {
                        self.function_input_depth += 1;
                        let fits = self.unify(left, right);
                        self.function_input_depth -= 1;
                        if !fits {
                            return false;
                        }
                        joined.push(right);
                        continue;
                    }
                    if self.unify(left, right) {
                        // A failure-set argument fits a wider one only where the
                        // parameter is covariant; elsewhere, as for `Handler<E>`
                        // holding a `fn(E)`, the two sets must be equal.
                        if self.resolve_inference_type(left) != self.resolve_inference_type(right)
                            && !self.type_parameter_is_covariant(left_symbol, index)
                            && !self.unify(right, left)
                        {
                            return false;
                        }
                        joined.push(right);
                        continue;
                    }
                    let union = match (
                        right_variable,
                        self.failure_set_of(left),
                        self.failure_set_of(right),
                    ) {
                        (Some(variable), Some(left), Some(right))
                            if self.function_input_depth.is_multiple_of(2)
                                && !self.bounded_inference_variables.contains(&variable)
                                && self.type_parameter_is_covariant(left_symbol, index) =>
                        {
                            self.join_failure_sets([left, right])
                        }
                        _ => return false,
                    };
                    joined.push(self.failure_set_type(union));
                    widened = true;
                }
                if let (true, Some(variable)) = (widened, right_variable) {
                    let joined = self.intern_type(CanonicalType::Named {
                        symbol: left_symbol,
                        arguments: joined,
                    });
                    self.inference_bindings.insert(variable, joined);
                }
                true
            }
            (CanonicalType::Generic(left), CanonicalType::Generic(right)) => left == right,
            // A failure-set value fits a set that includes it. Under an odd number
            // of function inputs the direction flips: a `fn(A)` must not pass for
            // a `fn(A | B)`, which may hand it a `B`. Where the expected side is
            // still being inferred outside inputs, two failure sets join by union.
            (CanonicalType::FailureSetValue(_), CanonicalType::FailureSetValue(_))
            | (CanonicalType::Generic(_), CanonicalType::FailureSetValue(_))
            | (CanonicalType::FailureSetValue(_), CanonicalType::Generic(_))
            | (CanonicalType::Named { .. }, CanonicalType::FailureSetValue(_))
            | (CanonicalType::FailureSetValue(_), CanonicalType::Named { .. }) => {
                let (Some(actual), Some(expected)) =
                    (self.failure_set_of(left), self.failure_set_of(right))
                else {
                    return false;
                };
                if !self.function_input_depth.is_multiple_of(2) {
                    return self.failure_set_is_subset(expected, actual);
                }
                if self.failure_set_is_subset(actual, expected) {
                    return true;
                }
                let Some(variable) = right_variable
                    .filter(|variable| !self.bounded_inference_variables.contains(variable))
                else {
                    return false;
                };
                let union = self.join_failure_sets([actual, expected]);
                let joined = self.failure_set_type(union);
                self.inference_bindings.insert(variable, joined);
                true
            }
            (CanonicalType::Function(left), CanonicalType::Function(right)) => {
                left.inputs.len() == right.inputs.len()
                    && left
                        .inputs
                        .into_iter()
                        .zip(right.inputs)
                        .all(|(left, right)| {
                            self.function_input_depth += 1;
                            let unified = self.unify(left, right);
                            self.function_input_depth -= 1;
                            unified
                        })
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
            (CanonicalType::Tuple(left), CanonicalType::Tuple(right)) => {
                left.len() == right.len()
                    && left
                        .into_iter()
                        .zip(right)
                        .all(|(left, right)| self.unify(left, right))
            }
            (
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
                CanonicalType::Function(_),
            ) => {
                let left = self.instantiate_function_value(&parameters, contract, Some(right));
                self.unify(left, right)
            }
            (
                CanonicalType::Function(_),
                CanonicalType::PolymorphicFunction {
                    parameters,
                    contract,
                },
            ) => {
                let right = self.instantiate_function_value(&parameters, contract, Some(left));
                self.unify(left, right)
            }
            _ => false,
        }
    }

    /// Binds `variable` to `type_id` unless `type_id` contains `variable`: a
    /// binding such as `T = List<T>` would make an infinite type, so the two
    /// types do not unify and the caller reports the mismatch.
    fn bind_inference_variable(&mut self, variable: u32, type_id: TypeId) -> bool {
        if self.inference_variable_occurs(variable, type_id) {
            return false;
        }
        self.inference_bindings.insert(variable, type_id);
        true
    }

    fn inference_variable_occurs(&self, variable: u32, type_id: TypeId) -> bool {
        let occurs = |id: TypeId| self.inference_variable_occurs(variable, id);
        let contract_occurs = |contract: &CallableContract| {
            contract.inputs.iter().copied().any(occurs) || occurs(contract.success)
        };
        match &self.types[self.resolve_inference_type(type_id).raw() as usize] {
            CanonicalType::InferenceVariable(found) => *found == variable,
            CanonicalType::Named { arguments, .. } => arguments.iter().copied().any(occurs),
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => contract_occurs(contract),
            CanonicalType::Record(fields) => fields.iter().any(|field| occurs(field.type_id)),
            CanonicalType::Tuple(elements) => elements.iter().copied().any(occurs),
            CanonicalType::Generic(_)
            | CanonicalType::FailureSetValue(_)
            | CanonicalType::Poison => false,
        }
    }

    /// Instantiates a generic function value (`wrap`, `Some`, `Map.get`) where
    /// it meets a function type, so it can flow into a monomorphic or
    /// differently generic function-typed slot. Value parameters become fresh
    /// inference variables. Rows have no inference variables, so a failure or
    /// needs row parameter of the value takes the row the `slot` function type
    /// has in the same position (`run<E>` in a slot `fn(fn() -> Int errors
    /// Oops) -> Int errors Oops` gets `E = Oops`). A row parameter the slot
    /// does not determine instantiates to the empty row: leaving it in place
    /// would leak the callee's own row parameter into the caller's contract.
    fn instantiate_function_value(
        &mut self,
        parameters: &[SymbolId],
        contract: CallableContract,
        slot: Option<TypeId>,
    ) -> TypeId {
        self.instantiate_function_value_with(parameters, contract, slot)
            .0
    }

    /// `instantiate_function_value`, also returning what each parameter became.
    fn instantiate_function_value_with(
        &mut self,
        parameters: &[SymbolId],
        contract: CallableContract,
        slot: Option<TypeId>,
    ) -> (TypeId, CallSubstitution) {
        let mut substitution = CallSubstitution::default();
        if let Some(slot) = slot {
            let value = self.intern_type(CanonicalType::Function(contract.clone()));
            self.bind_value_rows_from_slot(slot, value, parameters, &mut substitution);
        }
        for parameter in parameters {
            if substitution.failures.contains_key(parameter)
                || substitution.needs.contains_key(parameter)
            {
                continue;
            }
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
        self.check_value_instantiation(parameters, &substitution);
        (
            self.intern_type(CanonicalType::Function(contract)),
            substitution,
        )
    }

    /// Queues the requirement checks for a generic function value instantiated
    /// at the expression being inferred, as a call to it would.
    fn check_value_instantiation(
        &mut self,
        parameters: &[SymbolId],
        substitution: &CallSubstitution,
    ) {
        let Some((module_id, range)) = self.expression_site else {
            return;
        };
        let owner = self.polymorphic_owners.get(parameters).copied();
        for parameter in parameters {
            let callee = owner.map_or_else(
                || self.generic_owner_name(*parameter),
                |symbol| self.callable_display_name(symbol),
            );
            let (Some(requirement), Some(argument)) = (
                self.instantiated_requirement(*parameter),
                substitution.values.get(parameter).copied(),
            ) else {
                continue;
            };
            self.property_checks.push(PropertyCheck {
                module_id,
                type_id: argument,
                property: requirement.property,
                site: PropertySite::Instantiation {
                    range,
                    callee,
                    parameter: *parameter,
                    requirement,
                },
            });
        }
    }

    /// Walks `slot` and `value` in parallel and binds each row parameter of
    /// the value (one of `parameters`) to the residual of the slot's row in
    /// the same position. Value types are left to the later unification.
    fn bind_value_rows_from_slot(
        &mut self,
        slot: TypeId,
        value: TypeId,
        parameters: &[SymbolId],
        substitution: &mut CallSubstitution,
    ) {
        let slot = self.resolve_inference_type(slot);
        let value = self.resolve_inference_type(value);
        match (
            self.types[slot.raw() as usize].clone(),
            self.types[value.raw() as usize].clone(),
        ) {
            (CanonicalType::Function(slot), CanonicalType::Function(value)) => {
                if slot.inputs.len() != value.inputs.len() {
                    return;
                }
                for (slot, value) in slot.inputs.into_iter().zip(value.inputs) {
                    self.bind_value_rows_from_slot(slot, value, parameters, substitution);
                }
                self.bind_value_rows_from_slot(
                    slot.success,
                    value.success,
                    parameters,
                    substitution,
                );
                let value_failures = &self.failure_rows[value.failures.raw() as usize];
                if let [parameter] = value_failures.parameters.as_slice()
                    && parameters.contains(parameter)
                    && !substitution.failures.contains_key(parameter)
                {
                    let mut probe = substitution.clone();
                    if self.match_call_failure_row(slot.failures, value.failures, &mut probe) {
                        *substitution = probe;
                    }
                }
                let value_needs = &self.needs_rows[value.needs.raw() as usize];
                if let [parameter] = value_needs.parameters.as_slice()
                    && parameters.contains(parameter)
                    && !substitution.needs.contains_key(parameter)
                {
                    let mut probe = substitution.clone();
                    if self.match_call_needs_row(slot.needs, value.needs, &mut probe) {
                        *substitution = probe;
                    }
                }
            }
            (
                CanonicalType::Named {
                    symbol: slot_symbol,
                    arguments: slot_arguments,
                },
                CanonicalType::Named {
                    symbol: value_symbol,
                    arguments: value_arguments,
                },
            ) if slot_symbol == value_symbol && slot_arguments.len() == value_arguments.len() => {
                for (slot, value) in slot_arguments.into_iter().zip(value_arguments) {
                    self.bind_value_rows_from_slot(slot, value, parameters, substitution);
                }
            }
            (CanonicalType::Record(slot), CanonicalType::Record(value))
                if slot.len() == value.len() =>
            {
                for (slot, value) in slot.into_iter().zip(value) {
                    self.bind_value_rows_from_slot(
                        slot.type_id,
                        value.type_id,
                        parameters,
                        substitution,
                    );
                }
            }
            (CanonicalType::Tuple(slot), CanonicalType::Tuple(value))
                if slot.len() == value.len() =>
            {
                for (slot, value) in slot.into_iter().zip(value) {
                    self.bind_value_rows_from_slot(slot, value, parameters, substitution);
                }
            }
            _ => {}
        }
    }

    /// The failure and needs row parameters that occur anywhere in `type_id`.
    fn type_row_parameters(&self, type_id: TypeId) -> Vec<SymbolId> {
        let mut parameters = Vec::new();
        self.collect_type_row_parameters(type_id, &mut parameters, &mut HashSet::new());
        parameters
    }

    fn collect_type_row_parameters(
        &self,
        type_id: TypeId,
        parameters: &mut Vec<SymbolId>,
        visiting: &mut HashSet<TypeId>,
    ) {
        let type_id = self.resolve_inference_type(type_id);
        if !visiting.insert(type_id) {
            return;
        }
        match &self.types[type_id.raw() as usize] {
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => {
                parameters.extend(&self.failure_rows[contract.failures.raw() as usize].parameters);
                parameters.extend(&self.needs_rows[contract.needs.raw() as usize].parameters);
                for input in &contract.inputs {
                    self.collect_type_row_parameters(*input, parameters, visiting);
                }
                self.collect_type_row_parameters(contract.success, parameters, visiting);
            }
            CanonicalType::Named { arguments, .. } => {
                for argument in arguments {
                    self.collect_type_row_parameters(*argument, parameters, visiting);
                }
            }
            CanonicalType::Record(fields) => {
                for field in fields {
                    self.collect_type_row_parameters(field.type_id, parameters, visiting);
                }
            }
            CanonicalType::Tuple(elements) => {
                for element in elements {
                    self.collect_type_row_parameters(*element, parameters, visiting);
                }
            }
            CanonicalType::FailureSetValue(failures) => {
                parameters.extend(&self.failure_rows[failures.raw() as usize].parameters);
            }
            CanonicalType::Generic(_)
            | CanonicalType::InferenceVariable(_)
            | CanonicalType::Poison => {}
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
            (CanonicalType::Tuple(left), CanonicalType::Tuple(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| self.types_equivalent(*left, *right))
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
            ModuleDeclarationKind::Constant => ProjectSymbolKind::Constant,
            ModuleDeclarationKind::ProcessName => ProjectSymbolKind::ProcessName,
            ModuleDeclarationKind::NativeType => ProjectSymbolKind::NativeType,
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
        self.push_type_diagnostic(Diagnostic::error(
            DiagnosticStage::Type,
            code,
            file_id,
            range,
            message,
        ));
    }

    /// Records a type diagnostic once: body inference runs to a fixed point, so
    /// the same finding recurs on every pass.
    fn push_type_diagnostic(&mut self, diagnostic: Diagnostic) {
        if self.diagnostics.iter().any(|existing| {
            existing.stage() == DiagnosticStage::Type
                && existing.code() == diagnostic.code()
                && existing.file_id() == diagnostic.file_id()
                && existing.range() == diagnostic.range()
        }) {
            return;
        }
        self.diagnostics.push(diagnostic);
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
/// The smallest non-negative integer that no integer pattern in the first column
/// names, or `None` when that column tests no integers.
fn unmatched_integer(matrix: &[Vec<MatchPattern>]) -> Option<MatchPattern> {
    let present = matrix
        .iter()
        .filter_map(|row| match row.first() {
            Some(MatchPattern::Constructor {
                tag: MatchTag::Integer(value),
                ..
            }) => Some(*value),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if present.is_empty() {
        return None;
    }
    (0..)
        .find(|value| !present.contains(value))
        .map(|value| MatchPattern::Constructor {
            tag: MatchTag::Integer(value),
            fields: Vec::new(),
        })
}

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
