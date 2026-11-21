"""Mutants of R4 step 3 PR 5c-2: placement. A process incarnation runs on
its membership id modulo the carrier count.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/placement_mutations.py
Results: scripts/mutation/placement_results.txt.
"""

CRATE = "runtime/chimi-fiber"

P = "src/scheduler/process.rs"
S = "src/scheduler.rs"
SPREAD = "members_spread_over_the_carriers_and_a_restart_stays_put"


def m(name, file, old, new, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("an incarnation runs where it was started", P,
      "        let home = self.membership_home(membership);",
      "        let home = self.current;", SPREAD),
    m("every incarnation runs on carrier 0", P,
      "        let home = u32::try_from(membership.get() % carriers).expect(\"below the carrier count\");",
      "        let home = u32::try_from(membership.get() % 1 + carriers * 0).expect(\"below the carrier count\");", SPREAD),
    m("placement is off by one", P,
      "        let home = u32::try_from(membership.get() % carriers).expect(\"below the carrier count\");",
      "        let home = u32::try_from((membership.get() + 1) % carriers).expect(\"below the carrier count\");", SPREAD),
    m("a child is homed where it is started", S,
      "        let home = parent.map_or(self.current, |parent| self.tasks[&parent].carrier);",
      "        let home = self.current;", "a_child_is_homed_on_its_parents_carrier"),
]
