"""Mutants of R4 step 4d-1: a carrier spins briefly on the kernel lock.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/lock_spin_mutations.py
Results: scripts/mutation/lock_spin_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, old, new):
    return dict(name=name, file="src/scheduler/carrier.rs", edits=[(old, new)], targets=["lib"], filter="")


# The spin changes only how long a carrier waits, so the two mutants that
# remove or lengthen it are expected to survive: the measurement, not a
# test, shows the gain (multi-carrier-baseline.md).
MUTATIONS = [
    m("a poisoned lock is not taken over during the spin",
      "            Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),",
      "            Err(std::sync::TryLockError::Poisoned(_)) => panic!(\"poisoned\"),"),
    m("a poisoned lock is not taken over after the spin",
      "            return lock.lock().unwrap_or_else(PoisonError::into_inner);\n        }\n        for _ in 0..32 {",
      "            return lock.lock().expect(\"unpoisoned\");\n        }\n        for _ in 0..32 {"),
    m("no spin (performance only)",
      "            Err(std::sync::TryLockError::WouldBlock) => {}",
      "            Err(std::sync::TryLockError::WouldBlock) => return lock.lock().unwrap_or_else(PoisonError::into_inner),"),
    m("the spin never gives up (performance only)",
      "        if std::time::Instant::now() >= spin_until {",
      "        if std::time::Instant::now() < spin_until && false {"),
]
