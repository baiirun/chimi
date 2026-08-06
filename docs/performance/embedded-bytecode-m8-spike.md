# M8 Embedded Bytecode Spike

## Result

The host-only embedded-bytecode spike succeeds. Every fixed workload encodes
into one executable and reproduces the interpreter oracle's JSONL output byte
for byte in off, boundary, and full trace modes. This validates an opaque
`BackendProgram -> embedded bytecode image -> host link` seam; it does not yet
select Chimi's permanent backend.

Each Core node becomes one opcode plus operands in immutable generated tables.
A single interpreter switch dispatches operations into the same boxed values,
environments, control channels, dependency behavior, and tracing runtime as the
native AOT candidate. Small generated per-node shims adapt the interpreter to
the prototype runtime's existing child-evaluation ABI and contain no semantic
operation logic.

## Environment

- Date: 2026-08-06
- Revision snapshot: M8 embedded-bytecode changes uncommitted on top of the native spike
- Mode: Cargo release profile, C runtime and VM compiled with `cc -O2`
- Host: Apple M1, macOS 26.5.2 arm64, 8 logical cores, 16 GiB memory
- Rust: 1.92.0
- Sampling: 3 warmups and 40 measured samples per trace mode; steady state uses 100 executions per sample
- Command: `cargo run -p chimi --release --features embedded-bytecode-prototype --bin chimi-bytecode-prototype -- --samples 40`

## Conformance

| Workload | Core nodes | Terminal outcome | Off | Boundary | Full |
| --- | ---: | --- | --- | --- | --- |
| Rich success | 46 | success | exact match | exact match | exact match |
| Checkout | 40 | success | exact match | exact match | exact match |
| Retry exhaustion | 34 | expected failure | exact match | exact match | exact match |
| Defect | 4 | defect | exact match | exact match | exact match |
| Cancellation | 2 | cancellation | exact match | exact match | exact match |

The comparison includes event order, computation and execution identity, causal
spans, Core and source origins, dependency-call metadata, terminal values,
expected failures, defects, and cancellation.

## Measurements

| Workload | Encode | Runtime + VM archive | Image compile | Link | Cold p50/p95 | Off p50/p95 | Boundary p50/p95 | Full p50/p95 | Peak RSS | Executable |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rich success | 0.52 ms | 199.75 ms | 30.20 ms | 34.95 ms | 1.57/1.78 ms | 6.75/7.20 us | 11.43/11.61 us | 144.24/144.69 us | 1.38 MiB | 74,088 B |
| Checkout | 1.10 ms | 172.32 ms | 29.84 ms | 35.03 ms | 1.59/1.98 ms | 7.03/7.39 us | 13.30/13.63 us | 121.69/122.19 us | 1.38 MiB | 73,896 B |
| Retry exhaustion | 0.31 ms | 172.69 ms | 28.91 ms | 34.61 ms | 1.59/1.89 ms | 6.83/7.30 us | 13.09/13.31 us | 138.96/139.38 us | 1.38 MiB | 73,704 B |
| Defect | 0.20 ms | 175.37 ms | 23.79 ms | 34.90 ms | 1.57/1.82 ms | 0.44/0.49 us | 3.46/3.51 us | 14.56/14.68 us | 1.31 MiB | 72,744 B |
| Cancellation | 0.19 ms | 172.70 ms | 23.42 ms | 35.37 ms | 1.56/1.75 ms | 0.03/0.06 us | 3.35/3.42 us | 3.35/3.39 us | 1.31 MiB | 72,680 B |

The runtime plus VM archive is 41,816 bytes. The application-specific bytecode
image objects range from 2,968 to 12,888 bytes. `otool -L` reports only
`/usr/lib/libSystem.B.dylib` beyond the executable itself on this host.

Cold time is process creation plus one boundary-traced execution. Steady time
is measured inside the executable; traced modes format and export canonical JSON
events to a null sink. Runtime compilation is repeated per workload so it is
visible here, although the archive is independently cacheable and identical
across workloads.

## Native comparison

The bytecode image object is 12–21% smaller than the native candidate's combined
program-code and program-data objects for these workloads. Its encoding step is
also cheaper than native object emission. Those advantages mostly disappear in
the linked result: bytecode executables are between 128 bytes larger and 256
bytes smaller, cold startup and RSS are effectively equal, and the substantial
workloads' off-mode p50 differs by less than 0.25 microseconds from the recorded
native run.

This is evidence that either dispatch strategy fits Chimi's single-executable
and observability contracts. It is not evidence that bytecode and native code
have the same performance ceiling. Both candidates box every value, linearly
search metadata, call a C helper for every Core operation, and retain allocations
until process exit. That shared runtime dominates these small programs and hides
optimization opportunities that native lowering could eventually exploit.

## Interpretation

The experiment establishes an embedded-runtime path without introducing a
separately installed VM, package tree, or backend-specific source semantics.
The generated image object and runtime/VM archive remain separate before the
final link, so a real compiler could cache them independently.

The prototype image is deliberately encoded as C initializers before becoming
an object file. A production bytecode format would need a compact, versioned
binary serialization, validation at its trust boundary, and a direct embedding
path. The next M8 decision should therefore compare operational architecture—
portability, compilation latency, optimization ceiling, runtime integration,
durability, and debugging—not treat the nearly tied microbenchmarks as a backend
selection.
