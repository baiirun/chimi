"""Mutants of the runner's operation profile (T6).

Run: scripts/mutation/mutate.py scripts/mutation/runtime_profile_mutations.py
Each mutant must turn native_profile_counts_evaluations_and_allocations_per_operation red.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["cli"],
                filter="native_profile_counts_evaluations_and_allocations_per_operation")


MUTATIONS = [
 m("a node does not hand allocations back to its parent",
   [("  if (profile_rows) current_heap->profiled_node_plus_one = outer;", "  (void)outer;")]),
 m("profiling leaves direct loops on",
   [("  if (!runtime->silent || profile_rows) return chimi_rt_loop(", "  if (!runtime->silent) return chimi_rt_loop(")]),
 m("profiling leaves direct integer roots on",
   [("{ return runtime->full_trace || profile_rows; }", "{ return runtime->full_trace; }")]),
 m("rows are written lightest first",
   [("return x->allocated_bytes > y->allocated_bytes ? -1 : 1;", "return x->allocated_bytes > y->allocated_bytes ? 1 : -1;")]),
 m("allocations are not counted",
   [("    ++row->allocations;\n", "")]),
]
