"""Mutants of `chimi trace show`'s follow-ups (CHI-282): span lengths, runs
that share an execution id, and process names from `process_started`.

Run: scripts/mutation/mutate.py scripts/mutation/trace_show_followup_mutations.py
"""

CRATE = "compiler/chimi"

V = "src/trace_view.rs"


def mutant(name, edits):
    return dict(name=name, file=V, edits=edits, targets=["trace_show"], filter="")


MUTATIONS = [
    mutant("an untimed run shows lengths", [(
        "        self.timed |= time.is_some_and(|time| time != 0);",
        "        self.timed |= time.is_some();",
    )]),
    mutant("a completion's time is not kept", [(
        "                    self.spans[span].completed_ns = time;\n",
        "",
    )]),
    mutant("microseconds print as nanoseconds", [(
        "        ..1_000_000 => format!(\"{:.1} µs\", value / 1e3),",
        "        ..1_000_000 => format!(\"{nanoseconds} ns\"),",
    )]),
    mutant("seconds keep one decimal", [(
        "        _ => format!(\"{:.2} s\", value / 1e9),",
        "        _ => format!(\"{:.1} s\", value / 1e9),",
    )]),
    mutant("a rerun merges into the first run", [(
        "            let rerun = event == \"computation_started\"",
        "            let rerun = false && event == \"computation_started\"",
    )]),
    mutant("runs are not numbered", [(
        "                execution.run = Some(*run);",
        "                let _ = run;",
    )]),
    mutant("the start's process name is ignored", [(
        "                        incarnation.and_then(|id| self.process_names.get(&id).cloned())",
        "                        None",
    )]),
]
