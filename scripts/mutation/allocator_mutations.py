"""Mutants of the allocator's per-allocation paths in Rust (CHI-305 step 5,
phase 1; runtime/chimi-runtime/src/heap/allocator.rs): allocate, release,
and the live-byte and footprint accounting they keep. They are caught by the
memory-report, flatness, handle-release and carrier tests, which run the
real runtime archive.

Run: scripts/mutation/mutate.py scripts/mutation/allocator_mutations.py
Results: scripts/mutation/allocator_results.txt.
"""

CRATE = "compiler/chimi"
ALLOCATOR = "../../runtime/chimi-runtime/src/heap/allocator.rs"
TARGETS = ["cli", "runtime_state", "task_release", "carriers"]


def m(name, old, new):
    return dict(name=name, file=ALLOCATOR, edits=[(old, new)], targets=TARGETS, filter="")


MUTATIONS = [
    m("a release keeps its footprint",
      "            footprint_sub(here, HEADER + bytes);\n", ""),
    m("a release stays a live allocation",
      "            (*here).heap_totals.live_allocations = (*here).heap_totals.live_allocations.wrapping_sub(1);\n", ""),
    m("a freed handle keeps its count",
      "            chimi_runtime_release_counted_value(header.add(1).cast());\n", ""),
    m("young bytes are not deferred to the footprint",
      "            (*here).pending_footprint_bytes = (*here).pending_footprint_bytes.wrapping_add((HEADER + bytes) as i64);\n",
      ""),
    m("the peak is never raised",
      "            (*here).heap_totals.peak_footprint_bytes = footprint;\n", ""),
    m("several carriers never fold",
      "    if mode != CARRIER_THREADS {", "    if true {"),
    m("unscoped bytes are not counted",
      "            (*here).unscoped_allocated_bytes = (*here).unscoped_allocated_bytes.wrapping_add(bytes as u64);\n", ""),
    m("a block outside the young chunks is not a heap's live bytes",
      "            heap_add_live((*area).heap, bytes);\n", ""),
    m("an allocation is not profiled",
      "            profile(heap, bytes);\n            return header.add(1).cast();",
      "            return header.add(1).cast();"),
]
