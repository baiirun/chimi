"""Mutants of the runtime's inline compare steps (heap/compare.rs since
CHI-305 step 5, phase 4).

Run: scripts/mutation/mutate.py scripts/mutation/compare_stack_mutations.py
Each mutant must turn comparisons_past_the_inline_compare_steps_agree_with_the_interpreter red.
"""

CRATE = "compiler/chimi"

R = "../../runtime/chimi-runtime/src/heap/compare.rs"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["language"],
                filter="comparisons_past_the_inline_compare_steps")


MUTATIONS = [
 m("moving to the heap drops the inline steps",
   [("            self.spilled.extend_from_slice(written);\n", "")]),
 m("moving to the heap keeps only the first inline step",
   [("core::slice::from_raw_parts(self.inline.as_ptr().cast::<Step>(), self.length)",
     "core::slice::from_raw_parts(self.inline.as_ptr().cast::<Step>(), self.length.min(1))")]),
]
