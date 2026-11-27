"""Mutants of the loop nodes and the intrinsic routing in Rust (CHI-305
step 5, phase 7 part 3): `chimi_rt_loop`, `chimi_rt_direct_loop`, the loop
passes and `end_measured_scope` in runtime/chimi-runtime/src/eval.rs, and
runtime/chimi-runtime/src/intrinsics/dispatch.rs. The older mutants of the
C loops were retargeted where they live (clock_timeout, direct_loop,
runtime_profile); these cover the rest.

Run: scripts/mutation/mutate.py scripts/mutation/loop_mutations.py
Results: scripts/mutation/loop_results.txt.
"""

CRATE = "compiler/chimi"

EVAL = "../../runtime/chimi-runtime/src/eval.rs"
DISPATCH = "../../runtime/chimi-runtime/src/intrinsics/dispatch.rs"


def m(name, edits, targets=("language",), filter="", file=EVAL):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    m("a pass keeps the initial state", [(
        "            *state_slot = state;\n", "")]),
    m("a break leaves any enclosing loop", [(
        "            if (*pass).kind == ControlKind::BREAK && (*pass).node == i64::from(node.a) {",
        "            if (*pass).kind == ControlKind::BREAK {")]),
    m("a loop pass is no cancellation point", [(
        "            if cancel_requested(runtime) {\n                break cancelled();\n            }\n            if scheduled",
        "            if scheduled")], ("cli", "clock")),
    m("a fired scope does not stop a loop", [(
        "            if scheduled && scope_check_due(&mut polls) && chimi_runtime_loop_scope_check_v1() {",
        "            if scheduled && scope_check_due(&mut polls) && !chimi_runtime_loop_scope_check_v1() {")],
      ("language", "cli", "clock")),
    m("a pass keeps all it allocated", [(
        "                if collector::collect(current_carrier(), passes, &raw mut state, ptr::null_mut()) != HeapStatus::OK {",
        "                if false {")], ("language", "cli")),
    m("a loop's region adds nothing to the totals", [(
        "        let top_scope = (*scope).parent.is_null();", "        let top_scope = false;")], ("language", "cli")),
    m("the peak is the last scope's", [(
        "            if scope.peak_live_bytes > totals.peak_live_bytes {",
        "            if true {")], ("language", "cli")),
    m("a direct loop's break gives its state", [(
        "                DIRECT_PASS_BREAK => return success(integer_value(ValueKind::INTEGER, next)),",
        "                DIRECT_PASS_BREAK => return success(integer_value(ValueKind::INTEGER, state)),")]),
    m("a fallback pass reruns from zero", [(
        "                        integer_value(ValueKind::INTEGER, state),",
        "                        integer_value(ValueKind::INTEGER, 0),")]),
    m("a direct loop ignores cancellation", [(
        "            if cancel_requested(runtime) {\n                return cancelled();\n            }\n            let mut next",
        "            let mut next")], ("signals",), "direct_integer_loop"),
    m("the scalar intrinsics go to C", [(
        "            _ if is_scalar(intrinsic) => scalar_operation(intrinsic, arguments(values, count), call_node),\n",
        "")], file=DISPATCH),
    m("Map.remove goes to C", [(
        "            Intrinsic::MAP_REMOVE..=Intrinsic::MAP_REDUCE_ENTRIES | Intrinsic::MAP_EMPTY..=Intrinsic::MAP_PUT => {",
        "            Intrinsic::MAP_REDUCE_ENTRIES..=Intrinsic::MAP_REDUCE_ENTRIES | Intrinsic::MAP_EMPTY..=Intrinsic::MAP_PUT => {")],
      file=DISPATCH),
    m("a file read never sees a cancellation", [(
        "                let cancel = ptr::read_volatile(&raw const (*runtime).cancel) != 0;",
        "                let cancel = false;")], ("cli", "clock"), file=DISPATCH),
    m("std/test recorders go to C", [(
        "            Intrinsic::TEST_RECORDER..=Intrinsic::TEST_RECEIVED => {",
        "            Intrinsic::TEST_RECEIVED..=Intrinsic::TEST_RECEIVED => {")], ("std_modules",), "standard_", file=DISPATCH),
]
