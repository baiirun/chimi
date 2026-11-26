"""Mutants of the lock around the native handle counts (R4 step 2; since
CHI-308 step 1 it counts native handles only, and the scheduler counts
channels and tasks; since CHI-309 part 2 the map itself is Rust, mutated in
handle_counts_mutations.py; since CHI-305 step 5 phase 0 the lock is a pthread
mutex the Rust side takes, in chimi-runtime pthread_locked.rs).

Run: scripts/mutation/mutate.py scripts/mutation/handle_table_mutations.py
Each mutant must turn the named tests red. The queue mutants of R4 step 2
went with the queue; handle_count_mutations.py covers the scheduler's.
Results: scripts/mutation/handle_table_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"
LOCK = "../../runtime/chimi-runtime/src/pthread_locked.rs"
LOCK_CALL = "        if unsafe { libc::pthread_mutex_lock(self.mutex.get()) } != 0 {"
UNLOCK = "        let _unlock = Unlock(self.mutex.get());"
STRESS = (["runtime_state"], "the_handle_table_counts_across_threads")


def m(name, edits, targets, filter="", file=R):
    return dict(name=name, file=file, edits=edits, targets=targets, filter=filter)


MUTATIONS = [
    m("the counts are reached without the lock",
      [(LOCK_CALL, "        if false {"), (UNLOCK, "")], *STRESS, file=LOCK),
    m("a granted lock aborts",
      [(LOCK_CALL, LOCK_CALL.replace("!= 0", "== 0"))], *STRESS, file=LOCK),
    m("the lock is never released",
      [(UNLOCK, "")], *STRESS, file=LOCK),
    m("a retain is not counted",
      [("  if (!chimi_handle_counts_retain_v1(key)) exhausted_memory();",
        "  (void)key;")],
      ["runtime_state", "net"]),
]
