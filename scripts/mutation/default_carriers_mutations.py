"""Mutants of the default carrier count: one carrier per core the process
may run on when `--carriers` is not given (parallel-scheduler.md, step 3
item 6, decided 2026-11-23).

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/default_carriers_mutations.py
Results: scripts/mutation/default_carriers_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"


def m(name, old, new):
    return dict(name=name, file=C, edits=[(old, new)], targets=["carriers"], filter="")


MUTATIONS = [
    m("a given count is replaced by the default",
      "  if (!carriers_given) runtime.carriers = default_carriers(runtime.virtual_clock);",
      "  runtime.carriers = default_carriers(runtime.virtual_clock);"),
    m("the virtual clock takes a carrier per core",
      "  if (!CHIMI_REQUIRES_SCHEDULER || virtual_clock) return 1;",
      "  if (!CHIMI_REQUIRES_SCHEDULER) return 1;"),
    m("the default stays one carrier",
      "  if (cores <= 1) return 1;",
      "  if (cores >= 1) return 1;"),
    m("the default counts one core too few",
      "  return cores > CARRIERS_MAX ? CARRIERS_MAX : (uint32_t)cores;",
      "  return cores > CARRIERS_MAX ? CARRIERS_MAX : (uint32_t)cores - 1;"),
]
