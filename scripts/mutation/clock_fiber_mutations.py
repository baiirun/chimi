"""Mutants of chimi_scheduler_run's task and channel release hooks, which the
native runtime uses since the Clock.sleep slice (ts-6ccbc9 decision 6).

Run: scripts/mutation/mutate.py scripts/mutation/clock_fiber_mutations.py
Each mutant must turn the chimi-fiber release tests red: they run every
release scenario through both entry points and require the same releases.
Results: scripts/mutation/clock_results.txt.
"""

CRATE = "runtime/chimi-fiber"

ABI = "src/scheduler/process_abi.rs"


def release(name, edits):
    return dict(name=name, file=ABI, edits=edits, targets=["lib"], filter="release_tests")


MUTATIONS = [
    release("run drops the task value hook", [(
        "        release_task_value: config.release_task_value,",
        "        release_task_value: None,")]),
    release("run drops the channel value hook", [(
        "        release_channel_value: config.release_channel_value,",
        "        release_channel_value: None,")]),
    release("run drops the unrun context hook", [(
        "        release_task_context: config.release_task_context,",
        "        release_task_context: None,")]),
    release("run releases the outcome value it hands out", [(
        "                self.hand_out_outcome_value(root, outcome);\n",
        "")]),
]
