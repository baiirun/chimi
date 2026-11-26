"""Mutants of the heap layout mirror (CHI-305 step 5, phase 0): a C change
the static asserts cannot see, and Rust mirror mistakes, each of which the
runtime_layout test must reject. C changes the asserts do see fail the C
compile instead (checked by hand when the block was blessed).

Run: scripts/mutation/mutate.py scripts/mutation/layout_mutations.py
Results: scripts/mutation/layout_results.txt.
"""

CRATE = "compiler/chimi"
R = "runtime/native/runtime.c"
LAYOUT = "../../runtime/chimi-runtime/src/heap/layout.rs"
TESTS = ["runtime_layout"]


def m(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], targets=TESTS)


MUTATIONS = [
    m("a value kind appended in C", R,
      "               VALUE_NATIVE_HANDLE } ValueKind;",
      "               VALUE_NATIVE_HANDLE, VALUE_NEW } ValueKind;"),
    m("a field added in an allocation header's padding", R,
      "  AllocationKind kind;\n  bool in_chunk;\n};",
      "  AllocationKind kind;\n  bool in_chunk;\n  bool pinned;\n};"),
    m("the mirror's task id is 32 bits", LAYOUT,
      "    pub task: u64,",
      "    pub task: u32,"),
    m("the mirror's control node is unsigned", LAYOUT,
      "    pub node: i64,",
      "    pub node: u64,"),
    m("the mirror's chunk flag is a byte", LAYOUT,
      "    pub in_chunk: bool,",
      "    pub in_chunk: u8,"),
    m("the mirror swaps two map node links", LAYOUT,
      "    pub left: *mut MapNode,\n    pub right: *mut MapNode,",
      "    pub right: *mut MapNode,\n    pub left: *mut MapNode,"),
]
