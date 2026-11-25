"""Mutants of the failure-set polish (CHI-218, CHI-222): written variants narrowed
where a closed set is expected, call arguments joined by union while only
covariant uses bound them, forwarded generics promoted to failure sets, and the
T4203 note naming the failure-set parameter.

Run: scripts/mutation/mutate.py scripts/mutation/failure_set_polish_mutations.py
"""

CRATE = "compiler/chimi"

F = "src/typing/failure_sets.rs"
Y = "src/typing.rs"


def lang(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def unit(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


MUTATIONS = [
    lang("written variant: never narrowed", Y, [(
        "        if !closed_set {\n            return actual;\n        }",
        "        if closed_set || !closed_set {\n            return actual;\n        }",
    )], "written_variant"),
    lang("written variant: block tail not followed", Y, [(
        "        while let ModuleExpression::Block { items, .. } = tail {",
        "        while let ModuleExpression::Block { items, .. } = tail {\n            if items.len() < 100 { return actual; }",
    )], "written_variant"),
    lang("written variant: call arguments not narrowed", Y, [(
        "                success: self.narrow_constructed_value(module_id, value, argument.success, expected),",
        "                success: argument.success,",
    )], "written_variant"),
    lang("written variant: let annotation not narrowed", Y, [(
        "                                    let value_type = self.narrow_constructed_value(\n                                        module_id,\n                                        value,\n                                        value_fact.success,\n                                        annotated,\n                                    );",
        "                                    let value_type = value_fact.success;",
    )], "written_variant"),
    lang("join: a fixing use does not stop widening", Y, [(
        "        let fixes = input || self.invariant_argument_depth > 0;",
        "        let fixes = input && self.invariant_argument_depth > 0;",
    )], "two_call_arguments"),
    lang("join: value generics never widen", Y, [(
        "                    if widenable\n                        && !fixes\n                        && let Some(joined) = self.join_alternative_types(actual, bound)",
        "                    if false\n                        && let Some(joined) = self.join_alternative_types(actual, bound)",
    )], "two_call_arguments"),
    lang("join: failure-set generics never widen", Y, [(
        "            if widenable && !fixes {\n                let bound_type = self.failure_set_type(bound);",
        "            if false {\n                let bound_type = self.failure_set_type(bound);",
    )], "two_call_arguments"),
    unit("join: failure-set generic widens two plain enums", Y, [(
        "                if let Some(widened) = self\n                    .join_alternative_types(actual_type, bound_type)\n                    .and_then(|joined| self.failure_set_of(joined))",
        "                if let Some(widened) = Some(self.union_failure_rows([bound, actual]))",
    )], "failure_set_value_arguments_must_match_the_bound_failure_row"),
    lang("join: a callback use does not mark the binding fixed", Y, [(
        "        if fixes {\n            substitution.widenable_failures.remove(&parameter);\n        }",
        "",
    )] + [(
        "                let widenable = if fixes {\n                    substitution.widenable_values.remove(&parameter)\n                } else {\n                    substitution.widenable_values.contains(&parameter)\n                };",
        "                let widenable = substitution.widenable_values.contains(&parameter);",
    )], "two_call_arguments"),
    lang("forwarding: generic not promoted", F, [(
        "            self.generic_kinds\n                .insert(parameter, GenericKind::FailureSet);\n        }\n    }\n\n    /// Why a call argument",
        "        }\n    }\n\n    /// Why a call argument",
    )], "forwarding_a_generic"),
    lang("forwarding: promotion does not trigger another pass", Y, [(
        "requirements_changed || kinds_changed",
        "requirements_changed",
    )], "forwarding_a_generic"),
    lang("variance: a row under a function input stays covariant", F, [(
        "                    || (inside_input\n                        && (self.failure_rows[contract.failures.raw() as usize]",
        "                    || (false\n                        && (self.failure_rows[contract.failures.raw() as usize]",
    )], "a_failure_row_under_a_function_input_is_not_covariant"),
    lang("note: not appended", Y, [(
        "                            message = format!(\"{message}: {note}\");",
        "",
    )], "a_value_passed_for_a_failure_set_generic"),
]
