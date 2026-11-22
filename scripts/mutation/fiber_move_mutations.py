"""Mutants of R4 step 4 PR 4b: a parked fiber may move to another carrier.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/fiber_move_mutations.py
Results: scripts/mutation/fiber_move_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, file, old, new, filter):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=filter)


MUTATIONS = [
    m("a resumed fiber does not restore its park context", "src/lib.rs",
      "    set_park_context(yielder, capture_backtraces);\n    command\n}",
      "    command\n}",
      "moves_between_threads"),
    m("a moved fiber keeps its old thread", "src/scheduler.rs",
      "        if let Some(fiber) = record.fiber.as_mut() {\n            fiber.release_home();\n        }",
      "",
      "moved_to_another_carrier"),
    m("a moved task keeps its old carrier", "src/scheduler.rs",
      "        record.carrier = to;\n",
      "",
      "moved_to_another_carrier"),
    m("releasing a home keeps it", "src/scheduler.rs",
      "    fn release(&mut self) {\n        self.0 = None;\n    }",
      "    fn release(&mut self) {}",
      "moved_to_another_carrier"),
]
