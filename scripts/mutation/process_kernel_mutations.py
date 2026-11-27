"""Mutants of the process kernel's message-free requests in Rust (CHI-305
step 5, phase 7, tenth PR): `Process.receive` and its monitor notices, and
`Supervisor.wait`'s escalation. (g27, managed_copy and process_u7 hold the
retargeted mutants of own address, mailbox counts and removal.)

Run: scripts/mutation/mutate.py scripts/mutation/process_kernel_mutations.py
Results: scripts/mutation/process_kernel_results.txt.
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
    mutant("a forced receive is a defect", [(
        "            Scheduler::RESPONSE_CANCELLED | Scheduler::RESPONSE_FORCED => cancelled(),",
        "            Scheduler::RESPONSE_CANCELLED => cancelled(),",
    )]),
    mutant("a cancelled receive keeps its message", [(
        "            if request.status == Scheduler::RESPONSE_SUCCEEDED {\n                fragment::free(",
        "            if false {\n                fragment::free(",
    )]),
    mutant("an escalated wait reports a failed request", [(
        "                if !stop && request.status == Scheduler::RESPONSE_DEFECTED {",
        "                if false {",
    )]),
]
