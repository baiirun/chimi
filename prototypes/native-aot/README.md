# Native AOT backend prototype

This disposable prototype tests whether the backend-neutral `CoreProgram` can
become one host executable without giving up Chimi's observable execution
contract.

Each Core node becomes a native Cranelift function. The function calls one
operation-specific runtime ABI helper, while child evaluation continues through
the compiled node-function table. Generated C data contains immutable symbols,
origins, constructor shapes, and operation operands. The host linker combines:

- the Cranelift program object;
- the generated program-data object; and
- a static archive containing the prototype C runtime.

The executable dynamically depends only on the host system library on the
measured macOS configuration. This is a threaded native graph, not a final
backend: runtime-helper calls, boxed values, linear metadata lookup, and
process-lifetime allocation are deliberate prototype shortcuts.

Run every fixed backend workload from the repository root:

```sh
cargo run -p chimi --release --features native-aot-prototype \
  --bin chimi-aot-prototype -- --samples 20
```

The command fails if off, boundary, or full JSONL output differs from the
interpreter oracle. Off mode prints only the terminal outcome and bypasses
event construction. Successful output uses the
`chimi.native-aot-prototype.v1` JSONL schema and includes object emission,
runtime/data compilation, linking, cold start, tracing-free steady state, peak
RSS, artifact sizes, and dynamic dependencies.

The generated executables and intermediate artifacts are written below
`target/native-aot-prototype/`.
