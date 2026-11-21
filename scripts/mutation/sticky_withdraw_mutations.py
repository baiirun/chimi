"""Mutants of R4 step 3 PR 5b-2: a withdraw is sticky.

Run: scripts/mutation/mutate.py scripts/mutation/sticky_withdraw_mutations.py
Each mutant must turn its filtered test red.
Results: scripts/mutation/sticky_withdraw_results.txt.
"""

CRATE = "runtime/chimi-fiber"

R = "src/scheduler/reactor.rs"
C = "src/scheduler/child.rs"
IO = "a_wait_on_a_withdrawn_key_answers_at_once"
TABLE = "the_withdrawn_table_keeps_the_newest_generation_of_each_slot"
CHILD = "a_child_wait_on_a_withdrawn_key_answers_at_once"


def m(name, file, old, new, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("a withdraw is not recorded", R,
      "            self.reactor.record_withdraw(request.channel);\n", "", IO),
    m("only the exact generation is withdrawn", R,
      "self.withdrawn.get(&slot).is_some_and(|newest| generation <= *newest)",
      "self.withdrawn.get(&slot).is_some_and(|newest| generation == *newest)", IO),
    m("a newer generation counts as withdrawn", R,
      "self.withdrawn.get(&slot).is_some_and(|newest| generation <= *newest)",
      "self.withdrawn.get(&slot).is_some_and(|newest| generation <= newest.saturating_add(1))", IO),
    m("the slot is ignored", R,
      "    ((resource >> 32) as u32, resource as u32)",
      "    (0, resource as u32)", IO),
    m("an older withdraw replaces a newer one", R,
      "        *newest = (*newest).max(generation);",
      "        *newest = generation;", TABLE),
    m("an IO wait ignores the withdrawn table", R,
      "        if request.channel != 0 && self.reactor.is_withdrawn(request.channel) {",
      "        if false {", IO),
    dict(name="the withdrawn table is read only once the descriptor waits", file=R, edits=[(
        "        if request.channel != 0 && self.reactor.is_withdrawn(request.channel) {\n"
        "            request.status = RESPONSE_PENDING;\n"
        "            let _ = self.wake_front(owner);\n"
        "            return;\n"
        "        }\n", ""), (
        "            Check::Waiting => {\n",
        "            Check::Waiting if request.channel != 0 && self.reactor.is_withdrawn(request.channel) => {\n"
        "                request.status = RESPONSE_PENDING;\n"
        "                let _ = self.wake_front(owner);\n"
        "            }\n"
        "            Check::Waiting => {\n")], targets=["lib"], filter=IO),
    m("a child wait ignores the withdrawn table", C,
      "        if pid > 0 && request.channel != 0 && self.reactor.is_withdrawn(request.channel) {",
      "        if false {", CHILD),
]
