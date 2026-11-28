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
CALLS = "../../runtime/chimi-runtime/src/intrinsics/binding_calls.rs"
HANDLES = "../../runtime/chimi-runtime/src/handles.rs"


def m(name, edits, file=R):
    return dict(name=name, file=file, edits=edits, targets=["net"],
                filter="a_socket_no_value_names_is_closed_at_any_end")


MUTATIONS = [
    m("a new handle is not counted", [(
        "        chimi_runtime_retain_counted_value(value);\n        value\n", "        value\n")], file=CALLS),
    m("a value never names its handle's key", [(
        "  if (value->kind == VALUE_NATIVE_HANDLE)\n"
        "    return native_handle_key(value->as.handle.slot, value->as.handle.generation);\n", "")]),
    m("an unreferenced handle is not closed", [(
        "    unsafe { close(slot, key as u32, ptr::null(), ptr::null()) };",
        "    let _ = slot;")], file=CALLS),
    m("an unreferenced key ignores the generation", [(
        "            Some(Slot::Open(live)) if live.generation == generation => Some(live),",
        "            Some(Slot::Open(live)) => Some(live),")], file=HANDLES),
    m("a native key is counted like a channel", [(
        "static void counted_handle_drop(uint64_t key) {\n  if (!is_native_handle_key(key)) {",
        "static void counted_handle_drop(uint64_t key) {\n  if (true) {")]),
]
