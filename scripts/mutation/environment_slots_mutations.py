"""Mutants of per-body native environments (ts-51bbf7, CHI-176): each native
environment holds one slot per local of the body it runs, numbered per body.

Run: scripts/mutation/mutate.py scripts/mutation/environment_slots_mutations.py
Each mutant must turn the named test red.

Not listed: skipping a closure body in `body_locals`. Its locals then fall in
the shared range below every body's own slots, which is still collision-free
and costs only memory, so no behaviour test can tell (it survived, as an
equivalent mutant, in the first run).
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
DATA = "src/backend/program_data.rs"
CLOSURES = "a_function_literal_with_pattern_parameters_runs_and_captures_on_both_backends"


def language(name, file, edits, filter=CLOSURES):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def cli(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=filter)


MUTATIONS = [
    cli("environments keep the program-wide slot count", DATA, [(
        "        counts.push(next);\n",
        "        counts.push(core.symbols.len() as u32);\n")],
        "native_call_environments_do_not_grow_with_other_functions_locals"),
    language("a closure's environment is sized by nothing", DATA, [(
        "            *count = next.max(*count);\n",
        "            *count = 0;\n")]),
    language("a function value's calls get no local slots", RUNTIME, [(
        "      callable->local_count = function->local_count;\n"
        "      callable->environment = capture_requirements(environment);",
        "      callable->environment = capture_requirements(environment);")]),
    dict(name="a task capture skips the environment's locals", file=RUNTIME, edits=[(
        "        for (uint32_t slot = environment->local_count; slot-- > 0;)",
        "        for (uint32_t slot = 0; slot-- > 0;)")],
        targets=["cli"], filter="native_tasks_capture_render_encode_and_send_deeply_nested_values"),
]
