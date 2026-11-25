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


def m(name, edits, file=R):
    return dict(name=name, file=file, edits=edits, targets=["runtime_state"],
                filter="the_runtime_state_holds_across_carrier_threads")


MUTATIONS = [
    m("a binding call holds none of its handles", [(
        "      held[index] = arguments[index]->kind == VALUE_NATIVE_HANDLE &&\n",
        "      held[index] = false && arguments[index]->kind == VALUE_NATIVE_HANDLE &&\n")]),
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
        "  pthread_mutex_lock(&binding_handle_lock);\n  void *state = chimi_handles_state_v1(value->as.handle.slot, value->as.handle.generation, type);\n  pthread_mutex_unlock(&binding_handle_lock);",
        "  void *state = chimi_handles_state_v1(value->as.handle.slot, value->as.handle.generation, type);")]),
    m("making a handle takes no lock", [(
        "  pthread_mutex_lock(&binding_handle_lock);\n  bool opened = chimi_handles_open_v1(type, state, &slot, &generation);\n  pthread_mutex_unlock(&binding_handle_lock);",
        "  bool opened = chimi_handles_open_v1(type, state, &slot, &generation);")]),
    m("standard input has no lock", [(
        "static void input_lock(ByteInput *input) {\n  if (input->lock) pthread_mutex_lock(input->lock);\n}",
        "static void input_lock(ByteInput *input) {\n  (void)input;\n}"), (
        "static void input_unlock(ByteInput *input) {\n  if (input->lock) pthread_mutex_unlock(input->lock);\n}",
        "static void input_unlock(ByteInput *input) {\n  (void)input;\n}")]),
    m("a console line is written without the lock", [(
        "  console_enter();\n  int outcome = console_write_line_locked(runtime, stream, text, length);\n  console_leave();",
        "  int outcome = console_write_line_locked(runtime, stream, text, length);")]),
]
