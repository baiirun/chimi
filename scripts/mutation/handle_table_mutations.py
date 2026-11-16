"""Mutants of the shared handle table and per-carrier release queues (R4 step 2).

Run: scripts/mutation/mutate.py scripts/mutation/handle_table_mutations.py
Each mutant must turn the named tests red. The carrier-simulation mutants run
task tests with CHIMI_TEST_RUNTIME_DEFINES=-DCHIMI_CARRIER_SIMULATION, so every
resume continues on the other carrier.
Results: scripts/mutation/handle_table_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"

SIMULATED = ["env", "CHIMI_TEST_RUNTIME_DEFINES=-DCHIMI_CARRIER_SIMULATION",
             "cargo", "test", "-q", "--test", "task_release", "--test", "cli",
             "--", "release", "channel", "handle",
             # Split allocator caches move this peak (handle_table_results.txt).
             "--skip", "native_dropping_a_channel_frees_its_queued_messages"]


def m(name, edits, targets, filter=""):
    return dict(name=name, file=R, edits=edits, targets=targets, filter=filter)


def simulated(name, edits):
    return dict(name=name, file=R, edits=edits, command=SIMULATED,
                expect=["test result: FAILED", "panicked"])


MUTATIONS = [
    m("retain does not take the table's lock",
      [("static void channel_endpoint_retain(uint64_t channel) {\n  pthread_mutex_lock(&handle_table.lock);",
        "static void channel_endpoint_retain(uint64_t channel) {\n"),
       ("  ++slots[slot].endpoints;\n  pthread_mutex_unlock(&handle_table.lock);",
        "  ++slots[slot].endpoints;\n")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("drop does not take the table's lock",
      [("  pthread_mutex_lock(&handle_table.lock);\n  bool last = handle_table_drop(channel);\n  pthread_mutex_unlock(&handle_table.lock);",
        "  bool last = handle_table_drop(channel);")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("a drop reports the last reference one early",
      [("  if (--slots[slot].endpoints != 0) return false;",
        "  if (--slots[slot].endpoints > 1) return false;")],
      ["runtime_state", "task_release"]),
    m("a key at zero is never queued",
      [("  if (last) release_queue_push(channel);\n", "  (void)last;\n")],
      ["task_release", "cli"], "release"),
    simulated("the flush drains the queue of the carrier it started on",
      [("  while (carrier->released_handle_count) {\n    uint64_t key = carrier->released_handles[--carrier->released_handle_count];",
        "  Carrier *start = carrier;\n  while (start->released_handle_count) {\n    uint64_t key = start->released_handles[--start->released_handle_count];")]),
    simulated("a park keeps the caller's fiber context on the old carrier",
      [("  FiberContext fiber = carrier->fiber;\n  ChimiFiberCommand command = flush_channel_releases();\n  if (command != CHIMI_FIBER_CANCEL) command = carrier_request(request);\n  carrier->fiber = fiber;",
        "  Carrier *start = carrier;\n  FiberContext fiber = carrier->fiber;\n  ChimiFiberCommand command = flush_channel_releases();\n  if (command != CHIMI_FIBER_CANCEL) command = carrier_request(request);\n  start->fiber = fiber;")]),
]
