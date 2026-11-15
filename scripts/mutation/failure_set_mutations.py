"""Mutants of failure-set types and std/result (CHI-200): parsing and formatting `|`,
member resolution, subset and union joins, covariance, projection, pattern domains,
and the std/result programs.

Run: scripts/mutation/mutate.py scripts/mutation/failure_set_mutations.py
Each mutant runs the failure-set language tests or the std/result programs; it must
turn one red.
"""

CRATE = "compiler/chimi"

F = "src/typing/failure_sets.rs"
Y = "src/typing.rs"
R = "src/resolution.rs"
P = "src/syntax.rs"
M = "src/formatter.rs"


def lang(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def std(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"], filter="")


MUTATIONS = [
    # Syntax and formatter.
    lang("parser: `+` is not a failure-row separator", P, [(
        "while self.at(TokenKind::Bar) || (accepts_plus && self.at(TokenKind::Plus)) {",
        "while self.at(TokenKind::Bar) {")], "a_plus_between"),
    lang("formatter: keeps `+` in a failure row", M, [(
        "TokenKind::Bar | TokenKind::Plus if token.parent().kind() == NodeKind::UnionType => {",
        "TokenKind::Bar if token.parent().kind() == NodeKind::UnionType => {")], "a_plus_between"),
    # Resolution.
    lang("resolution: a variant is not a type", R, [(
        "        variant.then_some(ReferenceResolution::Resolved(member))",
        "        let _ = variant;\n        None")], "variant"),
    # Members.
    lang("members: accept any named type", F, [(
        "        failure_enum.then(|| {",
        "        (failure_enum || true).then(|| {")], "a_failure_set_member_must_be"),
    # Subset and joins.
    lang("unify: a set does not fit a wider set", Y, [(
        "                if self.failure_set_is_subset(actual, expected) {\n                    return true;\n                }",
        "                if false && self.failure_set_is_subset(actual, expected) {\n                    return true;\n                }")], "a_failure_set_parameter_accepts"),
    lang("unify: an inferred set does not widen to the union", Y, [(
        "                let union = self.join_failure_sets([actual, expected]);\n                let joined = self.failure_set_type(union);\n                self.inference_bindings.insert(variable, joined);\n                true",
        "                let _ = variable;\n                false")], "no_check_has_bounded"),
    lang("covariance: no argument is covariant", F, [(
        "        self.parameter_is_covariant_within(owner, index, 0)\n",
        "        false && self.parameter_is_covariant_within(owner, index, 0)\n")], "join"),
    lang("covariance: function inputs count as covariant", F, [(
        "                contract.inputs.iter().any(|input| recurse(*input, true))",
        "                contract.inputs.iter().any(|input| recurse(*input, inside_input))")], "function_input_does_not_join"),
    lang("covariance: a nested data type passes its input through", F, [(
        "                    recurse(*argument, inside_input || !covariant)",
        "                    recurse(*argument, inside_input && !covariant)")], "function_input_does_not_join"),
    lang("joins: two bare enums join silently", F, [(
        "            && (inside_argument\n                || self.is_failure_set_type(left)",
        "            && (true\n                || self.is_failure_set_type(left)")], "bare_enums"),
    lang("T4346: no variant is missing", F, [(
        "            .filter(|variant| !self.failure_row_may_contain(expected, *variant))",
        "            .filter(|variant| !self.failure_row_may_contain(expected, *variant) && false)")], "missing_variants"),
    # Projection.
    lang("projection: a lone parameter row stays a set value", Y, [(
        "        ) && self.generic_kinds.get(parameter) == Some(&GenericKind::FailureSet)\n        {",
        "        ) && false\n        {")], ""),
    lang("instantiation: a failure set takes any type", Y, [(
        "                        return matches!(\n                            self.types[actual.raw() as usize],\n                            CanonicalType::InferenceVariable(_) | CanonicalType::Poison\n                        );",
        "                        substitution.values.insert(parameter, actual);\n                        return true;")], "takes_only_a_failure_set"),
    lang("patterns: a failure-set parameter is not an open set", F, [(
        "        if self.generic_kinds.get(&parameter) != Some(&GenericKind::FailureSet) {\n            return type_id;\n        }\n        let row",
        "        if self.generic_kinds.get(&parameter) != Some(&GenericKind::FailureSet) || true {\n            return type_id;\n        }\n        let row")], "needs_a_catch_all"),
    lang("patterns: a failure type selects variants not in the row", Y, [(
        "            .filter(|variant| self.failure_row_may_contain(failures, *variant))\n            .collect::<Vec<_>>();\n        if variants.is_empty() {",
        "            .collect::<Vec<_>>();\n        if variants.is_empty() {")], "binds_only_the_variants"),
    # Variance and stale widening (review fixes).
    lang("variance: an invariant argument fits a wider one", Y, [(
        "                            && !self.type_parameter_is_covariant(left_symbol, index)\n                            && !self.unify(right, left)",
        "                            && false\n                            && !self.unify(right, left)")], "a_sender_of_one_enum"),
    lang("variance: function inputs keep the subset direction", Y, [(
        "                if !self.function_input_depth.is_multiple_of(2) {\n                    return self.failure_set_is_subset(expected, actual);",
        "                if false {\n                    return self.failure_set_is_subset(expected, actual);")], "a_callback_for_a_narrower"),
    lang("widening: a bounded variable still widens", Y, [(
        "                    .filter(|variable| !self.bounded_inference_variables.contains(variable))",
        "                    .filter(|_| true)")], "does_not_widen_later"),
    # std/result.
    std("domain: an uninhabited constructor still needs an arm", Y, [(
        "        domain.retain(|(_, fields)| !fields.iter().any(|field| self.is_uninhabited(*field)));",
        "")]),
    std("get: a raised generic stays a value", F, [(
        "            self.generic_kinds\n                .insert(parameter, GenericKind::FailureSet);",
        "            let _ = parameter;")]),
]
