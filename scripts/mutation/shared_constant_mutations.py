"""Mutants of R4 step 3 PR 5d-1: list_slice copies a module constant's
window, rather than compacting it in place, when carriers run on threads.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/shared_constant_mutations.py
Results: scripts/mutation/shared_constant_results.txt.
Each mutant runs tests/runtime_state.rs, whose carrier-threads-stress
fixture is built with ThreadSanitizer.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["runtime_state"], filter="")


MUTATIONS = [
    m("a shared constant is compacted",
      "    if (list_retains_too_much(length, list->as.list.length) || list_shared_across_threads(list))",
      "    if (list_retains_too_much(length, list->as.list.length))"),
    m("a constant is shared on one carrier too",
      "  return carrier_count > 1 && allocation_header(list)->area->role == AREA_STATIC;",
      "  return carrier_count >= 1 && allocation_header(list)->area->role == AREA_STATIC;"),
    m("no constant is shared",
      "  return carrier_count > 1 && allocation_header(list)->area->role == AREA_STATIC;",
      "  return false && allocation_header(list)->area->role == AREA_STATIC;"),
    m("every list is shared on threads",
      "  return carrier_count > 1 && allocation_header(list)->area->role == AREA_STATIC;",
      "  return carrier_count > 1;"),
    m("a shared constant's window is shared, not copied",
      "    if (list_retains_too_much(length, list->as.list.length) || list_shared_across_threads(list))\n      return list_value(list_copy_window(list, from, length, NULL), 0, length);",
      "    if (list_retains_too_much(length, list->as.list.length) || list_shared_across_threads(list))\n      return list_value(list->as.list.vector, list->as.list.start + from, length);"),
]
