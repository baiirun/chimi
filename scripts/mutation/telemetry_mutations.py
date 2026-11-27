"""Mutants of the telemetry event writers in Rust (CHI-305 step 5, phase 7):
runtime/chimi-runtime/src/telemetry.rs, which runtime.c's emit_event,
emit_dependency, emit_process_body and emit_scheduler_event were. The
native runs of the language, CLI, clock and trace tests compare telemetry
lines, so a changed byte shows. The trace lock's own mutant is in
carrier_threads_mutations.py, whose stress fixture checks whole lines.

Run: scripts/mutation/mutate.py scripts/mutation/telemetry_mutations.py
Results: scripts/mutation/telemetry_results.txt.
"""

CRATE = "compiler/chimi"

TELEMETRY = "../../runtime/chimi-runtime/src/telemetry.rs"


def m(name, edits, targets=("language", "cli", "clock", "trace_export")):
    return dict(name=name, file=TELEMETRY, edits=edits, targets=list(targets), filter="")


MUTATIONS = [
    m("sequence numbers do not advance", [(
        "            (*runtime).next_sequence += 1;\n", "")]),
    m("a root span has a parent", [(
        "            if parent < 0 { out.bytes(b\"null\") } else { out.signed(parent) }",
        "            out.signed(parent);")]),
    m("a control character is written raw", [(
        "                0..0x20 => b\"\",", "                0..0x20 => continue,")]),
    m("a newline is escaped as a code", [(
        "                b'\\n' => b\"\\\\n\",\n", "")]),
    m("the computation start has a time", [(
        "            out.signed(if started { 0 } else { chimi_fiber_now() });",
        "            out.signed(chimi_fiber_now());")]),
    m("the outcome is always success", [(
        "        ControlKind::FAILURE => b\"expected_failure\",\n", "")]),
    m("a task id of 0 is written", [(
        "    if absent || id == 0 { out.bytes(b\"null\") } else { out.unsigned(id) }",
        "    if absent { out.bytes(b\"null\") } else { out.unsigned(id) }")]),
    m("a clock event's cause is a task", [(
        "            optional(&out, b\"cause_task_id\", event.cause, clock || process);",
        "            optional(&out, b\"cause_task_id\", event.cause, process);")]),
    m("scheduler event names are off by one", [(
        "    (event.kind as usize).checked_sub(1).and_then(|index| names.get(index))",
        "    (event.kind as usize).checked_sub(0).and_then(|index| names.get(index))")]),
    m("a process defect names no message", [(
        "                named = (*runtime).last_defect_message;", "                named = ptr::null();")],
      targets=("process_lab", "language", "cli")),
    m("a dependency call names no method", [(
        "            out.json_c(callable.name);", "            out.json(b\"\");")]),
    m("a process body names no incarnation", [(
        "            out.identity(b\"incarnation_id\", incarnation);", "            out.identity(b\"incarnation_id\", 0);")]),
]
