"""Scheduler mutants of G29 (CHI-231) whose proof is a chimi-fiber test rather
than a compiled program.

Run: scripts/mutation/mutate.py scripts/mutation/g29_fiber_mutations.py
Results: scripts/mutation/g29_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def variant(name, edits):
    return dict(name=name, file="src/channel.rs", edits=edits, targets=["lib"],
                filter="variant_tests")


MUTATIONS = [
    variant("parked receivers are served newest first", [(
        "    if let Some(receiver) = channel.waiting_receivers.pop_front() {",
        "    if let Some(receiver) = channel.waiting_receivers.pop_back() {")]),
    variant("a new publish overtakes a parked one", [(
        "                    if hub.waiting_senders.is_empty() && self.subscriptions_have_room(id) {",
        "                    if self.subscriptions_have_room(id) {")]),
    variant("a subscription that ends does not admit a parked publish", [(
        "            state.subscriptions.retain(|subscription| *subscription != queue);\n        }\n        self.admit_publishers(hub, transition);",
        "            state.subscriptions.retain(|subscription| *subscription != queue);\n        }")]),
    variant("every subscription shares one value", [(
        "            Some(copy) => copy(value),",
        "            Some(_) => value,")]),
    variant("a select on a shared queue wakes only one watcher", [(
        "        .extend(std::mem::take(&mut channel.select_watchers));",
        "        .extend(channel.select_watchers.pop());")]),
]
