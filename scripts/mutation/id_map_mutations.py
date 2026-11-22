"""Mutants of R4 step 4d-2: the task table is a hash map keyed by a cheap
hash of the task id.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/id_map_mutations.py
Results: scripts/mutation/id_map_results.txt.
"""

CRATE = "runtime/chimi-fiber"


def m(name, old, new, file="src/scheduler/id_map.rs"):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="")


# A worse hash leaves every lookup correct and only slower, so the tests
# that catch these check the spread itself, not the scheduler.
MUTATIONS = [
    m("every id hashes to zero",
      "    fn finish(&self) -> u64 {\n        self.0",
      "    fn finish(&self) -> u64 {\n        0"),
    m("an even constant drops the low bit",
      "const SPREAD: u64 = 0x9e37_79b9_7f4a_7c15;",
      "const SPREAD: u64 = 0x9e37_79b9_7f4a_7c14;"),
    m("the id is not multiplied",
      "        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SPREAD);",
      "        self.0 = self.0.rotate_left(5) ^ word;"),
    m("bytes are ignored",
      "            self.0 = (self.0.rotate_left(5) ^ u64::from(byte)).wrapping_mul(SPREAD);",
      "            let _ = byte;"),
    m("bytes are not chained",
      "            self.0 = (self.0.rotate_left(5) ^ u64::from(byte)).wrapping_mul(SPREAD);",
      "            self.0 ^= u64::from(byte).wrapping_mul(SPREAD);"),
    # The table is unordered, so what reports tasks sorts them by id.
    # "Program roots" is expected to survive: production starts one root
    # (process_abi.rs), so no test has several roots to terminate.
    m("the deadlock report keeps the table's order",
      "        parked.sort_by_key(|parked| parked.task);",
      "",
      file="src/scheduler.rs"),
    m("program roots are forced in the table's order",
      "        roots.sort_unstable();",
      "",
      file="src/scheduler/process.rs"),
    m("unrun contexts are released in the table's order",
      "        unrun_contexts.sort_unstable_by_key(|(task, _)| *task);",
      "",
      file="src/scheduler.rs"),
]
