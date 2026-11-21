"""Mutants of R4 step 3 PR 5c-6: carriers on real threads behind the
test-only `run_root_on_threads`.

Run: scripts/mutation/mutate.py --timeout 180 scripts/mutation/carrier_spawn_mutations.py
Results: scripts/mutation/carrier_spawn_results.txt.
A carrier that never runs or never exits hangs its test, which the test's
watchdog aborts (killed by a crash or a timeout).
"""

CRATE = "runtime/chimi-fiber"

P = "src/scheduler/process_abi.rs"


def m(name, old, new):
    return dict(name=name, file=P, edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("carriers start before the last run's end is cleared",
      "        // Before any carrier starts, so none sees the last run's end.\n        held.begin_run(root);\n",
      ""),
    m("marks left from before the run stay set",
      "        drop(held.take_wakes());\n",
      ""),
    m("carrier 1 gets no thread",
      "        for index in 1..carriers {\n            let kernel = &kernel;",
      "        for index in 2..carriers {\n            let kernel = &kernel;"),
    m("the last carrier gets no thread",
      "        for index in 1..carriers {\n            let kernel = &kernel;",
      "        for index in 1..carriers.saturating_sub(1).max(1) {\n            let kernel = &kernel;"),
    m("every thread reaches the kernel as carrier 0",
      "                    super::carrier::run_carrier(&mut Locked::new(kernel, Carrier(index)));",
      "                    super::carrier::run_carrier(&mut Locked::new(kernel, Carrier(index - index)));"),
    m("two threads reach the kernel as carrier 1",
      "                    super::carrier::run_carrier(&mut Locked::new(kernel, Carrier(index)));",
      "                    super::carrier::run_carrier(&mut Locked::new(kernel, Carrier(1)));"),
    m("the kernel is not handed back",
      "    drop(std::mem::replace(\n        scheduler,\n        kernel.into_inner().unwrap_or_else(PoisonError::into_inner),\n    ));",
      "    drop(kernel);"),
]
