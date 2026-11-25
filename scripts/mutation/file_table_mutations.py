"""Mutants of the open-file table in the Rust runtime
(runtime/chimi-runtime/src/files.rs), which runtime.c and the interpreter
share, and the lock runtime.c holds around every call into it.

Run: scripts/mutation/mutate.py scripts/mutation/file_table_mutations.py
Each must turn the_runtime_state_holds_across_carrier_threads in
runtime_state.rs red: its fixture checks the take, close and give rules on
one thread and shares one file between carrier threads.
Results: scripts/mutation/file_table_results.txt. These replace the C-side
file mutants that carrier_threads_mutations.py held before the move.

ThreadSanitizer cannot see the Rust table (the archive is not instrumented),
so a missing lock is caught by the fixture's cross-thread loan test, not by
a race report. Not listed: dropping the lock in fs_insert, which every
thread's handle is inserted before the threads start, so no test races it.
"""

CRATE = "compiler/chimi"

FILES = "../../runtime/chimi-runtime/src/files.rs"
R = "runtime/native/runtime.c"


def m(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_state"],
                filter="the_runtime_state_holds_across_carrier_threads")


MUTATIONS = [
    m("the table has no lock in fs_take", R, [(
        "static int fs_take(int64_t handle) {\n  pthread_mutex_lock(&fs_file_lock);\n  int fd = chimi_files_take_v1(handle);\n  pthread_mutex_unlock(&fs_file_lock);",
        "static int fs_take(int64_t handle) {\n  int fd = chimi_files_take_v1(handle);",
    )]),
    m("the table has no lock in fs_give", R, [(
        "  pthread_mutex_lock(&fs_file_lock);\n  int fd = chimi_files_give_v1(handle);\n  pthread_mutex_unlock(&fs_file_lock);",
        "  int fd = chimi_files_give_v1(handle);",
    )]),
    m("the table has no lock in fs_close", R, [(
        "  pthread_mutex_lock(&fs_file_lock);\n  int fd = chimi_files_close_v1(handle);\n  pthread_mutex_unlock(&fs_file_lock);",
        "  int fd = chimi_files_close_v1(handle);",
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
