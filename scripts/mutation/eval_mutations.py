"""Mutants of the native evaluator in Rust (CHI-305 step 5, phase 7):
runtime/chimi-runtime/src/eval.rs, which runtime.c's core nodes,
`invoke`, the match helpers and the node frame were. The mutants that
older suites already held for these pieces were retargeted where they
live (cause, environment_slots, host_need_value, host_stack, lane_fixes,
native_allocation, process_u5, process_u7, runtime_profile, using_binding,
carrier_measure, carrier_threads, carrier_tls); these cover the rest.

Run: scripts/mutation/mutate.py scripts/mutation/eval_mutations.py
Results: scripts/mutation/eval_results.txt.
"""

CRATE = "compiler/chimi"

EVAL = "../../runtime/chimi-runtime/src/eval.rs"
STANDARD = "../../runtime/chimi-runtime/src/intrinsics/standard.rs"


def m(name, edits, targets=("language",), filter="", file=EVAL):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    m("an overflow is reported as a division by zero", [(
        "    let what: &[u8] = if right == 0 && matches!(operator, Core::BINARY_DIVIDE | Core::BINARY_REMAINDER) {",
        "    let what: &[u8] = if matches!(operator, Core::BINARY_DIVIDE | Core::BINARY_REMAINDER) {")]),
    m("the smallest Int's remainder by -1 overflows", [(
        "        Core::BINARY_REMAINDER if right == -1 => Some(0),\n", "")]),
    m("a float remainder divides", [(
        "            if operator == Core::BINARY_DIVIDE { left / right } else { unsafe { fmod(left, right) } }",
        "            left / right")]),
    m("a float without a finite result passes", [(
        "        if !output.is_finite() {", "        if output.is_nan() {")]),
    m("a decimal overflow names one operand twice", [(
        "                    .text(r.text().as_bytes())\n", "                    .text(l.text().as_bytes())\n")]),
    m("negating the smallest Int wraps", [(
        "            ValueKind::INTEGER if (*value).payload.integer == i64::MIN => numeric_defect(",
        "            ValueKind::INTEGER if false => numeric_defect(")]),
    m("an inclusive range ends before its bound", [(
        "                end += 1;\n", "")]),
    m("<= is <", [(
        "            Core::BINARY_LESS_EQUAL => order <= 0,", "            Core::BINARY_LESS_EQUAL => order < 0,")]),
    m("a construction's failure starts nowhere", [(
        "        if (*result).kind == ControlKind::FAILURE {\n            (*result).node = i64::from(node_id);\n        }\n",
        "")], ("language", "cli")),
    m("a call's raised failure keeps no location", [(
        "            return new_control(ControlKind::FAILURE, (*result).value, ptr::null(), i64::from(node_id));",
        "            return result;")], ("language", "cli")),
    m("a closure captures nothing", [(
        "            *(*captured).locals.add(local) = *(*source).locals.add(local);\n", "")]),
    m("a provision skips the clock slot", [(
        "            *requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize) = value;\n", "")],
      ("language", "clock")),
    m("a caller's providers do not reach the callee", [(
        "            if !provided.is_null() {\n                *(*environment).requirements.add(index) = provided;\n            }\n",
        "")], ("language", "clock")),
    m("console.write_error writes to standard output", [(
        "                (Core::TYPE_CONSOLE, 1) => Core::HOST_WRITE_ERROR,",
        "                (Core::TYPE_CONSOLE, 1) => Core::HOST_WRITE_LINE,")], ("language", "cli")),
    m("a guard that is false still takes its arm", [(
        "                if !(*(*guard).value).payload.boolean {\n                    continue;\n                }\n", "")]),
    m("an unmatched failure is a defect", [(
        "            if value(subject).kind == MatchKind::Failure {\n                return value(subject).control;\n            }\n",
        "")]),
    m("a list rest binds the whole list", [(
        "        let view = list::slice(current_carrier(), value, prefix, suffix_start);",
        "        let view = value;")]),
    m("a break carries Void", [(
        "            value = (*control).value;\n        }\n        new_control(ControlKind::BREAK",
        "            let _ = (*control).value;\n        }\n        new_control(ControlKind::BREAK")]),
    m("a full trace reports no node start", [(
        "            trace(runtime, c\"operation_started\", id, span, parent, operation, ptr::null());\n", "")],
      ("language", "cli", "trace_export")),
    m("a dependency call takes no span", [(
        "                        let dependency_span = take_spans(runtime, 1);",
        "                        let dependency_span = span;")], ("language", "cli", "trace_export")),
    m("True and False share one value", [(
        "    unsafe { shared_value(if raw { 2 } else { 1 }) }", "    unsafe { shared_value(1) }")],
      file=STANDARD),
    m("a cancelled run keeps evaluating", [(
        "        if cancel_requested(runtime) {\n            return cancelled();\n        }\n        let function",
        "        let function")], ("cli", "clock")),
]
