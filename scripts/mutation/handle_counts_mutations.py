"""Mutants of the native handle counts in Rust (CHI-309 part 2,
runtime/chimi-runtime/src/handle_counts.rs), caught by its unit tests. The C
side's lock around them is mutated in handle_table_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/handle_counts_mutations.py
Results: scripts/mutation/handle_counts_results.txt.
"""

CRATE = "runtime/chimi-runtime"
COUNTS = "src/handle_counts.rs"
COUNTS_ABI = "src/handle_counts_abi.rs"


def m(name, old, new, file=COUNTS):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="handle_counts")


MUTATIONS = [
    m("a drop reports the last reference one early",
      "        if entry.references != 0 {",
      "        if entry.references > 1 {"),
    m("growing loses the old entries",
      "        for entry in old.into_iter().filter(|entry| entry.key != 0) {",
      "        for entry in old.into_iter().filter(|_| false) {"),
    m("removal does not shift a run back",
      "            if (next.wrapping_sub(home) & mask) >= (next.wrapping_sub(hole) & mask) {",
      "            if false {"),
    m("removal shifts an entry past its home",
      "            if (next.wrapping_sub(home) & mask) >= (next.wrapping_sub(hole) & mask) {",
      "            if true {"),
    m("an unknown key is counted",
      "        if entry.key != key {\n            return false;\n        }",
      ""),
    m("key 0 is counted",
      "        if key == 0 {\n            return true;\n        }",
      ""),
    m("a reset keeps the counts",
      "        self.entries.iter_mut().for_each(|entry| *entry = Entry::default());",
      ""),
    m("the C reset keeps the counts",
      "    unsafe { COUNTS.with(HandleCounts::reset) }",
      "",
      file=COUNTS_ABI),
    m("a probe runs off the end instead of wrapping",
      "            at = (at + 1) & self.mask();",
      "            at += 1;"),
    m("removal stops at the end instead of wrapping",
      "            next = (next + 1) & mask;",
      "            next += 1;"),
    m("removal starts past the end of the last entry",
      "        let mut next = (hole + 1) & mask;",
      "        let mut next = hole + 1;"),
    m("an entry is shifted back onto its own home's later run",
      "            if (next.wrapping_sub(home) & mask) >= (next.wrapping_sub(hole) & mask) {",
      "            if (next.wrapping_sub(home) & mask) > (next.wrapping_sub(hole) & mask) {"),
    m("a reset keeps the key count",
      "        self.entries.iter_mut().for_each(|entry| *entry = Entry::default());\n        self.len = 0;",
      "        self.entries.iter_mut().for_each(|entry| *entry = Entry::default());"),
    m("a removal keeps the key count",
      "            *slot = Entry::default();\n        }\n        self.len = self.len.saturating_sub(1);",
      "            *slot = Entry::default();\n        }"),
    m("the last hole is not cleared",
      "        if let Some(slot) = self.entries.get_mut(hole) {\n            *slot = Entry::default();\n        }\n        self.len",
      "        self.len"),
    # A map that never grows or fills completely (a new key left uncounted,
    # a threshold at full) or whose size is not a power of two (a fixed
    # growth step) makes a probe loop forever, so the unit tests time out
    # rather than fail; those mutants are left out, and the threshold
    # mutant here stops short of full.
    m("the map grows only at three quarters full",
      "        if self.len.saturating_add(1).saturating_mul(2) > self.entries.len() && !self.grow() {",
      "        if self.len.saturating_add(1).saturating_mul(4) > self.entries.len().saturating_mul(3) && !self.grow() {"),
]
