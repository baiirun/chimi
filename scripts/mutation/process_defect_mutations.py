"""Mutants of process defect messages in the scheduler (CHI-308 step 3): a
body reports why it defected, the scheduler keeps the message with its
incarnation and hands it to the runtime's defect report.

Run: scripts/mutation/mutate.py scripts/mutation/process_defect_mutations.py
Each mutant must turn `cli.rs` `native_process_stack_depth_and_named_defects`
red (a body's own message and a stack overflow both reach the outcome), or
`native_a_deadlocked_root_names_the_last_process_defect`, and
the scheduler-side ones the chimi-fiber unit test
`a_defected_bodys_message_reaches_the_defect_report` too.
Results: scripts/mutation/process_defect_results.txt.

Equivalent mutants, not listed: clearing the message at each resume
(`set_resume_context`) only backs up the completion path, which takes the
message after every entry; the guard against a terminal cancel matters only
for an end the scheduler has decided, which reports no defect.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"
TASK_ENTRY = "../../runtime/chimi-runtime/src/intrinsics/task_entry.rs"
SCOPE = "../../runtime/chimi-fiber/src/scheduler/scope.rs"
PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"


def m(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli"],
                filter="native_process_stack_depth_and_named_defects")


MUTATIONS = [
    m("a defected body reports nothing", TASK_ENTRY, [(
        "                chimi_fiber_report_defect((*control).message);\n", "")]),
    m("the report ignores the body's message", R, [(
        "strdup(termination->stack_overflow ? \"Process stack overflow\" : termination->message);",
        "strdup(termination->stack_overflow ? \"Process stack overflow\" : \"defect\");")]),
    # The outcome event names an overflow itself; the defect report's name
    # shows in the message of a root that ends after the process defected.
    dict(name="a stack overflow is not named", file=R, edits=[(
        "  if (termination->stack_overflow || termination->message) {",
        "  if (termination->message) {")], targets=["cli"],
        filter="native_a_deadlocked_root_names_the_last_process_defect"),
    m("the scheduler drops a reported message", SCOPE, [(
        "    DEFECT_MESSAGE.with(|cell| *cell.borrow_mut() = Some(copy));",
        "    drop(copy);")]),
    m("an incarnation does not keep its message", PROCESS, [(
        "            record.defect_message = Some(message);",
        "            drop(message);")]),
    m("the termination record carries no message", PROCESS, [(
        "                    .map_or(std::ptr::null(), |message| message.as_ptr()),",
        "                    .map_or(std::ptr::null(), |_| std::ptr::null()),")]),
]
