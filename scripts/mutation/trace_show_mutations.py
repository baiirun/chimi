"""Mutants of `chimi trace show` (Phase 9, T7): which records are accepted,
how spans form a tree, what the failure path folds, and which processes and
tasks are reported.

Run: scripts/mutation/mutate.py scripts/mutation/trace_show_mutations.py
"""

CRATE = "compiler/chimi"

T = "src/trace_view.rs"
P = "src/project.rs"


def mutant(name, file, edits, targets=("trace_show",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("any schema is accepted", T, [(
        '            if record.text("schema") != Some(SCHEMA) {',
        '            if record.text("schema").is_none() {',
    )]),
    mutant("an outcome after events is dropped", T, [(
        "                    Some(last) => last.outcome = Some(outcome),",
        "                    Some(_) => {}",
    )]),
    mutant("a returned process is reported", T, [(
        '                if cause != "returned" {',
        "                if true {",
    )]),
    mutant("a terminal task is still running", T, [(
        '                    "task_cleanup_completed" | "task_terminal" => "ended",',
        '                    "task_cleanup_completed" => "ended",',
    )]),
    mutant("every task is listed", T, [(
        '            .filter(|(_, task)| task.state != "ended")',
        "            .filter(|_| true)",
    )]),
    mutant("no channel for a parked task", T, [(
        '                if let Some(channel) = task.channel.filter(|_| task.state == "parked") {',
        '                if let Some(channel) = task.channel.filter(|_| false) {',
    )]),
    mutant("a dependency call never finishes", T, [(
        '            outcome: (event == "dependency_called").then(|| "success".to_owned()),',
        "            outcome: None,",
    )]),
    mutant("every span is a root", T, [(
        "            Some(&parent) => self.spans[parent].children.push(span),",
        "            Some(_) => self.roots.push(span),",
    )]),
    mutant("every failure is where it started", T, [(
        "                if failed_children.is_empty() {",
        "                if true {",
    )]),
    mutant("succeeded children are shown by default", T, [(
        "        let shown: &[usize] = if all {",
        "        let shown: &[usize] = if true {",
    )]),
    mutant("--all still folds", T, [(
        "        let shown: &[usize] = if all {",
        "        let shown: &[usize] = if false {",
    )]),
    mutant("an unfinished span reads as a success", T, [(
        '            None => report.push_str("  (did not finish)"),',
        "            None => {}",
    )]),
    mutant("columns are zero-based", P, [(
        '            Some(format!("{path}:{}:{}", position.line + 1, position.character + 1))',
        '            Some(format!("{path}:{}:{}", position.line + 1, position.character))',
    )]),
    mutant("a channel wait does not name its channel", T, [(
        '            "channel_receive_parked" | "channel_send_parked" => {',
        '            "channel_receive_parked_never" => {',
    )]),
    mutant("an outcome goes to the newest execution", T, [(
        "            last_slot = Some(slot);",
        "            last_slot = Some(trace.executions.len() - 1);",
    )]),
    mutant("indentation never stops growing", T, [(
        "    if depth <= MAX_INDENT {",
        "    if true {",
    )]),
    mutant("a source that misses places is not flagged", "src/main.rs", [(
        "            if with_source && missed.get() {",
        "            if with_source && false {",
    )]),
]
