"""Mutants of a forced body freeing its handles and heap (CHI-265).

Run: scripts/mutation/mutate.py scripts/mutation/forced_release_mutations.py
Each mutant must turn the named tests red.
Results: scripts/mutation/forced_release_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"

TASKS = "../../runtime/chimi-runtime/src/intrinsics/tasks.rs"

GUARD = (
    "                    if !((*interrupted).kind == ControlKind::CANCEL && cancel_is_terminal()) {\n"
    "                        children.cancel_live();\n"
    "                    }\n"
)
ALWAYS = "                    children.cancel_live();\n"


def m(name, edits, targets, filter="", file=R):
    return dict(name=name, file=file, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
    m("a forced combinator cancels its children with requests",
      [(GUARD, ALWAYS)],
      ["task_release"], "a_forced_body_frees", TASKS),
    m("an interrupted combinator never cancels its children",
      [(GUARD, "")],
      ["task_combinators", "task_release"], file=TASKS),
    m("only a forced combinator cancels its children",
      [(GUARD, GUARD.replace("if !((*interrupted)", "if ((*interrupted)"))],
      ["task_combinators", "task_release"], file=TASKS),
    m("a forced body asks for its address to remember a defect",
      [("  if (chimi_fiber_cancel_is_terminal()) return;\n  ChimiProcessRequest self", "  ChimiProcessRequest self")],
      ["task_release"], "a_forced_body_frees"),
    m("a cancelled combinator cancels its children with requests",
      [(GUARD, ALWAYS)],
      ["task_combinators"], "task_cancel_of_a_task_waiting_in_a_combinator", TASKS),
]
