"""Mutants of R4 step 3 PR 3a: the per-carrier host stack limit.

Run: scripts/mutation/mutate.py scripts/mutation/carrier_tls_mutations.py
Each mutant must turn the deep-recursion tests in cli.rs red.
Results: scripts/mutation/carrier_tls_results.txt.

The thread-local carrier pointer has no mutant: with one carrier every
thread's pointer names carrier 0, so binding the wrong one cannot show until
PR 5 runs a second carrier thread.
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

R = "runtime/native/runtime.c"


def m(name, edits, file=R):
    return dict(name=name, file=file, edits=edits, targets=["cli"],
                filter="ends_with_a_stack_overflow_defect")


MUTATIONS = [
    m("the guard never sets the carrier's limit", [(
        "  carrier->host_stack_limit = here - bottom > STACK_UNLIMITED_BYTES",
        "  uintptr_t unused_limit = here - bottom > STACK_UNLIMITED_BYTES")]),
    m("the check reads no limit", [(
        "    unsafe { here < (*current_carrier()).host_stack_limit }",
        "    here == 0")], EVAL),
]
