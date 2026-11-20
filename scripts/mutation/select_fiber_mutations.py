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
        "        self.clear_select(owner);\n        self.clear_io_wait(owner);",
        "        self.clear_io_wait(owner);")]),
    # A committed receive must take its value in the select step, or a rival
    # receiver of the same queue served first takes it (step 3, PR 1).
    dict(name="a committed receive leaves its value in the queue",
         file="src/scheduler/select.rs", targets=["lib"], filter="a_committed_select",
         edits=[(
             "                    if source.kind == SELECT_RECEIVE {\n"
             "                        self.commit_receive(owner, source);\n"
             "                    }\n",
             "")]),
]
