# Native Allocation, Part 1 (R2, CHI-91)

R2 in the [language completion plan](../plans/language-completion-plan.md) is
allocation speed in the native runtime. This part takes the three candidate
wins named in CHI-91 (`ts-bad344`), each measured on its own. It does not
change the allocator itself; that is the next part.

## Workloads

Both programs run 10,000,000 passes of a `loop` with `--trace=off`.

`loop` (the CHI-91 workload):

```chimi
pub fn main() -> Int {
  loop count = 0 {
    case count {
      10000000 -> break count
      _ -> count + 1
    }
  }
}
```

`loop-bool` matches on a comparison instead, so each pass also produces a
`Bool`:

```chimi
pub fn main() -> Int {
  loop count = 0 {
    case count >= 10000000 {
      True -> break count
      False -> count + 1
    }
  }
}
```

## Results

Each column is a compiler built from one more change than the column before
it, and each row is 9 interleaved runs per compiler. Values are the p50 wall
time per pass; peak RSS was 14,600 KB (`loop`) and 14,680 KB (`loop-bool`)
for every compiler.

| Workload | `main` (`1efc595`) | + block clone | + match array | + shared Void and Bool | Total change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `loop` | 149 ns | 126 ns | 116 ns | 116 ns | -22% |
| `loop-bool` | 242 ns | 214 ns | 194 ns | 186 ns | -23% |

The three changes:

1. **Block clone.** A block (`chimi_rt_sequence`) cloned its locals on every
   evaluation, even when no step binds. It now clones at the first `let`, so a
   loop body such as `{ case ... }` allocates no environment. Two allocations
   a pass fewer.
2. **Match array.** `chimi_rt_match` allocated its array of subject values on
   every evaluation. Up to eight subjects now live on the C stack. One
   allocation a pass fewer.
3. **Shared Void and Bool.** `Void`, `True` and `False` are each one value in
   the static area, which is never collected, copied or freed. Comparisons
   and effects no longer allocate their result value. It does nothing for
   `loop`, and saves one allocation a pass in `loop-bool`.

Callgrind on a 200,000-pass `loop` shows 10 allocations a pass before and 7
after, and 2,895 instructions a pass before and 2,337 after.

## Comparisons without a heap work stack

Every `==`, `<`, `>=` and Map key lookup walks its values with an explicit
work stack, so that deep values never use host stack. That stack was
allocated with `realloc` and released with `free` on every comparison, even
for two integers: about 300 instructions, or 7% of a `loop-bind` pass. Its
first 32 steps now live in the comparing function's frame, and only longer
walks move to the heap.

| Workload | Before (`cd8d7f6`) | After |
| --- | ---: | ---: |
| `loop-bind` | 240 ns | 211 ns |

10,000,000 passes, 9 interleaved runs, p50, with a load average of about 2.

## What is left

About 60% of the remaining instructions in `loop` are the allocator:
`allocate`, `release_allocation`, `small_block_take`, the `memset` that zeroes
a reused block, and the minor collection at each pass boundary. Each
allocation costs about 170 instructions over its life. Of the 7 allocations
left, 4 are success controls (`ChimiControl`) that hold a different value
each pass, so sharing them is not possible. The candidate for part 2 is bump
allocation in the young area of a region, which turns allocation into a
pointer increment and frees a pass's garbage in bulk. That changes the
collector, so it needs its own design and review.

## Method

- Host: Intel Xeon at 2.30 GHz, 4 cores, Ubuntu 24.04, gcc 13.3 as `cc`,
  Rust 1.97.0. The 1-minute load average was 0.8 to 0.9.
- Each compiler: `cargo build --release -p chimi --bins`, which prebuilds the
  runtime archive at `-O2`.
- Each program: `chimi build --root <dir> app/main -o app`, then
  `app --chimi-runner --trace=off --`. Wall time is measured around the
  process; peak RSS is `ru_maxrss` from `wait4`.
- Instruction counts: `valgrind --tool=callgrind` on the same program with
  200,000 passes.
