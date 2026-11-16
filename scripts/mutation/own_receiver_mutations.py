"""Mutants of refusing a send that carries its channel's own receivers (CHI-214).

Run: scripts/mutation/mutate.py scripts/mutation/own_receiver_mutations.py
Each mutant must turn the chimi-fiber parcel tests red.
Results: scripts/mutation/own_receiver_results.txt.
"""

CRATE = "runtime/chimi-fiber"

CHANNEL = "src/channel.rs"


def own(name, edits):
    return dict(name=name, file=CHANNEL, edits=edits, targets=["lib"], filter="parcel_tests")


MUTATIONS = [
    own("a send carrying its own receiver goes through", [(
        "        if self.carries_every_receiver(sender.channel, &carried) {",
        "        if false && self.carries_every_receiver(sender.channel, &carried) {")]),
    own("the other receivers of the queue are ignored", [(
        "        live.peek().is_some() && live.all(carries)",
        "        carries(queue)")]),
    own("a tap that ended still counts", [(
        "            .filter(|tap| self.channels.get(tap).is_some_and(|tap| !tap.receiver_ended))\n            .chain(",
        "            .chain(")]),
]
