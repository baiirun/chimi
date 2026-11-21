"""Mutants of R4 step 3 PR 5b-1: ready work is routed by carrier.

Run: scripts/mutation/mutate.py scripts/mutation/carrier_routing_mutations.py
Each mutant must turn its filtered test red (a_woken_task_waits_on_its_home_carrier,
a_child_is_homed_on_its_parents_carrier or the_kernel_lock_enters_its_carrier).
Every ready push reads the home from the task's record in `enqueue`, so its
callers have no carrier to get wrong. The first is
the ready-queue mutant PR 2 left alive, which no one-carrier test can kill.
Results: scripts/mutation/carrier_routing_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
TEST = "a_woken_task_waits_on_its_home_carrier"


def m(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=TEST)


MUTATIONS = [
    m("ready work goes to the current carrier", S, [(
        "        let carrier = self.tasks[&task].carrier;\n        if front {",
        "        let carrier = self.current;\n        if front {")]),
    m("a carrier pops carrier 0's queue", S, [(
        "while let Some(task) = self.ready.pop_front(self.current) {",
        "while let Some(task) = self.ready.pop_front(ready::Carrier(0)) {")]),
    m("a root task is homed on carrier 0", S, [(
        "        let carrier = parent.map_or(self.current, |parent| self.tasks[&parent].carrier);",
        "        let carrier = parent.map_or(ready::Carrier(0), |parent| self.tasks[&parent].carrier);")]),
    dict(name="a child is homed on carrier 0", file=S, edits=[(
        "        let carrier = parent.map_or(self.current, |parent| self.tasks[&parent].carrier);",
        "        let carrier = parent.map_or(self.current, |_parent| ready::Carrier(0));")],
        targets=["lib"], filter="a_child_is_homed_on_its_parents_carrier"),
    dict(name="a child is homed on the current carrier", file=S, edits=[(
        "        let carrier = parent.map_or(self.current, |parent| self.tasks[&parent].carrier);",
        "        let carrier = self.current;")],
        targets=["lib"], filter="a_child_is_homed_on_its_parents_carrier"),
    dict(name="the lock does not enter its carrier", file="src/scheduler/carrier.rs", edits=[(
        "        kernel.enter_carrier(self.carrier);\n",
        "")], targets=["lib"], filter="the_kernel_lock_enters_its_carrier"),
]
