"""Mutants of the idle drain, seen from a compiled program: releases no
fiber's request carried are applied before the carrier idles.

Until CHI-308 step 1 the runtime handed these over through a take_release
hook; the scheduler now keeps the counts and the queue itself
(runtime/chimi-fiber/src/handles.rs), so the mutants sit there and the
program test in cli.rs must still turn red.

Run: scripts/mutation/mutate.py scripts/mutation/drain_hook_mutations.py
Results: scripts/mutation/drain_hook_results.txt.
"""

CRATE = "compiler/chimi"

S = "../../runtime/chimi-fiber/src/scheduler.rs"
H = "../../runtime/chimi-fiber/src/handles.rs"


def m(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli"],
                filter="native_releases_no_request_carries_are_done_before_the_carrier_idles")


MUTATIONS = [
    m("the idle carrier takes nothing", S, [(
        "        let mut drained = false;\n        while let Some(released) = crate::handles::take() {",
        "        let mut drained = false;\n        while let Some(released) = None::<Released> {")]),
    m("a dropped key is never queued", H, [(
        "    if with_counts(|counts| counts.drop_one(key)) {\n        queue(key);\n    }",
        "    let _ = with_counts(|counts| counts.drop_one(key));")]),
    dict(name="a task key is released as a channel", file=H, edits=[(
        "        if key & TASK_HANDLE_KEY != 0 {",
        "        if false {")], targets=["task_release"],
        filter="a_loop_of_finished_tasks_holds_a_bounded_number_of_records"),
]
