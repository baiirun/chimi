"""Mutants of CHI-234 (a failed type join binds nothing) and CHI-198 (std check
names a missing module once).

Run: scripts/mutation/mutate.py scripts/mutation/join_rollback_std_check_mutations.py
"""

CRATE = "compiler/chimi"

F = "src/typing/failure_sets.rs"
Y = "src/typing.rs"
O = "src/operations.rs"


def lang(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def cli(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=filter)


MUTATIONS = [
    lang("join: a failed join keeps its bindings", F, [(
        "        self.tentatively(|checker| checker.join_types_within(left, right, false, 0))",
        "        self.join_types_within(left, right, false, 0)",
    )], "a_failed_function_join"),
    lang("tentatively: never restores", Y, [(
        "        if result.is_none() {\n            self.restore_inference(snapshot);\n        } else {",
        "        if false {\n            self.restore_inference(snapshot);\n        } else {",
    )], "a_failed_function_join"),
    lang("restore: a new binding not removed", Y, [(
        "                InferenceChange::Binding(variable, None) => {\n                    self.inference_bindings.remove(&variable);",
        "                InferenceChange::Binding(_variable, None) => {",
    )], "a_failed_function_join"),
    lang("journal: bindings not recorded", Y, [(
        "        if self.tentative_depth > 0 {\n            self.inference_journal\n                .push(InferenceChange::Binding(variable, previous));",
        "        if self.tentative_depth > 99 {\n            self.inference_journal\n                .push(InferenceChange::Binding(variable, previous));",
    )], "a_failed_function_join"),
    lang("call: generic argument unify not tentative", Y, [(
        "                    let unified = self.unify_tentatively(actual, bound);",
        "                    let unified = self.unify(actual, bound);",
    )], "a_failed_function_join"),
    cli("std check: a missing module listed per operation", O, [(
        "        if !modules.contains(module_of(operation.id)) {\n            continue;\n        }\n        match declarations",
        "        match declarations",
    )], "std_check_verifies"),
    cli("std check: a missing module not reported", O, [(
        "    for (module, count) in &missing_modules {",
        "    for (module, count) in missing_modules.iter().take(0) {",
    )], "std_check_verifies"),
]
