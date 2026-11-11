# Phase 1 Regression Follow-up

This follows up findings 1 and 2 of [the Phase 1 baseline](phase-1-baseline.md):
the semantic pipeline regression (`ts-23b5b9`) and the cold-build regression
(`ts-1db9f0`). It records the profile, the fix for the first, and the
measurements for the second.

## Environment

The host and toolchain are the same as in the baseline: Apple M1 (4 performance
and 4 efficiency cores), macOS 26.6.2, Rust 1.97.1, Apple clang 21.0.0. Other
agents were building at the same time, so the 1-minute load average was 4 to 18.
Each row gives its load.

Under load, the benchmark process sometimes runs on the efficiency cores. When
it does, the whole run is about 2x slower and the result is bimodal. For
example, `f0f1e38` measured 43.7 ms and 87.3 ms on the local edit in two
consecutive runs. To handle this, the A/B ran each binary at least twice,
interleaved, with one process per case. Rows report the faster mode, and slow
runs are listed beside them.

## 1. Semantic pipeline (M7 cases)

### Profile

The bench was built with symbols (`CARGO_PROFILE_BENCH_STRIP=none
CARGO_PROFILE_BENCH_DEBUG=line-tables-only`). It was then sampled with macOS
`sample` for 30 s while it ran `--semantic-case local`:

```sh
cargo bench -p chimi --bench frontend --features frontend-benchmarks --no-run
$BENCH --warmups 1 --samples 40 --semantics-only --semantic-case local --bench &
sample $! 30 -f sample.txt
```

The top-of-stack leader was one closure: 10,518 samples, more than all of
`malloc` and `free` combined (about 4,500). The closure is the filter in
`typing::constants::constant_references` (`compiler/chimi/src/typing/constants.rs`).
`constants::check` runs at the end of every type check, both clean and
incremental. It built the constant reference graph for the whole project before
it looked at any constant. For each constant in each module, it also scanned
every resolved reference in the project. The cost is O(constants × references).
The generated M7 project has 2 constants per declaration (from `452ead3`, which
migrated the bench's module `let` to `const`), so the scan dominated every case.
It even dominated the local edit, which rechecks only one module.

Constant evaluation (`execution/constants.rs`) was not the cause. The type
pipeline that the bench and the language server use (`type_project`,
`CompilerSession::analyze_project`) never lowers or evaluates. Only
`verify_project` (`chimi check`), `build`, and `run` do.

### Fix

The fix is in `typing/constants.rs` only:

- The graph is only needed to name an `E5205` cycle among constants that have
  no usable type. It is now built lazily, with a `OnceCell`, the first time such
  a constant is found. A project with no type cycle never builds it.
- When the graph is built, it is linear. Constant-targeted references are
  grouped by file and sorted by start. Each initializer then takes a binary
  search plus the references in its own range.

`E52xx` diagnostics and where they are reported have not changed: `chimi check`
still evaluates constants. The specification was not amended.

### Results

Commands: three bench binaries, each built as
`CARGO_TARGET_DIR=$T cargo bench -p chimi --bench frontend --features frontend-benchmarks --no-run`
from `git archive` trees. `pre` is `f0f1e38` (before constants), `before` is
`4aad5ea` (`main` with the regression) and `after` is this fix. Each case was
run as:

```sh
$BENCH --warmups 5 --samples 40 --semantics-only --semantic-case <clean|local|fanout|no-op> --bench
```

The p50 and p95 values are in ms, from the fast mode.

| Workload | `pre` p50 / p95 | `before` p50 / p95 | `after` p50 / p95 | after vs before | after vs pre |
| --- | ---: | ---: | ---: | ---: | ---: |
| Clean type pipeline | 151.8 / 153.8 (load 10.7) | 328.9 / 330.7 (load 9.5) | 179.4 / 181.0 (load 13.9) | -45% | +18% |
| Local implementation edit | 43.7 / 44.1 (load 5.3) | 193.2 / 213.4 (load 3.6) | 43.4 / 43.6 (load 13.1) | -78% | -1% |
| Shared-contract fanout edit | 92.5 / 94.7 (load 11.3) | 259.2 / 335.2 (load 8.8) | 102.2 / 107.5 (load 5.4) | -61% | +10% |
| No-op reanalysis | n/a | 0.17 / 0.19 | 0.17 / 0.19 | 0 | n/a |

Slow-mode runs, for completeness: on the local edit, `after` measured 78.5,
78.6 and 87.2 ms p50 (p95 107 to 121 ms), and `pre` measured 86.0 and 87.3 ms
p50. `before` never ran faster than 193 ms.

| Allocation | `pre` | `before` | `after` |
| --- | ---: | ---: | ---: |
| Clean type pipeline allocated | 489.93 MB | 567.93 MB | 566.84 MB |
| Local edit allocated | 68.52 MB | 73.58 MB | 72.50 MB |
| Fanout edit allocated | 128.34 MB | 140.29 MB | 139.21 MB |

On fast cores, the local edit is back to its pre-constants time and well under
the 100 ms p95 editor target. The clean and fanout cases remain 10% to 18% above
`pre`, which is within the 20% threshold. The allocation difference (+16%) is
unchanged by the fix. It comes from typing the constants that the bench now
declares (`const` initializers carry type facts that the old module `let` did
not). This is real added work, not overhead.

## 2. Cold `chimi build` (two-module fixture)

### Measurement

The fixture is the build-cache test project (`app/main` imports `lib/answer`).
The release `chimi` with the fix above was used. There were 7 runs at a load of
13 to 16, each cold build in a fresh working directory. The runtime archive
compile was timed alone with the exact command that `backend/native.rs` runs,
against the embedded `runtime.c`, `runtime.h` and `chimi_fiber.h`:

```sh
$C build --root $D/project app/main -o $W/app            # cold, fresh $W
cc -std=c11 -O2 -I $INC -c $INC/runtime.c -o runtime.o    # also -O1, -O0
```

| Step | p50 |
| --- | ---: |
| Cold build, whole command | 791 ms |
| Warm build (all cache hits) | 90 ms |
| `runtime.c` at `-O2` (current, 4,373 lines) | 663 ms |
| `runtime.c` at `-O1` | 543 ms |
| `runtime.c` at `-O0` | 117 ms |
| `runtime.c` at `-O2`, baseline commit `27ad088` (568 lines), 5 runs | 140 ms |

The runtime compile is 84% of a cold build, and cold minus warm (700 ms) is
about the compile alone. From `27ad088` to now the compile grew by 523 ms, which
accounts for the 474 ms cold-build regression in the baseline. Clang's
`-ftime-trace` shows the cost spread over the translation unit: the inliner
CGSCC pipeline takes 235 ms and codegen 273 ms. No single function exceeds 32 ms
(`invoke` is the largest), so there is no one pathological function to fix. The
growth is inherent: the runtime now carries per-owner heaps, tasks, channels,
JSON, and debug rendering.

### Decision: not changed

- `-O1` saves only 120 ms (18% of the compile). It also slows the runtime that
  every native program executes, and none of the runtime benchmarks has been
  measured at `-O1`. This is not a clear win.
- Splitting the translation unit so it compiles in parallel would cut wall time
  on 8 cores. But it is a large refactor of a single-file runtime with static
  helpers shared across the file, so it is not low risk.
- The archive is already cached by source, target and `cc` identity. The cache
  lives under `<cwd>/target/.cache/native-v1`, though, so each new project or
  working directory pays the full compile again.

Recommended follow-up (not done here): build the runtime archive once per
compiler, not once per project. One way is to compile it at `cargo build` time,
as `libchimi_fiber.a` already is, and locate it next to the executable. The
other is to key a user-level cache by the existing runtime cache key. Either
way, an isolated cold build drops to about 130 ms (the measured cold build minus
the compile), with no change to runtime code generation.
