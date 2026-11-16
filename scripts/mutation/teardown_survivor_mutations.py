"""Mutants of the release of what teardown survivors hold (CHI-223).

Run: scripts/mutation/mutate.py scripts/mutation/teardown_survivor_mutations.py
Each mutant must turn the chimi-fiber process tests red.
Results: scripts/mutation/teardown_survivor_results.txt.
"""

CRATE = "runtime/chimi-fiber"

PROCESS = "src/scheduler/process.rs"
FILTER = "releasing_survivors"


def survivor(name, edits):
    return dict(name=name, file=PROCESS, edits=edits, targets=["lib"], filter=FILTER)


MUTATIONS = [
    survivor("queued messages stay held", [(
        "                    Entry::Message(message) => self.discard_message(message),\n"
        "                    Entry::Notice(_) => self.release_message(entry.handle()),\n"
        "                }\n"
        "            }\n"
        "        }\n"
        "        for record in",
        "                    Entry::Message(_) => {}\n"
        "                    Entry::Notice(_) => self.release_message(entry.handle()),\n"
        "                }\n"
        "            }\n"
        "        }\n"
        "        for record in")]),
    survivor("queued notices stay held", [(
        "                    Entry::Notice(_) => self.release_message(entry.handle()),\n"
        "                }\n"
        "            }\n"
        "        }\n"
        "        for record in",
        "                    Entry::Notice(_) => {}\n"
        "                }\n"
        "            }\n"
        "        }\n"
        "        for record in")]),
    survivor("monitor records stay held", [(
        "        for record in std::mem::take(&mut self.kernel.monitors).into_values() {\n"
        "            self.release_message(record.handle());\n",
        "        for record in std::mem::take(&mut self.kernel.monitors).into_values() {\n"
        "            let _ = record;\n")]),
    survivor("monitor records are released but kept, so teardown releases them again", [(
        "        for record in std::mem::take(&mut self.kernel.monitors).into_values() {\n"
        "            self.release_message(record.handle());\n",
        "        let handles: Vec<u64> = self.kernel.monitors.values().map(|r| r.handle()).collect();\n"
        "        for handle in handles {\n"
        "            self.release_message(handle);\n")]),
    survivor("the mailbox is copied, not taken, so teardown releases it again", [(
        "            let mailbox = std::mem::take(&mut record.mailbox);",
        "            let mailbox = record.mailbox.clone();")]),
]
