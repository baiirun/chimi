"""Mutants of R4 step 3 PR 3b, runtime side: the carrier's take_release hook.

Run: scripts/mutation/mutate.py scripts/mutation/drain_hook_mutations.py
Each mutant must turn the idle-drain test in cli.rs red.
Results: scripts/mutation/drain_hook_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["cli"],
                filter="native_releases_no_request_carries_are_done_before_the_carrier_idles")


MUTATIONS = [
    m("the runtime passes no hook", [(
        "    .take_release = take_carrier_release,\n", "")]),
    m("the hook hands over nothing", [(
        "  return carrier_release_take(kind, id);",
        "  (void)kind; (void)id;\n  return false;")]),
    m("a task key is handed over as a channel", [(
        "  *kind = key & TASK_HANDLE_KEY ? CHIMI_REQUEST_TASK_RELEASE : CHIMI_REQUEST_CHANNEL_RELEASE;",
        "  *kind = CHIMI_REQUEST_CHANNEL_RELEASE;")]),
]
