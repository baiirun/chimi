"""Mutants of the process kernel's requests in Rust (CHI-305 step 5, phase 7,
tenth and eleventh PRs): `Process.receive` and what it makes of a monitor
notice, and `Process.send` and `Process.monitor`. (g27, managed_copy and
process_u7 hold the retargeted mutants of own address, mailbox counts and
removal; process_u5, mailbox and notice_capability those of the send and
monitor hand-over.)

Run: scripts/mutation/mutate.py scripts/mutation/process_kernel_mutations.py
Results: scripts/mutation/process_kernel_results.txt.

Left out, because no test outside a leak checker can see them:
- A receive answered FORCED or CANCELLED that becomes a defect. The scheduler
  sends those replies with a cancel command (`respond_cancel`), so the fiber
  takes the cancelled path before the status is read; the arm only mirrors the
  C.
- A receive cancelled after its message arrived that keeps the message. The
  fragment then leaks, which only LeakSanitizer reports, and the race needs a
  cancel landing between delivery and resumption.
- `Supervisor.wait` answering a failed request where its supervisor
  escalated. A root escalation ends the program with the scheduler's own
  `Supervisor.RestartLimitExceeded` defect (lab ES01), so the waiter's defect
  is never the one reported.
- `Process.monitor` of a test address that reaches the scheduler. The
  scheduler refuses an address it does not know, so the watcher defects
  either way, and only the defect's text differs; a test run does not show a
  process's defect.
"""

CRATE = "compiler/chimi"

PROCESS_KERNEL = "../../runtime/chimi-runtime/src/intrinsics/process_kernel.rs"


def mutant(name, edits, targets=("process_lab", "cli")):
    return dict(name=name, file=PROCESS_KERNEL, edits=edits, targets=list(targets), filter="")


MUTATIONS = [
    mutant("a monitor notice is returned as the message", [(
        "                if request.down == 0 {\n                    return success(message);",
        "                if true {\n                    return success(message);",
    )]),
    mutant("every notice is Down.Returned", [(
        "                    StandardConstructor::DOWN_RETURNED + (request.down - Scheduler::DOWN_RETURNED),",
        "                    StandardConstructor::DOWN_RETURNED,",
    )]),
    mutant("Unavailable is reported as Full", [(
        "failure(standard_value(StandardConstructor::PROCESS_SEND_UNAVAILABLE, &[message]))",
        "failure(standard_value(StandardConstructor::PROCESS_SEND_FULL, &[message]))",
    )]),
    mutant("a sent message's copy is not counted", [(
        "        match hand_over(&mut request, message, true, foreign, node) {",
        "        match hand_over(&mut request, message, false, foreign, node) {",
    )]),
    mutant("a monitor handle names no monitor", [(
        "                (*value).payload.address.target = request.monitor;\n",
        "",
    )]),
]
