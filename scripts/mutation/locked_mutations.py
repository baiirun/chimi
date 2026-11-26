"""Mutants of the handle counts' lock (CHI-305 step 5, phase 0,
runtime/chimi-runtime/src/locked.rs), caught by its unit tests. The
handle-table stress fixture (tests/runtime_state.rs) runs the same lock from
C threads.

Run: scripts/mutation/mutate.py scripts/mutation/locked_mutations.py
Results: scripts/mutation/locked_results.txt.
"""

CRATE = "runtime/chimi-runtime"
LOCKED = "src/locked.rs"
ABI = "src/handle_counts_abi.rs"


def m(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="locked")


MUTATIONS = [
    m("the body runs without the lock", LOCKED,
      "        if unsafe { libc::pthread_mutex_lock(self.mutex.get()) } != 0 {",
      "        if false && unsafe { libc::pthread_mutex_lock(self.mutex.get()) } != 0 {"),
    m("the lock is never released", LOCKED,
      "        let _unlock = Unlock(self.mutex.get());",
      "        let _unlock = ();"),
    m("a granted lock aborts", LOCKED,
      "        if unsafe { libc::pthread_mutex_lock(self.mutex.get()) } != 0 {",
      "        if unsafe { libc::pthread_mutex_lock(self.mutex.get()) } == 0 {"),
    m("a panicking body keeps the lock", LOCKED,
      "        let _unlock = Unlock(self.mutex.get());\n        // SAFETY: the mutex is held, so this is the only reference.\n        body(unsafe { &mut *self.value.get() })",
      "        // SAFETY: the mutex is held, so this is the only reference.\n        let result = body(unsafe { &mut *self.value.get() });\n        drop(Unlock(self.mutex.get()));\n        result"),
]
