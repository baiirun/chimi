"""Mutants of native handle counting: a socket, listener, TLS session or exec
handle that no value names is closed, whichever way its holder ended
(Byron's "Close at any end", 2026-11-17).

Run: scripts/mutation/mutate.py scripts/mutation/native_handle_count_mutations.py
Each mutant must turn `net.rs` `a_socket_no_value_names_is_closed_at_any_end`
red.
Results: scripts/mutation/native_handle_count_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["net"],
                filter="a_socket_no_value_names_is_closed_at_any_end")


MUTATIONS = [
    m("a new handle is not counted", [(
        "  counted_handle_retain(native_handle_key(slot, generation));\n", "")]),
    m("a value never names its handle's key", [(
        "  if (value->kind == VALUE_NATIVE_HANDLE)\n"
        "    return native_handle_key(value->as.handle.slot, value->as.handle.generation);\n", "")]),
    m("an unreferenced handle is not closed", [(
        "  binding_handle_close((uint32_t)((key & ~NATIVE_HANDLE_KEY) >> 32), (uint32_t)key, NULL, NULL);",
        "  (void)key;")]),
    m("an unreferenced key ignores the generation", [(
        "  if (entry && entry->type != NULL && entry->generation == generation && (!type || entry->type == type)) {",
        "  if (entry && entry->type != NULL && (!type || entry->type == type)) {")]),
    m("a native key is counted like a channel", [(
        "static void counted_handle_drop(uint64_t key) {\n  if (!is_native_handle_key(key)) {",
        "static void counted_handle_drop(uint64_t key) {\n  if (true) {")]),
]
