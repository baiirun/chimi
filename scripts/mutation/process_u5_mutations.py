"""Mutants of supervised-process stage U5 (ts-6ccbc9): the process kernel in
runtime.c, its trace family, the memory-report fields, pay-for-play linking, and
the lab's U5 runners. Each mutant breaks one decision a U5 scenario guards and
must turn that scenario red with the failure it names.

Run: scripts/mutation/mutate.py scripts/mutation/process_u5_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

Lab mutants run one scenario through the lab binary with `--through U5` and are
scored on that scenario's own failure text; tests/process_lab.rs runs the same
scenarios in the default suite.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
NATIVE = "src/backend/native.rs"
TYPING = "src/typing.rs"
FIXTURE = "src/process_lab/fixture.rs"
PROCESSES = "src/execution/processes.rs"

TRANSCRIPT = "expected.transcript line"
OUTCOME = "expected.outcome line"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U5"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


def language(name, file, test, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=test)


SEND_REQUEST = "      ChimiProcessRequest request = {\n        .kind = CHIMI_REQUEST_PROCESS_SEND,"
SEND_FREE = "      if (request.status != CHIMI_RESPONSE_OK) fragment_free(message);"
TRACE_MODE = ".trace_mode = runtime->silent ? 0 : (runtime->full_trace ? 2 : 1),"

MUTATIONS = [
    # Messages cross as copies made in the sender's heap.
    lab("the message is the sender's original value", RUNTIME, "CB01",
        [OUTCOME, "exited with"], [(
            SEND_REQUEST, "      message->roots[0] = arguments[1];\n" + SEND_REQUEST)]),
    lab("an accepted message is freed by its sender", RUNTIME, "MB01",
        [OUTCOME, TRANSCRIPT, "exited with"], [(SEND_FREE, "      fragment_free(message);")]),
    lab("a refused message is never freed", RUNTIME, "CB03",
        ["fragments_live is not 0"], [(
            SEND_FREE, "      if (request.status == CHIMI_RESPONSE_UNSET) fragment_free(message);")]),
    lab("a message copy ignores sharing", RUNTIME, "CB02",
        ["copied_values is not 21"], [(
            "  void *existing = copy_table_find(copy, pointer);\n  if (existing) return existing;\n",
            "")]),
    lab("Full is reported as Unavailable", RUNTIME, "MB02",
        [OUTCOME], [(
            "case CHIMI_RESPONSE_FULL: return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL, fields, 1));",
            "case CHIMI_RESPONSE_FULL: return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_UNAVAILABLE, fields, 1));")]),
    # CB07: a foreign pointer defects in the sender and never reaches a mailbox.
    lab("a send skips the foreign-pointer check", RUNTIME, "CB07",
        ["did not defect"], [(
            "      copy_counting = false;\n      if (status != HEAP_OK)",
            "      copy_counting = false;\n      if (status != HEAP_OK && false)")]),
    lab("the defect names only the sender", RUNTIME, "CB07",
        ["does not name both owners"], [(
            "fragment_copy_foreign_owner, current_heap->owner),",
            "current_heap->owner, current_heap->owner),")]),
    # Adopted messages join the receiver's region, so its heap stays flat.
    lab("received messages join the heap's base area", RUNTIME, "MF01",
        ["process_heap_peak_value_bytes differs", "heap_chunks_live differs"], [(
            "  heap_add_live(heap, area->bytes);\n  if (scope) {",
            "  heap_add_live(heap, area->bytes);\n  if (scope && false) {")]),
    lab("each small fragment costs a 64 KiB chunk", RUNTIME, "MF04",
        ["fragment_bytes_peak exceeds"], [(
            "  footprint_add(sizeof(*fragment));\n  fragment_footprint_add(sizeof(*fragment));",
            "  footprint_add(sizeof(*fragment));\n  fragment_footprint_add(sizeof(*fragment) + 65536);\n"
            "  heap_totals.fragment_footprint_bytes -= 65536;")]),
    # Trace family 3: identities, labels, and modes.
    lab("the incarnation is written from the task field", RUNTIME, "OB07",
        [TRANSCRIPT, "in its task field"], [(
            '    json_identity(output, "incarnation_id", event->incarnation);',
            '    json_identity(output, "incarnation_id", event->task);')]),
    lab("a name id is the bare symbol index", RUNTIME, "MB01",
        [TRANSCRIPT, "has no name", "has name"], [(
            "      return success(process_address_value(CHIMI_ADDRESS_NAMED, (uint64_t)symbol + 1));",
            "      return success(process_address_value(CHIMI_ADDRESS_NAMED, (uint64_t)symbol));")]),
    lab("boundaries mode runs the scheduler in full mode", RUNTIME, "OB02",
        ["expected.transcript (boundaries)"], [(
            TRACE_MODE, ".trace_mode = runtime->silent ? 0 : 2,")]),
    lab("trace off still traces the scheduler", RUNTIME, "OB01",
        ["wrote telemetry under --trace=off", "trace_events_constructed is not 0"], [(
            TRACE_MODE, ".trace_mode = runtime->silent ? 1 : (runtime->full_trace ? 2 : 1),")]),
    # MF06: only process programs link the process kernel.
    lab("every program links the process kernel", NATIVE, "MF06",
        ["defines `chimi_rt_process_entry`"], [(
            "    let process_kernel = requires_process_runtime(&program.core);",
            "    let process_kernel = true;")]),
    # The lab's U5 runners.
    lab("defect outcomes compared as whole lines", FIXTURE, "IP01",
        [OUTCOME], [(
            "        .map(|(expected, actual)| match defect_fields(expected) {",
            "        .map(|(expected, actual)| match None::<String> {")]),
    lab("the interpreter names no process runtime", PROCESSES, "IP01",
        [OUTCOME], [(
            '"{}.{} requires the supervised process runtime"',
            '"{}.{} is not supported here"')]),
    # Standard process enums are closed in a case (needed to read `Full(message)`).
    language("SendError has no variant domain", TYPING,
             "process_standard_enum_values_match_exhaustively", [(
                 "                    | StandardType::ProcessSendError\n", "")]),
]
