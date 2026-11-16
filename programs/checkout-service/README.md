# Checkout service

This forcing program (CHI-207) is the Phase 3 service: a checkout service
that uses failure-set types, `std/result`, causes, retry, timeouts and
deadlines, `Task.all`, `Task.results`, `Task.race`, `Select.first`, a
broadcast, and a watch together. What it showed about choice syntax and
error propagation is written up in
[the evidence note](../../docs/research/phase-3-forcing-program-evidence.md).

- `app/checkout.chimi`: one checkout. Stock and fraud checks at once, a
  charge with a per-attempt timeout and backoff, a race between two
  carriers, notifications whose failures are counted rather than raised, and
  a deadline over the whole checkout, stored as a `Result`.
- `app/worker.chimi`: a worker that selects on the shared order queue, the
  settings watch, and a heartbeat timer, and reports on a broadcast.
- `app/fakes.chimi`: deterministic stand-ins for the services a checkout
  calls. They wait on the clock like network calls and fail by rule.
- `app/main.chimi`: starts two workers and an audit task, submits seven
  orders, pauses and resumes the workers, and prints the report.
- `app/checkout_test.chimi`: `chimi test` cases that pin the timing of
  retries, timeouts and the deadline under the virtual clock.

The program runs natively only, like the rest of the channel runtime. From
the repository root:

```sh
cargo run -p chimi -- build --root programs/checkout-service app/main -o target/checkout-service
target/checkout-service --chimi-runner --clock=virtual --outcome-file=target/checkout-outcome.jsonl --
cargo run -p chimi -- test --root programs/checkout-service app/checkout_test
```

The run prints `expected.stdout` exactly, and its outcome record is
`expected.outcome` (seven orders handled). It shows:

- Order 1 meets a payment outage, retries after 50 ms of backoff, and
  succeeds in 155 ms. SMS bounces for odd order ids, so it reports 1 of 2
  notices.
- Orders 2, 3 and 4 fail with a stock, a fraud and a payment failure. Each
  cause shows the context frame of the step that failed.
- Order 5's charge is slower than the 200 ms attempt timeout, so every
  attempt times out and the 800 ms order deadline ends it.
- Order 6 fails because the express carrier answers first with `NoRoute`;
  `Task.race` takes the first child to finish, failed or not (CHI-245).
- Order 7 arrives while the workers are paused and waits until they resume.
  Its fraud service is unreachable, so it goes on for review and succeeds.
- The audit counts 2 completed and 5 rejected orders, and the heartbeats the
  idle workers sent while paused.

`compiler/chimi/tests/cli.rs` (`native_checkout_service_forces_the_phase_3_surface`)
builds and runs the program, compares both expected files, checks the
select and scope trace events, and runs the tests.
