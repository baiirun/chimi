"""Mutants of the console output queue (CHI-266, moved to Rust in CHI-308):
runtime/chimi-runtime/src/console.rs, killed by its unit tests.

The end-to-end mutants, through runtime.c, are in console_output_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/rust_console_mutations.py
"""

CRATE = "runtime/chimi-runtime"

CONSOLE = "src/console.rs"


def unit(name, edits):
    return dict(name=name, file=CONSOLE, edits=edits, targets=["lib"], filter="console::")


MUTATIONS = [
    unit("a failure also fails lines already written",
         [("            if line.end > self.resolved {", "            if true {")]),
    unit("a failure keeps the queued bytes",
         [("        self.resolved += self.queued() as u64;\n        self.bytes.clear();\n", "")]),
    unit("a write does not count toward the lines",
         [("        self.resolved += count as u64;\n", "")]),
    unit("a line ends before its last byte",
         [("        let end = self.resolved + self.queued() as u64;", "        let end = self.resolved + self.queued() as u64 - 1;")]),
    unit("a pending line reads as written",
         [("            Some(line) if self.resolved < line.end => LineState::Pending,\n", "")]),
    unit("a failed line reads as pending or written",
         [("            Some(line) if line.error != 0 => LineState::Failed(line.error),\n", "")]),
    unit("the queue grows instead of moving its bytes to the front",
         [("        if self.start != 0 && self.bytes.len() + bytes.len() > self.bytes.capacity() {",
           "        if false {")]),
    unit("an ended line is kept",
         [("        self.lines.retain(|line| line.id != id);", "")]),
]
