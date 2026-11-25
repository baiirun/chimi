//! Failure-set types (`docs/specification/time-and-outcomes.md`, "Failure-set
//! types", "Projection into type arguments", and "Joins").
//!
//! A failure set is a [`FailureRow`]: the same normalized set of variants and
//! failure-set parameters denotes a failure row after `errors` and a value type
//! anywhere else. Interning normalizes a row, so duplicates collapse, a variant
//! listed with its whole enum collapses into the enum, and member order does not
//! matter.
//!
//! The value projection of a row has one canonical [`TypeId`]
//! ([`ProjectTypeChecker::failure_set_type`]), so a written annotation, a
//! `Failure(error)` binding, and a type argument instantiated from a row compare
//! equal when they denote the same set. Later checks (`T4346` for retry
//! predicates, `Task.all` rows) use the subset and join functions here.

use super::{
    CallableContract, CanonicalType, FailureRow, FailureVariantArguments, GenericKind,
    ProjectTypeChecker,
};
use crate::ids::{FailureRowId, ModuleId, SymbolId, TextRange, TypeId};
use crate::resolution::ProjectSymbolKind;
use crate::semantic::TypeUse;
use crate::standard::{StandardSymbol, StandardType};
use std::collections::HashMap;

impl ProjectTypeChecker {
    /// The failure row written after `errors`: one member, or `A | B | ...`.
    /// Ensures: a member that is not a failure-set member is reported (`T4102`
    /// for a lone member, `T4340` inside a union) and adds nothing.
    pub(super) fn lower_failure_row(
        &mut self,
        module_id: ModuleId,
        type_use: &TypeUse,
        generics: &HashMap<String, SymbolId>,
    ) -> FailureRowId {
        match type_use {
            TypeUse::Union { members, .. } => {
                self.lower_failure_members(module_id, members, false, generics)
            }
            TypeUse::Named { path, .. } => {
                if let Some(row) = self.failure_member_row(module_id, type_use, false, generics) {
                    return row;
                }
                if let Some(name) = path.last() {
                    self.type_error(
                        module_id,
                        "T4102",
                        name,
                        format!(
                            "failure row expects an enum or row parameter, found {}",
                            name.text
                        ),
                    );
                }
                FailureRowId::new(0)
            }
            TypeUse::Function { .. } | TypeUse::Tuple { .. } | TypeUse::Record { .. } => {
                FailureRowId::new(0)
            }
        }
    }

    /// The value type `A | B | ...` written in an ordinary type position.
    /// Ensures: each member's own type fact is recorded, and a generic member is a
    /// failure set (it has both projections).
    pub(super) fn lower_failure_set_type(
        &mut self,
        module_id: ModuleId,
        members: &[TypeUse],
        generics: &HashMap<String, SymbolId>,
    ) -> TypeId {
        let row = self.lower_failure_members(module_id, members, true, generics);
        self.failure_set_type(row)
    }

    /// The value type of a written `Enum.Variant`: the one-member failure set.
    pub(super) fn variant_failure_set_type(&mut self, variant: SymbolId) -> TypeId {
        let row = self.variant_failure_row(variant);
        self.failure_set_type(row)
    }

    /// The canonical value type of the failure set `row`: the enum for exactly
    /// one whole enum, the parameter for exactly one unrestricted failure-set
    /// parameter, and a `FailureSetValue` otherwise, the empty set included.
    pub(crate) fn failure_set_type(&mut self, row: FailureRowId) -> TypeId {
        self.failure_set_argument_type(row)
    }

    /// The failure set a value type denotes, if it is one: an enum, a
    /// failure-set parameter, or a written or inferred set. `None` for any other
    /// type, including an unresolved inference variable.
    pub(crate) fn failure_set_of(&mut self, type_id: TypeId) -> Option<FailureRowId> {
        let resolved = self.resolve_inference_type(type_id);
        if matches!(
            self.types[resolved.raw() as usize],
            CanonicalType::InferenceVariable(_)
        ) {
            return None;
        }
        self.failure_row_for_type(resolved)
    }

    /// Whether every member of `actual` is a member of `expected`: the subtype
    /// rule for failure-set values and the fit rule for failure rows.
    pub(crate) fn failure_set_is_subset(
        &self,
        actual: FailureRowId,
        expected: FailureRowId,
    ) -> bool {
        self.failure_row_is_subset(actual, expected)
    }

    /// The variants of `actual` that `expected` lacks, in symbol order, for a
    /// diagnostic that names them (`T4346`). A parameter of `actual` that
    /// `expected` lacks is not a variant and is not listed.
    pub(crate) fn failure_set_missing_variants(
        &self,
        actual: FailureRowId,
        expected: FailureRowId,
    ) -> Vec<SymbolId> {
        let actual = &self.failure_rows[actual.raw() as usize];
        actual
            .variants
            .iter()
            .copied()
            .filter(|variant| !self.failure_row_may_contain(expected, *variant))
            .collect()
    }

    /// The union of failure sets: the join of two failure-set values, and the
    /// composition of two failure rows.
    pub(crate) fn join_failure_sets(
        &mut self,
        sets: impl IntoIterator<Item = FailureRowId>,
    ) -> FailureRowId {
        self.union_failure_rows(sets)
    }

    /// Whether the type parameter at `index` of the data type `owner` occurs only
    /// in covariant positions (fields and success types, never a function input),
    /// so two instances whose arguments there are failure sets may join by union
    /// ("Joins"). Inferred from the declaration; `List`, `Option`, `Task`, and
    /// `Select.Event` only hand their arguments out.
    pub(crate) fn type_parameter_is_covariant(&self, owner: SymbolId, index: usize) -> bool {
        self.parameter_is_covariant_within(owner, index, 0)
    }

    /// `type_parameter_is_covariant`, `depth` data types deep. Past the depth
    /// limit, as for a recursive type that nests itself, the answer is the
    /// conservative "not covariant".
    fn parameter_is_covariant_within(&self, owner: SymbolId, index: usize, depth: usize) -> bool {
        const DEPTH_LIMIT: usize = 8;
        if depth > DEPTH_LIMIT {
            return false;
        }
        if self.is_builtin_type(owner, "List")
            || matches!(
                self.resolved.standard_symbol(owner),
                Some(StandardSymbol::Type(
                    StandardType::Option | StandardType::Task | StandardType::SelectEvent
                ))
            )
        {
            return true;
        }
        let Some(parameter) = self
            .named_type_generic_parameters(owner)
            .get(index)
            .copied()
        else {
            return false;
        };
        let mut shapes = vec![owner];
        if self.resolved.symbol(owner).kind == ProjectSymbolKind::Enum {
            shapes.extend(self.enum_variant_symbols(owner));
        }
        shapes
            .iter()
            .filter_map(|shape| self.fields.get(shape))
            .flatten()
            .all(|field| !self.occurs_contravariantly(field.type_id, parameter, false, depth))
    }

    /// Whether the type parameter at `index` of the data type `owner` occurs
    /// only in function inputs, as `E` of `Retry.Policy<E>` holding a
    /// `fn(E) -> Bool` does. Such an argument fits the other way round: a
    /// `Policy<A | B>` stands in for a `Policy<A>`, since its predicate takes
    /// every `A`. `false` for a parameter that also occurs covariantly, for one
    /// that does not occur at all, and for built-in types.
    pub(crate) fn type_parameter_is_contravariant(&self, owner: SymbolId, index: usize) -> bool {
        self.parameter_is_contravariant_within(owner, index, 0)
    }

    fn parameter_is_contravariant_within(
        &self,
        owner: SymbolId,
        index: usize,
        depth: usize,
    ) -> bool {
        const DEPTH_LIMIT: usize = 8;
        if depth > DEPTH_LIMIT {
            return false;
        }
        let Some(parameter) = self
            .named_type_generic_parameters(owner)
            .get(index)
            .copied()
        else {
            return false;
        };
        let mut shapes = vec![owner];
        if self.resolved.symbol(owner).kind == ProjectSymbolKind::Enum {
            shapes.extend(self.enum_variant_symbols(owner));
        }
        let fields = shapes
            .iter()
            .filter_map(|shape| self.fields.get(shape))
            .flatten()
            .collect::<Vec<_>>();
        fields
            .iter()
            .any(|field| self.occurs_contravariantly(field.type_id, parameter, false, depth))
            && fields
                .iter()
                .all(|field| !self.occurs_covariantly(field.type_id, parameter, false, depth))
    }

    /// Whether `parameter` may occur in `type_id` at an output position, counting
    /// `inside_input` for the position `type_id` itself is in. Unlike
    /// `occurs_contravariantly`, an input of an input flips back to an output,
    /// and every uncertain case, such as a failure or needs row that names the
    /// parameter, counts as an output, so a `false` is safe to rely on.
    fn occurs_covariantly(
        &self,
        type_id: TypeId,
        parameter: SymbolId,
        inside_input: bool,
        depth: usize,
    ) -> bool {
        let recurse =
            |id: TypeId, input: bool| self.occurs_covariantly(id, parameter, input, depth);
        match &self.types[self.resolve_inference_type(type_id).raw() as usize] {
            CanonicalType::Generic(symbol) => !inside_input && *symbol == parameter,
            CanonicalType::FailureSetValue(row) => {
                !inside_input
                    && self.failure_rows[row.raw() as usize]
                        .parameters
                        .contains(&parameter)
            }
            CanonicalType::Named { symbol, arguments } => {
                arguments.iter().enumerate().any(|(index, argument)| {
                    if self.parameter_is_covariant_within(*symbol, index, depth + 1) {
                        recurse(*argument, inside_input)
                    } else if self.parameter_is_contravariant_within(*symbol, index, depth + 1) {
                        recurse(*argument, !inside_input)
                    } else {
                        recurse(*argument, false) || recurse(*argument, true)
                    }
                })
            }
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => {
                self.failure_rows[contract.failures.raw() as usize]
                    .parameters
                    .contains(&parameter)
                    || self.needs_rows[contract.needs.raw() as usize]
                        .parameters
                        .contains(&parameter)
                    || contract
                        .inputs
                        .iter()
                        .any(|input| recurse(*input, !inside_input))
                    || recurse(contract.success, inside_input)
            }
            CanonicalType::Record(fields) => fields
                .iter()
                .any(|field| recurse(field.type_id, inside_input)),
            CanonicalType::Tuple(elements) => elements
                .iter()
                .any(|element| recurse(*element, inside_input)),
            CanonicalType::InferenceVariable(_) | CanonicalType::Poison => false,
        }
    }

    /// Whether `parameter` occurs in `type_id` at an input position, counting
    /// `inside_input` for the position `type_id` itself is in.
    fn occurs_contravariantly(
        &self,
        type_id: TypeId,
        parameter: SymbolId,
        inside_input: bool,
        depth: usize,
    ) -> bool {
        let recurse =
            |id: TypeId, input: bool| self.occurs_contravariantly(id, parameter, input, depth);
        match &self.types[self.resolve_inference_type(type_id).raw() as usize] {
            CanonicalType::Generic(symbol) => inside_input && *symbol == parameter,
            CanonicalType::FailureSetValue(row) => {
                inside_input
                    && self.failure_rows[row.raw() as usize]
                        .parameters
                        .contains(&parameter)
            }
            // A nested data type may itself use its argument as an input, so an
            // occurrence inside one counts unless that argument is covariant.
            CanonicalType::Named { symbol, arguments } => {
                arguments.iter().enumerate().any(|(index, argument)| {
                    let covariant = self.parameter_is_covariant_within(*symbol, index, depth + 1);
                    recurse(*argument, inside_input || !covariant)
                })
            }
            CanonicalType::Function(contract)
            | CanonicalType::PolymorphicFunction { contract, .. } => {
                // An input of an input is covariant again, but treating every
                // occurrence under an input as contravariant is the safe reading.
                // A function's rows sit beside its success: a callback of type
                // `fn(fn() -> Int errors E)` consumes `E` failures.
                contract.inputs.iter().any(|input| recurse(*input, true))
                    || recurse(contract.success, inside_input)
                    || (inside_input
                        && (self.failure_rows[contract.failures.raw() as usize]
                            .parameters
                            .contains(&parameter)
                            || self.needs_rows[contract.needs.raw() as usize]
                                .parameters
                                .contains(&parameter)))
            }
            CanonicalType::Record(fields) => fields
                .iter()
                .any(|field| recurse(field.type_id, inside_input)),
            CanonicalType::Tuple(elements) => elements
                .iter()
                .any(|element| recurse(*element, inside_input)),
            CanonicalType::InferenceVariable(_) | CanonicalType::Poison => false,
        }
    }

    /// The join of two alternatives that do not unify, as two case arms or two
    /// list elements ("Joins"): failure-set types join by union, and two
    /// instances of one data type join argument by argument where the argument
    /// is covariant and the two arguments are failure sets. `None` when they do
    /// not join, including two different enums at the top, which is `T4341`.
    /// `preferred`, unless it is a generic function and `other` is not.
    fn fixed_function_side(&self, preferred: TypeId, other: TypeId) -> TypeId {
        if self.is_polymorphic_function(preferred) && !self.is_polymorphic_function(other) {
            other
        } else {
            preferred
        }
    }

    pub(crate) fn join_alternative_types(&mut self, left: TypeId, right: TypeId) -> Option<TypeId> {
        self.join_types_within(left, right, false, 0)
    }

    /// `inside_argument`: the types are arguments of a covariant parameter, where
    /// two enums are failure-set arguments, as `E` of two `Result` captures is.
    fn join_types_within(
        &mut self,
        left: TypeId,
        right: TypeId,
        inside_argument: bool,
        depth: usize,
    ) -> Option<TypeId> {
        const DEPTH_LIMIT: usize = 16;
        if depth > DEPTH_LIMIT {
            return None;
        }
        // A generic function joined with a fixed one unifies by instantiating
        // the generic side, so the join is the fixed type: an alternative that
        // may hold the fixed function must not keep the generic type.
        if self.unify(left, right) {
            return Some(self.fixed_function_side(right, left));
        }
        if self.unify(right, left) {
            return Some(self.fixed_function_side(left, right));
        }
        // A record joined with an interface it conforms to is that interface.
        if let Some(Ok(())) = self.record_interface_conformance(left, right) {
            return Some(right);
        }
        if let Some(Ok(())) = self.record_interface_conformance(right, left) {
            return Some(left);
        }
        if let (Some(left_set), Some(right_set)) =
            (self.failure_set_of(left), self.failure_set_of(right))
            && ((inside_argument
                && !self.has_type_arguments(left)
                && !self.has_type_arguments(right))
                || self.is_failure_set_type(left)
                || self.is_failure_set_type(right))
        {
            let union = self.join_failure_sets([left_set, right_set]);
            return Some(self.failure_set_type(union));
        }
        let left = self.resolve_inference_type(left);
        let right = self.resolve_inference_type(right);
        if let (CanonicalType::Function(left_contract), CanonicalType::Function(right_contract)) = (
            self.types[left.raw() as usize].clone(),
            self.types[right.raw() as usize].clone(),
        ) {
            return self.join_function_contracts(left_contract, right_contract, depth);
        }
        let (
            CanonicalType::Named {
                symbol,
                arguments: left_arguments,
            },
            CanonicalType::Named {
                symbol: right_symbol,
                arguments: right_arguments,
            },
        ) = (
            self.types[left.raw() as usize].clone(),
            self.types[right.raw() as usize].clone(),
        )
        else {
            return None;
        };
        if symbol != right_symbol || left_arguments.len() != right_arguments.len() {
            return None;
        }
        let mut arguments = Vec::with_capacity(left_arguments.len());
        for (index, (left, right)) in left_arguments.into_iter().zip(right_arguments).enumerate() {
            if self.unify(left, right) && self.unify(right, left) {
                arguments.push(right);
                continue;
            }
            if !self.type_parameter_is_covariant(symbol, index) {
                return None;
            }
            arguments.push(self.join_types_within(left, right, true, depth + 1)?);
        }
        Some(self.intern_type(CanonicalType::Named { symbol, arguments }))
    }

    /// Two monomorphic function types join when their inputs agree: the joined
    /// function may fail with either row and need either row, and returns the
    /// join of the successes, so a list of thunks with different failures
    /// (`Task.all([load, save])`) takes the union, as two captures do.
    fn join_function_contracts(
        &mut self,
        left: CallableContract,
        right: CallableContract,
        depth: usize,
    ) -> Option<TypeId> {
        if left.inputs.len() != right.inputs.len() {
            return None;
        }
        for (left_input, right_input) in left.inputs.iter().zip(&right.inputs) {
            if !(self.unify(*left_input, *right_input) && self.unify(*right_input, *left_input)) {
                return None;
            }
        }
        let success =
            if self.unify(left.success, right.success) && self.unify(right.success, left.success) {
                right.success
            } else {
                self.join_types_within(left.success, right.success, false, depth + 1)?
            };
        let failures = self.join_failure_sets([left.failures, right.failures]);
        let needs = self.union_needs_rows([left.needs, right.needs]);
        Some(self.intern_type(CanonicalType::Function(CallableContract {
            inputs: right.inputs,
            success,
            failures,
            needs,
        })))
    }

    /// Whether `type_id` is a data type applied to arguments, such as
    /// `Result<Int, A>`. Such a type is joined argument by argument, never as
    /// a whole-enum failure-set member.
    fn has_type_arguments(&self, type_id: TypeId) -> bool {
        matches!(
            &self.types[self.resolve_inference_type(type_id).raw() as usize],
            CanonicalType::Named { arguments, .. } if !arguments.is_empty()
        )
    }

    /// Whether `type_id` is a failure-set type in the join sense: written or
    /// inferred as a set (`FailureSetValue`), or a failure-set parameter. A
    /// plain enum is not, so two different enums do not join silently.
    fn is_failure_set_type(&self, type_id: TypeId) -> bool {
        match &self.types[self.resolve_inference_type(type_id).raw() as usize] {
            CanonicalType::FailureSetValue(_) => true,
            CanonicalType::Generic(parameter) => {
                self.generic_kinds.get(parameter) == Some(&GenericKind::FailureSet)
            }
            _ => false,
        }
    }

    /// Whether `left` and `right` are two different enums that may hold
    /// failures, so their failed join is `T4341` with an `A | B` suggestion.
    pub(crate) fn are_distinct_enums(&mut self, left: TypeId, right: TypeId) -> bool {
        let left = self.resolve_inference_type(left);
        let right = self.resolve_inference_type(right);
        let enum_symbol = |checker: &Self, id: TypeId| match &checker.types[id.raw() as usize] {
            CanonicalType::Named { symbol, .. }
                if checker.resolved.symbol(*symbol).kind == ProjectSymbolKind::Enum
                    || matches!(
                        checker.resolved.standard_symbol(*symbol),
                        Some(StandardSymbol::Type(kind)) if kind.is_failure_enum()
                    ) =>
            {
                Some(*symbol)
            }
            _ => None,
        };
        matches!(
            (enum_symbol(self, left), enum_symbol(self, right)),
            (Some(left), Some(right)) if left != right
        )
    }

    /// Makes a value generic of the enclosing declaration that the body raises
    /// with `Failure(error)` a failure set, so `Result.get`'s `E` in
    /// `Result<T, E>` also denotes its failure row ("Projection into type
    /// arguments"). A generic that is already a failure set, or that is not the
    /// enclosing declaration's, is left as it is.
    pub(super) fn promote_raised_generic(
        &mut self,
        raised: TypeId,
        generics: &HashMap<String, SymbolId>,
    ) {
        let CanonicalType::Generic(parameter) =
            self.types[self.resolve_inference_type(raised).raw() as usize]
        else {
            return;
        };
        if !generics.values().any(|declared| *declared == parameter) {
            return;
        }
        if matches!(
            self.generic_kinds.get(&parameter),
            None | Some(GenericKind::Unconstrained | GenericKind::Value)
        ) {
            self.generic_kinds
                .insert(parameter, GenericKind::FailureSet);
        }
    }

    /// Makes a value generic of the function being checked a failure set when a
    /// call hands it to a callee's failure-set parameter, as
    /// `wrap<X>(r: Result<Int, X>) { Result.get(r) }` does: forwarding `X` to
    /// `get`'s `E` raises it as surely as `Failure(...)` would, so it gets both
    /// projections the same way (CHI-222).
    pub(super) fn promote_forwarded_generic(&mut self, forwarded: TypeId) {
        let CanonicalType::Generic(parameter) =
            self.types[self.resolve_inference_type(forwarded).raw() as usize]
        else {
            return;
        };
        if self.body_generics.contains(&parameter)
            && matches!(
                self.generic_kinds.get(&parameter),
                None | Some(GenericKind::Unconstrained | GenericKind::Value)
            )
        {
            self.generic_kinds
                .insert(parameter, GenericKind::FailureSet);
        }
    }

    /// Why a call argument cannot bind the callee's failure sets, for a `T4203`
    /// message (CHI-222): the first failure-set parameter in `expected` (the
    /// declared parameter type) whose place in `actual` holds a type that is not
    /// a failure set, as `Text` in `Result<Int, Text>` for `get`'s `E`.
    pub(super) fn failure_set_binding_note(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        callee: Option<SymbolId>,
    ) -> Option<String> {
        let (parameter, found) = self.non_failure_set_binding(actual, expected, 0)?;
        let parameter = &self.resolved.symbol(parameter).name;
        let user = callee.map_or_else(
            || "the callee".to_owned(),
            |callee| format!("`{}`", self.resolved.symbol(callee).name),
        );
        let found = self.formatter().format_type(found);
        Some(format!(
            "{user} uses `{parameter}` as a failure set, and `{found}` is not one (a failure \
             set is an enum, one enum variant, a set written `A | B`, or a failure-set generic)"
        ))
    }

    fn non_failure_set_binding(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        depth: usize,
    ) -> Option<(SymbolId, TypeId)> {
        const DEPTH_LIMIT: usize = 8;
        if depth > DEPTH_LIMIT {
            return None;
        }
        let actual = self.resolve_inference_type(actual);
        let expected = self.resolve_inference_type(expected);
        match (
            self.types[actual.raw() as usize].clone(),
            self.types[expected.raw() as usize].clone(),
        ) {
            (CanonicalType::InferenceVariable(_) | CanonicalType::Poison, _) => None,
            (_, CanonicalType::Generic(parameter))
                if self.generic_kinds.get(&parameter) == Some(&GenericKind::FailureSet) =>
            {
                self.failure_set_of(actual)
                    .is_none()
                    .then_some((parameter, actual))
            }
            (
                CanonicalType::Named {
                    symbol,
                    arguments: actual_arguments,
                },
                CanonicalType::Named {
                    symbol: expected_symbol,
                    arguments: expected_arguments,
                },
            ) if symbol == expected_symbol => actual_arguments
                .into_iter()
                .zip(expected_arguments)
                .find_map(|(actual, expected)| {
                    self.non_failure_set_binding(actual, expected, depth + 1)
                }),
            (CanonicalType::Function(actual), CanonicalType::Function(expected))
                if actual.inputs.len() == expected.inputs.len() =>
            {
                actual
                    .inputs
                    .into_iter()
                    .zip(expected.inputs)
                    .chain(std::iter::once((actual.success, expected.success)))
                    .find_map(|(actual, expected)| {
                        self.non_failure_set_binding(actual, expected, depth + 1)
                    })
            }
            (CanonicalType::Tuple(actual), CanonicalType::Tuple(expected))
                if actual.len() == expected.len() =>
            {
                actual
                    .into_iter()
                    .zip(expected)
                    .find_map(|(actual, expected)| {
                        self.non_failure_set_binding(actual, expected, depth + 1)
                    })
            }
            (CanonicalType::Record(actual), CanonicalType::Record(expected))
                if actual.len() == expected.len() =>
            {
                actual
                    .into_iter()
                    .zip(expected)
                    .find_map(|(actual, expected)| {
                        self.non_failure_set_binding(actual.type_id, expected.type_id, depth + 1)
                    })
            }
            _ => None,
        }
    }

    /// `type_id` itself, unless it is a failure-set parameter `E`: then the
    /// one-parameter set `{E}` as a `FailureSetValue`, the form pattern checks
    /// and exhaustiveness read as an open set that needs a catch-all arm.
    pub(super) fn open_failure_set_view(&mut self, type_id: TypeId) -> TypeId {
        let resolved = self.resolve_inference_type(type_id);
        let CanonicalType::Generic(parameter) = self.types[resolved.raw() as usize] else {
            return type_id;
        };
        if self.generic_kinds.get(&parameter) != Some(&GenericKind::FailureSet) {
            return type_id;
        }
        let row = self.intern_failure_row(FailureRow {
            variants: Vec::new(),
            variant_arguments: Vec::new(),
            parameters: vec![parameter],
            exclusions: Vec::new(),
        });
        self.intern_type(CanonicalType::FailureSetValue(row))
    }

    /// The `T4346` message when `actual` and `expected` are failure sets, one of
    /// them written or inferred as a set, and `expected` lacks some of
    /// `actual`'s variants: it names them. The sets may also sit inside the two
    /// types, as the inputs of two functions or the arguments of two `Policy`
    /// instances do; under an input the narrower side must be `actual`, so a
    /// predicate over `HttpError` for an operation that also times out is named
    /// as lacking `Timeout.TimedOut`. `None` for any other mismatch.
    pub(super) fn failure_set_shortfall(
        &mut self,
        subject: &str,
        actual: TypeId,
        expected: TypeId,
    ) -> Option<String> {
        let (wider, missing) = self.failure_set_shortfall_within(actual, expected, false, 0)?;
        let formatter = self.formatter();
        let names = missing
            .iter()
            .map(|variant| {
                let name = &self.resolved.symbol(*variant).name;
                match self.enum_owner_for_variant(*variant) {
                    Some(owner) => format!("`{}.{name}`", self.resolved.symbol(owner).name),
                    None => format!("`{name}`"),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "{subject} has type `{}`, but `{}` lacks {names}",
            formatter.format_type(actual),
            formatter.format_type(wider),
        ))
    }

    /// The set that must hold more, and the variants it lacks, for the first
    /// pair of failure sets in `actual` and `expected` that do not fit.
    /// `flipped`: the pair is under an odd number of function inputs, where
    /// `expected` must fit within `actual`.
    fn failure_set_shortfall_within(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        flipped: bool,
        depth: usize,
    ) -> Option<(TypeId, Vec<SymbolId>)> {
        const DEPTH_LIMIT: usize = 8;
        if depth > DEPTH_LIMIT {
            return None;
        }
        let (narrower, wider) = if flipped {
            (expected, actual)
        } else {
            (actual, expected)
        };
        if self.is_failure_set_type(actual) || self.is_failure_set_type(expected) {
            let (Some(narrower_set), Some(wider_set)) =
                (self.failure_set_of(narrower), self.failure_set_of(wider))
            else {
                return None;
            };
            let missing = self.failure_set_missing_variants(narrower_set, wider_set);
            return (!missing.is_empty()).then_some((wider, missing));
        }
        let actual = self.resolve_inference_type(actual);
        let expected = self.resolve_inference_type(expected);
        match (
            self.types[actual.raw() as usize].clone(),
            self.types[expected.raw() as usize].clone(),
        ) {
            (
                CanonicalType::Named {
                    symbol,
                    arguments: actual_arguments,
                },
                CanonicalType::Named {
                    symbol: expected_symbol,
                    arguments: expected_arguments,
                },
            ) if symbol == expected_symbol
                && actual_arguments.len() == expected_arguments.len() =>
            {
                actual_arguments
                    .into_iter()
                    .zip(expected_arguments)
                    .enumerate()
                    .find_map(|(index, (actual, expected))| {
                        let covariant = self.type_parameter_is_covariant(symbol, index);
                        let contravariant = self.type_parameter_is_contravariant(symbol, index);
                        let within = |checker: &mut Self, flipped| {
                            checker.failure_set_shortfall_within(
                                actual,
                                expected,
                                flipped,
                                depth + 1,
                            )
                        };
                        match (covariant, contravariant) {
                            (true, _) => within(self, flipped),
                            (false, true) => within(self, !flipped),
                            // An invariant argument, as of `Sender<E>`, has no
                            // narrower side to name; it stays a plain mismatch.
                            (false, false) => None,
                        }
                    })
            }
            (CanonicalType::Function(actual), CanonicalType::Function(expected))
                if actual.inputs.len() == expected.inputs.len() =>
            {
                actual
                    .inputs
                    .iter()
                    .zip(&expected.inputs)
                    .find_map(|(actual, expected)| {
                        self.failure_set_shortfall_within(*actual, *expected, !flipped, depth + 1)
                    })
                    .or_else(|| {
                        self.failure_set_shortfall_within(
                            actual.success,
                            expected.success,
                            flipped,
                            depth + 1,
                        )
                    })
            }
            _ => None,
        }
    }

    fn lower_failure_members(
        &mut self,
        module_id: ModuleId,
        members: &[TypeUse],
        value_position: bool,
        generics: &HashMap<String, SymbolId>,
    ) -> FailureRowId {
        let mut rows = Vec::new();
        for member in members {
            let Some(row) = self.failure_member_row(module_id, member, value_position, generics) else {
                let found = match member {
                    TypeUse::Named { path, .. } => path
                        .iter()
                        .map(|part| part.text.as_str())
                        .collect::<Vec<_>>()
                        .join("."),
                    TypeUse::Function { .. } => "a function type".to_owned(),
                    TypeUse::Tuple { .. } => "a tuple type".to_owned(),
                    TypeUse::Record { .. } => "a record type".to_owned(),
                    TypeUse::Union { .. } => "a failure set".to_owned(),
                };
                self.type_error_range(
                    module_id,
                    "T4340",
                    type_use_range(member),
                    format!(
                        "`{found}` cannot be a member of a failure set; a member is an enum, one \
                         enum variant, a failure-set generic, or `Never`"
                    ),
                );
                continue;
            };
            let member_type = self.failure_set_type(row);
            self.type_facts
                .insert((module_id, member.semantic_id()), member_type);
            rows.push(row);
        }
        self.union_failure_rows(rows)
    }

    /// The row one written member denotes, or `None` when the member is not an
    /// enum, an enum variant, a failure-set parameter, or `Never`. An unresolved
    /// name is the empty row: resolution has reported it.
    fn failure_member_row(
        &mut self,
        module_id: ModuleId,
        member: &TypeUse,
        value_position: bool,
        generics: &HashMap<String, SymbolId>,
    ) -> Option<FailureRowId> {
        let TypeUse::Named {
            semantic_id, path, ..
        } = member
        else {
            return None;
        };
        let name = path.last()?;
        let Some(symbol) = self.resolved_type_symbol(module_id, *semantic_id) else {
            return Some(FailureRowId::new(0));
        };
        if self.is_builtin_type(symbol, "Never") {
            return Some(FailureRowId::new(0));
        }
        let kind = self.resolved.symbol(symbol).kind;
        // An alias of a failure set is that set, after `errors` as anywhere else.
        if kind == ProjectSymbolKind::TypeAlias {
            let TypeUse::Named { arguments, .. } = member else {
                unreachable!("checked above");
            };
            let aliased = self.alias_use_type(module_id, symbol, name, arguments);
            if aliased == TypeId::new(0) {
                return Some(FailureRowId::new(0));
            }
            return self.failure_set_of(aliased);
        }
        if kind == ProjectSymbolKind::GenericParameter {
            self.constrain_generic(module_id, symbol, GenericKind::FailureRow, name);
            if value_position {
                self.constrain_generic(module_id, symbol, GenericKind::Value, name);
            }
            return Some(self.intern_failure_row(FailureRow {
                variants: Vec::new(),
                variant_arguments: Vec::new(),
                parameters: vec![symbol],
                exclusions: Vec::new(),
            }));
        }
        if self.is_failure_variant(symbol) {
            return Some(self.variant_failure_row(symbol));
        }
        let failure_enum = kind == ProjectSymbolKind::Enum
            || matches!(
                self.resolved.standard_symbol(symbol),
                Some(StandardSymbol::Type(kind)) if kind.is_failure_enum()
            );
        if !failure_enum {
            return None;
        }
        let variants = self.enum_variant_symbols(symbol);
        // A generic enum written with its arguments, as `W<Int>`, keeps them on
        // every variant, the way a raised `W<Int>.Bad` does, so the set holds
        // `W<Int>` and not every `W`.
        let TypeUse::Named { arguments, .. } = member else {
            unreachable!("checked above");
        };
        let declared = self.named_type_generic_parameters(symbol).len();
        let variant_arguments = if !arguments.is_empty() && arguments.len() == declared {
            let lowered = arguments
                .iter()
                .map(|argument| self.lower_value_type(module_id, argument, generics))
                .collect::<Vec<_>>();
            variants
                .iter()
                .map(|variant| FailureVariantArguments {
                    variant: *variant,
                    arguments: lowered.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        Some(self.intern_failure_row(FailureRow {
            variants,
            variant_arguments,
            parameters: Vec::new(),
            exclusions: Vec::new(),
        }))
    }

    /// Whether `symbol` is a variant of a user enum or of a standard failure enum,
    /// which a type position may name as a one-member failure set.
    pub(super) fn is_failure_variant(&self, symbol: SymbolId) -> bool {
        match self.resolved.standard_symbol(symbol) {
            Some(StandardSymbol::Constructor(constructor)) => {
                constructor.type_kind().is_failure_enum()
            }
            _ => self.resolved.symbol(symbol).kind == ProjectSymbolKind::EnumConstructor,
        }
    }

    fn variant_failure_row(&mut self, variant: SymbolId) -> FailureRowId {
        self.intern_failure_row(FailureRow {
            variants: vec![variant],
            variant_arguments: Vec::new(),
            parameters: Vec::new(),
            exclusions: Vec::new(),
        })
    }
}

/// The source range a written type spans, from the first to the last name in
/// it. A type use keeps the ranges of its names only.
fn type_use_range(type_use: &TypeUse) -> TextRange {
    fn names(type_use: &TypeUse, ranges: &mut Vec<TextRange>) {
        match type_use {
            TypeUse::Named {
                path, arguments, ..
            } => {
                ranges.extend(path.iter().map(|name| name.range));
                for argument in arguments {
                    names(argument, ranges);
                }
            }
            TypeUse::Function {
                inputs,
                success,
                failures,
                ..
            } => {
                for input in inputs {
                    names(input, ranges);
                }
                names(success, ranges);
                if let Some(failures) = failures {
                    names(failures, ranges);
                }
            }
            TypeUse::Tuple { elements, .. }
            | TypeUse::Union {
                members: elements, ..
            } => {
                for element in elements {
                    names(element, ranges);
                }
            }
            TypeUse::Record { fields, .. } => {
                for field in fields {
                    ranges.push(field.name.range);
                    names(&field.type_use, ranges);
                }
            }
        }
    }
    let mut ranges = Vec::new();
    names(type_use, &mut ranges);
    let start = ranges.iter().map(|range| range.start()).min().unwrap_or(0);
    let end = ranges
        .iter()
        .map(|range| range.end())
        .max()
        .unwrap_or(start);
    TextRange::new(start as usize, end as usize)
}
