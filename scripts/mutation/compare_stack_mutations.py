"""Mutants of the runtime's inline compare steps.

Run: scripts/mutation/mutate.py scripts/mutation/compare_stack_mutations.py
Each mutant must turn comparisons_past_the_inline_compare_steps_agree_with_the_interpreter red.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["language"],
                filter="comparisons_past_the_inline_compare_steps")


MUTATIONS = [
 m("moving to the heap drops the inline steps",
   [("    if (!on_heap) memcpy(steps, stack->inline_steps, stack->length * sizeof(*steps));\n", "")]),
 m("moving to the heap keeps only the first inline step",
   [("memcpy(steps, stack->inline_steps, stack->length * sizeof(*steps))",
     "memcpy(steps, stack->inline_steps, sizeof(*steps))")]),
]
