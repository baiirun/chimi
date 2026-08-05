---
status: accepted
---

# Use Rust for the bootstrap compiler

Chimi's first real compiler will be implemented in Rust. An equivalent
Go-versus-Rust spike found that Go rebuilt faster, while Rust provided better
frontend throughput and tail latency, substantially lower peak memory, a
smaller executable, and direct control over compact compiler representations;
those runtime properties matter more for the compiler and long-lived language
server than Go's subsecond iteration advantage.

## Considered Options

- **Rust:** selected for its measured runtime profile, explicit memory/layout
  control, native executable distribution, and mature profiling, fuzzing, and
  backend integration paths.
- **Go:** rejected as the bootstrap host. Its simpler implementation model and
  faster rebuilds remain attractive, but did not outweigh its garbage-collected
  process footprint and reduced control over hot representation layout.

## Consequences

Rust is a bootstrap implementation detail, not Chimi's backend, runtime, memory
model, error model, or permanent implementation language. Compiler phase
interfaces remain host-neutral, and the released compiler is still expected to
become reproducibly self-hosted in Chimi. The disposable comparison remains the
primary evidence on the `codex/compiler-host-spike` branch; its accepted
measurements are recorded on `prog` ticket `ts-0846e5`.
