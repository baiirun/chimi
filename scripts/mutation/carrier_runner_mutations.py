"""Mutants of R4 step 3 PR 5d-2: the carrier-thread runner
`process_abi::run_on_carriers` as production code, with its start gate.

Run: scripts/mutation/mutate.py --timeout 180 scripts/mutation/carrier_runner_mutations.py
Results: scripts/mutation/carrier_runner_results.txt.
A carrier that never runs or never leaves hangs its test, which the test's
watchdog aborts (killed by a crash or a timeout).
"""

CRATE = "runtime/chimi-fiber"

P = "src/scheduler/process_abi.rs"


def m(name, old, new, file=P):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("a failed spawn lets the spawned carriers run",
      "                gate.open(false);",
      "                gate.open(true);"),
    m("a spawned carrier runs without waiting at the gate",
      "                if gate.wait() {",
      "                if true {"),
    m("a spawned carrier never runs",
      "        gate.open(true);\n        abort_on_panic(",
      "        gate.open(false);\n        abort_on_panic("),
    m("a failed spawn is ignored",
      "            if spawn_carrier(scope, index, body).is_err() {",
      "            if spawn_carrier(scope, index, body).is_err() && false {"),
    m("a failed spawn still runs the root",
      "                return RootRun {\n                    outcome: defect_outcome(SCHEDULER_DEFECT_CODE),\n                    deadlock: Vec::new(),\n                };",
      "                gate.open(true);"),
    m("the gate opens before every carrier is spawned",
      "        for index in 1..carriers {\n            let (kernel, gate) = (&kernel, &gate);",
      "        gate.open(true);\n        for index in 1..carriers {\n            let (kernel, gate) = (&kernel, &gate);"),
    m("a waiting carrier takes a wake without a decision as a run",
      "            if let Some(run) = *decision {\n                return run;\n            }",
      "            if let Some(run) = *decision {\n                return run;\n            }\n            return true;"),
]
