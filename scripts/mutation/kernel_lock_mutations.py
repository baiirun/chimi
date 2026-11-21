"""Mutants of R4 step 3 PR 4: the carrier loop and the kernel lock.

Run: scripts/mutation/mutate.py scripts/mutation/kernel_lock_mutations.py
Each mutant must turn the chimi-fiber unit tests red.
Results: scripts/mutation/kernel_lock_results.txt.

Not mutated: the check in `finish_io_wait` that a registration is still
there after the wait. With one carrier nothing can withdraw one while the
carrier waits, so no test can tell; PR 5's multi-carrier tests cover it.
"""

CRATE = "runtime/chimi-fiber"

C = "src/scheduler/carrier.rs"


def m(name, edits, filter):
    return dict(name=name, file=C, edits=edits, targets=["lib"], filter=filter)


MUTATIONS = [
    m("a fiber resumes under the kernel lock", [(
        "        let event = resume_fiber(runnable.fiber, runnable.command);\n"
        "        next = kernel.with(|kernel| {\n",
        "        next = kernel.with(|kernel| {\n"
        "            let event = resume_fiber(runnable.fiber, runnable.command);\n")],
        "fibers_the_carrier_resumes_run_without_the_kernel_lock"),
    m("the lock is not marked held", [(
        "        #[cfg(test)]\n        let _held = HeldMark::set();\n", "")],
        "the_kernel_lock_is_held_while_the_carrier_applies_a_request"),
]
