"""Mutants of the open-file table in the Rust runtime
(runtime/chimi-runtime/src/files.rs), which the native runtime and the
interpreter share, and the lock the native runtime takes around every call
into it (intrinsics/host.rs's FILES, a PthreadLocked).

Run: scripts/mutation/mutate.py scripts/mutation/file_table_mutations.py
Each must turn the_runtime_state_holds_across_carrier_threads in
runtime_state.rs red: its fixture checks the take, close and give rules on
one thread and shares one file between carrier threads.
Results: scripts/mutation/file_table_results.txt. These replace the C-side
file mutants that carrier_threads_mutations.py held before the move.

ThreadSanitizer cannot see the Rust table (the archive is not instrumented),
so a missing lock is caught by the fixture's cross-thread loan test, not by
a race report. The lock mutant drops every PthreadLocked lock at once, since
the table's is reached only through `with`.
"""

CRATE = "compiler/chimi"

FILES = "../../runtime/chimi-runtime/src/files.rs"
LOCK = "../../runtime/chimi-runtime/src/pthread_locked.rs"


def m(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_state"],
                filter="the_runtime_state_holds_across_carrier_threads")


MUTATIONS = [
    m("the table has no lock", LOCK, [(
        "if unsafe { libc::pthread_mutex_lock(self.mutex.get()) } != 0 {",
        "if false {",
    ), (
        "unsafe { libc::pthread_mutex_unlock(self.0) };",
        "let _ = self.0;",
    )]),
    m("a take does not count a loan", FILES, [(
        "        entry.loans += 1;\n",
        "",
    )]),
    m("a close ignores the loans", FILES, [(
        "        if entry.loans != 0 {\n            return Closed::Lent;\n        }\n",
        "",
    )]),
    m("the last give does not release", FILES, [(
        "        if entry.loans == 0 && !entry.open {",
        "        if entry.loans == 0 && !entry.open && false {",
    )]),
    m("a slot is reused while its descriptor is lent", FILES, [(
        "position(|slot| slot.resource.is_none())",
        "position(|slot| !slot.open)",
    )]),
    m("a close keeps the generation", FILES, [(
        "        entry.generation = entry.generation.wrapping_add(1);\n",
        "",
    )]),
]
