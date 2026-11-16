"""Mutants of releasing a task once no value names it (CHI-264 part 1).

Run: scripts/mutation/mutate.py scripts/mutation/task_release_mutations.py
Each mutant must turn the chimi-fiber scheduler tests red.
Results: scripts/mutation/task_release_results.txt.
"""

CRATE = "runtime/chimi-fiber"

SCHEDULER = "src/scheduler.rs"


def release(name, edits):
    return dict(name=name, file=SCHEDULER, edits=edits, targets=["lib"], filter="scheduler::tests")


MUTATIONS = [
    release("a task released before it ends is never removed", [(
        "        if record.handle_released {\n            self.released_terminal.push(task);\n        }\n",
        "")]),
    release("drive never removes released tasks", [(
        "            self.run(task);\n            self.remove_released_terminal();\n",
        "            self.run(task);\n")]),
    release("a released task keeps its result", [(
        "                    unsafe { release(context, word) };",
        "                    let _ = (release, context, word);")]),
    release("a released defected task gives up its defect code too", [(
        "            if let Some(TaskValue::Result(word)) = self.task_values.get(&task).copied() {",
        "            if let Some(TaskValue::Result(word) | TaskValue::Defect(word)) = self.task_values.get(&task).copied() {")]),
    release("a handle can be released twice", [(
        "        if record.process.is_some() || record.handle_released || live_root {",
        "        if record.process.is_some() || live_root {")]),
    release("a released terminal task is kept until the next drive", [(
        "            self.released_terminal.push(task);\n            self.remove_released_terminal();\n        }\n        Ok(())",
        "            self.released_terminal.push(task);\n        }\n        Ok(())")]),
    release("a live root can be released", [(
        "        if record.process.is_some() || record.handle_released || live_root {",
        "        if record.process.is_some() || record.handle_released {")]),
]
