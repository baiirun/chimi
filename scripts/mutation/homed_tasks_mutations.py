"""Mutants of R4 step 4e-1: a task's home moves out of its record, beside
the record's cell in `Homed`.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/homed_tasks_mutations.py
Results: scripts/mutation/homed_tasks_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, old, new, file="src/scheduler.rs"):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("a first resume keeps the placed home",
      "        if !record.ran {\n            *home = self.current;\n        }",
      "        if !record.ran {\n        }"),
    m("a move leaves the home",
      "        self.tasks.set_home(&task, to);",
      ""),
    m("set_home changes nothing",
      "        self.slots.get_mut(id).expect(\"a filed record\").home = home;",
      "        let _ = (self.slots.get_mut(id).expect(\"a filed record\"), home);",
      file="src/scheduler/homed.rs"),
    m("enqueue ignores the home",
      "        let carrier = self.tasks.home(&task).expect(\"a live task\");\n        if front",
      "        let carrier = ready::Carrier(0);\n        if front"),
    m("a child is homed on the current carrier",
      "parent.map_or(self.current, |parent| self.tasks.home(&parent).expect(\"a live parent\"))",
      "parent.map_or(self.current, |_| self.current)"),
    m("an unwind job goes to the current carrier",
      "            let carrier = self.tasks.home(&task).expect(\"a live task\");\n            self.shards.push_job",
      "            let carrier = self.current;\n            self.shards.push_job"),
    m("any carrier may resume a started fiber",
      "            || self.tasks.home(&task) == Some(self.current)\n            || (!record.ran",
      "            || true\n            || (!record.ran"),
    m("retire_carrier checks other carriers' tasks",
      ".filter(|(_, home, record)| *home == current && record.fiber.is_some())",
      ".filter(|(_, home, record)| *home != current && record.fiber.is_some())"),
]
