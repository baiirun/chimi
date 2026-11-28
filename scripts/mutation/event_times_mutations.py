"""Mutants of event times (Phase 7c step 7, PR 1): the runner's and the
interpreter's `time_ns` and `start_unix_ns`, and the fiber clock read they
use.

Run: scripts/mutation/mutate.py scripts/mutation/event_times_mutations.py
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
EXECUTION = "src/execution.rs"
RUN = "../../runtime/chimi-runtime/src/run.rs"
CLOCK = "../../runtime/chimi-fiber/src/scheduler/clock.rs"
SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"


def mutant(name, file, edits, filter="semantic_events", targets=("clock",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a fiber reads virtual time as the origin", CLOCK, [(
        "            Self::Virtual { now } => Reading::Virtual { now: *now },",
        "            Self::Virtual { now } => Reading::Virtual { now: { let _ = now; Instant::ORIGIN } },",
    )]),
    mutant("a resume publishes no clock", SCHEDULER, [(
        "        clock::set_resume_clock(clock);\n",
        "        let _ = clock;\n",
    )]),
    mutant("a dependency call is at the origin", RUNTIME, [(
        "  fprintf(output, \",\\\"method_semantic_id\\\":%u,\\\"time_ns\\\":%\" PRId64 \"}}\\n\", callable->method,\n          chimi_fiber_now());",
        "  fprintf(output, \",\\\"method_semantic_id\\\":%u,\\\"time_ns\\\":%\" PRId64 \"}}\\n\", callable->method,\n          INT64_C(0));",
    )]),
    mutant("an operation is at the origin", RUNTIME, [(
        "started ? INT64_C(0) : chimi_fiber_now());",
        "INT64_C(0));",
    )]),
    mutant("the start ignores --clock-start", RUN, [(
        "        return unsafe { (*runtime).clock_start };",
        "        return 0;",
    )]),
    mutant("the real start is the origin", RUN, [(
        "    clock_nanos(libc::CLOCK_REALTIME)\n",
        "    0\n",
    )]),
    mutant("the interpreter writes no time", EXECUTION, [(
        "            output.push_str(\"\\\"time_ns\\\":0\");",
        "            output.push_str(\"\\\"t\\\":0\");",
    )], filter="interpreter_writes"),
    mutant("the interpreter writes no start", EXECUTION, [(
        "            if event.kind == EventKind::ComputationStarted {\n                output.push_str(",
        "            if false {\n                output.push_str(",
    )], filter="interpreter_writes"),
]
