"""Mutants of the process kernel's message-free requests in Rust (CHI-305
step 5, phase 7, tenth PR): `Process.receive` and its monitor notices, and
what it makes of a notice. (g27, managed_copy and process_u7 hold the
retargeted mutants of own address, mailbox counts and removal.)

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
]
