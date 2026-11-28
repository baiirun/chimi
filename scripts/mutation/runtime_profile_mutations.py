"""Mutants of the runner's operation profile (T6).

Run: scripts/mutation/mutate.py scripts/mutation/runtime_profile_mutations.py
Each mutant must turn native_profile_counts_evaluations_and_allocations_per_operation red.
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

R = "runtime/native/runtime.c"
REPORTS = "../../runtime/chimi-runtime/src/reports.rs"


def m(name, edits, file=R):
    return dict(name=name, file=file, edits=edits, targets=["cli"],
                filter="native_profile_counts_evaluations_and_allocations_per_operation")


MUTATIONS = [
 m("a node does not hand allocations back to its parent",
   [("            (*(*current_carrier()).fiber.heap).profiled_node_plus_one = frame.outer;",
     "            let _ = frame.outer;")], EVAL),
 m("profiling leaves direct loops on",
   [("        if !(*runtime).silent || !profile_rows().is_null() {", "        if !(*runtime).silent {")], file=EVAL),
 m("profiling leaves direct integer roots on",
   [("{ return runtime->full_trace || profile_rows; }", "{ return runtime->full_trace; }")]),
 m("rows are written lightest first",
   [("y.allocated_bytes.cmp(&x.allocated_bytes)", "x.allocated_bytes.cmp(&y.allocated_bytes)")], REPORTS),
 m("ties are written fewest evaluations first",
   [(".then(y.evaluations.cmp(&x.evaluations))", ".then(x.evaluations.cmp(&y.evaluations))")], REPORTS),
 m("a profile adds up benchmark repeats",
   [("  if (profile_path && benchmark_repeats) {", "  if (false) {")]),
 m("allocations are not counted",
   [("    ++row->allocations;\n", "")]),
]
