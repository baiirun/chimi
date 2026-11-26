"""Mutants of counting task handles in runtime.c (CHI-264 parts 2 and 3).

Run: scripts/mutation/mutate.py scripts/mutation/task_handle_mutations.py
Each mutant must turn the task-release tests red.
Results: scripts/mutation/task_handle_results.txt.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"


def handle(name, edits):
    return dict(name=name, file=RUNTIME, edits=edits, targets=["task_release"], filter="")


MUTATIONS = [
    handle("task handles are not counted", [(
        "  if (value->kind == VALUE_TASK) return value->as.task | CHIMI_TASK_HANDLE_KEY;\n",
        "")]),
    handle("a copied task handle is not retained", [(
        "  uint64_t key = counted_handle_key(value);\n  if (key) counted_handle_retain(key);\n}",
        "  uint64_t key = counted_handle_key(value);\n  if (key && !(key & CHIMI_TASK_HANDLE_KEY)) counted_handle_retain(key);\n}")]),
    handle("a freed allocation keeps its task handle counted", [(
        "  uint64_t key = counted_handle_key(value);\n  if (key) counted_handle_drop(key);\n}",
        "  uint64_t key = counted_handle_key(value);\n  if (key && !(key & CHIMI_TASK_HANDLE_KEY)) counted_handle_drop(key);\n}")]),
    handle("a dead task handle in a loop pass is not dropped", [(
        "      if (handles_live() && header->kind == ALLOCATION_VALUE) {\n        uint64_t key = counted_handle_key((ChimiValue *)(header + 1));\n        if (key) counted_handle_drop(key);",
        "      if (handles_live() && header->kind == ALLOCATION_VALUE) {\n        uint64_t key = counted_handle_key((ChimiValue *)(header + 1));\n        if (key && !(key & CHIMI_TASK_HANDLE_KEY)) counted_handle_drop(key);")]),
    dict(name="the scheduler ignores a task release", file="../../runtime/chimi-fiber/src/scheduler.rs",
         edits=[("                    let _ = self.release_task(TaskId(id));",
                 "                    let _ = id;")],
         targets=["task_release"], filter=""),
    handle("the memory report omits the task peak", [(
        "          chimi_scheduler_peak_task_records());",
        "          (size_t)0);")]),
]
