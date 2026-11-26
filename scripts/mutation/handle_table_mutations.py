"""Mutants of runtime.c's lock around the native handle counts (R4 step 2;
since CHI-308 step 1 it counts native handles only, and the scheduler counts
channels and tasks; since CHI-309 part 2 the map itself is Rust, mutated in
handle_counts_mutations.py).

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
    m("retain does not take the lock",
      [("  pthread_mutex_lock(&handle_count_lock);\n  if (!chimi_handle_counts_retain_v1(key)) exhausted_memory();\n  pthread_mutex_unlock(&handle_count_lock);",
        "  if (!chimi_handle_counts_retain_v1(key)) exhausted_memory();")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("drop does not take the lock",
      [("  pthread_mutex_lock(&handle_count_lock);\n  bool last = chimi_handle_counts_drop_v1(key);\n  pthread_mutex_unlock(&handle_count_lock);",
        "  bool last = chimi_handle_counts_drop_v1(key);")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("a retain is not counted",
      [("  if (!chimi_handle_counts_retain_v1(key)) exhausted_memory();",
        "  (void)key;")],
      ["runtime_state", "net"]),
]
