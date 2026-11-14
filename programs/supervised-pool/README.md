# Supervised pool

This forcing program (lab scenario FP01,
`docs/specification/supervised-process-conformance.md`, "Forcing program")
runs three named workers under one root supervisor of capacity 3. It forces
one defect and restart while the peers keep their state, one fail-fast
admission at capacity, one stale direct address, one message lost with the
defect, one removal, and the final stop.

The program runs natively only: the reference interpreter has no process
runtime yet, so `chimi run` defects with "requires the supervised process
runtime" (lab scenario IP01).

From the repository root:

```sh
cargo run -p chimi -- build --root programs/supervised-pool app/main -o target/supervised-pool
target/supervised-pool \
  --chimi-runner \
  --clock=virtual \
  --trace=full \
  --telemetry-file=target/supervised-pool-trace.jsonl \
  --outcome-file=target/supervised-pool-outcome.jsonl \
  --
```

The outcome is `PoolResult(a: 3, b: 30, c: 300, stale: 99, rejected: 1)`:

- `a = 3` and `c = 300`: the peers kept their state across B's defect.
- `b = 30`: B's replacement started from 0. `Add(20)` was accepted behind
  `Crash` and lost with it.
- `stale = 99`: the direct address to B's first incarnation returned the
  unsent message.
- `rejected = 1`: the fourth start failed fast at capacity 3.

`expected.outcome` and `expected.transcript` hold the exact outcome line and
the `--trace=full` transcript the lab compares (`chimi-process-lab forcing`).
`expected.memory` requires that `--trace=off` constructs no telemetry event.
