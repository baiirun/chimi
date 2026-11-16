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
      ["task_release"], "a_forced_body_frees"),
    m("an interrupted combinator never cancels its children",
      [(GUARD + "        cancel_live_children(&children);\n", "")],
      ["task_combinators", "task_release"]),
    m("only a forced combinator cancels its children",
      [(GUARD, "      if (interrupted->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal())\n")],
      ["task_combinators", "task_release"]),
    m("a forced body asks for its address to remember a defect",
      [("  if (chimi_fiber_cancel_is_terminal()) return;\n  ChimiProcessRequest self", "  ChimiProcessRequest self")],
      ["task_release"], "a_forced_body_frees"),
    m("a cancelled combinator cancels its children with requests",
      [(GUARD, "")],
      ["task_combinators"], "task_cancel_of_a_task_waiting_in_a_combinator"),
]
