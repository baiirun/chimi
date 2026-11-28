"""Mutants of the memory report in Rust (CHI-305 step 5, phase 8, eleventh PR).

Run: scripts/mutation/mutate.py scripts/mutation/memory_report_mutations.py
Results: scripts/mutation/memory_report_results.txt.
The peak fold's mutants are in carrier_peak_mutations.py and the task peak's
in task_handle_mutations.py.
"""

CRATE = "compiler/chimi"

REPORTS = "../../runtime/chimi-runtime/src/reports.rs"


def report(name, old, new):
    return dict(name=name, file=REPORTS, edits=[(old, new)], targets=["cli"], filter="native_")


MUTATIONS = [
    report("the report leaves frame environments out of environments",
           "by_kind(AllocationKind::ENVIRONMENT).wrapping_add(by_kind(AllocationKind::FRAME_ENVIRONMENT)),",
           "by_kind(AllocationKind::ENVIRONMENT),"),
    report("the report writes no unscoped bytes",
           "(b\",\\\"unscoped_allocated_bytes\\\":\", sum.unscoped_allocated_bytes),",
           "(b\",\\\"unscoped_allocated_bytes\\\":\", 0),"),
    report("the report writes the fragment peak as the footprint peak",
           "(b\",\\\"peak_footprint_bytes\\\":\", sum.peak_footprint_bytes as u64),",
           "(b\",\\\"peak_footprint_bytes\\\":\", sum.peak_fragment_footprint_bytes as u64),"),
]
