"""Mutants of the scheduler's handle counts (CHI-308 step 1,
runtime/chimi-fiber/src/handles.rs and their use in the scheduler).

Run: scripts/mutation/mutate.py scripts/mutation/handle_count_mutations.py
Each must fail a chimi-fiber unit test. handle_count_c_mutations.py holds the
runtime.c side.
"""

CRATE = "runtime/chimi-fiber"

H = "src/handles.rs"
S = "src/scheduler.rs"


def unit(name, file, edits, test_filter=""):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=test_filter)


MUTATIONS = [
    unit("a drop never reports the last reference", H, [(
        "        counts.remove(&key);\n        true\n",
        "        counts.remove(&key);\n        false\n",
    )]),
    unit("a drop reports zero one reference early", H, [(
        "        *count -= 1;\n        if *count != 0 {",
        "        *count -= 1;\n        if *count > 1 {",
    )]),
    unit("a queue keeps an earlier run's keys", H, [(
        "            if queue.run != run {\n                queue.keys.clear();\n                queue.run = run;\n                return None;\n            }\n",
        "",
    )]),
    unit("a task key reads as a channel", H, [(
        "        if key & TASK_HANDLE_KEY != 0 {",
        "        if key & TASK_HANDLE_KEY == 0 {",
    )]),
    unit("a request does not carry the releases its fiber dropped", S, [(
        "        if !self.apply_task_releases(owner) {\n            return;\n        }\n",
        "",
    )]),
    unit("an idle carrier does not apply its queued releases", S, [(
        "        while let Some(released) = crate::handles::take() {\n            drained = true;",
        "        while let Some(released) = None::<Released> {\n            drained = true;",
    )]),
    unit("a run does not start the counts afresh", "src/scheduler/process_abi.rs", [(
        "    crate::handles::begin_run();\n",
        "",
    )]),
]
