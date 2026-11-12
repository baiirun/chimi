"""Mutants of the deep-value walks (ts-0e026f, ts-9b1ec1): the interpreter's
release queue and outcome writer, and the native runtime's outcome writer, debug
renderer, and generational marking.

Run: scripts/mutation/mutate.py scripts/mutation/deep_value_mutations.py
Each mutant runs the tests named in its filter; it must turn one red. A walk that
recurses again overflows a small test stack, which aborts the test binary.
"""

CRATE = "compiler/chimi"

EXECUTION = "src/execution.rs"
TEARDOWN = "src/execution/teardown.rs"
RUNTIME = "runtime/native/runtime.c"


def lib(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="execution::tests")


def language(name, edits, filter):
    return dict(name=name, file=RUNTIME, edits=edits, targets=["language"], filter=filter)


MUTATIONS = [
    lib("release frees nested parts in place", TEARDOWN, [(
        "        if queue.draining.get() {",
        "        if false {",
    )]),
    lib("maps are released the derived way", EXECUTION, [(
        "                | Self::Map(_)\n                | Self::Callable(_)",
        "                | Self::Callable(_)",
    )]),
    lib("closures are released the derived way", EXECUTION, [(
        "                | Self::Map(_)\n                | Self::Callable(_)",
        "                | Self::Map(_)",
    )]),
    lib("outcome records are freed the derived way", EXECUTION, [(
        "            Self::List(_) | Self::Tuple(_) | Self::Record(_) | Self::Constructed { .. }",
        "            Self::List(_) | Self::Tuple(_) | Self::Constructed { .. }",
    )]),
    lib("outcome writer writes a field's value before its name", EXECUTION, [(
        "        steps.push(JsonStep::Value(&field.value));\n        steps.push(JsonStep::Name(&field.name));",
        "        steps.push(JsonStep::Name(&field.name));\n        steps.push(JsonStep::Value(&field.value));",
    )]),
    language("native outcome writer writes a field's value before its name", [(
        "    json_step(steps, JSON_STEP_VALUE, fields[index].value);\n    if (named) json_step(steps, JSON_STEP_NAME, fields[index].name);",
        "    if (named) json_step(steps, JSON_STEP_NAME, fields[index].name);\n    json_step(steps, JSON_STEP_VALUE, fields[index].value);",
    )], "deeply_nested"),
    language("native debug renderer queues list parts first to last", [(
        "      for (uint32_t i = length; i-- > 0;) debug_part(steps, i, false, NULL, list_get(value, i));",
        "      for (uint32_t i = 0; i < length; ++i) debug_part(steps, i, false, NULL, list_get(value, i));",
    )], "render"),
    language("native debug renderer closes before restoring the depth", [(
        "        --renderer->depth;\n        debug_close(renderer, step.pointer, step.spaced);",
        "        debug_close(renderer, step.pointer, step.spaced);\n        --renderer->depth;",
    )], "render"),
    dict(
        name="native marking rescans the whole state on every pass",
        file=RUNTIME,
        edits=[(
            "  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline &&\n"
            "              survivors - scope->full_baseline >= REGION_FULL_MIN_GROWTH;",
            "  bool full = true; (void)survivors;",
        )],
        targets=["cli"],
        filter="native_marking_work_is_linear",
    ),
]
