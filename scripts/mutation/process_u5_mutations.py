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

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"
# The scheduler trace writers are Rust since CHI-305 step 5, phase 7.
TELEMETRY = "../../runtime/chimi-runtime/src/telemetry.rs"

RUNTIME = "runtime/native/runtime.c"
NATIVE = "src/backend/native.rs"
TYPING = "src/typing.rs"
# The fragment copy and adoption are Rust since CHI-305 step 5 phase 3.
FRAGMENT = "../../runtime/chimi-runtime/src/heap/fragment.rs"
FIXTURE = "src/process_lab/fixture.rs"
PROCESSES = "src/execution/processes.rs"
# Process.send and monitor hand their copies over in Rust since CHI-305
# step 5, phase 7; the foreign-pointer message is shared with test mailboxes.
PROCESS_KERNEL = "../../runtime/chimi-runtime/src/intrinsics/process_kernel.rs"
TEST_LOG = "../../runtime/chimi-runtime/src/intrinsics/test_log.rs"

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


def cli(name, file, test, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=test)


KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"
STACK_TEST = "native_process_stack_depth_and_named_defects"


SEND_COPY = "        let mut carried = Carried::of((*message).roots[0]);"
SEND_FREE = "        if request.status != Scheduler::RESPONSE_OK {\n            free_fragment(message);\n        }"
TRACE_MODE = ".trace_mode = runtime->silent ? 0 : (runtime->full_trace ? 2 : 1),"

MUTATIONS = [
    # Messages cross as copies made in the sender's heap.
    lab("the message is the sender's original value", PROCESS_KERNEL, "CB01",
        [OUTCOME, "exited with"], [(
            SEND_COPY, "        (*message).roots[0] = value.cast();\n" + SEND_COPY)]),
    lab("an accepted message is freed by its sender", PROCESS_KERNEL, "MB01",
        [OUTCOME, TRANSCRIPT, "exited with"], [(SEND_FREE, "        free_fragment(message);")]),
    lab("a refused message is never freed", PROCESS_KERNEL, "CB03",
        ["fragments_live is not 0"], [(
            SEND_FREE, SEND_FREE.replace("!= Scheduler::RESPONSE_OK", "== Scheduler::RESPONSE_UNSET"))]),
    lab("a message copy ignores sharing", FRAGMENT, "CB02",
        ["copied_values is not 21", "exceeded the 60 s run limit"], [(
            "        let existing = copy.table.find(pointer);\n        if !existing.is_null() {\n            return existing;\n        }\n",
            "")]),
    lab("Full is reported as Unavailable", PROCESS_KERNEL, "MB02",
        [OUTCOME], [(
            "failure(standard_value(StandardConstructor::PROCESS_SEND_FULL, &[message]))",
            "failure(standard_value(StandardConstructor::PROCESS_SEND_UNAVAILABLE, &[message]))")]),
    # Messages the scheduler discards unread come back through release_message.
    lab("discarded messages are never freed", RUNTIME, "MB06",
        ["fragments_live is not 0"], [(
            "  (void)context;\n  fragment_free((RuntimeFragment *)(uintptr_t)message);",
            "  (void)context;\n  (void)message;")]),
    # Fiber stacks and named process defects.
    cli("scheduler fibers keep a 64 KiB stack", RUNTIME, STACK_TEST, [(
        "#define CHIMI_FIBER_STACK_BYTES (1024u * 1024u)",
        "#define CHIMI_FIBER_STACK_BYTES (64u * 1024u)")]),
    cli("a stack overflow is not named", TELEMETRY, STACK_TEST, [(
        '                named = c"Process stack overflow".as_ptr();\n',
        "")]),
    cli("the scheduler does not flag an overflowed body", KERNEL, STACK_TEST, [(
        "        let stack_overflow = matches!(outcome, TaskOutcome::StackOverflow { .. });",
        "        let stack_overflow = false;")]),
    cli("a body's defect message is dropped", RUNTIME, STACK_TEST, [(
        "      chimi_fiber_report_defect(control->message);\n", "")]),
    # CB07: a foreign pointer defects in the sender and never reaches a mailbox.
    lab("a send skips the foreign-pointer check", PROCESS_KERNEL, "CB07",
        ["did not defect"], [(
            "            return Err(defect_with(foreign_message(foreign.operation, foreign.subject, foreign.side), node));",
            "            return Err(success(void_value()));")]),
    lab("the defect names only the sender", TEST_LOG, "CB07",
        ["does not name both owners"], [(
            "            .integer((*here).fragment_copy_foreign_owner as i64)",
            "            .integer((*(*here).fiber.heap).owner as i64)")]),
    # Adopted messages join the receiver's region, so its heap stays flat.
    lab("received messages join the heap's base area", FRAGMENT, "MF01",
        ["process_heap_peak_value_bytes differs", "heap_chunks_live differs"], [(
            "        if scope.is_null() {\n            (*area).role = AreaRole::BASE;",
            "        if true {\n            (*area).role = AreaRole::BASE;")]),
    lab("each small fragment costs a 64 KiB chunk", FRAGMENT, "MF04",
        ["fragment_bytes_peak exceeds"], [(
            "        footprint_add(here, bytes);\n        fragment_footprint_add(here, bytes);",
            "        footprint_add(here, bytes);\n        fragment_footprint_add(here, bytes + 65536);"),
            ("        size_of::<RuntimeFragment>()\n            + (*fragment).area.bytes",
             "        65536 + size_of::<RuntimeFragment>()\n            + (*fragment).area.bytes")]),
    # Trace family 3: identities, labels, and modes.
    lab("the incarnation is written from the task field", TELEMETRY, "OB07",
        [TRANSCRIPT, "in its task field", "has a null incarnation_id"], [(
            '        out.identity(b"incarnation_id", event.incarnation);',
            '        out.identity(b"incarnation_id", event.task);')]),
    lab("a name id is the bare symbol index", EVAL, "MB01",
        [TRANSCRIPT, "has no name", "has name"], [(
            "                (*value).payload.address.target = u64::from(symbol_index) + 1;",
            "                (*value).payload.address.target = u64::from(symbol_index);")]),
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
