"""Mutants of the prototype's per-owner heap (c/heap.c).

Run: scripts/mutation/mutate.py scripts/mutation/heap_mutations.py
Each mutant runs tests/heap.rs and tests/stress.rs; it must turn one red.
"""

CRATE = "prototypes/supervised-process-runtime"
CARGO_TEST_ARGS = ["--release"]

H = "c/heap.c"


def m(name, edits, filter=""):
    return dict(name=name, file=H, edits=edits, targets=["heap", "stress"], filter=filter)


MUTATIONS = [
 m("full collection on every pass", [("  bool full = region->survivors.bytes > 0 &&\n              region->survivors.bytes >= 2 * region->survivor_bytes_at_last_full;", "  bool full = true;")]),
 m("never run a full collection", [("  bool full = region->survivors.bytes > 0 &&\n              region->survivors.bytes >= 2 * region->survivor_bytes_at_last_full;", "  bool full = false;")]),
 m("rescan all survivors every pass", [("  pass.scan = scan_start(&region->survivors);\n", "  pass.scan = scan_start(&region->survivors);\n  { Chunk *oldest = region->survivors.chunks; while (oldest->next) oldest = oldest->next; pass.scan.chunk = oldest; pass.scan.offset = HEADER_BYTES; }\n")]),
 m("drop region check in evacuate", [("  if (chunk->region != pass->region ||\n      (chunk->role", "  if (\n      (chunk->role")]),
 m("adopt into base area", [("  Area *area = allocation_area(heap, &region, &role);\n  size_t bytes = fragment->area.bytes;", "  Area *area = allocation_area(heap, &region, &role);\n  area = &heap->base; region = NULL; role = ROLE_BASE;\n  size_t bytes = fragment->area.bytes;")]),
 m("exit tags spliced chunks as base", [("  ChunkRole role = outer ? ROLE_YOUNG : ROLE_BASE;", "  ChunkRole role = ROLE_BASE;")], "inner_passes"),
 m("exit drops inner survivors", [("  area_splice(into, &region->survivors, heap->owner, outer, role);\n", "")], "inner_passes"),
 m("ignore the owner tag", [("  if (chunk->owner != pass->heap->owner) {", "  if (0) {"), ("  if (chunk_of(value)->owner != copy->owner) {", "  if (0) {")], "another_heap"),
 m("unadopted fragment keeps its chunks", [("  area_release(&fragment->area);\n  free(fragment);", "  free(fragment);")]),
]
