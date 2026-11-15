"""Mutants of bump allocation in loop regions (CHI-219, R2 part 2).

Run: scripts/mutation/mutate.py scripts/mutation/bump_allocation_mutations.py
Each mutant must turn a language test or a native cli test red.
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
 m("a large allocation is bumped",
   [("  if (!allocation_target && innermost && bytes <= YOUNG_CHUNK_LARGEST) {",
     "  if (!allocation_target && innermost) {")], LANGUAGE),
 m("list allocations of the pass are not rewritten",
   [("  forward_marked_list(&scope->young);\n", "")], LANGUAGE),
 m("the root keeps pointing into the chunk",
   [("  if (value) *value = forward_young(*value);\n", "  if (value) (void)forward_young(*value);\n")],
   LANGUAGE),
 m("a kept chunk is not zeroed",
   [("    memset(first->data, 0, (size_t)(first->used - first->data));\n", "")], LANGUAGE),
 m("dead channel endpoints are not dropped",
   [("        if (is_channel_endpoint(value)) channel_endpoint_drop(value->as.capability.channel);\n      }\n    }\n  }\n  scope->live_bytes -= dead_bytes;",
     "        (void)value;\n      }\n    }\n  }\n  scope->live_bytes -= dead_bytes;")],
   dict(targets=["cli", "language"], filter=None)),
 m("the counters of the pass are never added",
   [("  flush_young_counts(scope);\n  collecting_scope = scope;", "  collecting_scope = scope;")], HEAP),
]
