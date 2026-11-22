"""Mutants of R4 step 4 PR 4b: `carrier` reads the thread pointer at every use.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_accessor_mutations.py
Results: scripts/mutation/carrier_accessor_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


MUTATIONS = [
    m("the offset is never taken",
      "  carrier_tls_offset = (char *)&carrier_this_thread - carrier_thread_pointer();",
      "  carrier_tls_offset = 0;"),
    m("every thread reads the main thread's carrier",
      "  return *(Carrier **)(carrier_thread_pointer() + carrier_tls_offset);",
      "  static char *first;\n  if (!first) first = carrier_thread_pointer();\n  return *(Carrier **)(first + carrier_tls_offset);"),
]
