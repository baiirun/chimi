"""Mutants of CHI-316 (a wrong-owner Task handle names the handle in its
defect) and CHI-312 (each process body is a traced span naming its
incarnation).

Run: scripts/mutation/mutate.py scripts/mutation/task_handle_trace_mutations.py
Results: scripts/mutation/task_handle_trace_results.txt.
"""

CRATE = "compiler/chimi"

SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
RUNTIME_C = "runtime/native/runtime.c"
REPORTS = "../../runtime/chimi-runtime/src/scheduler_reports.rs"
TASK_ENTRY = "../../runtime/chimi-runtime/src/intrinsics/task_entry.rs"

BACKSTOP = "native_backstop_a_task_handle_hidden_in_an_interface_value_defects_its_reader"
BODY_SHOW = "a_process_body_is_listed_under_its_incarnation"
BODY_EXPORT = "a_process_body_is_a_span_tagged_with_its_incarnation"

MUTATIONS = [
    dict(
        name="the scheduler reports no task-handle violation",
        file=SCHEDULER,
        edits=[("let violation = ChimiCapabilityViolation::task_handle(owner, error, request_kind);",
                "let violation: Option<ChimiCapabilityViolation> = None;")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="the intruder is reported as the handle's owner",
        file=SCHEDULER,
        edits=[("} => (VIOLATION_WRONG_OWNER, handle, expected),",
                "} => (VIOLATION_WRONG_OWNER, handle, Some(task)),")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="a cancel is reported as a result read",
        file=SCHEDULER,
        edits=[("return Err(self.task_handle_violation(owner, error, REQUEST_TASK_CANCEL));",
                "return Err(self.task_handle_violation(owner, error, REQUEST_TASK_RESULT));")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="an unknown task is reported as a wrong owner",
        file=SCHEDULER,
        edits=[("SchedulerError::UnknownTask(handle) => (VIOLATION_UNKNOWN_TASK, handle, None),",
                "SchedulerError::UnknownTask(handle) => (VIOLATION_WRONG_OWNER, handle, None),")],
        command=["cargo", "test", "-p", "chimi-fiber", "--lib",
                 "a_task_handle_used_by_another_task_reports_the_handle_and_its_owner"],
        expect=["assertion `left == right` failed"],
    ),
    dict(
        name="the runtime words a task violation like a channel one",
        file=REPORTS,
        edits=[("        let text = if violation.capability == CapabilityViolation::TASK {",
                "        let text = if false {")],
        targets=["cli"], filter=BACKSTOP,
    ),
    dict(
        name="a process body writes no start span",
        file=TASK_ENTRY,
        edits=[("            chimi_trace_process_body_v1(runtime, span, incarnation, ptr::null());\n", "")],
        targets=["trace_show"], filter=BODY_SHOW,
    ),
    dict(
        name="a process body's span names no incarnation",
        file=TASK_ENTRY,
        edits=[("if traced { chimi_fiber_current_incarnation() } else { 0 }", "0")],
        targets=["trace_show"], filter=BODY_SHOW,
    ),
    dict(
        name="a resumed fiber is told no incarnation",
        file=SCHEDULER,
        edits=[("            incarnation: self.tasks[&task].process,\n",
                "            incarnation: None,\n")],
        targets=["trace_show"], filter=BODY_SHOW,
    ),
    dict(
        name="trace show leaves the process name off a body",
        file="src/trace_view.rs",
        edits=[("process_label(name.map(String::as_str), incarnation)",
                "process_label(None, incarnation)")],
        targets=["trace_show"], filter=BODY_SHOW,
    ),
    dict(
        name="the OTLP export drops the incarnation attribute",
        file="src/trace_export.rs",
        edits=[("            int_attribute(output, \"chimi.incarnation_id\", incarnation);\n", "")],
        targets=["trace_export"], filter=BODY_EXPORT,
    ),
]
