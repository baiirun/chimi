# Supervisor Restart Composition Prototype

> PROTOTYPE: disposable API and state-model evidence, not Chimi runtime code.

## Question

Can ordinary `Retry` composition replace supervisor-owned process restart
without losing the shared rolling restart budget, containment, lifecycle
ownership, and defect observability that make supervision useful?

The prototype compares the strongest plausible forms of both candidates. It
does not treat retrying the start thunk as sufficient: that thunk returns once
the process starts and cannot observe a later defect. The ordinary-retry
candidate therefore wraps a lifecycle thunk that waits for termination and
reifies the defect for `Retry` to inspect.

## Run

```sh
cargo run --manifest-path prototypes/supervisor-restart-composition/Cargo.toml
```

Press `1` or `2` to defect either member, `s` to switch approaches, and `r` to
reset. Alternate defects across members to expose the difference between one
supervisor-owned budget and independent per-member retry budgets.

For a non-interactive transcript:

```sh
cargo run --quiet --manifest-path prototypes/supervisor-restart-composition/Cargo.toml -- --all
```

## Candidate A: supervisor-owned restart

```chimi
let counter =
  Counter.start
  |> Supervisor.process(
    shutdown_within: Duration.seconds(5),
  )
  |> Supervisor.restart_on_defect

let cache =
  Cache.start
  |> Supervisor.process(
    shutdown_within: Duration.seconds(5),
  )
  |> Supervisor.restart_on_defect

let workers = Supervisor.policy(
  processes: [counter, cache],
  supervisors: [],
  capacity: 16,
  restarts: Supervisor.restarts(
    limit: 2,
    within: Duration.seconds(30),
  ),
)

pub fn main() {
  workers
  |> Supervisor.start
  |> Supervisor.wait
}
```

`Supervisor.restart_on_defect` is an ordinary function over an inert process
or supervisor specification. The owning supervisor—not the function—performs
replacement, serializes it with removal and shutdown, and charges its shared
rolling budget.

## Candidate B: ordinary retry around each lifecycle

```chimi
let counter =
  fn() { Process.run(Counter.start) }
  |> Retry.with(Retry.defects(
    limit: 2,
    within: Duration.seconds(30),
  ))
  |> Supervisor.process(
    shutdown_within: Duration.seconds(5),
  )

let cache =
  fn() { Process.run(Cache.start) }
  |> Retry.with(Retry.defects(
    limit: 2,
    within: Duration.seconds(30),
  ))
  |> Supervisor.process(
    shutdown_within: Duration.seconds(5),
  )

let workers = Supervisor.policy(
  processes: [counter, cache],
  supervisors: [],
  capacity: 16,
  restarts: Supervisor.no_restarts,
)

pub fn main() {
  workers
  |> Supervisor.start
  |> Supervisor.wait
}
```

This requires the invented `Process.run` operation to turn a process's later
terminal defect into something ordinary `Retry` can observe. Each transformed
thunk naturally owns a separate retry budget. Giving it the supervisor's
shared budget, membership lock, removal state, and shutdown state recreates a
supervisor-specific restart operation behind a generic name.

## Verdict

Ordinary retry remains the right abstraction for expected failures while
starting or handling work. Defect restart is a containment operation owned by
the supervisor because it must share restart accounting and serialize
replacement with membership removal and shutdown. The remaining user-facing
question is whether
`Supervisor.restart_on_defect` is the clearest spelling for the specification
transformation; the prototype does not assume that name is final.
