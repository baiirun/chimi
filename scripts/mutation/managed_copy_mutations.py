"""Mutants of the managed-handle backstop for broadcast and watch (CHI-240).

Run: scripts/mutation/mutate.py scripts/mutation/managed_copy_mutations.py
Each mutant must turn the channel-variant tests red.
Results: scripts/mutation/managed_copy_results.txt.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"


def variant(name, edits):
    return dict(name=name, file=RUNTIME, edits=edits, targets=["channel_variants"], filter="managed")


MUTATIONS = [
    variant("managed handles are not counted", [(
        "      if (transfer->collect) ++transfer->managed_count;\n",
        "")]),
    variant("broadcast publishes a managed handle", [(
        "      if (carried.managed_count && intrinsic != CHIMI_INTRINSIC_CHANNEL_SEND) {",
        "      if (carried.managed_count && false) {")]),
    variant("an ordinary channel refuses a managed handle too", [(
        "      if (carried.managed_count && intrinsic != CHIMI_INTRINSIC_CHANNEL_SEND) {",
        "      if (carried.managed_count) {")]),
    variant("a watch starts with a managed handle", [(
        "      if (carried_count || carried.managed_count) {",
        "      if (carried_count) {")]),
    variant("a second remove keeps the generic message", [(
        "                                    \"; it was removed already, through this handle or a copy of it, \"",
        "                                    \"; scheduler request failed \"")]),
]
