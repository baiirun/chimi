"""Mutants of runtime.c's side of the handle counts (CHI-308 step 1): the
split between the scheduler's channel and task counts and the native handle
table.

Run: scripts/mutation/mutate.py scripts/mutation/handle_count_c_mutations.py
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"

MUTATIONS = [
    dict(name="a channel or task value is not counted", file=R, edits=[(
        "  atomic_store_explicit(&scheduler_handles_counted, true, memory_order_relaxed);\n  chimi_fiber_handle_retain(key);\n",
        "  atomic_store_explicit(&scheduler_handles_counted, true, memory_order_relaxed);\n",
    )], targets=["task_release"], filter=""),
    dict(name="a freed channel or task value is not uncounted", file=R, edits=[(
        "  if (!is_native_handle_key(key)) {\n    chimi_fiber_handle_drop(key);\n    return;\n  }\n",
        "  if (!is_native_handle_key(key)) return;\n",
    )], targets=["task_release"], filter=""),
    dict(name="the freers skip values once only the scheduler counts handles", file=R, edits=[(
        "  return atomic_load_explicit(&handle_table.count, memory_order_relaxed) != 0 ||\n         atomic_load_explicit(&scheduler_handles_counted, memory_order_relaxed);",
        "  return atomic_load_explicit(&handle_table.count, memory_order_relaxed) != 0;",
    )], targets=["task_release"], filter=""),
    dict(name="a native handle no value names stays open", file=R, edits=[(
        "  if (handle_table_release(key)) native_handle_unreferenced(key);\n",
        "  (void)handle_table_release(key);\n",
    )], targets=["exec", "net"], filter=""),
]
