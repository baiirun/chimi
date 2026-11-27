"""Mutants of the supervisor starts in Rust (CHI-305 step 5, phase 7, twelfth
PR): the 32-bit limits the scheduler's structs impose on a spec's mailbox
capacity and a policy's capacity and restart limit. (process_u6, process_u7,
process_u9 and g28 hold the retargeted mutants of restart codes, limits,
replies, names and handles.)

Run: scripts/mutation/mutate.py scripts/mutation/process_starts_mutations.py
Results: scripts/mutation/process_kernel_results.txt.

Left out, because no test outside a leak checker can see them:
- A refused start that keeps the contexts it never sent, or frees the arrays
  late. Either only leaks, which LeakSanitizer reports.
- A handoff whose argument points into another heap. A program cannot build
  such a value; the check guards the runtime's own invariant.
"""

CRATE = "compiler/chimi"

STARTS = "../../runtime/chimi-runtime/src/intrinsics/process_starts.rs"


def mutant(name, edits):
    return dict(name=name, file=STARTS, edits=edits, targets=["cli"],
                filter="native_supervisor_starts_refuse_counts_past_32_bits")


MUTATIONS = [
    mutant("a mailbox capacity past 32 bits is truncated", [(
        "            if capacity > i64::from(u32::MAX) {\n                return Err(too_large(function, b\": mailbox capacity",
        "            if false {\n                return Err(too_large(function, b\": mailbox capacity",
    )]),
    mutant("a policy capacity past 32 bits is truncated", [(
        "            if capacity > i64::from(u32::MAX) {\n                return Err(too_large(function, b\": capacity",
        "            if false {\n                return Err(too_large(function, b\": capacity",
    )]),
    mutant("a restart limit past 32 bits is truncated", [(
        "            if limit > i64::from(u32::MAX) {",
        "            if limit > i64::from(u32::MAX) + 1 {",
    )]),
    mutant("the restart limit stops one short of 32 bits", [(
        "            if limit > i64::from(u32::MAX) {",
        "            if limit >= i64::from(u32::MAX) {",
    )]),
    mutant("a member's spec names the nested start", [(
        "                    (*slot).process = self.spec(starter, spec, function, call_node)?;",
        "                    (*slot).process = self.spec(starter, spec, b\"Supervisor.process\", call_node)?;",
    )]),
]
