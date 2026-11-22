"""Mutants of R4 step 3 PR 6b: the memory report's peaks are the process's on several carriers.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_peak_mutations.py
Results: scripts/mutation/carrier_peak_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


MUTATIONS = [
    m("a fall is never folded",
      "  carrier->heap_totals.footprint_bytes -= (int64_t)bytes;\n  footprint_fold(footprint_now(),",
      "  carrier->heap_totals.footprint_bytes -= (int64_t)bytes;\n  if (0) footprint_fold(footprint_now(),"),
    m("a fragment fall is never folded",
      "  carrier->heap_totals.fragment_footprint_bytes -= (int64_t)bytes;\n  fragment_footprint_fold();",
      "  carrier->heap_totals.fragment_footprint_bytes -= (int64_t)bytes;"),
    m("the report keeps the largest carrier peak",
      "  if (carrier_count == 1) return;\n  int64_t peak = atomic_load_explicit",
      "  if (carrier_count >= 1) return;\n  int64_t peak = atomic_load_explicit"),
    m("a fold never raises the peak",
      "  while (value > high &&",
      "  while (value > high && 0 &&"),
    m("every change folds at once",
      "  if (change < FOOTPRINT_FOLD_BYTES && change > -FOOTPRINT_FOLD_BYTES) return;",
      "  if (change == 0) return;"),
    m("a fold forgets what it folded",
      "  *folded = now;\n  atomic_raise",
      "  atomic_raise"),
    m("the report ignores the footprint at exit",
      "  totals->peak_footprint_bytes = peak > totals->footprint_bytes ? peak : totals->footprint_bytes;",
      "  totals->peak_footprint_bytes = peak;"),
]

MUTATIONS += [
    m("a new young chunk never folds",
      "    footprint_fold(footprint_now(), &carrier->folded_footprint_bytes, &process_footprint_bytes,\n                   &process_peak_footprint_bytes);\n  }",
      "  }"),
]
