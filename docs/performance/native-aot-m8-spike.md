# M8 Native AOT Spike

## Result

The host-only native AOT spike succeeds. Every fixed workload compiles into one
executable and reproduces the interpreter oracle's full JSONL output byte for
byte in off, boundary, and full trace modes. This result validates the
`BackendProgram -> Cranelift object -> host link` seam; it does not select the
permanent Chimi backend.

The candidate emits one native function per Core node. Those functions call a
small C-compatible runtime ABI for dynamic values, environments, control
channels, dependency dispatch, matching, and tracing. The final executable
links the Cranelift program object, generated immutable program data, and a
static runtime archive.

## Environment

- Date: 2026-08-06
- Revision snapshot: M8 native AOT changes uncommitted on top of the backend oracle
- Mode: Cargo release profile, Cranelift 0.131.3, C runtime compiled with `cc -O2`
- Host: Apple M1, macOS 26.5.2 arm64, 8 logical cores, 16 GiB memory
- Rust: 1.92.0
- Sampling: 3 cold-process warmups and 20 measured samples; steady state uses 100 tracing-free executions per sample
- Command: `cargo run -p chimi --release --features native-aot-prototype --bin chimi-aot-prototype -- --samples 20`

Cranelift 0.131.3 is pinned because newer available releases require a newer
Rust compiler than the repository's Rust 1.92 contract.

## Conformance

| Workload | Core nodes | Terminal outcome | Off | Boundary | Full |
| --- | ---: | --- | --- | --- | --- |
| Rich success | 46 | success | exact match | exact match | exact match |
| Checkout | 40 | success | exact match | exact match | exact match |
| Retry exhaustion | 34 | expected failure | exact match | exact match | exact match |
| Defect | 4 | defect | exact match | exact match | exact match |
| Cancellation | 2 | cancellation | exact match | exact match | exact match |

The comparisons include event order, computation and execution identity,
causal spans, Core and source origins, dependency-call metadata, terminal
values, expected failures, defects, and cancellation.

## Measurements

| Workload | Object emit | Runtime + archive | Data compile | Link | Cold p50/p95 | Steady p50/p95 | Peak RSS | Executable |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rich success | 4.70 ms | 277.95 ms | 23.54 ms | 35.60 ms | 1.42/1.57 ms | 8.09/9.57 us | 1.34 MiB | 74,248 B |
| Checkout | 1.22 ms | 146.40 ms | 22.58 ms | 35.04 ms | 1.78/2.35 ms | 7.81/9.63 us | 1.38 MiB | 74,008 B |
| Retry exhaustion | 1.19 ms | 147.68 ms | 22.36 ms | 34.21 ms | 1.48/1.65 ms | 8.20/10.48 us | 1.38 MiB | 73,768 B |
| Defect | 0.46 ms | 147.63 ms | 22.56 ms | 34.52 ms | 1.46/1.58 ms | 0.56/0.70 us | 1.33 MiB | 72,536 B |
| Cancellation | 0.42 ms | 148.04 ms | 21.62 ms | 35.47 ms | 1.44/1.54 ms | <0.01/0.01 us | 1.31 MiB | 72,472 B |

The static runtime archive is 39,048 bytes. Program objects range from 1,128
to 5,304 bytes. `otool -L` reports only `/usr/lib/libSystem.B.dylib` beyond the
executable itself on this host.

Cold time is wall-clock process creation plus one boundary-traced execution.
Steady time is measured inside the executable through the public off mode and
therefore excludes tracing and process startup. Peak RSS comes from `/usr/bin/time -l`.
The cancellation steady-state value is effectively clock-resolution noise
because it performs no semantic operation.

## Interpretation

The experiment establishes that native AOT does not conflict with Chimi's
single-executable or observability goals. The backend-specific code stays
behind the opaque `BackendProgram`, and runtime events retain the same semantic
identity as the oracle.

This is not production runtime evidence. The prototype deliberately boxes all
values, uses linear metadata searches, calls a runtime helper for every Core
node, and retains allocations until process exit. Runtime compilation is also
repeated per workload rather than cached. Those choices make the semantic and
linking boundary visible, but they would distort a permanent runtime design.

The native object, generated data object, and static runtime archive remain
separate before the final link, so a real compiler can cache them independently.
The embedded-bytecode spike should now use the same corpus and reporting schema
before the permanent backend decision.
