"""Mutants of the root-overflow fix: a root task that overflows its fiber stack
inside a loop region must end with the "Task stack overflow" defect, not crash
by allocating through the region scope its abandoned frames held.

Run: scripts/mutation/mutate.py scripts/mutation/root_overflow_mutations.py
Each mutant must turn the cli regression test red.
"""

CRATE = "compiler/chimi"

RUNTIME = "../../runtime/chimi-runtime/src/intrinsics/task_entry.rs"
TEST = "native_a_root_overflowing_inside_a_loop_ends_with_a_stack_overflow_defect"
RESTORE = "        (*main_heap).innermost = entry_scope;\n"

MUTATIONS = [
    dict(
        name="the root's abandoned regions stay open",
        file=RUNTIME,
        edits=[(RESTORE, "")],
        targets=["cli"],
        filter=TEST,
    ),
    dict(
        name="only a root that succeeded drops its regions",
        file=RUNTIME,
        edits=[(RESTORE, "        if outcome.state == FiberResult::SUCCEEDED {\n            (*main_heap).innermost = entry_scope;\n        }\n")],
        targets=["cli"],
        filter=TEST,
    ),
    dict(
        name="only a root that defected drops its regions",
        file=RUNTIME,
        edits=[(RESTORE, "        if outcome.state == FiberResult::DEFECTED {\n            (*main_heap).innermost = entry_scope;\n        }\n")],
        targets=["cli"],
        filter=TEST,
    ),
]
