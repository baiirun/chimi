# Bump Allocation in Loop Regions: Results (R2 part 2, CHI-219)

This change implements [the design](native-bump-allocation-design.md). A loop
region allocates a pass's small values by bumping a pointer through 16 KiB
chunks, and at the pass boundary it moves what the state still reaches into the
survivor area and resets the chunks for the next pass.

## What it buys

Release builds on a quiet 4-core cloud machine (load under 1.5), median of 7 to
11 interleaved runs, nanoseconds a loop pass:

| Workload | main (`513f153`) | Bump, always | Bump, as shipped |
| --- | ---: | ---: | ---: |
| `loop-bind`, 10M passes (most of a pass dies) | 230 | 164 | 179 |
| Map-building loop, 1M passes (about 30% of a pass survives) | 1,050 | 1,160 | 1,086 |

`loop-bind` is 22% faster than main, which clears the design's 20% bar. The
map-building loop puts one entry into a 1,000-entry map each pass, and that
pass's new path through the tree survives. Always bumping made it 11 to 13%
slower than main, because moving the survivors costs more than bumping saves.
As shipped it is within noise of main (+1 to 3%). Peak RSS of both programs is
unchanged (14.7 MB).

## Linking a pass that will mostly survive

A region now chooses per pass. When over a quarter of the last pass's young
bytes survived, the next pass links its allocations to the young list as main
does, and its survivors are relinked, not moved. After a mostly dead pass it
bumps again. The first pass of a region always bumps. A pass that bumped
nothing skips moving and resetting its chunks.

The quarter comes from these two workloads: `loop-bind` keeps 11% of its pass
and the map loop keeps 30%. The check costs `loop-bind` some of its gain (164
against 179 ns), and only part of that is the extra instructions (about 1.6%
under callgrind). The rest moves when unrelated code moves, so it is likely
code layout.

## What the review found

The Opus soundness review of the change found two bugs, both now fixed with a
test:

- A `Debug.crash` message built inside a loop pass lived in the pass's chunk,
  which the region frees when the defect leaves it, so the message came out
  empty. Defect messages are now allocated outside the heap, like the numeric
  ones. The same bug was latent on main: the payload happened to survive the
  block's reuse.
- `peak_footprint_bytes` was under-reported. The chunk counters are added at
  the pass boundary, so a child task's heap freed mid-pass lowered the global
  footprint before the pass's bytes were counted. The peak now includes the
  bytes still waiting to be counted.

## Known limit

Each open region keeps at least one 16 KiB chunk, and the memory report counts
allocations, not chunks. A recursion whose every frame runs a loop pass that
recurses again holds one chunk per frame: at depth 2,000 that is 36.5 MB of RSS
against 13 MB on main, with the same reported footprint. Ordinary nesting (a
loop in a loop, a few deep) is unaffected. Smaller first chunks would fix this
if it matters.

## Verification

- A checking build (`-DCHIMI_POISON_YOUNG_CHUNKS`) fills and frees every chunk
  at every boundary. The native heap, list fuzz and process heap tests build
  with it under AddressSanitizer, so a pointer kept into a reset chunk fails
  as a use after free.
- New tests cover a state of shared records in a list and a map, a compacted
  list window, a nested loop, a large text, task results kept across passes,
  a loop that flips between linking and bumping, a value reached twice (moved
  once), a crash message built in a loop, and the peak footprint with a child
  task.
- `scripts/mutation/bump_allocation_mutations.py` holds 10 mutants: skipped
  slot, list and cause rewrites, a forgotten forwarding address, chunks reset
  before the move, a root left in the chunk, counters never added, and the linking switch
  never turning on or never turning off. The chunk mutants (a large allocation
  bumped, a kept chunk not zeroed, dead endpoints not dropped, a pass that
  used one chunk) moved to `scripts/mutation/young_mutations.py` with the
  young chunks themselves.
