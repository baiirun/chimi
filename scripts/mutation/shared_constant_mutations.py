"""Mutants of R4 step 3 PR 5d-1: list_slice copies a module constant's
window, rather than compacting it in place, when carriers run on threads.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/shared_constant_mutations.py
Results: scripts/mutation/shared_constant_results.txt.
Each mutant runs tests/runtime_state.rs, whose carrier-threads-stress
fixture is built with ThreadSanitizer.
"""

CRATE = "compiler/chimi"

C = "../../runtime/chimi-runtime/src/heap/list.rs"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["runtime_state"], filter="")


# The slice moved to Rust in CHI-305 step 5, phase 4 (heap/list.rs). The Rust
# archive is not built with ThreadSanitizer, so these are caught by the
# fixture's own checks (the constant keeps its vector, a window of it is a
# copy, a long window shares), not by a race report.
MUTATIONS = [
    m("a shared constant is compacted",
      "            if retains_too_much(length, window.length) || shared {",
      "            if retains_too_much(length, window.length) {"),
    m("a constant is shared on one carrier too",
      "            let shared = carriers_on_threads() && (*area).role == AreaRole::STATIC;",
      "            let shared = (*area).role == AreaRole::STATIC;"),
    m("no constant is shared",
      "            let shared = carriers_on_threads() && (*area).role == AreaRole::STATIC;",
      "            let shared = false;"),
    m("every list is shared on threads",
      "            let shared = carriers_on_threads() && (*area).role == AreaRole::STATIC;",
      "            let shared = carriers_on_threads();"),
    m("a shared constant's window is shared, not copied",
      "                return list_value(copy_window(here, list, from, length, ptr::null_mut()), 0, length);",
      "                return list_value(window.vector, window.start + from, length);"),
    m("every constant's window is copied on threads, however long",
      "        if retains_too_much(length, window.vector.length) {",
      "        if retains_too_much(length, window.vector.length)\n"
      "            || (carriers_on_threads() && (*(*header_of(list)).area).role == AreaRole::STATIC)\n"
      "        {"),
]
