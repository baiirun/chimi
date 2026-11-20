"""Mutants of R4 step 3 PR 3b, scheduler side: the carrier drains the
releases no fiber's request carried before it idles.

Run: scripts/mutation/mutate.py scripts/mutation/drain_hook_fiber_mutations.py
Each mutant must turn the chimi-fiber unit tests red.
Results: scripts/mutation/drain_hook_fiber_results.txt.

A drain that returns after its first release is an equivalent mutant: drive
loops while a drain returns true, so it asks again at once.
"""

CRATE = "runtime/chimi-fiber"

TEST = "a_release_queued_in_a_release_hook_is_applied_before_the_carrier_idles"


def m(name, edits):
    return dict(name=name, file="src/scheduler.rs", edits=edits, targets=["lib"],
                filter=TEST)


MUTATIONS = [
    m("the loop never drains", [(
        "            if !self.drain_host_releases() {",
        "            if true {")]),
    m("a drained channel release is not applied", [(
        "                        Ok(transition) => {\n"
        "                            self.apply_channel_transition(transition);\n"
        "                            true",
        "                        Ok(transition) => {\n"
        "                            drop(transition);\n"
        "                            true")]),
]
