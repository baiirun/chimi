# Service pool

This forcing program (lab scenario FP02,
`docs/specification/supervised-process-conformance.md`, "Forcing program")
is the Phase 4 service: a dispatcher hands jobs round-robin to a dynamic pool
of three workers, and each worker reports the jobs it finishes to a ledger.
`main` starts the pool, submits six jobs, and then waits on the root
supervisor like a service does, until a stop signal ends it.
`docs/specification/application-lifecycle.md` gives the rules it exercises.

The program runs natively only: the reference interpreter has no process
runtime yet (lab scenario IP01).

From the repository root, with the stop signal injected at a virtual instant
so the run is exact:

```sh
cargo run -p chimi -- build --root programs/service-pool app/main -o target/service-pool
target/service-pool \
  --chimi-runner \
  --clock=virtual \
  --signal=TERM@1500ms \
  --trace=full \
  --telemetry-file=target/service-pool-trace.jsonl \
  --outcome-file=target/service-pool-outcome.jsonl \
  --
```

Without `--signal`, under the real clock, the program runs until you press
Ctrl-C or send SIGTERM, and the same shutdown follows.

The run shows:

- A poison job defects `worker_1` at once and the supervisor restarts it; the
  pool keeps serving.
- Jobs 1 to 3 run for a second each and reach the ledger at 1.001 s. Jobs 4
  and 5 are queued behind them.
- SIGTERM arrives at 1.5 s (`clock_signal_received signal=15`). It cancels
  `main`, which stops the root supervisor. Members stop one at a time,
  newest first, so `worker_3` drains first while the older members keep
  running: `worker_1` and `worker_2` finish jobs 4 and 5, queued when the
  signal arrived, and the ledger receives them at 2.001 s. Nothing accepted
  is cut off.
- Job 6 needs 30 s, longer than its worker's 5 s `shutdown_within`, so
  `worker_3` is forced at 6.5 s. The other members then have nothing left
  and end `cancelled` at once.
- The outcome is `cancellation`, and the program exits 143 (128 + SIGTERM),
  the status an injected signal reports. A real SIGTERM ends the program by
  that signal instead.

`expected.outcome` and `expected.transcript` hold the exact outcome line and
the `--trace=full` transcript the lab compares (`chimi-process-lab forcing`).
`expected.memory` requires that `--trace=off` constructs no telemetry event.
