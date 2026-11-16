"""Mutants of R4 step 1: the runtime's per-thread state in one Carrier (CHI-267).

Run: scripts/mutation/mutate.py scripts/mutation/carrier_state_mutations.py
Each mutant must turn the named tests red.
Results: scripts/mutation/carrier_state_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits, targets, filter=""):
    return dict(name=name, file=R, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
    m("a resumed fiber keeps the heap another fiber left current",
      [("  carrier->fiber = fiber;\n  return command;", "  return command;")],
      ["task_release", "task_combinators"]),
    m("a resumed fiber gets its heap back but not its allocation target",
      [("  carrier->fiber = fiber;\n  return command;",
        "  carrier->fiber.heap = fiber.heap;\n  return command;")],
      ["task_release", "task_combinators", "cli"]),
    m("the carrier starts with no current heap",
      [("static Carrier carrier_state = {.fiber = {.heap = &main_heap}};",
        "static Carrier carrier_state;")],
      ["language"], "booleans_stay_booleans"),
    m("a new file-scope counter is added outside the carrier",
      [("static ChimiFiberCommand scheduler_request(void *request) {\n",
        "static unsigned scheduler_requests;\n"
        "static ChimiFiberCommand scheduler_request(void *request) {\n"
        "  if (++scheduler_requests == 0) abort();\n")],
      ["runtime_state"]),
    m("a function-local static is added outside the carrier",
      [("static ChimiFiberCommand scheduler_request(void *request) {\n",
        "static ChimiFiberCommand scheduler_request(void *request) {\n"
        "  static unsigned requests;\n  if (++requests == 0) abort();\n")],
      ["runtime_state"]),
]
