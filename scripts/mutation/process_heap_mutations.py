"""Mutants of supervised-process stages U3 and U4 (ts-6ccbc9): each breaks a heap
mechanism a lab scenario or its tests/cli.rs stand-in guards, and must turn it red.

Run: scripts/mutation/mutate.py scripts/mutation/process_heap_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

The lab mutants run the scenario through `--through U4` with the lab binary,
because CURRENT_STAGE stays below U3 until U2 lands; tests/process_lab.rs covers
the same scenarios once the stage is bumped.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
NATIVE = "src/backend/native.rs"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U4"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


def cli(name, file, test, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=test)


FOREIGN_MARK = (
    "    if (area->heap != collecting_scope->heap && area->role != AREA_STATIC) collecting_foreign = true;\n"
    "    return;\n"
)
FOREIGN_COPY = (
    "  if (!copy_owns(copy, header->area)) {\n"
    "    copy->status = HEAP_FOREIGN_POINTER;\n"
    "    return NULL;\n"
    "  }"
)

LOOP_PASS = "      if (allocation_scope_collect(&passes, state, NULL) != HEAP_OK) {"


def batched(every):
    return ("      static uint64_t batched_passes;\n"
            f"      if (++batched_passes % {every} == 0 && "
            "allocation_scope_collect(&passes, state, NULL) != HEAP_OK) {")


MUTATIONS = [
    # CB06: a pointer into another heap is reported and not followed.
    lab("a region pass ignores foreign pointers", RUNTIME, "CB06",
        ["a region pass accepted a pointer into another heap"], [(
        FOREIGN_MARK, "    return;\n")]),
    lab("a region pass reports and then follows foreign pointers", RUNTIME, "CB06",
        ["the pass followed a foreign pointer"], [(
        FOREIGN_MARK,
        "    if (area->heap != collecting_scope->heap && area->role != AREA_STATIC) collecting_foreign = true;\n"
        "    if (area->heap == collecting_scope->heap || area->role == AREA_STATIC) return;\n")]),
    lab("a fragment copy ignores foreign pointers", RUNTIME, "CB06",
        ["a fragment copy accepted a pointer into another heap", "the copy followed a foreign pointer"], [(
        FOREIGN_COPY, "  (void)copy_owns;")]),
    lab("a fragment copy reports and then follows foreign pointers", RUNTIME, "CB06",
        ["the copy followed a foreign pointer"], [(
        FOREIGN_COPY, "  if (!copy_owns(copy, header->area)) copy->status = HEAP_FOREIGN_POINTER;")]),
    # MF05: loops reclaim every pass when the scheduler is linked (ts-da83af).
    lab("loop passes skipped under CHIMI_REQUIRES_SCHEDULER", RUNTIME, "MF05",
        ["scoped_peak_live_bytes differs"], [(LOOP_PASS,
        "      if (!CHIMI_REQUIRES_SCHEDULER && allocation_scope_collect(&passes, state, NULL) != HEAP_OK) {")]),
    # Equality across sizes accepts batched reclamation; the 7 KiB bound does not.
    lab("loops reclaim every 1000th pass", RUNTIME, "MF05",
        ["scoped_peak_live_bytes differs", "scoped_peak_live_bytes exceeds 7168"],
        [(LOOP_PASS, batched(1000))]),
    lab("loops reclaim every second pass", RUNTIME, "MF05",
        ["scoped_peak_live_bytes exceeds 7168"], [(LOOP_PASS, batched(2))]),
    # CB04's task analogue: a channel value is a copy, not a pointer into the sender's heap.
    cli("Channel.send queues the sender's original value", RUNTIME,
        "native_a_value_a_finished_task_put_on_a_channel_survives_its_heap", [(
            "      for (;;) {\n        ChimiSchedulerRequest request = {\n          .kind = CHIMI_REQUEST_CHANNEL_SEND,",
            "      message->roots[0] = arguments[1];\n      for (;;) {\n        ChimiSchedulerRequest request = {\n          .kind = CHIMI_REQUEST_CHANNEL_SEND,")]),
    # MF06's no-concurrency and tasks-only half: pay-for-play linking.
    cli("every program links the fiber runtime", NATIVE,
        "native_only_a_task_program_links_the_fiber_runtime", [(
            "fn requires_fiber_runtime(core: &CoreProgram) -> bool {\n",
            "fn requires_fiber_runtime(core: &CoreProgram) -> bool {\n    if !core.symbols.is_empty() {\n        return true;\n    }\n")]),
]
