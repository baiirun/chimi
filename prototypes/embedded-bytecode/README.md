# Embedded bytecode backend prototype

This disposable prototype asks whether the same opaque `BackendProgram` can be
encoded as bytecode, embedded with Chimi's runtime in one executable, and retain
the native candidate's observable behavior with a smaller, faster-built program
artifact.

Each Core node becomes one opcode plus operands in immutable generated tables.
A single C interpreter switch dispatches those opcodes into the exact semantic,
control-channel, dependency, and tracing runtime used by the native AOT spike.
Generated per-node shims only adapt the bytecode interpreter to that prototype
runtime's existing child-evaluation ABI; they do not contain operation logic.

Run every fixed backend workload from the repository root:

```sh
cargo run -p chimi --release --features embedded-bytecode-prototype \
  --bin chimi-bytecode-prototype -- --samples 40
```

The command fails if off, boundary, or full JSONL output differs from the
interpreter oracle. Generated executables and cacheable runtime/image objects
are written below `target/embedded-bytecode-prototype/`.
