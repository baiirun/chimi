"""Mutants of the host-stack guard (CHI-230): a native program without tasks
that recurses past the host thread's stack must end with the "Stack overflow"
defect, and recursion that fits must still succeed.

Run: scripts/mutation/mutate.py scripts/mutation/host_stack_mutations.py
Each mutant must turn the cli regression test red.
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

RUNTIME = "runtime/native/runtime.c"
TEST = "native_deep_recursion_without_tasks_ends_with_a_stack_overflow_defect"


def cli(name, edits, file=RUNTIME):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=TEST)


MUTATIONS = [
    cli("calls never check the stack", [(
        '        if host_stack_exhausted() {\n            return defect(c"Stack overflow", -1);\n        }\n', "")], EVAL),
    cli("the main thread is never guarded", [(
        "    guard_host_stack();\n", "")]),
    cli("the check compares the wrong way", [(
        "return (uintptr_t)__builtin_frame_address(0) < host_stack_limit;",
        "return (uintptr_t)__builtin_frame_address(0) > host_stack_limit;")]),
    cli("the limit leaves no headroom", [(
        "  host_stack_limit = bottom + STACK_HEADROOM_BYTES;",
        "  host_stack_limit = bottom;")]),
    cli("the limit sits far above the stack bottom", [(
        "  host_stack_limit = bottom + STACK_HEADROOM_BYTES;",
        "  host_stack_limit = here - 64 * 1024;")]),
]
