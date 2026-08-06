# Backend comparison workloads

These fixtures are shared input for Chimi's permanent backend decision. The
native AOT and embedded bytecode candidates must consume the same prepared
`BackendProgram` and match the existing interpreter's terminal outcome and
boundary trace events.

The workload set covers:

- structural values, closures, calls, lists, records, matching, arithmetic,
  provision, and dependency calls in `rich-success.chimi`;
- the larger checkout dependency forcing program;
- generic thunk composition and an exhausted expected failure in
  `../retry-composition`;
- terminal defect and cancellation behavior.

Run the current oracle baseline from the repository root:

```sh
cargo bench --bench backend --features backend-benchmarks -- \
  --warmups 5 --samples 40
```

The runner emits `chimi.backend-comparison.v1` JSON Lines. Each workload records
the backend-neutral operation and value-layout profile, prepare and execution
latency, allocator traffic, retained output, and full versus boundary event
counts. Startup and artifact size remain `null` until an out-of-process
candidate produces an executable; the harness does not estimate unavailable
measurements.
