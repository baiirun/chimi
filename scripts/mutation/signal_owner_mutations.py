"""Mutants of R4 step 4d-3: signals are read and delivered on carrier 0 only.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/signal_owner_mutations.py
Results: scripts/mutation/signal_owner_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, old, new):
    return dict(name=name, file="src/scheduler.rs", edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("every carrier takes signals",
      "        if self.current == ready::Carrier(0) {\n            self.collect_user_signals();",
      "        if true {\n            self.collect_user_signals();"),
    m("no carrier takes signals",
      "        if self.current == ready::Carrier(0) {\n            self.collect_user_signals();",
      "        if false {\n            self.collect_user_signals();"),
    m("carrier 0 collects but never delivers",
      "            self.collect_user_signals();\n            self.deliver_signal();\n        }",
      "            self.collect_user_signals();\n        }"),
]
