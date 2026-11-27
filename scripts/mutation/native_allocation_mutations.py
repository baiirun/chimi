"""Mutants of the R2 allocation wins in the native runtime (CHI-91).

Run: scripts/mutation/mutate.py scripts/mutation/native_allocation_mutations.py
Each mutant runs one native test in tests/language.rs; it must turn it red.

Not listed: a block that binds into its incoming locals without cloning. Every
binding has its own environment slot, so no program observes that; the clone
keeps the documented rule that only a binding boundary writes a table.
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

R = "runtime/native/runtime.c"


def m(name, edits, targets, filter="", file=R):
    return dict(name=name, file=file, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
 m("a block always binds into a fresh clone",
   [("                if eval.environment == incoming {\n"
     "                    eval.environment = clone_environment(incoming, true, false);\n"
     "                }\n",
     "                eval.environment = clone_environment(incoming, true, false);\n")],
   ["language"], "an_inner_block_binding", EVAL),
 m("shared values live in the current area",
   [("    carrier->fiber.target = &static_area;\n    ChimiValue *value = new_value(index == 0 ? VALUE_VOID : VALUE_BOOLEAN);",
     "    ChimiValue *value = new_value(index == 0 ? VALUE_VOID : VALUE_BOOLEAN);")],
   ["language"], "booleans_stay_booleans"),
 m("the stack array holds one subject fewer than the bound",
   [("            MATCH_STACK_SUBJECTS];", "            MATCH_STACK_SUBJECTS - 1];")],
   ["language"], "a_match_reads_every_one", EVAL),
 m("True and False share one value",
   [("static ChimiValue *boolean_value(bool raw) { return shared_value(raw ? 2 : 1); }",
     "static ChimiValue *boolean_value(bool raw) { return shared_value(raw ? 1 : 1); }")],
   ["language"], "booleans_stay_booleans"),
]
