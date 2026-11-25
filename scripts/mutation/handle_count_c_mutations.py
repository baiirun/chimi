"""Mutants of runtime.c's side of the handle counts (CHI-308 step 1): the
split between the scheduler's channel and task counts and the native handle
table.

Run: scripts/mutation/mutate.py scripts/mutation/handle_count_c_mutations.py
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"

MUTATIONS = [
    dict(name="a channel or task value is not counted", file=R, edits=[(
        "  else chimi_fiber_handle_retain(key);\n",
        "",
    )], targets=["task_release"], filter=""),
    dict(name="a freed channel or task value is not uncounted", file=R, edits=[(
        "  if (!is_native_handle_key(key)) {\n    chimi_fiber_handle_drop(key);\n    return;\n  }\n",
        "  if (!is_native_handle_key(key)) return;\n",
    )], targets=["task_release"], filter=""),
    dict(name="a retain does not tell the freers to look", file=R, edits=[(
        "static void counted_handle_retain(uint64_t key) {\n  note_handle_counted();\n",
        "static void counted_handle_retain(uint64_t key) {\n",
    )], targets=["task_release"], filter=""),
    dict(name="a native handle no value names stays open", file=R, edits=[(
        "  if (native_handle_release(key)) native_handle_unreferenced(key);\n",
        "  (void)native_handle_release(key);\n",
    )], targets=["net", "std_exec"], filter=""),
    dict(name="a new execution keeps the last one's native handle counts", file=R, edits=[(
        "  chimi_handle_counts_reset_v1();\n",
        "",
    )], targets=["runtime_state"], filter="the_handle_table_counts_across_threads"),
]
