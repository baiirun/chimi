"""Mutants of the young chunks and the regions' start and end in Rust
(CHI-305 step 5, phase 1, second PR; runtime/chimi-runtime/src/heap/young.rs
and the bump in allocator.rs). They are caught by the language suite, the
native cli tests and the process lab, which run the real runtime archive.

Run: scripts/mutation/mutate.py scripts/mutation/young_mutations.py
Results: scripts/mutation/young_results.txt.
"""

CRATE = "compiler/chimi"
YOUNG = "../../runtime/chimi-runtime/src/heap/young.rs"
ALLOCATOR = "../../runtime/chimi-runtime/src/heap/allocator.rs"
TARGETS = ["language", "cli", "process_lab", "task_release"]


def m(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=TARGETS, filter="")


MUTATIONS = [
    m("a large allocation is bumped", ALLOCATOR,
      "        if (*innermost).links_young || bytes > YOUNG_CHUNK_LARGEST {",
      "        if (*innermost).links_young {"),
    m("a kept chunk is not zeroed", YOUNG,
      "        if keep_first && !poisoning() {\n            zero_used(first);\n",
      "        if keep_first && !poisoning() {\n"),
    m("a cached chunk is not zeroed", YOUNG,
      "            zero_used(chunk);\n            (*chunk).next = (*here).young_chunk_cache;",
      "            (*chunk).next = (*here).young_chunk_cache;"),
    m("a pass that bumped only into its first chunk moves nothing", YOUNG,
      "        !first.is_null() && (!(*first).next.is_null() || (*scope).bump != data(first))",
      "        !first.is_null() && !(*first).next.is_null()"),
    m("dead handles in the chunks keep their counts", YOUNG,
      "                    chimi_runtime_release_counted_value(header.add(1).cast());\n",
      ""),
    m("a moved allocation is freed too", YOUNG,
      "                if !(*header).next.is_null() {\n                    continue;\n                }\n",
      ""),
    m("dead chunk allocations keep their footprint", YOUNG,
      "        footprint_sub(here, dead_count.wrapping_mul(HEADER).wrapping_add(dead_bytes));\n",
      ""),
    m("dead chunk allocations stay live bytes", YOUNG,
      "        (*scope).live_bytes = (*scope).live_bytes.wrapping_sub(dead_bytes);\n",
      ""),
    m("flushed young bytes are not live in the heap", YOUNG,
      "        heap_add_live((*scope).heap, bytes);\n        scope_add_live(scope, bytes);",
      "        scope_add_live(scope, bytes);"),
    m("flushed young bytes stay pending in the footprint", YOUNG,
      "        (*here).pending_footprint_bytes = (*here).pending_footprint_bytes.wrapping_sub(footprint as i64);\n",
      ""),
    m("a nested region does not flush its parent", YOUNG,
      "        if !parent.is_null() {\n            flush_counts(here, parent);\n        }\n",
      ""),
    m("a region's end does not restore its parent", YOUNG,
      "        (*heap).innermost = parent;\n",
      ""),
    m("a nested region's peak is not added to its parent's", YOUNG,
      "        if nested_peak > (*parent).peak_live_bytes {",
      "        if false && nested_peak > (*parent).peak_live_bytes {"),
    m("a nested region's live bytes are lost at its end", YOUNG,
      "        (*parent).live_bytes = (*parent).live_bytes.wrapping_add((*scope).live_bytes);\n",
      ""),
    m("a nested region's collections are not counted", YOUNG,
      "        (*parent).collections = (*parent).collections.wrapping_add((*scope).collections);\n",
      ""),
    m("a full chunk cache keeps growing", YOUNG,
      "        if ((*here).young_chunk_cache_count as usize) < YOUNG_CHUNK_CACHE {",
      "        if true {"),
]
