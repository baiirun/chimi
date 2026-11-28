"""Mutants of R4 step 3 PR 5a-2: binding handles, the console, standard input
and the signal installers under several carrier threads.

Run: scripts/mutation/mutate.py scripts/mutation/carrier_tables_mutations.py
Each mutant must turn the_runtime_state_holds_across_carrier_threads in
runtime_state.rs red.
Results: scripts/mutation/carrier_tables_results.txt.

The binding handle slots are in Rust since CHI-309 part 2. ThreadSanitizer
cannot see the Rust table, so "handle lookups take no lock" survives (see the
results).
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


HANDLES = "../../runtime/chimi-runtime/src/handles.rs"
CONSOLE_IO = "../../runtime/chimi-runtime/src/intrinsics/console_io.rs"
INPUT = "../../runtime/chimi-runtime/src/intrinsics/input.rs"
CALLS = "../../runtime/chimi-runtime/src/intrinsics/binding_calls.rs"


def m(name, edits, file=R):
    return dict(name=name, file=file, edits=edits, targets=["runtime_state"],
                filter="the_runtime_state_holds_across_carrier_threads")


MUTATIONS = [
    m("a binding call holds none of its handles", [(
        "                *(*call).held.add(index as usize) = (*value).kind == ValueKind::NATIVE_HANDLE\n",
        "                *(*call).held.add(index as usize) = false && (*value).kind == ValueKind::NATIVE_HANDLE\n")], file=CALLS),
    m("a close ignores the holders", [(
        "        if live.users > 0 {\n            if let Some(entry)",
        "        if false {\n            if let Some(entry)")], file=HANDLES),
    m("the last holder does not release", [(
        "                if live.users > 0 {\n                    return None;",
        "                if live.users > 0 || true {\n                    return None;")], file=HANDLES),
    # Removed as equivalent: "a closed handle reads as open while held" (keeping
    # `type` set beside `closing`). The close bumps the generation, so every
    # lookup already misses the slot, and the last holder still releases once.
    m("handle lookups take no lock", [(
        "    lock();\n    // SAFETY: the lock is held; `value` is a live handle.\n    let state = unsafe { chimi_handles_state_v1(",
        "    // SAFETY: the lock is held; `value` is a live handle.\n    let state = unsafe { chimi_handles_state_v1("), (
        "kind.cast()) };\n    unlock();\n    state",
        "kind.cast()) };\n    state")], file=CALLS),
    m("making a handle takes no lock", [(
        "    lock();\n    // SAFETY: the lock is held; the outputs are writable.\n    let opened = unsafe { chimi_handles_open_v1(kind.cast(), state, &mut slot, &mut generation) };\n    unlock();",
        "    // SAFETY: the lock is held; the outputs are writable.\n    let opened = unsafe { chimi_handles_open_v1(kind.cast(), state, &mut slot, &mut generation) };")], file=CALLS),
    m("standard input has no lock", [(
        "        if !(*input).lock.is_null() && libc::pthread_mutex_lock((*input).lock) != 0 {",
        "        if false && libc::pthread_mutex_lock((*input).lock) != 0 {"), (
        "        if !(*input).lock.is_null() {\n            libc::pthread_mutex_unlock((*input).lock);",
        "        if false {\n            libc::pthread_mutex_unlock((*input).lock);")], file=INPUT),
    m("a console line is written without the lock", [(
        "    enter();\n    // SAFETY: the caller's contract.\n    let outcome = unsafe { write_line_locked(runtime, index, text) };\n    leave();",
        "    // SAFETY: the caller's contract.\n    let outcome = unsafe { write_line_locked(runtime, index, text) };")], file=CONSOLE_IO),
]
