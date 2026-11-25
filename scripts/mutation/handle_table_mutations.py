"""Mutants of the shared native handle table (R4 step 2; since CHI-308 step 1
it counts native handles only, and the scheduler counts channels and tasks).

Run: scripts/mutation/mutate.py scripts/mutation/handle_table_mutations.py
Each mutant must turn the named tests red. The queue mutants of R4 step 2
went with the queue; handle_count_mutations.py covers the scheduler's.
Results: scripts/mutation/handle_table_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits, targets, filter=""):
    return dict(name=name, file=R, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
    m("retain does not take the table's lock",
      [("static void native_handle_retain(uint64_t channel) {\n  pthread_mutex_lock(&handle_table.lock);",
        "static void native_handle_retain(uint64_t channel) {\n"),
       ("  ++slots[slot].endpoints;\n  pthread_mutex_unlock(&handle_table.lock);",
        "  ++slots[slot].endpoints;\n")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("drop does not take the table's lock",
      [("  pthread_mutex_lock(&handle_table.lock);\n  bool last = handle_table_drop(channel);\n  pthread_mutex_unlock(&handle_table.lock);\n  return last;",
        "  return handle_table_drop(channel);")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("a drop reports the last reference one early",
      [("  if (--slots[slot].endpoints != 0) return false;",
        "  if (--slots[slot].endpoints > 1) return false;")],
      ["runtime_state", "net"]),
]
