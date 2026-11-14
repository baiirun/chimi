# Supervised-Process Conformance Baseline

This record holds the first full run of the supervised-process conformance
lab and its performance gates for `prog show ts-1765ba` (stage U10 of
[the lab specification](../specification/supervised-process-conformance.md)).
It is a local measurement on one host, not a cross-machine promise. The
format follows the
[concurrency kernel baseline](concurrency-kernel-conformance-baseline.md).

## Run

- Date: 2026-11-14
- Commit: `3745a33` (branch `lane-u10-pool`). The next commit gave PG10 its
  own build of the workload, so a runtime that traces under `off` fails PG10
  and not only the timing runs; PG07 and PG10 were rerun there with the same
  results (0 B and 0).
- Host: macOS 26.6.2 (25G83), Apple M1, arm64
- Toolchain: rustc 1.100.0 (0ef1bc386 2026-11-09), Apple clang 21.0.0
- Every command ran through `scripts/heavy.sh`, which holds the repository's
  heavy-command lock, so no other lane's build or test ran during a
  measurement. It runs the command at nice 10.

Commands, from the worktree root, with `CARGO_TARGET_DIR` set to a private
target directory:

```sh
scripts/heavy.sh cargo build --release -p chimi --bin chimi-process-lab
scripts/heavy.sh "$CARGO_TARGET_DIR/release/chimi-process-lab" performance --through U10
scripts/heavy.sh "$CARGO_TARGET_DIR/release/chimi-process-lab" all --through U10
scripts/heavy.sh cargo build -p chimi --bin chimi-process-lab
scripts/heavy.sh "$CARGO_TARGET_DIR/debug/chimi-process-lab" performance --through U10
```

The release `performance` report line is saved next to this file as
[`supervised-process-conformance-baseline.release.json`](supervised-process-conformance-baseline.release.json).
Compare a later run with it:

```sh
chimi-process-lab performance --through U10 \
  --baseline docs/performance/supervised-process-conformance-baseline.release.json
```

## Conformance result

`chimi-process-lab all --through U10` (release) ran all 131 scenarios: 130
pass and PG02 fails. Every static, Core, runtime, heap, observability, and
forcing scenario passes, including FP01, MF02, and MF03.

`CURRENT_STAGE` stays at U9 because PG02 fails (see
[Open failure](#open-failure)). The default suite runs FP01, MF02, and MF03
on their own with `--scenario <ID> --through U10` until the stage unlocks.

## Release gates

| ID | Metric | Result | Release gate | Status |
| --- | --- | ---: | ---: | --- |
| PG01 | ns per message, trace off | 877 | < 2,000 | pass |
| PG02 | `boundaries` over `off` | 1.05x | < 1.25x | pass |
| PG02 | `full` over `off` | 122.4x | < 5x | **fail** |
| PG03 | µs per restart cycle | 6.5 | < 40 | pass |
| PG04 | µs per process to start and first park | 9.4 | < 50 | pass |
| PG04 | µs per process to stop | 8.2 | < 30 | pass |
| PG05 | per-process cost at 1,000 over 100 | 0.38 | < 2.0 | pass |
| PG06 | ns per message, Text 4 KiB | 1,515 | < 2,000 | pass |
| PG06 | ns per message, List of 1,000 Ints | 43,217 | < 70,000 | pass |
| PG06 | ns per message, 21-value DAG | 2,480 | < 5,000 | pass |
| PG07 | resident growth, PG01 at N2 against N1 | 0 B | < 1 MiB | pass |
| PG08 | resident memory of 1,000 parked processes | 20,512,768 B | < 64 MiB | pass |
| PG09 | cold start p95, FP01, trace off | 8.95 ms | < 25 ms | pass |
| OB06 | wall time of 2m35s of virtual time | 9.6 ms | < 2,000 ms | pass |
| PG10 | trace-off events at N2 | 0 | exactly 0 | pass |

Recorded without a gate:

| Metric | Result |
| --- | ---: |
| FP01 executable size | 3,070,648 B |
| Process-free executable size (`pub fn main() -> Int { 1 }`) | 177,464 B |
| PG02 trace-off ns per message (10,000 to 100,000) | 883 |
| PG02 boundaries ns per message (10,000 to 100,000) | 926 |
| PG02 full ns per message (640 to 6,400) | 108,090 |
| PG04 µs per start at 100 processes | 28.0 |
| PG04 µs per stop at 100 processes | 18.8 |
| PG07 resident size at N1 | 2,064,384 B |

## Debug gates

A debug lab builds its workloads against the debug scheduler archive, so
debug time limits are 10 times the release ones. Memory, ratio, and count
limits are unchanged.

| ID | Metric | Result | Debug gate | Status |
| --- | --- | ---: | ---: | --- |
| PG01 | ns per message, trace off | 3,242 | < 20,000 | pass |
| PG02 | `boundaries` over `off` | 1.18x | < 1.25x | pass |
| PG02 | `full` over `off` | 34.7x | < 5x | **fail** |
| PG03 | µs per restart cycle | 16.7 | < 400 | pass |
| PG04 | µs per start; per stop | 20.3; 27.1 | < 500; < 300 | pass |
| PG05 | per-process cost at 1,000 over 100 | 0.44 | < 2.0 | pass |
| PG06 | ns per message: Text; List; DAG | 3,624; 43,608; 4,586 | < 20,000; < 700,000; < 50,000 | pass |
| PG07 | resident growth | 0 B | < 1 MiB | pass |
| PG08 | parked resident memory | 20,643,840 B | < 64 MiB | pass |
| PG09 | cold start p95 | 12.5 ms | < 250 ms | pass |
| OB06 | wall time of the virtual run | 8.9 ms | < 20,000 ms | pass |
| PG10 | trace-off events | 0 | exactly 0 | pass |

The debug `boundaries` ratio of 1.18 is close to its 1.25 limit, because the
debug scheduler's event construction costs more per send. The ratio limit
is not scaled for debug builds.

## Method

The lab specification's method applies, with these details fixed by the
harness (`compiler/chimi/src/process_lab/performance.rs`, and
`docs/decisions.md`):

- Each timing is the p95 of 7 samples, which for 7 samples is the slowest
  one. Every executable runs once unmeasured first: the first exec of a
  newly linked file on macOS pays a one-time system check of about 200 ms.
- Wall time runs from spawn to `wait4`. Resident size is the child's maximum
  resident size from `wait4`.
- Every measured run must exit 0 with its workload's expected outcome.
- PG01, PG02, PG07, and PG10 run MF01's program (one process, Int messages
  in batches of 64). MF01 rounds N down to whole batches, so the PG01 sizes
  are 9,984 and 1,000,000 messages.
- PG02 measures `off` and `boundaries` at 10,000 and 100,000 messages, and
  `full` at 640 and 6,400. Native full mode records every Core operation, so
  1,000,000 messages would write about 15 GB of telemetry per run. Each
  ratio divides by the trace-off cost at 10,000 to 100,000. A
  differential cost does not depend on the sizes.
- PG03 runs `tests/process-lab/performance/app/restarts` at 1,000 and 10,000
  cycles, with a restart limit above N and a one-hour window.
- PG04 and PG05 run `app/members` at 100 and 1,000 dynamic processes under
  `--trace=boundaries --clock=real`. Start is the time from
  `supervisor_started` to the first `process_terminated`: every start, every
  first park, and one report round trip. Stop is the time from there to
  `supervisor_stopped`. Virtual time carries no wall time, and the workload
  never sleeps, so the real clock changes no behaviour. PG05 divides the
  start-plus-stop cost per process at 1,000 by the cost at 100. The ratio is
  below 1 because the fixed cost of the first start and the trace file is
  spread over fewer processes at 100.
- PG06 runs `app/text_payload`, `app/list_payload`, and `app/dag_payload`
  at 640 and 64,000 messages (12,800 for the List).
- PG08 is the maximum resident size with 1,000 parked processes minus the
  same program with none. Scheduler fibers reserve a guarded 1 MiB stack
  each, not 64 KiB (`docs/decisions.md`, 2026-11-12). Pages are touched on
  use, so the resident cost is about 20 KiB per parked process.
- PG09 runs the FP01 executable under `--trace=off`. It also gates OB06's
  2 s wall bound, which the decision ledger assigns to this run.

## Open failure

PG02 full fails at about 120 times trace-off, against a gate of 5. The
native runner's full mode records `operation_started` and
`operation_completed` for every Core operation. A 20,000-message run wrote
877,867 telemetry lines (815,128 operation events and 19,969 `process_send`
events), which is 301 MB. The 5x gate came from the prototype, whose full
mode recorded process events only. The decision is `prog show ts-8f201b`.
The gate and the runner are unchanged until it is decided.
