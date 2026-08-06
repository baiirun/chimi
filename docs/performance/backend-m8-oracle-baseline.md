# M8 Backend Comparison Oracle Baseline

## Purpose

This baseline fixes the program corpus, backend-neutral executable profile, and
interpreter results that the native AOT and embedded bytecode candidates must
share. It does not select a backend. The existing interpreter is a disposable
conformance oracle for terminal outcomes and trace boundaries.

## Environment

- Date: 2026-08-06
- Revision snapshot: `a8f80e4` with the M8 harness changes uncommitted
- Mode: Cargo `bench` release profile
- Host: Apple M1, macOS 26.5.2 arm64, 8 logical cores, 16 GiB memory
- Rust: 1.92.0
- Sampling: 5 warmups and 40 measured iterations
- Command: `cargo bench --bench backend --features backend-benchmarks -- --warmups 5 --samples 40`

The allocator reports requested bytes, peak live bytes above the operation
baseline, and bytes retained by the prepared program or execution report. True
cold-process startup and artifact size require a candidate executable and
remain `null` in the oracle output rather than being estimated.

## Fixed workloads

| Workload | Terminal outcome | Main runtime obligations |
| --- | --- | --- |
| Rich success | success | Structural values, closures, calls, lists, records, selection, matching, arithmetic, provision, and a dependency call |
| Checkout | success | Nested constructors, two structural providers, two dependency calls, success and failure control paths |
| Retry exhaustion | expected failure | Two modules, polymorphic thunk transformation, closure calls, two operation attempts, final expected failure |
| Defect | defect | Division-by-zero defect with source and core origin |
| Cancellation | cancellation | Cancellation before entry evaluation and a terminal cancellation record |

The workload sources live in `prototypes/backend-comparison`,
`prototypes/retry-composition`, and the accepted checkout golden fixture.

## Prepared `CoreProgram` profiles

| Workload | Types | Symbols | Functions | Bindings | Nodes/origins | Full events | Boundary events |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rich success | 16 | 33 | 3 | 1 | 46/46 | 91 | 3 |
| Checkout | 50 | 51 | 2 | 2 | 40/40 | 72 | 4 |
| Retry exhaustion | 22 | 35 | 4 | 2 | 34/34 | 82 | 4 |
| Defect | 3 | 16 | 1 | 0 | 4/4 | 10 | 2 |
| Cancellation | 3 | 16 | 1 | 0 | 2/2 | 2 | 2 |

Every prepared profile accounts for every core node exactly once across
constants, references, selections, dependency methods, constructions, calls,
lists, records, unary and binary operations, identities, provision, sequences,
closures, and matches. The JSON Lines output also records aggregate and maximum
argument, field, element, sequence, closure, match, pattern, and literal shapes.

## Oracle measurements

| Workload | Off p50/p95 | Boundary p50/p95 | Full p50/p95 | Off retained | Boundary/full retained |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rich success | 34.25/40.75 us | 35.17/35.92 us | 54.67/55.63 us | 107 B | 1,234/26,312 B |
| Checkout | 38.46/43.92 us | 39.21/39.50 us | 50.67/51.13 us | 218 B | 1,715/25,631 B |
| Retry exhaustion | 16.17/16.54 us | 16.92/17.13 us | 28.54/28.75 us | 21 B | 1,518/26,155 B |
| Defect | 0.21/0.25 us | 0.46/0.50 us | 1.67/1.75 us | 24 B | 789/3,183 B |
| Cancellation | 0.08/0.08 us | 0.33/0.33 us | 0.33/0.33 us | 44 B | 815/815 B |

These 20-sample off-mode additions were recorded on 2026-08-06 after the
original baseline. Off mode produces zero events and avoids span sequencing and
dependency-event attribute construction. These interpreter timings are
baselines, not candidate performance targets; they make tracing costs visible
while the backend candidates establish their own results.

## Candidate contract

Both backend spikes must:

- consume the same opaque `BackendProgram` produced after verification and
  backend-neutral core lowering;
- reproduce the oracle terminal outcome for all five workloads;
- reproduce computation and dependency-call boundary events, including source,
  semantic, core, causal-span, and execution identity;
- report lowering or encoding time separately from execution;
- report off-mode warm execution p50/p95, allocation or peak memory, tracing overhead,
  true cold-process startup, executable size, and external native dependencies;
- link the application and required Chimi runtime support into one deployable
  executable;
- keep intermediate module artifacts independently cacheable before final link;
- leave source semantics, typed contracts, and `CoreProgram` free of
  candidate-specific types and lifecycle policy.

The selected architecture must be decided from these shared results plus future
concurrency, supervision, durability, cancellation, cross-target, and runtime
integration constraints. A fast microbenchmark alone is insufficient.
