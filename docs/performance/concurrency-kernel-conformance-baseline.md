# Concurrency Kernel Conformance Baseline

This baseline records the first connected evidence for `prog show ts-7c4508`.
It is a local forcing measurement, not a stable cross-machine performance
promise.

## Evidence boundary

`chimi-concurrency-lab` is one native executable with independently runnable
evidence kinds:

| Kind | Evidence |
| --- | --- |
| `static` | Real compiler fixtures for Task needs capture, exclusive transfer, parent-local handles, and recoverable send payloads |
| `runtime` | A generated-C finite worker pipeline over the maintained Scheduler and Channel runtime, plus expected-failure, defect, cancellation, and cleanup scenarios |
| `observability` | Trace-off construction, boundary filtering, full causal channel motion, and shared event sequencing |
| `performance` | Process cold start, repeated rendezvous throughput, trace overhead, parked RSS, retained RSS, runtime-state reclamation, trace export, and reserved stack space |
| `smoke` | Composition of the preceding static and runtime evidence in the same executable |

Run one kind or the complete lab:

    cargo run --release --bin chimi-concurrency-lab -- runtime
    cargo run --release --bin chimi-concurrency-lab -- all

The lab remains the focused performance and lifecycle harness. Chimi
source-level `Task` and `Channel` calls now connect to the same scheduler through
the native AOT runtime. `programs/concurrency-pipeline` is the composite source
forcing artifact; focused CLI integrations separately cover expected failures,
defects, cancellation, abort, close/drain, ownership transfer, and trace modes.

## Forcing pipeline

Two producers feed a capacity-one ingress channel. Its exclusive dispatcher
receiver routes FIFO work through capacity-zero rendezvous channels to two
workers. Workers apply a configured offset and share a capacity-one MPSC output
channel consumed by one exclusive receiver. The coordinator retains each
completion capability and closes ingress, worker, and output stages in order.

The scenario forces one parked producer, FIFO admission without barging,
exclusive receiver transfer, rendezvous, bounded buffering, graceful drain,
and recursive root cleanup. Separate terminal scenarios prove that expected
failures remain values, child defects cancel siblings and propagate causally,
and cancellation drains children before publishing the root outcome.

## Release baseline

- Date: 2026-08-11
- Host: macOS 26.5.2, Apple arm64
- Toolchain: repository Rust 1.92 toolchain, release profile
- Fiber stack reservation: 64 KiB per Task
- Throughput workload: p95 of 7 runs with 16,384 capacity-zero rendezvous messages each
- Parked workload: 256 Tasks

| Metric | Result | Regression gate |
| --- | ---: | ---: |
| Executable size | 1,644,896 B | recorded only |
| Cold process start p95 | 2,215,541 ns | < 25,000,000 ns |
| Trace-off rendezvous | 265 ns/message | < 5,000 ns/message |
| Boundary rendezvous | 263 ns/message | < 3x overhead |
| Full rendezvous | 418 ns/message | < 5x overhead |
| Parked resident delta | 4,784,128 B | < 33,554,432 B |
| Retained resident delta after reclamation | 753,664 B | < 8,388,608 B |
| Reserved stack address space | 16,777,216 B | exact from Task count and configured stack size |

Boundary timing was slightly below trace-off timing; the gate treats that as
zero overhead rather than claiming an improvement. Full tracing was 57.7%
slower because it constructs task and channel motion events.

The memory workload runs in a fresh subprocess and keeps the Scheduler alive.
It parks 256 Tasks with 256 channels under full tracing, then observes and
releases every terminal Task, releases every terminated channel, exports the
trace batch, and asserts that all four internal collections are empty before
measuring retained RSS. This distinguishes runtime reclamation from merely
dropping the entire Scheduler. Task termination uses an owner-indexed channel
set, so cleanup scales with channels owned by that Task rather than every
channel in the runtime.

The first performance run exposed a separate 268x full-trace regression:
Scheduler-created fibers captured and symbolized native backtraces at every
park, but no Task event consumed those snapshots. Scheduler semantic tracing
now leaves `capture_backtraces` disabled. Explicit raw-fiber diagnostics still
support and test parked-stack capture through `ChimiFiberConfig`.

## Maintained gates

Release builds gate cold start at 25 ms and trace-off rendezvous at 5 us per
message; debug builds use 250 ms and 50 us so the same integration test remains
useful without pretending unoptimized measurements are release baselines.
Boundary and full trace overhead gates are 3x and 5x respectively.

The CLI integration test runs every evidence kind independently. Unit tests
also exercise the static, runtime, observability, reclamation, trace export,
and composed smoke interfaces. The complete repository gate is:

    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
