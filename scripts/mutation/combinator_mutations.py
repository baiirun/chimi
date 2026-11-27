"""Mutants of `Task.all`, `Task.results`, and `Task.race` (CHI-205): the
scheduler's first-terminated wait, the native combinators, the function-type
join, `T4345`, and the transfer check of a combinator's thunk list.

Run: scripts/mutation/mutate.py scripts/mutation/combinator_mutations.py
Each mutant must turn a test in tests/task_combinators.rs red. The scheduler
mutants edit runtime/chimi-fiber, which the native build links, so they are
judged by what a compiled Chimi program does.
Results: scripts/mutation/combinator_results.txt.
"""

CRATE = "compiler/chimi"

SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
SCOPE = "../../runtime/chimi-fiber/src/scheduler/scope.rs"
PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
RUNTIME = "runtime/native/runtime.c"
TASKS = "../../runtime/chimi-runtime/src/intrinsics/tasks.rs"
RUST_CAUSES = "../../runtime/chimi-runtime/src/intrinsics/causes.rs"


def combinators(name, file, edits, filter=""):
    return dict(name=name, file=file, edits=edits, targets=["task_combinators"], filter=filter)


MUTATIONS = [
    # The wait answers in termination order.
    combinators("the last child to terminate is first", SCHEDULER, [(
        "            .min()\n            .map(|(_, task)| task);",
        "            .max()\n            .map(|(_, task)| task);")]),
    combinators("a terminating child does not wake its waiting parent", SCHEDULER, [(
        "                .waiting_any\n                .clear();\n            let _ = self.wake(parent);",
        "                .waiting_any\n                .clear();")]),
    # No mutant drops REQUEST_TASK_ANY from scope.rs's withdrawable list: a
    # firing scope cancels every child of the waiting task too, the first
    # cancelled child wakes the wait, and the call ends cancelled at the same
    # instant, so the mutant is equivalent.
    # The decided outcome cancels the rest.
    combinators("the losers are not cancelled", TASKS, [(
        "            let mut request = SchedulerRequest::of(Scheduler::REQUEST_TASK_CANCEL);\n            // SAFETY: below",
        "            let mut request = SchedulerRequest::of(11); // CHIMI_REQUEST_TIMER_NOW\n            // SAFETY: below")]),
    combinators("the losers are neither cancelled nor drained", TASKS, [(
        "        if children.live > 0 {\n            if !children.cancel_live() {",
        "        if false {\n            if !children.cancel_live() {")]),
    combinators("Task.race waits for every child", TASKS, [(
        "} else if intrinsic == Intrinsic::TASK_RACE || kind == ControlKind::FAILURE {",
        "} else if kind == ControlKind::FAILURE {")]),
    combinators("Task.results stops at the first failure", TASKS, [(
        "} else if intrinsic == Intrinsic::TASK_RESULTS {",
        "} else if intrinsic == Intrinsic::TASK_RESULTS && kind == ControlKind::SUCCESS {")]),
    combinators("successes land in finishing order", TASKS, [(
        "                        *values.add(position as usize) = (*outcome).value;\n                    }\n                }",
        "                        *values.add((count - 1 - children.live) as usize) = (*outcome).value;\n                    }\n                }")]),
    # Sibling causes.
    combinators("sibling failures are dropped", TASKS, [(
        "if collect_siblings && (*outcome).kind == ControlKind::FAILURE {",
        "if false && collect_siblings && (*outcome).kind == ControlKind::FAILURE {")]),
    combinators("a sibling cause has no reason", RUST_CAUSES, [(
        "        set(marked, Slot::REASON, int(REASON_SIBLING));\n",
        "")]),
    combinators("a stored Err loses its cause", TASKS, [(
        "success(if failed { with_stored_cause(stored, control_cause(outcome)) } else { stored })",
        "success(stored)")]),
    combinators("an empty race parks instead of defecting", TASKS, [(
        "            if intrinsic == Intrinsic::TASK_RACE {\n                return defect(c\"Task.race needs at least one operation\", node);\n            }\n",
        "")]),
    # Typing.
    combinators("a function join keeps only the left failures", "src/typing/failure_sets.rs", [(
        "        let failures = self.join_failure_sets([left.failures, right.failures]);",
        "        let failures = left.failures;")]),
    combinators("function types never join", "src/typing/failure_sets.rs", [(
        "            return self.join_function_contracts(left_contract, right_contract, depth);",
        "            return None;")]),
    combinators("T4345 is never reported", "src/typing/durations.rs", [(
        "        is_race\n            && matches!(",
        "        false && is_race\n            && matches!(")]),
    combinators("std/task does not load std/result", "src/standard_tree.rs", [(
        "            let implied = (path == \"std/task\").then(|| \"std/result\".to_owned());",
        "            let implied: Option<String> = None;")]),
    combinators("a combinator's thunks skip the transfer check", "src/capability.rs", [(
        "                        | StandardFunction::TaskAll\n                        | StandardFunction::TaskResults",
        "                        | StandardFunction::TaskResults")]),
]
