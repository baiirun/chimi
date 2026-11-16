"""Scheduler mutants of `Select.first` (CHI-203) whose proof is a chimi-fiber
test rather than a compiled program.

Run: scripts/mutation/mutate.py scripts/mutation/select_fiber_mutations.py
Results: scripts/mutation/select_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def select(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="select_tests")


MUTATIONS = [
    # A select woken by a source and then refused by a fired scope must not
    # leave its watchers behind.
    select("a request leaves an abandoned select registered", "src/scheduler.rs", [(
        "        self.clear_select(owner);\n        if process && self.park_lingers(owner, kind) {",
        "        if process && self.park_lingers(owner, kind) {")]),
]
