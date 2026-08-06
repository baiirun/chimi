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
- Sampling: 3 warmups and 40 measured samples per trace mode; steady state uses 100 executions per sample
- Command: `cargo run -p chimi --release --features native-aot-prototype --bin chimi-aot-prototype -- --samples 40`

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
| Rich success | 2.22 ms | 161.03 ms | 23.92 ms | 36.53 ms | 1.60/2.67 ms | 6.98/8.28 us | 1.38 MiB | 74,344 B |
| Checkout | 1.31 ms | 164.83 ms | 24.75 ms | 41.34 ms | 1.47/1.97 ms | 7.24/10.09 us | 1.38 MiB | 74,088 B |
| Retry exhaustion | 1.32 ms | 169.59 ms | 24.54 ms | 38.01 ms | 1.42/1.85 ms | 6.91/8.20 us | 1.38 MiB | 73,848 B |
| Defect | 0.45 ms | 163.44 ms | 23.26 ms | 38.92 ms | 1.41/1.91 ms | 0.43/0.50 us | 1.33 MiB | 72,632 B |
| Cancellation | 0.44 ms | 168.75 ms | 25.70 ms | 37.64 ms | 1.42/2.05 ms | 0.03/0.04 us | 1.33 MiB | 72,552 B |

The static runtime archive is 39,832 bytes. Program objects range from 1,128
to 5,304 bytes. `otool -L` reports only `/usr/lib/libSystem.B.dylib` beyond the
executable itself on this host.

Cold time is wall-clock process creation plus one boundary-traced execution.
Steady time is measured inside the executable through the public off mode and
therefore excludes tracing and process startup. Peak RSS comes from `/usr/bin/time -l`.
The cancellation steady-state value is effectively clock-resolution noise
because it performs no semantic operation.

## Native tracing overhead

| Workload | Off p50/p95 | Boundaries p50/p95 | Full p50/p95 | Boundary/full p50 multiple |
| --- | ---: | ---: | ---: | ---: |
| Rich success | 6.98/8.28 us | 11.77/18.04 us | 146.56/155.15 us | 1.69x/21.00x |
| Checkout | 7.24/10.09 us | 13.45/14.47 us | 123.72/135.10 us | 1.86x/17.09x |
| Retry exhaustion | 6.91/8.20 us | 13.48/14.77 us | 142.78/152.21 us | 1.95x/20.66x |
| Defect | 0.43/0.50 us | 3.45/3.78 us | 14.65/15.99 us | 8.02x/34.07x |
| Cancellation | 0.03/0.04 us | 3.35/3.50 us | 3.36/3.67 us | 111.67x/112.00x |

Each sample repeats one trace mode 100 times inside the process. Boundary and
full events take the real JSON formatting and stdio path, with stdout connected
to a null sink; the runtime flushes the sink before stopping its monotonic
clock. The result includes event construction, serialization, and minimal
export I/O but excludes process startup, filesystem storage, and a remote
collector. Off follows the same loop without constructing events.

The substantial workloads show boundary p50 overhead of 1.69–1.95x and full
trace overhead of 17.09–21.00x over off. The much larger ratios for defect and
cancellation reflect fixed lifecycle-event cost over almost no program work,
not a scalable per-operation ratio.

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
