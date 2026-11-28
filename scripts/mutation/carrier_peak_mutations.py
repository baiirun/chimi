"""Mutants of R4 step 3 PR 6b: the memory report's peaks are the process's on several carriers.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_peak_mutations.py
Results: scripts/mutation/carrier_peak_results.txt.
"""

CRATE = "compiler/chimi"

ALLOCATOR = "../../runtime/chimi-runtime/src/heap/allocator.rs"


def rust(name, old, new):
    return dict(name=name, file=ALLOCATOR, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


REPORTS = "../../runtime/chimi-runtime/src/reports.rs"


def report(name, old, new):
    return dict(name=name, file=REPORTS, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


# The folds moved to the Rust allocator in CHI-305 step 5 phase 1, and the
# report's merge of the process peaks to reports.rs in phase 8.
MUTATIONS = [
    rust("a fall is never folded",
         "        (*here).heap_totals.footprint_bytes = (*here).heap_totals.footprint_bytes.wrapping_sub(bytes as i64);\n        fold_footprint(here, mode);",
         "        (*here).heap_totals.footprint_bytes = (*here).heap_totals.footprint_bytes.wrapping_sub(bytes as i64);"),
    rust("a fragment fall is never folded",
         "        fold_fragment_footprint(here, mode());\n",
         ""),
    report("the report keeps the largest carrier peak",
           "    if let Some((folded, folded_fragments)) = folded_peaks() {",
           "    if let Some((folded, folded_fragments)) = None::<(i64, i64)> {"),
    rust("a fold never raises the peak",
         "    raise(peak, process.fetch_add(change, Relaxed) + change);",
         "    let _ = (peak, process.fetch_add(change, Relaxed));"),
    rust("every change folds at once",
         "    if change < FOOTPRINT_FOLD_BYTES && change > -FOOTPRINT_FOLD_BYTES {",
         "    if change == 0 {"),
    rust("a fold forgets what it folded",
         "    *folded = now;\n",
         ""),
    report("the report ignores the footprint at exit",
           "        *peak = folded.max(footprint);",
           "        *peak = folded;"),
    rust("a new young chunk never folds",
         "        fold_footprint(here, mode());\n    }\n}",
         "    }\n}"),
]
