//! The static rules for module constant initializers (`T4321`–`T4324`).
//!
//! A constant is evaluated once by the compiler, so its initializer has no caller to
//! handle a failure, no `using` boundary to supply a need, and its value must be data
//! the executable can store. Rule 4 (only `pure` operations) is dynamic and belongs to
//! build-time evaluation (`E5201`).

use super::{CanonicalType, ProjectTypeChecker, TypedProject};
use crate::diagnostic::{Diagnostic, DiagnosticStage};
use crate::ids::{FileId, ModuleId, SemanticId, SymbolId, TypeId};
use crate::resolution::{ProjectReference, ProjectSymbolKind, ReferenceResolution};
use crate::semantic::ModuleDeclarationKind;
use crate::standard::{StandardSymbol, StandardType};
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

impl ProjectTypeChecker {
    /// `T4325`: an interpolated value must be `Text`, `Int`, or `Float`. A part whose type
    /// is still open becomes `Text`, the type interpolation inserts unchanged.
    pub(super) fn check_interpolated_type(
        &mut self,
        module_id: ModuleId,
        part: &crate::semantic::ModuleExpression,
        type_id: TypeId,
    ) {
        let resolved = self.resolve_inference_type(type_id);
        let allowed = match &self.types[resolved.raw() as usize] {
            CanonicalType::InferenceVariable(_) => {
                let text = self.builtin_type("Text");
                self.unify_expression_result(module_id, part, resolved, text);
                true
            }
            CanonicalType::Poison => true,
            _ => ["Text", "Int", "Float"]
                .into_iter()
                .any(|name| resolved == self.builtin_type(name)),
        };
        if !allowed {
            let found = self.formatter().format_type(resolved);
            self.type_error_range(
                module_id,
                "T4325",
                part.range(),
                format!(
                    "an interpolated value must be Text, Int, or Float, found `{found}`; write \
                     `${{Debug.render(value)}}` for debugging output, or call a function that \
                     chooses the user-facing text"
                ),
            );
        }
    }

    /// Whether `id` has no unbound inference variable and no generic parameter left.
    /// `Poison` counts as determined: whatever produced it already reported an error.
    pub(super) fn type_is_determined(&self, id: TypeId) -> bool {
        let id = self.resolve_inference_type(id);
        match &self.types[id.raw() as usize] {
            CanonicalType::InferenceVariable(_)
            | CanonicalType::Generic(_)
            | CanonicalType::PolymorphicFunction { .. } => false,
            CanonicalType::Named { arguments, .. } => arguments
                .iter()
                .all(|argument| self.type_is_determined(*argument)),
            CanonicalType::Record(fields) => fields
                .iter()
                .all(|field| self.type_is_determined(field.type_id)),
            CanonicalType::Tuple(elements) => elements
                .iter()
                .all(|element| self.type_is_determined(*element)),
            CanonicalType::Function(contract) => {
                contract
                    .inputs
                    .iter()
                    .all(|input| self.type_is_determined(*input))
                    && self.type_is_determined(contract.success)
            }
            CanonicalType::FailureSetValue(_) | CanonicalType::Poison => true,
        }
    }
}

/// Reports `T4321`–`T4324` for every constant declared in `modules`, and `E5205` for
/// constants that only name each other, since those never get a type to evaluate.
///
/// Requires: `typed` holds the settled facts of the body fixed point, and
/// `undetermined` the constants whose final pass left their type open.
/// Ensures: at most one of `T4323` and `T4324` per constant, preferring `T4323`;
/// `T4321` and `T4322` are independent of the type.
pub(super) fn check(
    typed: &TypedProject,
    modules: &HashSet<ModuleId>,
    undetermined: &HashSet<(ModuleId, SemanticId)>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    // Only a constant without a usable type can be on an unreported cycle, so the
    // project-wide reference graph is built for the first such constant, not on every check.
    let references = OnceCell::new();
    for module in &typed.resolved.semantic.modules {
        if !modules.contains(&module.id) {
            continue;
        }
        let file_id = module.source.file_id();
        for declaration in &module.declarations {
            if declaration.kind != ModuleDeclarationKind::Constant {
                continue;
            }
            let Some(body) = &declaration.body else {
                continue;
            };
            let name = &declaration.name.text;
            let error = |code: &'static str, message: String| {
                Diagnostic::error(DiagnosticStage::Type, code, file_id, body.range(), message)
            };
            if let Some(fact) = typed.expression_facts.get(&(module.id, body.semantic_id())) {
                let failures = &typed.failure_rows[fact.failures.raw() as usize];
                if !failures.variants.is_empty() || !failures.parameters.is_empty() {
                    diagnostics.push(error(
                        "T4321",
                        format!(
                            "constant `{name}` cannot fail, but its initializer can fail with {}; \
                             handle the failure in the initializer or compute the value in a function",
                            typed.format_failure_row(failures)
                        ),
                    ));
                }
                let needs = &typed.needs_rows[fact.needs.raw() as usize];
                if !needs.entries.is_empty() || !needs.parameters.is_empty() {
                    diagnostics.push(error(
                        "T4322",
                        format!(
                            "constant `{name}` cannot depend on a provider, but its initializer needs {}; \
                             no `using` boundary encloses a constant",
                            typed.format_needs_row(needs)
                        ),
                    ));
                }
            }
            // A capability is reported even when the rest of the type is open: it is the
            // more useful fact, and no annotation could make the constant valid.
            let part = typed
                .declaration_types
                .get(&(module.id, declaration.semantic_id))
                .and_then(|type_id| {
                    unrepresentable_part(typed, *type_id, &HashMap::new(), &mut HashSet::new())
                });
            if let Some(part) = part {
                diagnostics.push(error(
                    "T4323",
                    format!(
                        "constant `{name}` cannot be stored in the executable: {part}; runtime \
                         capabilities are created by running code, so create it in a function"
                    ),
                ));
            } else if let Some(cycle) = typed
                .resolved
                .source_symbol(
                    module.id,
                    declaration.semantic_id,
                    ProjectSymbolKind::Constant,
                    name,
                )
                .filter(|_| {
                    undetermined.contains(&(module.id, declaration.semantic_id))
                        || typed
                            .declaration_types
                            .get(&(module.id, declaration.semantic_id))
                            .is_none_or(|type_id| contains_poison(typed, *type_id))
                })
                .and_then(|symbol| {
                    reference_cycle(
                        references.get_or_init(|| constant_references(typed)),
                        symbol,
                    )
                })
            {
                // Constants that only name each other have no type to infer, so the cycle
                // is found here instead of by evaluation. The cycle's first constant reports it.
                if cycle.first() == cycle.iter().min() {
                    let path = cycle
                        .iter()
                        .chain(cycle.first())
                        .map(|symbol| typed.resolved.symbol(*symbol).name.as_str())
                        .collect::<Vec<_>>()
                        .join(" -> ");
                    diagnostics.push(Diagnostic::error(
                        DiagnosticStage::ExecutionSetup,
                        "E5205",
                        file_id,
                        body.range(),
                        format!("constants depend on each other in a cycle: {path}"),
                    ));
                }
            } else if undetermined.contains(&(module.id, declaration.semantic_id)) {
                diagnostics.push(error(
                    "T4324",
                    format!(
                        "the type of constant `{name}` is not fully determined; a constant cannot be \
                         generic, so annotate it (`const {name}: Type = ...`) or the parameters and \
                         results of the functions it holds"
                    ),
                ));
            }
        }
    }
    diagnostics
}

/// The constants each constant's initializer names directly.
///
/// Ensures: linear in the number of references plus, per constant, a binary search
/// and the references inside its initializer.
fn constant_references(typed: &TypedProject) -> HashMap<SymbolId, Vec<SymbolId>> {
    let mut by_file = HashMap::<FileId, Vec<&ProjectReference>>::new();
    for reference in &typed.resolved.references {
        let ReferenceResolution::Resolved(target) = reference.resolution else {
            continue;
        };
        if typed.resolved.symbol(target).kind == ProjectSymbolKind::Constant {
            by_file
                .entry(reference.file_id)
                .or_default()
                .push(reference);
        }
    }
    for file_references in by_file.values_mut() {
        file_references.sort_by_key(|reference| reference.range.start());
    }
    let mut references = HashMap::<SymbolId, Vec<SymbolId>>::new();
    for module in &typed.resolved.semantic.modules {
        let file_references = by_file
            .get(&module.source.file_id())
            .map_or(&[][..], Vec::as_slice);
        for declaration in &module.declarations {
            let (ModuleDeclarationKind::Constant, Some(body)) =
                (declaration.kind, &declaration.body)
            else {
                continue;
            };
            let Some(symbol) = typed.resolved.source_symbol(
                module.id,
                declaration.semantic_id,
                ProjectSymbolKind::Constant,
                &declaration.name.text,
            ) else {
                continue;
            };
            let range = body.range();
            let first = file_references
                .partition_point(|reference| reference.range.start() < range.start());
            let targets = file_references[first..]
                .iter()
                .take_while(|reference| reference.range.start() < range.end())
                .filter(|reference| reference.range.end() <= range.end())
                .filter_map(|reference| match reference.resolution {
                    ReferenceResolution::Resolved(target) => Some(target),
                    _ => None,
                })
                .collect();
            references.insert(symbol, targets);
        }
    }
    references
}

/// The constants on a cycle of direct references that returns to `start`, beginning
/// with `start`, or `None` when no such cycle exists.
fn reference_cycle(
    references: &HashMap<SymbolId, Vec<SymbolId>>,
    start: SymbolId,
) -> Option<Vec<SymbolId>> {
    fn visit(
        references: &HashMap<SymbolId, Vec<SymbolId>>,
        start: SymbolId,
        path: &mut Vec<SymbolId>,
        seen: &mut HashSet<SymbolId>,
    ) -> bool {
        let current = *path.last().expect("path starts at the start constant");
        for next in references.get(&current).into_iter().flatten() {
            if *next == start {
                return true;
            }
            if seen.insert(*next) {
                path.push(*next);
                if visit(references, start, path, seen) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }
    let mut path = vec![start];
    visit(references, start, &mut path, &mut HashSet::from([start])).then_some(path)
}

fn contains_poison(typed: &TypedProject, id: TypeId) -> bool {
    match &typed.types[typed.resolve_type_id(id).raw() as usize] {
        CanonicalType::Poison | CanonicalType::InferenceVariable(_) => true,
        CanonicalType::Named { arguments, .. } => arguments
            .iter()
            .any(|argument| contains_poison(typed, *argument)),
        CanonicalType::Record(fields) => fields
            .iter()
            .any(|field| contains_poison(typed, field.type_id)),
        CanonicalType::Tuple(elements) => elements
            .iter()
            .any(|element| contains_poison(typed, *element)),
        CanonicalType::Function(contract) | CanonicalType::PolymorphicFunction { contract, .. } => {
            contract
                .inputs
                .iter()
                .any(|input| contains_poison(typed, *input))
                || contains_poison(typed, contract.success)
        }
        CanonicalType::Generic(_) | CanonicalType::FailureSetValue(_) => false,
    }
}

/// Describes every part of a value of type `id` that is not constant-representable,
/// such as "field `sender`: a `Sender` is a runtime capability", or `None` when every
/// reachable part is.
///
/// Function types are representable here: a function value's captures are checked when
/// the constant is evaluated, because only the value knows what it captured.
fn unrepresentable_part(
    typed: &TypedProject,
    id: TypeId,
    generics: &HashMap<SymbolId, TypeId>,
    visiting: &mut HashSet<TypeId>,
) -> Option<String> {
    let id = typed.resolve_type_id(id);
    if !visiting.insert(id) {
        return None;
    }
    let result = match &typed.types[id.raw() as usize] {
        CanonicalType::Named { symbol, arguments } => {
            if let Some(StandardSymbol::Type(
                StandardType::Task
                | StandardType::ChannelSender
                | StandardType::ChannelReceiver
                | StandardType::ChannelCompletion
                | StandardType::Console
                | StandardType::FileSystem
                | StandardType::Clock
                | StandardType::Instant
                | StandardType::ProcessInbox
                | StandardType::SupervisorRunning
                | StandardType::SupervisorManagedProcess
                | StandardType::SupervisorManagedSupervisor,
            )) = typed.resolved.standard_symbol(*symbol)
            {
                Some(format!(
                    "a `{}` is a runtime capability",
                    typed.resolved.symbol(*symbol).name
                ))
            } else if let Some(StandardSymbol::Type(
                StandardType::ProcessSpec | StandardType::ProcessHandoffSpec,
            )) = typed.resolved.standard_symbol(*symbol)
            {
                // A spec holds its body, a capacity, and a duration. Its type
                // arguments name the message, start argument, and failure types
                // the body handles, not values the spec stores.
                None
            } else {
                named_unrepresentable_part(typed, *symbol, arguments, generics, visiting)
            }
        }
        CanonicalType::Generic(parameter) => generics
            .get(parameter)
            .and_then(|bound| unrepresentable_part(typed, *bound, generics, visiting)),
        CanonicalType::Record(fields) => joined(fields.iter().filter_map(|field| {
            unrepresentable_part(typed, field.type_id, generics, visiting)
                .map(|part| field_part(&field.name, part))
        })),
        CanonicalType::Tuple(elements) => {
            joined(elements.iter().enumerate().filter_map(|(index, element)| {
                unrepresentable_part(typed, *element, generics, visiting)
                    .map(|part| format!("tuple element {}: {part}", index + 1))
            }))
        }
        CanonicalType::Function(_)
        | CanonicalType::PolymorphicFunction { .. }
        | CanonicalType::FailureSetValue(_)
        | CanonicalType::InferenceVariable(_)
        | CanonicalType::Poison => None,
    };
    visiting.remove(&id);
    result
}

fn named_unrepresentable_part(
    typed: &TypedProject,
    symbol: SymbolId,
    arguments: &[TypeId],
    outer: &HashMap<SymbolId, TypeId>,
    visiting: &mut HashSet<TypeId>,
) -> Option<String> {
    if let Some(part) = arguments
        .iter()
        .find_map(|argument| unrepresentable_part(typed, *argument, outer, visiting))
    {
        return Some(part);
    }
    let mut generics = outer.clone();
    for (parameter, argument) in typed
        .type_generic_parameters(symbol)
        .into_iter()
        .zip(arguments)
    {
        generics.insert(parameter, *argument);
    }
    let mut owners = vec![symbol];
    if typed.resolved.symbol(symbol).kind == ProjectSymbolKind::Enum {
        owners.extend(typed.enum_variants(symbol));
    }
    joined(owners.into_iter().flat_map(|owner| {
        typed
            .fields
            .get(&owner)
            .into_iter()
            .flatten()
            .filter_map(|field| {
                unrepresentable_part(typed, field.type_id, &generics, visiting)
                    .map(|part| field_part(&field.name, part))
            })
            .collect::<Vec<_>>()
    }))
}

/// Every offending part, so the diagnostic names each capability a value holds.
fn joined(parts: impl Iterator<Item = String>) -> Option<String> {
    let parts = parts.collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn field_part(field: &str, part: String) -> String {
    format!("field `{field}`: {part}")
}
