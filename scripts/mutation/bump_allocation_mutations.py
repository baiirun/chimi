"""Mutants of bump allocation in loop regions (CHI-219, R2 part 2).

Run: scripts/mutation/mutate.py scripts/mutation/bump_allocation_mutations.py
Each mutant must turn a language test or a native cli test red.
The bump itself, the chunk reset and the dead-handle drop are Rust since
CHI-305 step 5 phase 1; their mutants are in young_mutations.py.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"

LANGUAGE = dict(targets=["language"], filter=None)
HEAP = dict(targets=["cli"], filter="native_")


def m(name, edits, selection):
    return dict(name=name, file=R, edits=edits, **selection)


MUTATIONS = [
 m("a list node's slots are not rewritten",
   [("      for (uint32_t index = 0; index < node->count; ++index) FORWARD(node->slots[index]);\n", "")],
   LANGUAGE),
 m("a record's fields are not rewritten",
   [("      for (size_t index = 0; index < length; ++index) FORWARD(fields[index].value);\n", "")],
   LANGUAGE),
 m("a value moved twice gets two copies",
   [("  if (header->next) return header->next + 1;\n", "")], HEAP),
 m("chunks are reset before the survivors move",
   [("  forward_survivors(scope, value, control);\n  reset_young_chunks(scope, !ending);\n",
     "  reset_young_chunks(scope, !ending);\n  forward_survivors(scope, value, control);\n")],
   LANGUAGE),
 m("list allocations of the pass are not rewritten",
   [("  forward_marked_list(&scope->young);\n", "")], LANGUAGE),
 m("the root keeps pointing into the chunk",
   [("  if (value) *value = forward_young(*value);\n", "  if (value) (void)forward_young(*value);\n")],
   LANGUAGE),
 m("a region never links after a pass that mostly survives",
   [("  scope->links_young = 4 * (scope->promoted_bytes - promoted_before) > young_bytes;\n",
     "  scope->links_young = false;\n")], HEAP),
 m("a region that links never bumps again",
   [("  scope->links_young = 4 * (scope->promoted_bytes - promoted_before) > young_bytes;\n",
     "  scope->links_young = scope->links_young || 4 * (scope->promoted_bytes - promoted_before) > young_bytes;\n")],
   HEAP),
 m("a control's cause is not rewritten",
   [("      FORWARD(((ChimiControl *)pointer)->cause);\n", "")],
   dict(targets=["cli", "language"], filter=None)),
 m("the counters of the pass are never added",
   [("  flush_young_counts(scope);\n  here->collecting_scope = scope;", "  here->collecting_scope = scope;")], HEAP),
]
