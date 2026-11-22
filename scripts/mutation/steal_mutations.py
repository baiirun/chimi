"""Mutants of R4 step 4 PR 4c: an idle carrier steals from a sleeping one.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/steal_mutations.py
Results: scripts/mutation/steal_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, old, new):
    return dict(name=name, file="src/scheduler.rs", edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("no carrier steals",
      "        let victim = (1..carriers)",
      "        let victim = (carriers..carriers)"),
    m("a carrier that is awake is stolen from",
      "                self.parked[carrier.0 as usize]\n                    && ",
      "                "),
    m("a task that never ran is stolen",
      ".is_none_or(|record| record.ran)",
      ".is_none_or(|_record| true)"),
    m("carrier 0 is stolen from",
      "                self.parked[carrier.0 as usize]\n",
      "                (self.parked[carrier.0 as usize] || carrier.0 == 0)\n"),
    m("a steal keeps the victim's wake",
      "            self.wakes.unmark(victim);\n",
      ""),
    m("a steal always drops the victim's wake",
      "        if self.ready.is_empty_for(victim) && self.jobs.is_empty_for(victim) && self.run_end.is_none() {",
      "        if true {"),
    m("a steal drops the wake while the victim still has a job",
      "self.ready.is_empty_for(victim) && self.jobs.is_empty_for(victim) && self.run_end.is_none()",
      "self.ready.is_empty_for(victim) && self.run_end.is_none()"),
    m("a stale entry is moved",
      "        if self.tasks.get(&task).is_some_and(|record| record.phase == TaskPhase::Runnable) {\n            self.move_home",
      "        if self.tasks.contains_key(&task) {\n            self.move_home"),
    m("a stolen task keeps its old home",
      "            self.move_home(task, self.current);\n        }\n        Some(task)",
      "        }\n        Some(task)"),
]
