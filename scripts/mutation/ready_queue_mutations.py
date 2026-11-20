"""Mutants of R4 step 3 PR 2: fibers pinned to their home carrier and the
per-carrier ready queues.

Run: scripts/mutation/mutate.py scripts/mutation/ready_queue_mutations.py
Each mutant must turn the chimi-fiber unit tests red.
Results: scripts/mutation/ready_queue_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


MUTATIONS = [
    m("a fiber's home is never claimed", "src/scheduler.rs", [(
        "        *self.0.get_or_insert(here) == here",
        "        let _ = here;\n        true")], "a_fiber_home_is_the_thread_of_its_first_resume"),
    m("a fiber may be destroyed anywhere", "src/scheduler.rs", [(
        "        self.0.is_none_or(|home| home == std::thread::current().id())",
        "        true")], "a_fiber_home_is_the_thread_of_its_first_resume"),
    m("a front push goes to the back", "src/scheduler/ready.rs", [(
        "        self.queue(carrier).push_front(task);",
        "        self.queue(carrier).push_back(task);")], "each_carrier_keeps_its_own_order"),
    m("remove takes only the front", "src/scheduler/ready.rs", [(
        "            queue.retain(|queued| *queued != task);",
        "            let _ = task;")], "each_carrier_keeps_its_own_order"),
    m("is_empty looks at carrier 0 only", "src/scheduler/ready.rs", [(
        "        self.queues.iter().all(VecDeque::is_empty)",
        "        self.queues[0].is_empty()")], "each_carrier_keeps_its_own_order"),
    # A woken task goes to its own carrier. With one carrier every task's
    # carrier is 0, so this survives until PR 5 adds a second carrier.
    m("a woken task goes to the current carrier", "src/scheduler.rs", [(
        "            let carrier = record.carrier;\n            if front {",
        "            let carrier = self.current;\n            if front {")], "select_tests"),
]
