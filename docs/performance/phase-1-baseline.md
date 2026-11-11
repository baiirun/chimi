# Phase 1 Performance Baseline

This is the phase-boundary performance sweep after Phase 1 (core language)
landed at `65172a6`. It re-runs the recorded baselines in this directory on the
current tree, reports every metric that moved by more than 20%, and records a
new baseline for later phases. Nothing in the compiler or runtime was changed.

## Environment

- Host: Apple M1, 8 logical cores, 16 GiB, macOS 26.6.2 (Darwin 25.6.0)
- Toolchain: Rust 1.97.1 (the older baselines used 1.92), release profile,
  Cranelift 0.131.3, runtime ABI 15
- Source: `main` at `65172a6`. The working tree later advanced to `389e2e2`
  (seven commits that touch `semantic.rs`, `typing.rs`, tests, and docs; none
  touches the runtime, the ABI, or the fiber crate). The release binaries were
  built before the final measurements and may include some of them.
- Build directory: a separate `CARGO_TARGET_DIR` (`$P/target`), so concurrent
  agents did not share artifacts. `chimi build` caches under `<cwd>/target`, so
  each cold build ran in a fresh working directory.
- Statistic: median of at least 5 runs unless a row says otherwise.

## Load

Other agents were building at the same time. The first pass ran at a 1-minute
load average of 12 to 34 and is **noisy**; it is not used below, except where a
row says so. The reported rows were re-measured after the load fell. Each row
names its load average (1-minute, sampled before each run, median).

| Measurement set | Load | Status |
| --- | ---: | --- |
| Compiler throughput, forcing programs, lab A/B | 4.4 to 4.6 | clean |
| Direct-integer benchmark | 3.9 | clean |
| M7 semantic cases (one process per case) | 3.8 to 5.6 | clean |
| M7 syntax workloads | 5.6 rising to 10 | syntax p50 usable, p95 noisy; the semantic rows of this run are noisy and discarded |
| Two-module build-cache fixture | 4.8 to 5.2 (a first run at 10 to 12 was discarded) | clean |
| First pass of everything | 12 to 34 | noisy, discarded |

## Method and reproduction

Let `P` be the scratchpad directory and `C=$P/target/release/chimi`.

```sh
CARGO_TARGET_DIR=$P/target cargo build --release -p chimi --bins
```

Build the programs. Run the cold builds from an empty directory so the cache is
isolated; run the warm builds from the same directory twice.

```sh
$C build --root programs/chimi-trace-summary app/main -o $W/ts/chimi-trace-summary
$C build --root programs/concurrency-pipeline app/main -o $W/cp/concurrency-pipeline
$C build prototypes/golden/valid/dependencies_and_execution.chimi -o $W/co/checkout
$C build --root compiler/chimi/tests/fixtures/project-host-io app/main -o $W/hio/exe
```

Inputs. The recorded checkout input is the trace and outcome of the checkout
program (5 records, 1,699 B). The large input is the baseline's generator: 100,000
`operation_completed` events with sequences 0 to 99,999, execution `large`,
computation `main`, operation `call`, then one success outcome, 13,988,951 B.
The streaming input is 100,000 lines of `{"value":1}`, 1,200,000 B.

```sh
$W/co/checkout --chimi-runner --trace=boundaries --telemetry-file=t.jsonl --outcome-file=o.jsonl --
cat t.jsonl o.jsonl > checkout.execution.jsonl
rm -f rep.json   # the runner refuses to overwrite an existing report (exit 74)
/usr/bin/time -l $W/ts/chimi-trace-summary --chimi-runner --trace=off \
  --memory-report-file=rep.json -- large.jsonl
```

Compiler throughput uses `/usr/bin/time -l` around `chimi check` and
`chimi build`. Direct integer:

```sh
$C build prototypes/native-direct-lowering/integer-arithmetic.chimi -o int
int --chimi-runner --execution-id=x --trace=<mode> --telemetry-file=/dev/null \
  --benchmark-repeats=<10000|5000|200> --      # stderr is ns per execution
```

M7 frontend (the recorded command, then one process per semantic case):

```sh
cargo bench -p chimi --bench frontend --features frontend-benchmarks -- --warmups 5 --samples 40
$BENCH --warmups 5 --samples 40 --semantics-only --semantic-case <clean|no-op|local|fanout> --bench
```

Concurrency lab gates and an interleaved A/B against `2183184` (the commit that
recorded the baseline numbers; it was extracted with `git archive` and built in
a separate target directory):

```sh
cargo test --release -p chimi --test cli -- --ignored concurrency_lab_performance
$C_LAB performance          # 7 interleaved runs per binary, median per metric
```

## Results against the baselines

A negative change is an improvement. Rows over +20% are marked **REGRESSION**.

### Trace summary, 100,001 records (`chimi-trace-summary-baseline.md`, dense-slot row)

Load 4.5. The baseline measurement ran on commit `45aca20`; this run used the
same generated input shape and the same trace-off, memory-report invocation.

| Metric | Baseline | Now | Change |
| --- | ---: | ---: | ---: |
| Wall time (5 runs) | 1.91 s | 1.387 s | -27% |
| User plus system CPU time | n/a | 1.38 s | n/a |
| Throughput | 52,445 records/s | 72,200 records/s | +38% |
| Process peak RSS | 2,080,768 B (earlier row; the last row records none) | 1,753,088 B | -16% |
| Scoped total allocation | 3,705,770,639 B | 3,757,874,467 B | +1.4% |
| Scoped peak live | 37,903 B | 43,038 B | +13.5% |
| Scoped retained | 1,069 B | 1,160 B | +8.5% |
| Scoped collections | 100,002 | 100,003 | +1 |
| Unscoped allocated | not recorded | 1,070,550 B | n/a |

The wall-time gain is not attributed to a commit: the baseline machine state
(OS 25.5, Rust 1.92, load) differs. Allocation per record is 37,579 B, under the
45,000 B gate in `compiler/chimi/tests/cli.rs`. Peak live is under the 50,000 B
gate, with 14% headroom.

### Trace summary on the recorded checkout input and the concurrency pipeline

Load 4.5, 9 runs each. There is no recorded baseline for these two; they are
the new baseline.

| Program | Wall p50 | Peak RSS | Allocated | Peak live | Retained | Collections | Unscoped |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| trace-summary, 5 records | 4 ms | 1,753,088 B | 185,981 B | 48,661 B | 1,355 B | 7 | 1,070,857 B |
| concurrency-pipeline (result 25) | 5 ms | 1,900,544 B | 7,232 B | 3,128 B | 80 B | 3 | 8,040 B |

The 5-record peak live of 48,661 B is not below the 50,000 B budget by much,
but that gate is defined on the 1,025-record input, not on this one.

### Native streaming memory (`native-streaming-memory.md`)

Load 4.5, fixture `project-host-io` (same program as the baseline apart from
the Phase 1 syntax change), 100,000 records, trace off, stdout piped.

| Metric | Baseline | Now | Change |
| --- | ---: | ---: | ---: |
| Scoped total allocation | 3,004,100,032 B | 179,100,032 B | -94% |
| Scoped peak live | 30,464 B | 6,401 B | -79% |
| Scoped retained | 455 B | 493 B | +8% |
| Collections | 100,001 | 100,002 | +1 |
| Peak RSS | 1,769,472 B | 1,622,016 B | -8% |
| Wall time | 1.55 s | 0.187 s | -88% |

The allocation drop is large enough that the accounting may have changed with
the per-owner heap work (`0d439c9`, `5515ce3`); this sweep did not verify that.
The trace-summary rows above use the same counters and moved by under 2%, so
they are the safer comparison.

### Direct integer (`native-direct-integer-baseline.md`)

Load 3.9. Only the direct-lowering side can be reproduced: the helper side was
measured against a prototype executable that no longer exists.
Samples are 40, 40 and 100 for off, boundaries and full.

| Trace mode | Baseline direct p50 | Now p50 | Change | Now p95 |
| --- | ---: | ---: | ---: | ---: |
| Off | 175 ns | 138 ns | -21% | 388 ns |
| Boundaries | 4,088 ns | 3,047 ns | -25% | 3,854 ns |
| Full | 515,325 ns | 398,275 ns | -23% | 708,780 ns |

The p95 values are wider than the baseline's (181, 4,104, 518,505 ns). That is
consistent with a loaded machine rather than a code change; p50 is the
comparison.

### Concurrency lab (`concurrency-kernel-conformance-baseline.md`)

The ignored gate test `concurrency_lab_performance_gates_hold` passes (checked
at load 34 and again at load 4.5). The table is the median of 7 interleaved
runs per binary at load 4.4; "A/B base" is the lab built at `2183184`.

| Metric | Doc baseline | A/B base | Now | Change vs doc | Gate |
| --- | ---: | ---: | ---: | ---: | ---: |
| Executable size | 1,644,896 B | 1,650,112 B | 2,269,984 B | **+38% REGRESSION** | recorded only |
| Cold process start p95 | 2,215,541 ns | 1,931,458 ns | 1,852,916 ns | -16% | < 25,000,000 |
| Trace-off rendezvous | 265 ns/msg | 222 | 225 | -15% | < 5,000 |
| Boundary rendezvous | 263 ns/msg | 217 | 220 | -16% | < 3x |
| Full rendezvous | 418 ns/msg | 368 | 380 | -9% | < 5x |
| Parked resident delta | 4,784,128 B | 4,751,360 B | 4,866,048 B | +1.7% | < 33,554,432 |
| Retained resident delta | 753,664 B | 737,280 B | 901,120 B | +19.6% (+22% vs A/B base) | < 8,388,608 |
| Reserved stack | 16,777,216 B | 16,777,216 B | 16,777,216 B | 0 | exact |

The same A/B at load 22 showed cold start +84% and trace-off +37%; both vanished
at load 4.4, so they were load noise, not regressions.

### Compiler throughput on the forcing programs

Load 4.5. `check` is 7 runs; cold build is 5 runs, each in a fresh working
directory; warm build is 7 runs against a populated cache. There is no recorded
baseline for these programs.

| Program | `check` p50 | Build, cold | Build, warm | Peak RSS (cold / warm) |
| --- | ---: | ---: | ---: | ---: |
| chimi-trace-summary | 9 ms | 828 ms | 90 ms | 77.8 MB / 44.5 MB |
| concurrency-pipeline | 6 ms | 826 ms | 104 ms | 78.0 MB / 61.4 MB |

### Build cache (`native-build-cache-baseline.md`, not in the sweep list; run for context)

Load 4.8 to 5.2, the two-module fixture from the build-cache CLI test, 12 runs.

| Build class | Baseline p50 | Now p50 | Change |
| --- | ---: | ---: | ---: |
| Cold, isolated cache | 293.860 ms | 768 ms | **+161% REGRESSION** |
| Identical rebuild, all hits | 73.627 ms | 78 ms | +6% |
| Imported value edit | 103.615 ms | 105 ms | +1% |

### Compiler incrementality (`compiler-m7-baseline.md`)

Semantic cases ran one process each at load 3.8 to 5.6, 5 warmups, 40 samples.
The generated project is slightly larger than the baseline's (1,204,992 B and
10,318 types, against 1,184,512 B and 10,245 types), so the comparison is
approximate by about 2%.

| Workload | Baseline p50 | Now p50 | Change | Baseline p95 | Now p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Clean resolution | 105.09 ms | 104.68 ms | -0.4% | 106.21 ms | not reported |
| Clean type pipeline | 153.35 ms | 328.29 ms | **+114% REGRESSION** | 154.73 ms | 332.15 ms |
| No-op reanalysis | 0.18 ms | 0.17 ms | -4% | 0.20 ms | 0.19 ms |
| Local implementation edit | 34.03 ms | 194.12 ms | **+470% REGRESSION** | 34.53 ms | 197.87 ms |
| Shared-contract fanout edit | 78.47 ms | 257.88 ms | **+229% REGRESSION** | 79.88 ms | 273.59 ms |

| Allocation | Baseline | Now | Change |
| --- | ---: | ---: | ---: |
| Clean type pipeline allocated | 491.24 MB | 567.93 MB | +16% |
| Clean type pipeline peak live | 74.82 MB | 79.37 MB | +6% |
| Clean retained analysis | 72.86 MB | 77.72 MB | +7% |
| Local edit allocated | 63.31 MB | 73.58 MB | +16% |
| Local edit peak live | 12.63 MB | 13.79 MB | +9% |
| Fanout edit allocated | 122.09 MB | 140.29 MB | +15% |

The local edit now misses the provisional 100 ms p95 target (197.87 ms), which
the baseline met. Incrementality still works: the local edit reparses and
relowers one module, rechecks one, and reuses 86,700 of 87,040 type facts. The
cost is time, not invalidation breadth: a local edit now takes 59% of a clean
check where it took 22%.

Syntax and editor workloads (load 5.6 rising to 10, p50 only; p95 is noisy):

| Workload | Baseline p50 | Now p50 | Change |
| --- | ---: | ---: | ---: |
| Typical valid (58,297 B) | 1.12 ms | 1.26 ms | +12% |
| Prefix editor reparse | 2.24 ms | 2.25 ms | +0.4% |
| Middle-brace editor reparse | 2.29 ms | 2.35 ms | +3% |
| Malformed record colon | 2.25 ms | 2.26 ms | +0.4% |
| Incomplete pipeline | 2.25 ms | 2.30 ms | +2% |

Allocation and retained CST bytes for the syntax workloads match the baseline
to within 1%. `cst.project-storage` retains 54,697,984 B (24.42 retained bytes
per source byte, baseline 24.85).

## Findings

### 1. Compiler semantic pipeline is 2x to 6x slower (M7 cases)

The clean type pipeline is +114%, the local edit +470% and the fanout edit
+229%. Resolution alone and the no-op reanalysis are unchanged, so the added
time is in typing or lowering, not in resolution, parsing, or snapshot reuse.

Bisect. I built the frontend benchmark at several commits and ran the local-edit
case (15 samples; load varied, so treat it as a coarse split):

| Commit | Clean p50 | Local-edit p50 | Load |
| --- | ---: | ---: | ---: |
| `3f12355` (first commit with the M7 cases) | 260 ms | 62 ms | 7 to 10 |
| `f0f1e38` | 163 ms | 44 ms | 31 to 33 |
| `e88cf9e` | 311 ms | 193 ms | 5.8 to 6.5 |
| `0f22e4f`, `3ab8577`, `42859e2`, `0d439c9` (in history order) | 311 to 318 ms | 192 to 236 ms | 8 to 25 |

The step lies in `f0f1e38..e88cf9e`. Every later commit I sampled is flat. The
bench does not build at `452ead3`, `b0f32af` or `1511d38` (the three commits in
that range that precede `e88cf9e`), so the split cannot be narrowed further
with this harness. The likely cause is `452ead3` "feat(constants): add
build-time module constants and retire module let". It adds
`typing/constants.rs`, makes `executable::lower` evaluate every constant of every
loaded module on the interpreter, and says `chimi check` lowers too. It also
touches `benches/frontend.rs` (two lines). `b0f32af` (text interpolation) and
`1511d38` (`std/debug`) are secondary suspects. I did not profile, so the
cause is a hypothesis.

Reproduce:

```sh
git archive 452ead3~1 | tar -x -C $SRC   # then touch every file: git archive keeps old mtimes
CARGO_TARGET_DIR=$T cargo bench -p chimi --bench frontend --features frontend-benchmarks --no-run
$BENCH --warmups 3 --samples 15 --semantics-only --semantic-case local --bench
```

Run the same at `e88cf9e` and compare `p50_ns`.

### 2. Cold `chimi build` is 2.6x slower (two-module fixture)

768 ms against 293.9 ms p50. Warm builds and the edit rebuild are unchanged
(+6% and +1%). Cold builds compile the C runtime archive (and the fiber archive
for task programs), so the cause is probably growth of
`compiler/chimi/runtime/native/runtime.c` and its dependencies between the
baseline commit `27ad088` and `65172a6`. The largest changes in that range are
`0d439c9` (per-owner heaps, +1,109 lines changed), `37f2b13`, `2183184`
(+559), `f9dcf3c` (+318), `1511d38`, `9d3dcf2`, `db1c3cc` and `fe4d2eb`. I did not
time the C compile separately, so this is unverified. A cold build of the
trace-summary program is 828 ms, so the fixed cost dominates.

Reproduce: create the fixture from `compiler/chimi/tests/cli.rs` (the
`build-cache` test), then, from an empty working directory each time:

```sh
$C build --root $D/project app/main -o $D/cached-app
```

Compare against a binary built from `27ad088`.

### 3. Lab executable is 38% larger; retained RSS is at the threshold

`chimi-concurrency-lab` grew from 1,650,112 B (A/B base at `2183184`) to
2,269,984 B. Retained resident delta after reclamation rose 19.6% against the
doc (22% against the A/B base) and is far inside the 8 MiB gate. Timing metrics
did not regress. The range is `2183184..65172a6`, which adds the supervision
kernel, nested supervisors and virtual clock (`80cb8d1`, `d62ac5f`) and early
release of channels and task contexts (`84afc89`, `10eb0b6`) to
`runtime/chimi-fiber` (about 12,000 new lines, per `git diff --stat`).
Reproduce with the A/B in the method section.

### 4. Not regressions, for the record

- Trace-summary allocation per record is within 1.4% of the baseline and under
  its gate. Peak live memory is up 13.5% (still 14% under the gate).
- Streaming-memory, direct-integer, and trace-summary wall time all improved.
- The lab's performance gate test passes, at both load 34 and load 4.5.
- Syntax p95 latency in the loaded run (4.2 ms typical, 9.4 ms editor) is
  noise; the bench's own gate reported `pass: true` on the later run and
  `malformed_ratio_pass: false` on the earlier, heavily loaded run.

## Limits

- The baselines were measured on macOS 25.5 (or 26.5.2 for the lab) and Rust
  1.92. Differences of under 20% should not be read as real.
- The direct-integer helper comparison cannot be reproduced.
- The first-pass measurements at load above 8 were discarded rather than
  averaged in.
- The cold-build and M7 causes above are narrowed by commit range only; none was
  profiled.
