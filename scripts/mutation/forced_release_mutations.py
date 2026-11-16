"""Mutants of a forced body freeing its handles and heap (CHI-265).

Run: scripts/mutation/mutate.py scripts/mutation/forced_release_mutations.py
Each mutant must turn the named tests red.
Results: scripts/mutation/forced_release_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"

GUARD = "      if (!(interrupted->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()))\n"


def m(name, edits, targets, filter=""):
    return dict(name=name, file=R, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
    m("a forced combinator cancels its children with requests",
      [(GUARD, "")],
      ["task_release"], "a_body_forced_inside_a_combinator"),
    m("an interrupted combinator never cancels its children",
      [(GUARD + "        cancel_live_children(&children);\n", "")],
      ["task_combinators", "task_release"]),
    m("only a forced combinator cancels its children",
      [(GUARD, "      if (interrupted->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal())\n")],
      ["task_combinators", "task_release"]),
]
