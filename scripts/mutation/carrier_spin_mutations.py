"""Mutants of R4 step 3 PR 6c: an idle carrier spins on its wake flag before it blocks.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_spin_mutations.py
Results: scripts/mutation/carrier_spin_results.txt.
"""

CRATE = "runtime/chimi-fiber"

WAKE = "src/scheduler/wake.rs"
CARRIER = "src/scheduler/carrier.rs"


def m(name, file, old, new, filter=""):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=filter)


MUTATIONS = [
    m("a spin never sees the wake", WAKE,
      "            if flag.load(Ordering::Relaxed) && flag.swap(false, Ordering::Acquire) {",
      "            if false {",
      filter="spin"),
    m("a wake never sets the flag", WAKE,
      "            unsafe { &*pending }.store(true, Ordering::Release);\n        }",
      "            let _ = pending;\n        }",
      filter="spin"),
    m("a caught wake leaves its byte", WAKE,
      "                for &fd in self.as_slice() {\n                    super::signal::drain(fd);\n                }",
      "",
      filter="wake"),
    m("carriers beyond the cores spin", WAKE,
      "    if carriers as usize <= cores {",
      "    if carriers > 0 {",
      filter="cores"),
    m("a spin ignores a zero budget", WAKE,
      "        if budget.is_zero() {\n            return false;\n        }",
      "",
      filter="spin"),
    m("the carrier never watches its flag", "src/scheduler.rs",
      "            fds.watch(pending, spin);",
      "            let _ = (pending, spin);",
      filter="sees_a_wake_while_it_spins"),
    m("a park skips the spin", CARRIER,
      "            if !wake_fds.spin(wake_fds.spin_within(Duration::MAX)) {",
      "            if true {",
      filter="sees_a_wake_while_it_spins"),
]
