"""Mutant of R4 step 3 PR 5a-2, scheduler side: a withdraw wakes child waits.

Run: scripts/mutation/mutate.py scripts/mutation/child_withdraw_mutations.py
Results: scripts/mutation/carrier_tables_results.txt (with the runtime set).
"""

CRATE = "runtime/chimi-fiber"

MUTATIONS = [
    dict(name="a withdraw wakes no child wait", file="src/scheduler/reactor.rs", edits=[(
        "            let children = self.take_child_waits_on(request.channel);",
        "            let children: Vec<TaskId> = Vec::new();")],
        targets=["lib"], filter="a_withdraw_wakes_a_child_wait_on_its_resource"),
    dict(name="a child wait keeps no resource", file="src/scheduler/child.rs", edits=[(
        "                    resource: request.channel,",
        "                    resource: 0,")],
        targets=["lib"], filter="a_withdraw_wakes_a_child_wait_on_its_resource"),
]
