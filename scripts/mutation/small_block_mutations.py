"""Mutants of the small-block cache in Rust (CHI-305 step 5, phase 1;
runtime/chimi-runtime/src/heap/small_blocks.rs), caught by its unit tests.
The C it replaced was `small_block_take` and `small_block_give` in
runtime.c, which now call it.

Run: scripts/mutation/mutate.py scripts/mutation/small_block_mutations.py
Results: scripts/mutation/small_block_results.txt.
"""

CRATE = "runtime/chimi-runtime"
CACHE = "src/heap/small_blocks.rs"


def m(name, old, new):
    return dict(name=name, file=CACHE, edits=[(old, new)], targets=["lib"], filter="small_blocks")


MUTATIONS = [
    m("a reused block is not zeroed",
      "                    ptr::write_bytes(block.cast::<u8>(), 0, rounded);\n",
      ""),
    m("a full list keeps taking blocks",
      "            && (self.counts[class] as usize) < SMALL_BLOCK_CACHE\n",
      ""),
    m("a cached block is not counted",
      "            *cached_bytes += rounded as i64;\n            return;",
      "            return;"),
    m("a taken block stays counted",
      "                    *cached_bytes -= rounded as i64;\n",
      ""),
    m("the largest class is not cached",
      "(1..=SMALL_BLOCK_LIMIT).contains(&rounded)",
      "(1..SMALL_BLOCK_LIMIT).contains(&rounded)"),
    m("sizes are not rounded to the grain",
      "    let rounded = bytes.div_ceil(SMALL_BLOCK_GRAIN) * SMALL_BLOCK_GRAIN;",
      "    let rounded = bytes;"),
    m("the cache is used when it is off",
      "        if enabled && let Some(class) = class {",
      "        if let Some(class) = class {"),
]
