# Bump Allocation in Loop Regions: Design (R2 part 2, CHI-219)

This note settles how a loop region would allocate by bumping a pointer and
free a pass's garbage in bulk. It decides what happens to the values that
survive a pass, which is the hard part. The code follows in its own change,
which needs an Opus review because it is memory soundness.

## Why

[Native allocation part 1](native-allocation-r2.md) cut a runtime-path loop
pass to 7 allocations for `loop`. `loop-bind` (a `let` in the body) still
makes 13 a pass. Callgrind on a 200,000-pass `loop-bind` at `cd8d7f6`, about
4,400 instructions a pass:

| Function | Instructions a pass |
| --- | ---: |
| `allocate` (own work) | 962 |
| `release_allocation` | 598 |
| `small_block_take` and its `memset` | 571 |
| `promote_marked` and the pass's mark | 237 |
| `allocation_kind` | 90 |

That is about 2,450 instructions, or 190 per allocation over its life, and
more than half of the pass. Comparisons were another 300 a pass and are fixed
separately (PR #28).

## What the cost is made of

For every allocation, today's allocator:

1. rounds the size and pops a block from the size class's free list, or
   calls `calloc`;
2. zeroes the block;
3. links the header into its area's list and updates 9 counters (area,
   scope, heap, kind and global footprint);
4. at the pass boundary, walks the young list, and either relinks the
   header into the survivors or calls `release_allocation`, which checks for
   a channel endpoint, updates 4 counters and pushes the block back.

With bump allocation, steps 1 and 4 become a pointer increment and a bulk
reset, step 2 becomes one `memset` per chunk, and most of step 3's counters
can be added up once per pass.

## Decision: copy survivors at the pass boundary

A region's young area allocates from chunks the region owns, by bumping a
pointer. At a pass boundary the collector marks from the loop's state as it
does today. It then **copies** each marked young allocation into the
survivor area, which keeps today's per-allocation blocks. Finally it resets
the chunks for the next pass. Survivors, base areas, fragments and the
static area do not change.

Why copying is sound here, although the collector never moves values today:

- **Only the state is live.** At a pass boundary the pass's C frames have
  returned, and the loop frame holds just the state. Today's minor pass
  already frees every young allocation it did not reach from the state. Any
  other pointer into the young area would already be a use-after-free, so
  the root set the copy needs is exactly the one the collector already
  relies on.
- **Older values never point to younger ones.** Values are immutable, and
  list compaction allocates into the area of the list it updates. So no
  survivor, enclosing region or base value points into the young area, and
  only the copied values and the state need their pointers updated.
- **Address order is not observable.** `chimi_value_compare` orders
  callables, tasks, channel endpoints, inboxes and managed processes by
  address, but none of those types is equatable (`typing/properties.rs`), so
  no `==`, ordering or Map key sees that order. The pointer-keyed hash tables
  (fragment copies, capture transfers) live only within one copy.
- **Channel endpoint counts.** Moving a value is neither a copy nor a free,
  so a moved endpoint keeps its count. A dead endpoint must still be
  dropped. The pass end walks each chunk's headers in order (each records
  its size) and drops the unmarked endpoint values, as
  `release_allocation` does now.

Rejected alternative: keep values in place and keep a chunk while any of its
values survives. A pass usually keeps a small state, so each kept chunk would
hold a few live bytes. Survivors are reclaimed only by a full pass, so chunk
memory would grow by a chunk a pass until the next full pass. Copying keeps
the survivors compact and the young area empty after every pass.

## Shape of the change

- **Chunks.** A region keeps a list of fixed-size chunks (start at 16 KiB)
  and a bump pointer. After a pass it keeps the first chunk and returns the
  rest to a small global cache, so a steady loop allocates no chunks.
  Requests larger than a quarter of a chunk bypass the chunks and use
  today's per-allocation path, linked in a young large-object list.
- **Headers stay.** Each allocation keeps its header (area, bytes, kind,
  mark), because marking, `allocation_kind` and the collector read them. A
  young header needs no `next` link, because a chunk walk finds every
  allocation. On a copy, the old header's link field holds the forwarding
  address.
- **The copy.** A Cheney-style scan copies marked young allocations into
  survivor blocks. It then updates each copied allocation's child pointers
  through a pointer-slot visitor. That visitor is today's `mark_children`,
  changed to hand back each slot instead of only marking it, so marking and
  copying cannot disagree on what a kind points to.
- **What `allocate` decides.** Only the young area of the innermost region
  bumps. `allocation_target` (static constants, fragment copies and list
  compaction) and every other area keep the current path.
- **Accounting.** Per-allocation byte counts stay exact, so the memory
  report's `scoped_allocated_bytes_by_kind`, live bytes and collection counts
  keep their meaning. Footprint counts chunk bytes when chunks are taken and
  returned.
- **Profile.** `--profile-file` still charges each allocation to its node.

## Verification plan

- The existing reclamation, per-owner heap, fragment, channel release and
  memory report tests must pass unchanged.
- A debug build option poisons reset chunks (fills them with a fixed byte).
  CI runs the native language and cli tests with it, so a stale pointer into
  a reset chunk fails as a wrong value or a crash, not silently.
- New tests: a state that is a deep list, a map and a record, kept across
  passes, with values that share structure (to prove forwarding keeps
  sharing). Also a loop that sends and drops channel endpoints, a nested
  loop whose result survives into the outer pass, and an allocation larger
  than a quarter chunk.
- Mutants: skip a slot update, forget the forwarding address (duplicating a
  shared value), drop no dead endpoints, reset chunks before the copy, and
  treat a large allocation as a chunk allocation.
- Measure `loop-bind`, `collatz` and the allocation workloads in
  [native allocation part 1](native-allocation-r2.md), and check peak RSS.

The expected gain is most of the 2,450 instructions above, minus the copy of
the state, which is a few allocations a pass: roughly 30 to 40% off a
runtime-path loop pass. The change is worth shipping only if the measurement
shows at least 20%.
