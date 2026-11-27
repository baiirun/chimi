"""Mutants of std/wait in Rust (CHI-305 step 5, phase 7, eighth PR): the route
that sends the wait intrinsics to Rust, the inbox take through runtime.c's
Process.receive, `Wait.at`'s deadline, and the clock a timer snapshots.
(select_mutations.py covers the commit, the user-clock timers and the
scheduler side.)

Run: scripts/mutation/mutate.py scripts/mutation/waits_mutations.py
Results: scripts/mutation/waits_results.txt.
"""

CRATE = "compiler/chimi"

WAITS = "../../runtime/chimi-runtime/src/intrinsics/waits.rs"
DISPATCH = "../../runtime/chimi-runtime/src/intrinsics/dispatch.rs"


def mutant(name, file, edits, targets=("wait",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter="")


MUTATIONS = [
    mutant("a watch event goes to C", DISPATCH, [(
        "Intrinsic::SELECT_RECEIVE..=Intrinsic::SELECT_FIRST | Intrinsic::SELECT_WATCH =>",
        "Intrinsic::SELECT_RECEIVE..=Intrinsic::SELECT_FIRST =>",
    )], ("wait", "channel_variants")),
    mutant("an inbox event takes a task result", WAITS, [(
        "                chimi_runtime_process_receive_v1(call.runtime, source, call.span, call.call_node, call.environment)",
        "                channel_operation(Intrinsic::TASK_RESULT, &[source], call.call_node)",
    )], ("wait", "language")),
    mutant("Wait.at measures from now", WAITS, [(
        "            return Ok((now, source));",
        "            return Ok((now, now.wrapping_add(source)));",
    )]),
    mutant("a timer ignores the bound clock", WAITS, [(
        "            clock = *(*call.environment).requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize);",
        "            let _ = CHIMI_CLOCK_REQUIREMENT_SLOT;",
    )]),
    mutant("a user clock's own now is ignored", WAITS, [(
        "            now = (*(*control).value).payload.integer;",
        "            let _ = control;",
    )]),
    mutant("an overflowing Wait.after wraps", WAITS, [(
        "        match now.checked_add(source) {",
        "        match Some(now.wrapping_add(source)) {",
    )]),
]
