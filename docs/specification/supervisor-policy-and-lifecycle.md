# Supervisor Policy and Lifecycle

## Status

Contract for `prog show ts-535827`, inside the
[first supervised-process kernel](../wayfinding/first-supervised-process-kernel.md).
It consolidates behavior accepted in the
[first concurrency kernel](../wayfinding/first-concurrency-kernel.md) and by
`ts-3380c6`, `ts-a20d02`, and `ts-7aff55`; the remaining choices were resolved
with the user and are logged on the ticket. Source spelling remains
provisional.

This contract supersedes the first concurrency kernel's spelling: arguments are
positional, `Supervisor.process` takes a `Process.Spec` rather than a start
function, and restart is a `Supervisor.Restart` argument rather than the
`restart_on_defect` transform (`prog show ts-5961ee`).

Related contracts:
[process execution and mailboxes](process-execution-and-mailboxes.md),
[process identity and routing](process-identity-and-routing.md).

## Example

<!-- chimi: pending ts-6ccbc9 ts-f31a20 -->
```chimi
import std/process as Process
import std/supervisor as Supervisor

pub process name counter: CounterMessage

let counting = Process.spec(counter_loop, 64, Duration.seconds(5))
  |> Process.register(counter)
  |> Supervisor.process(Supervisor.Restart.OnDefect)

let caching = Process.spec(cache_loop, 256, Duration.seconds(5))
  |> Supervisor.process(Supervisor.Restart.OnDefect)

let workers = Supervisor.policy(
  [counting, caching],
  16,
  Supervisor.restarts(2, Duration.seconds(30)),
)

pub fn main() {
  let root = Supervisor.start(workers)
  Supervisor.wait(root)
}
```

## Values

Specifications are inert: building one starts no work. Only `Supervisor.start`
or an owning supervisor creates runtime work.

| Value | Created by | Meaning |
| --- | --- | --- |
| `Process.Spec<M, E>` | `Process.spec(body, mailbox_capacity, shutdown_within)`, then optionally `Process.register(spec, name)` | How to run one process incarnation |
| `Supervisor.Member` | `Supervisor.process(spec, restart)` or `Supervisor.supervisor(policy, restart)` | A process or nested supervisor as a declared member, without its message type |
| `Supervisor.Policy` | `Supervisor.policy(members, capacity, restarts)` | Static topology and limits |
| `Supervisor.Restarts` | `Supervisor.restarts(limit, within)` | A finite rolling restart limit |
| `Supervisor.Restart` | `Never` or `OnDefect` | Whether one member is replaced after a defect |
| `Supervisor.Running` | `Supervisor.start(policy)` | A running root supervisor |
| `Supervisor.ManagedProcess<M>` | `Supervisor.start_process(running, spec, restart)` | Membership of one dynamic process |
| `Supervisor.ManagedSupervisor` | `Supervisor.start_supervisor(running, policy, restart)` | Membership of one dynamic nested supervisor |

Exact types, failure rows, and compile-time rejections are in
[process and supervisor static contracts](process-and-supervisor-static-contracts.md).

- A member restarts only when it was given `Supervisor.Restart.OnDefect`.
- Specifications and policies are shareable values.
- `Supervisor.Running` is shareable, so processes the supervisor
  owns may add or remove dynamic members.
- Managed handles are exclusive and transferable. `remove_process` and
  `remove_supervisor` consume them.
- `restarts` is required. A supervisor that must never restart
  anything uses `Supervisor.restarts(0, within)` or simply gives
  every member `Supervisor.Restart.Never`; there is no separate
  `Supervisor.no_restarts`.

## Operations

| Operation | Parks? | Outcome |
| --- | --- | --- |
| `Supervisor.start(policy)` | Until initial members have started | `Supervisor.Running`, or the first start failure (for example `Supervisor.NameTaken`) after starting members are stopped |
| `Supervisor.start_process(running, spec, restart)` | Until the process has started | `ManagedProcess<M>`, or `Supervisor.CapacityReached` / `Supervisor.NameTaken` |
| `Supervisor.start_supervisor(running, policy, restart)` | Until its initial members have started | `ManagedSupervisor`, or `Supervisor.CapacityReached` / `Supervisor.NameTaken` |
| `Supervisor.address(managed)` | No | A direct address to the current incarnation |
| `Supervisor.remove_process(running, managed)` | Until no incarnation of that membership runs | `Void` |
| `Supervisor.remove_supervisor(running, managed)` | Until that subtree has stopped | `Void` |
| `Supervisor.stop(running)` | Until the whole subtree has stopped | `Void` |
| `Supervisor.wait(running)` | Until the supervisor terminates | `Void` after a stop; a restart-limit escalation at the root terminates the program |

- A supervisor runs until it is stopped or escalates. It does not
  end when its members end, because dynamic membership may add more.
- `Supervisor.stop` called from inside the subtree it stops does
  not wait for itself: the caller is cancelled as part of the shutdown.

## Admission

- `capacity` bounds the combined direct membership of processes and nested
  supervisors. Each nested supervisor bounds its own.
- Starting a dynamic member reserves a slot before initialization. Failed
  initialization releases it, restart keeps it, and removal or a terminal end
  without restart releases it.
- Admission is fail-fast: a request at capacity returns
  `Supervisor.CapacityReached`. Waiting or retry is composed above the
  supervisor. Rejection emits a structured admission event.
- Membership changes are serialized by the supervisor.

## Restart accounting

- Only a defect restarts a member, and only one given
  `Supervisor.Restart.OnDefect`. Return, expected failure, cancellation, and removal
  never restart.
- Each supervisor owns one rolling limit shared by all its direct restartable
  members. Every attempted replacement counts, including failed
  initialization, and is charged before the start runs.
- Replacement is immediate, with no supervisor-level backoff. It begins only
  after the terminated incarnation has released its name and its structured
  child tasks have been cancelled and drained, so incarnations never overlap.
- Exceeding the limit stops the supervisor's remaining members, and the
  supervisor defects with `Supervisor.RestartLimitExceeded`. A parent replaces
  it only if it was declared with `Supervisor.Restart.OnDefect`; that
  counts as one parent-level restart and rebuilds only its declared initial
  members. Dynamic members do not survive replacement of their supervisor.
- An escalation that reaches the root terminates the program.

## Removal races

- After `remove_process` or `remove_supervisor` returns, no incarnation of that
  membership runs and no pending replacement can create one.
- Removal ordered before a restart prevents it. Removal ordered after
  initialization has begun waits for or interrupts that initialization, then
  stops the replacement. An attempt that had begun still consumes restart
  budget.

## Shutdown

- Shutdown is out-of-band cancellation, never a mailbox message.
- `Supervisor.stop` first removes registered routing, then stops members in
  reverse order of the policy's members list (dynamic members, most recently added first,
  before declared ones). For each process it cancels and drains
  structured child tasks and allows cleanup within `shutdown_within`, then
  terminates it forcefully.
- Unread messages are discarded. Requested and forced shutdown are not defects
  and never restart.

## Observability

The runtime emits linked structured events for member start, admission
rejection, defect, restart attempt, exhausted limit, escalation, removal, and
shutdown, each carrying the supervisor, membership, and incarnation identities.

## Dependencies

`Duration` does not exist yet (`prog show ts-f31a20`). `shutdown_within` and
`Supervisor.restarts` need it before this contract can be implemented.
