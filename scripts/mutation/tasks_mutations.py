"""Mutants of `Task.start` and the combinators in Rust (CHI-305 step 5,
phase 7, sixth PR): the context a started child reads, the capability
transfer to it, and the combinators' bookkeeping of their children.
(combinator_mutations.py and forced_release_mutations.py cover the
combinators' outcomes.)

Run: scripts/mutation/mutate.py scripts/mutation/tasks_mutations.py
Results: scripts/mutation/tasks_results.txt.
"""

CRATE = "compiler/chimi"

TASKS = "../../runtime/chimi-runtime/src/intrinsics/tasks.rs"
TASK_ENTRY = "../../runtime/chimi-runtime/src/intrinsics/task_entry.rs"


def mutant(name, edits, targets, filter=""):
    return dict(name=name, file=TASKS, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    # Moved from compiler/chimi/tests/fixtures/native-heap-mutants.py with
    # task_reply (CHI-305 step 5, phase 8, seventh PR).
    dict(name="Task.result consumes the child's result", file=TASK_ENTRY, edits=[(
        "        fragment::copy_into_current(\n            current_carrier(),\n            (raw & !CHILD_RESULT_TAG)",
        "        fragment::adopt(\n            current_carrier(),\n            (raw & !CHILD_RESULT_TAG)",
    )], targets=["cli"], filter="native_task"),
    mutant("a child does not know its parent", [(
        "        (*context).parent = request.owner;\n",
        "",
    )], ["cli"], "native_a_receiver_handed_back_to_the_parent"),
    mutant("a child's exclusive captures are not transferred", [(
        "        if !chimi_runtime_transfer_to_child_v1(",
        "        if false && !chimi_runtime_transfer_to_child_v1(",
    )], ["cli"], "native_source_transfers_an_exclusive_receiver_to_a_child"),
    mutant("a combinator child's overflow is a request failure", [(
        "Scheduler::RESPONSE_STACK_OVERFLOW => Next::Outcome(defect(c\"Task stack overflow\", node), position),",
        "Scheduler::RESPONSE_STACK_OVERFLOW => Next::Interrupted(defect(c\"Task combinator scheduler request failed\", node)),",
    )], ["task_combinators", "cli"]),
    mutant("a removed child's position is not moved", [(
        "            *children.positions.add(found) = *children.positions.add(last);\n",
        "",
    )], ["task_combinators"]),
]
