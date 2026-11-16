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
             "--", "release", "channel", "handle", "forced"]


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
      [("  pthread_mutex_lock(&handle_table.lock);\n  bool last = handle_table_drop(channel);\n  pthread_mutex_unlock(&handle_table.lock);\n  return last;",
        "  return handle_table_drop(channel);")],
      ["runtime_state"], "the_handle_table_counts_across_threads"),
    m("a drop reports the last reference one early",
      [("  if (--slots[slot].endpoints != 0) return false;",
        "  if (--slots[slot].endpoints > 1) return false;")],
      ["runtime_state", "task_release"]),
    m("a key at zero is never queued",
      [("  if (handle_table_release(channel)) release_queue_push(channel);\n", "  (void)handle_table_release(channel);\n")],
      ["task_release", "cli"], "release"),
    simulated("a carrier's queue is drained only by a fiber that requests there",
      [("  if (carrier->released_handle_count) return carrier;\n  for (size_t index = 0; index < CARRIER_COUNT; ++index)\n    if (carrier_states[index].released_handle_count) return &carrier_states[index];\n  return NULL;",
        "  return carrier->released_handle_count ? carrier : NULL;")]),
    simulated("a park keeps the caller's fiber context on the old carrier",
      [("  FiberContext fiber = carrier->fiber;\n  ChimiFiberCommand command = flush_channel_releases();\n  if (command != CHIMI_FIBER_CANCEL) command = carrier_request(request);\n  carrier->fiber = fiber;",
        "  Carrier *start = carrier;\n  FiberContext fiber = carrier->fiber;\n  ChimiFiberCommand command = flush_channel_releases();\n  if (command != CHIMI_FIBER_CANCEL) command = carrier_request(request);\n  start->fiber = fiber;")]),
]
