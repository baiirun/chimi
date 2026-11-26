"""Mutants of R4 step 3 PR 6b: the memory report's peaks are the process's on several carriers.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_peak_mutations.py
Results: scripts/mutation/carrier_peak_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


ALLOCATOR = "../../runtime/chimi-runtime/src/heap/allocator.rs"


def rust(name, old, new):
    return dict(name=name, file=ALLOCATOR, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


# The folds moved to the Rust allocator in CHI-305 step 5 phase 1; the
# report's merge of the process peaks stays in runtime.c.
MUTATIONS = [
    rust("a fall is never folded",
         "        (*here).heap_totals.footprint_bytes = (*here).heap_totals.footprint_bytes.wrapping_sub(bytes as i64);\n        fold_footprint(here, mode);",
         "        (*here).heap_totals.footprint_bytes = (*here).heap_totals.footprint_bytes.wrapping_sub(bytes as i64);"),
    rust("a fragment fall is never folded",
         "        fold_fragment_footprint(here, mode());\n",
         ""),
    m("the report keeps the largest carrier peak",
      "  if (carrier_count == 1) return;\n  int64_t peak = atomic_load_explicit",
      "  if (carrier_count >= 1) return;\n  int64_t peak = atomic_load_explicit"),
    rust("a fold never raises the peak",
         "    raise(peak, process.fetch_add(change, Relaxed) + change);",
         "    let _ = (peak, process.fetch_add(change, Relaxed));"),
    rust("every change folds at once",
         "    if change < FOOTPRINT_FOLD_BYTES && change > -FOOTPRINT_FOLD_BYTES {",
         "    if change == 0 {"),
    rust("a fold forgets what it folded",
         "    *folded = now;\n",
         ""),
    m("the report ignores the footprint at exit",
      "  totals->peak_footprint_bytes = peak > totals->footprint_bytes ? peak : totals->footprint_bytes;",
      "  totals->peak_footprint_bytes = peak;"),
    rust("a new young chunk never folds",
         "        fold_footprint(here, mode());\n    }\n}",
         "    }\n}"),
]
