"""Mutants of R4 step 3 PR 5b-1: ready work is routed by carrier.

Run: scripts/mutation/mutate.py scripts/mutation/carrier_routing_mutations.py
Each mutant must turn a_woken_task_waits_on_its_home_carrier red. The first is
the ready-queue mutant PR 2 left alive, which no one-carrier test can kill.
Results: scripts/mutation/carrier_routing_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
TEST = "a_woken_task_waits_on_its_home_carrier"


def m(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=TEST)


MUTATIONS = [
    m("a woken task goes to the current carrier", S, [(
        "            let carrier = record.carrier;\n            self.enqueue(carrier, task, front);",
        "            let carrier = self.current;\n            self.enqueue(carrier, task, front);")]),
    m("a carrier pops carrier 0's queue", S, [(
        "while let Some(task) = self.ready.pop_front(self.current) {",
        "while let Some(task) = self.ready.pop_front(ready::Carrier(0)) {")]),
    m("a root task is homed on carrier 0", S, [(
        "        let carrier = parent.map_or(self.current, |parent| self.tasks[&parent].carrier);",
        "        let carrier = parent.map_or(ready::Carrier(0), |parent| self.tasks[&parent].carrier);")]),
    m("the lock does not enter its carrier", "src/scheduler/carrier.rs", [(
        "        kernel.enter_carrier(self.carrier);\n",
        "")]),
]
