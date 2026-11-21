"""Mutants of R4 step 3 PR 5a-2: binding handles, the console, standard input
and the signal installers under several carrier threads.

Run: scripts/mutation/mutate.py scripts/mutation/carrier_tables_mutations.py
Each mutant must turn the_runtime_state_holds_across_carrier_threads in
runtime_state.rs red.
Results: scripts/mutation/carrier_tables_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["runtime_state"],
                filter="the_runtime_state_holds_across_carrier_threads")


MUTATIONS = [
    m("a binding call holds none of its handles", [(
        "  binding_hold(arguments, argument_count, held, true);\n",
        "  for (uint32_t index = 0; index < argument_count; ++index) held[index] = false;\n")]),
    m("a close ignores the holders", [(
        "    ++entry->generation;\n    if (entry->users > 0) {",
        "    ++entry->generation;\n    if (false) {")]),
    m("the last holder does not release", [(
        "    if (--entry->users > 0 || entry->closing == NULL) continue;",
        "    if (--entry->users > 0 || entry->closing == NULL || true) continue;")]),
    m("a closed handle reads as open while held", [(
        "      entry->closing = entry->type;\n      entry->type = NULL;",
        "      entry->closing = entry->type;")]),
    m("handle lookups take no lock", [(
        "  if (value == NULL) return NULL;\n  pthread_mutex_lock(&binding_handle_lock);\n"
        "  BindingHandleSlot *entry = binding_handle_slot(value);\n"
        "  void *state = entry != NULL && entry->type == type ? entry->state : NULL;\n"
        "  pthread_mutex_unlock(&binding_handle_lock);",
        "  if (value == NULL) return NULL;\n"
        "  BindingHandleSlot *entry = binding_handle_slot(value);\n"
        "  void *state = entry != NULL && entry->type == type ? entry->state : NULL;")]),
    m("making a handle takes no lock", [(
        "  (void)call;\n  pthread_mutex_lock(&binding_handle_lock);\n  uint32_t slot = binding_free_slot;",
        "  (void)call;\n  uint32_t slot = binding_free_slot;"), (
        "  uint32_t generation = entry->generation;\n  pthread_mutex_unlock(&binding_handle_lock);",
        "  uint32_t generation = entry->generation;"), (
        "    if (!grown) {\n      pthread_mutex_unlock(&binding_handle_lock);",
        "    if (!grown) {")]),
    m("standard input has no lock", [(
        "static void input_lock(ByteInput *input) {\n  if (input->lock) pthread_mutex_lock(input->lock);\n}",
        "static void input_lock(ByteInput *input) {\n  (void)input;\n}"), (
        "static void input_unlock(ByteInput *input) {\n  if (input->lock) pthread_mutex_unlock(input->lock);\n}",
        "static void input_unlock(ByteInput *input) {\n  (void)input;\n}")]),
    m("a console line is written without the lock", [(
        "  console_enter();\n  int outcome = console_write_line_locked(runtime, stream, text, length);\n  console_leave();",
        "  int outcome = console_write_line_locked(runtime, stream, text, length);")]),
]
