"""Mutants of the R2 allocation wins in the native runtime (CHI-91).

Run: scripts/mutation/mutate.py scripts/mutation/native_allocation_mutations.py
Each mutant runs one native test in tests/language.rs; it must turn it red.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits, targets, filter=""):
    return dict(name=name, file=R, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
 m("a block binds into the enclosing locals",
   [("      if (environment == incoming) environment = clone_local_environment(incoming);\n", "")],
   ["language"], "an_inner_block_binding"),
 m("a block always binds into a fresh clone",
   [("      if (environment == incoming) environment = clone_local_environment(incoming);\n",
     "      environment = clone_local_environment(incoming);\n")],
   ["language"], "an_inner_block_binding"),
 m("shared values live in the current area",
   [("    allocation_target = &static_area;\n    ChimiValue *value = new_value(index == 0 ? VALUE_VOID : VALUE_BOOLEAN);",
     "    ChimiValue *value = new_value(index == 0 ? VALUE_VOID : VALUE_BOOLEAN);")],
   ["language"], "booleans_stay_booleans"),
 m("the stack array holds one subject fewer than the bound",
   [("  const ChimiNode *node = &CHIMI_NODES[node_id]; MatchValue stack_values[MATCH_STACK_SUBJECTS];",
     "  const ChimiNode *node = &CHIMI_NODES[node_id]; MatchValue stack_values[MATCH_STACK_SUBJECTS - 1];")],
   ["language"], "a_match_reads_every_one"),
 m("True and False share one value",
   [("static ChimiValue *boolean_value(bool raw) { return shared_value(raw ? 2 : 1); }",
     "static ChimiValue *boolean_value(bool raw) { return shared_value(raw ? 1 : 1); }")],
   ["language"], "booleans_stay_booleans"),
]
