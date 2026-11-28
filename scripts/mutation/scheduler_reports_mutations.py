"""Mutants of the scheduler report messages in Rust (CHI-305 step 5, phase
8, tenth PR: runtime/chimi-runtime/src/scheduler_reports.rs). The capability,
deadlock and process-defect messages end up in the root's defect, which the
native tests read.

Run: scripts/mutation/mutate.py scripts/mutation/scheduler_reports_mutations.py
Results: scripts/mutation/scheduler_reports_results.txt.
"""

CRATE = "compiler/chimi"

REPORTS = "../../runtime/chimi-runtime/src/scheduler_reports.rs"


def mutant(name, old, new, targets=("cli", "channel_variants")):
    return dict(name=name, file=REPORTS, edits=[(old, new)], targets=list(targets), filter="")


MUTATIONS = [
    mutant("a deadlock lists its waiters without separators",
           "text.push(if index == 0 { b\" task \" } else { b\"; task \" })",
           "text.push(b\" task \")"),
    mutant("a deadlocked receive is named a send",
           "        Scheduler::REQUEST_CHANNEL_RECEIVE => b\"Channel.receive\",\n        Scheduler::REQUEST_SELECT",
           "        Scheduler::REQUEST_CHANNEL_RECEIVE => b\"Channel.send\",\n        Scheduler::REQUEST_SELECT"),
    mutant("the latest process defect loses its incarnation",
           "        text.number(runtime.last_defect_incarnation).push(b\" of membership \");",
           "        text.number(0).push(b\" of membership \");"),
    mutant("a root keeps no process defect",
           "        if runtime.process_defect_count == 0 {\n            return message;",
           "        if true {\n            return message;"),
    mutant("a task violation names the reader as the owner",
           "        text.push(b\"task \").number(violation.owner).push(b\" owns it\");\n    }\n    text\n}",
           "        text.push(b\"task \").number(violation.task).push(b\" owns it\");\n    }\n    text\n}"),
    mutant("a task violation is always a wait",
           "        Scheduler::REQUEST_TASK_RESULT => b\"read the result of task \",",
           "        Scheduler::REQUEST_TASK_RESULT => b\"wait for task \","),
    mutant("sending a receiver through its own channel is worded as a release",
           "        CapabilityViolation::OWN_RECEIVER => {",
           "        CapabilityViolation::CHANNEL_STILL_LIVE => {"),
    mutant("a violation note is kept for the wrong task",
           "            if (*note).task == task {",
           "            if (*note).task != task {"),
]
