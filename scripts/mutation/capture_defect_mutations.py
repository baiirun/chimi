"""Mutants of `Test.capture_defect` and `T4401` (Phase 5, CHI-208, part 3):
telling an assertion from the defect under test, failing on any other
outcome, cancelling the operation's tasks, the scheduler's scope without a
deadline and its cancel request, and rejecting test-only operations reachable
from `main`.

Run: scripts/mutation/mutate.py scripts/mutation/capture_defect_mutations.py
Each mutant runs tests/testing.rs, the end-to-end conformance for both backends.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
I = "src/execution/core_operations.rs"
E = "src/executable.rs"
SCOPE = "../../runtime/chimi-fiber/src/scheduler/scope.rs"


def mutant(name, file, edits, targets=("testing",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("native: an inner assertion is captured", C, [(
        "bool captured = control->kind == CONTROL_DEFECT && !is_assertion_call(control->node);",
        "bool captured = control->kind == CONTROL_DEFECT;",
    )]),
    mutant("native: the operation's tasks are not cancelled", C, [(
        "    if (captured) {\n      ChimiSchedulerRequest cancel",
        "    if (0) {\n      ChimiSchedulerRequest cancel",
    )]),
    mutant("native: a success is returned as the message", C, [(
        "  if (control->kind == CONTROL_SUCCESS)\n    return capture_outcome_defect(",
        "  if (control->kind == CONTROL_SUCCESS)\n    return control;\n  if (0)\n    return capture_outcome_defect(",
    )]),
    mutant("native: an expected failure escapes", C, [(
        "  if (control->kind == CONTROL_FAILURE)\n    return capture_outcome_defect(",
        "  if (0)\n    return capture_outcome_defect(",
    )]),
    mutant("native: the node search misses the last entry", C, [(
        "  return low < CHIMI_ASSERTION_NODE_COUNT && (int64_t)CHIMI_ASSERTION_NODES[low] == node;",
        "  return low + 1 < CHIMI_ASSERTION_NODE_COUNT && (int64_t)CHIMI_ASSERTION_NODES[low] == node;",
    )]),
    mutant("interpreter: an inner assertion is captured", I, [(
        "                if !node.is_some_and(|node| self.is_assertion_call(node)) =>",
        "                if true =>",
    )]),
    mutant("interpreter: a success is returned", I, [(
        "            Control::Success(value) => not_a_defect(CAPTURE_RETURNED_HEADING, &value),",
        "            Control::Success(value) => Control::Success(value),",
    )]),
    mutant("assertion set: capture_defect's own failure is a plain defect", E, [(
        "                        | StandardFunction::TestFail\n                        | StandardFunction::TestCaptureDefect",
        "                        | StandardFunction::TestFail",
    )]),
    mutant("T4401: never reported", E, [(
        "    if let Some(found) = &reachable.test_only {",
        "    if let Some(found) = None::<&TestOnlyUse> {",
    )]),
    mutant("T4401: the path loses its callers", E, [(
        "                                state.callers.entry(symbol).or_insert(current);",
        "                                let _ = (symbol, current);",
    )]),
    mutant("scheduler: a scope cancel leaves the children running", SCOPE, [(
        "        record.scopes[index].draining = true;\n        let nested",
        "        record.scopes[index].draining = true;\n        return true;\n        #[allow(unreachable_code)]\n        let nested",
    )]),
    mutant("scheduler: a scope without a deadline still gets a timer", SCOPE, [(
        "let timer = (deadline != Instant::NEVER).then(|| {",
        "let timer = true.then(|| {",
    )]),
]
