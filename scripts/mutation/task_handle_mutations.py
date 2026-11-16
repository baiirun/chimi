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
        "  if (value->kind == VALUE_TASK) return value->as.task | TASK_HANDLE_KEY;\n",
        "")]),
    handle("a task's last handle sends a channel release", [(
        "  if (key & TASK_HANDLE_KEY)\n",
        "  if (false)\n")]),
    handle("a copied task handle is not retained", [(
        "    uint64_t key = counted_handle_key(duplicate);\n    if (key) channel_endpoint_retain(key);",
        "    uint64_t key = counted_handle_key(duplicate);\n    if (key && !(key & TASK_HANDLE_KEY)) channel_endpoint_retain(key);")]),
    handle("a freed allocation keeps its task handle counted", [(
        "    uint64_t key = counted_handle_key((ChimiValue *)(header + 1));\n    if (key) channel_endpoint_drop(key);\n  }\n  if (header->area->heap)",
        "    uint64_t key = counted_handle_key((ChimiValue *)(header + 1));\n    if (key && !(key & TASK_HANDLE_KEY)) channel_endpoint_drop(key);\n  }\n  if (header->area->heap)")]),
    handle("a dead task handle in a loop pass is not dropped", [(
        "      if (handles_live() && header->kind == ALLOCATION_VALUE) {\n        uint64_t key = counted_handle_key((ChimiValue *)(header + 1));\n        if (key) channel_endpoint_drop(key);",
        "      if (handles_live() && header->kind == ALLOCATION_VALUE) {\n        uint64_t key = counted_handle_key((ChimiValue *)(header + 1));\n        if (key && !(key & TASK_HANDLE_KEY)) channel_endpoint_drop(key);")]),
    dict(name="the scheduler ignores a task release", file="../../runtime/chimi-fiber/src/scheduler.rs",
         edits=[("                request.status = if self.release_task(TaskId(request.task)).is_ok() {",
                 "                request.status = if true {")],
         targets=["task_release"], filter=""),
    handle("the memory report omits the task peak", [(
        "          chimi_scheduler_peak_task_records());",
        "          (size_t)0);")]),
]
