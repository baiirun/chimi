"""Mutants of `chimi run -- <arguments>` (ts-b37746) in compiler/chimi/src/main.rs.

Run: scripts/mutation/mutate.py scripts/mutation/run_arguments_mutations.py
Each mutant runs the `run_` CLI tests and the argument parser unit tests.
"""

CRATE = "compiler/chimi"

TARGETS = ["cli"]

MUTATIONS = [
    dict(
        name="arguments parsed but not passed to main",
        file="src/main.rs",
        edits=[("ExecutionConfig::for_process().with_application_arguments(arguments)",
                "ExecutionConfig::for_process().with_application_arguments(Vec::<String>::new())")],
        targets=TARGETS, filter="run_",
    ),
    dict(
        name="zero-input main silently drops arguments",
        file="src/main.rs",
        edits=[("if !arguments.is_empty() && !program.entry_accepts_arguments() {",
                "if false && !program.entry_accepts_arguments() {")],
        targets=TARGETS, filter="run_",
    ),
    dict(
        name="a later separator ends the arguments",
        file="src/main.rs",
        edits=[("                    let Ok(values) = arguments\n",
                "                    let Ok(values) = arguments\n                        .take_while(|value| value != \"--\")\n")],
        targets=TARGETS, filter="run_",
    ),
    dict(
        name="arguments accepted without the separator",
        file="src/main.rs",
        edits=[("                Some(separator) if separator == \"--\" => {",
                "                Some(_) if true => {")],
        targets=TARGETS, filter="run_",
    ),
]
