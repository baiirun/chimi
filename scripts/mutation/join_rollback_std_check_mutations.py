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
        "        if result.is_none() {\n            self.restore_inference(snapshot);\n        }\n        result",
        "        drop(snapshot);\n        result",
    )], "a_failed_function_join"),
    lang("restore: bindings not restored", Y, [(
        "        self.inference_bindings = snapshot.bindings;\n",
        "",
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
