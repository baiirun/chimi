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
    m("the map never grows",
      "        if self.len.saturating_add(1).saturating_mul(2) > self.entries.len() && !self.grow() {",
      "        if self.entries.is_empty() && !self.grow() {"),
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
]
