# Scope-Owned Resources

## Status

This is the normative contract for scope-owned resources (G12) from the
[language completion plan](../plans/language-completion-plan.md), part of
Phase 6b (CHI-209), together with its part of CHI-215: a timed-out
operation's channels are freed at the timeout, not at task exit.
`Resource.scoped` is implemented on both backends. Byte streams (G18) build on
it and get their own contract.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative. The
module follows the [standard library conventions](standard-library-conventions.md).

## Goals

- One primitive that ties a resource a program creates (a pooled connection,
  a temporary file, a lock, a stream) to the code that uses it, so that
  libraries can define their own scope-owning functions such as
  `Sql.transaction` or `Http.with_connection` in Chimi.
- Release runs however the use ends: a value, a failure, a defect, a
  timeout, or cancellation of the task.
- A timeout or cancellation never cuts a release short, and a release can
  still bound itself with its own timeout.
- Plain words over FP jargon: `scoped`, `acquire`, `release`, `use`.

## Example

<!-- chimi: check -->
```chimi
import std/channel as Channel
import std/channel.{Receive}
import std/resource as Resource

enum PoolError {
  Empty
}

/// Takes a connection from an idle pool, or fails when none is left.
fn take(idle: Channel.Receiver<Int>) -> Int {
  let next = Channel.receive(idle)
  case next {
    Receive.Received(connection) -> connection
    Receive.Drained -> Failure(PoolError.Empty)
  }
}

/// Runs `query` on a pooled connection and always gives the connection back,
/// even when `query` fails or a timeout around this call ends it.
fn with_connection(idle: Channel.Receiver<Int>, back: Channel.Sender<Int>, query: fn(Int) -> Int) -> Int {
  Resource.scoped(
    fn() { take(idle) },
    fn(connection) { Channel.send(back, connection) },
    fn(connection) { query(connection) },
  )
}
```

## Contract

```text
pub native fn scoped<H, T, EA, ER, EU, RA, RR, RU>(
  acquire: fn() -> H errors EA needs RA,
  release: fn(H) -> Void errors ER needs RR,
  use: fn(H) -> T errors EU needs RU,
) -> T errors EA | ER | EU needs RA + RR + RU
```

- `scoped` MUST call `acquire` once. When `acquire` does not succeed it MUST
  NOT call `use` or `release`, and its outcome is `acquire`'s.
- When `acquire` succeeds, `scoped` MUST call `use` once with the acquired
  value, then `release` once with the same value, whether `use` succeeds,
  fails, defects, or is cancelled.
- The outcome is decided in this order:
  1. a defect in `use`;
  2. otherwise a defect in `release`;
  3. otherwise a cancellation of `use`;
  4. otherwise a failure or cancellation of `release`, which replaces the
     value or the failure of `use`, as a file that cannot be closed replaces
     the result of reading it;
  5. otherwise the outcome of `use`.

  This is the order `process_utf8_lines` already follows
  ([host capabilities](host-capabilities-and-execution-properties.md)).
- The arguments are in the order they run when the program is read: how to
  get the resource, how to give it back, then what to do with it. Keeping
  `release` next to `acquire` puts the pair a reviewer checks together side
  by side, as Haskell's `bracket` does.

## Cancellation

- **Masking.** From the moment `release` begins until it returns,
  cancellation that comes from outside the release MUST NOT be delivered: a
  timeout that fires, `Task.cancel`, a parent that ends, or a child's
  defect. It takes effect at the first cancellation point after the release
  returns.
- **Timeouts inside a release.** A `Task.timeout` or `Task.deadline` written
  inside `release` MUST still fire and be delivered. A release that may wait
  on something slow SHOULD bound itself this way.
- **Timeouts around a use.** A timeout that fires while `use` runs cancels
  `use`. The release then runs masked, and only after it returns does the
  timeout return `Timeout.TimedOut`.
- **Cancelling the task.** Cancelling a task that is inside a `scoped` call
  MUST NOT end it at once. The task keeps running until its releases have
  run, innermost first, and then ends with the cancellation's outcome (a
  child's defect stays a defect). A task that holds no resource ends at
  once, as before.
- **Children.** A child started while a release runs belongs to the
  release: a timeout or cancellation from outside does not cancel it, so a
  release may hand work to a helper task and wait for it. A child started
  before the `scoped` call, outside every scope, MUST stay alive until the
  cancelled task's releases are done, because a release may still need it
  (the task that serves a pool, a log writer). Children started inside the
  cancelled part are cancelled at once.
- **Forced ends.** A deadlock, a capability-contract violation, a process
  forced after `shutdown_within`, or the end of the program ends every task
  at once, and releases that have not started do not run. A release that can
  never return therefore ends in the deadlock defect rather than hanging, and
  in a supervised process it is bounded by the child's `shutdown_within`.

## Freeing what a timed-out scope gained (CHI-215)

- When a `Task.timeout` or `Task.deadline` ends in `TimedOut`, or a
  cancellation unwinds through it, every exclusive channel capability the
  task gained inside that scope MUST be aborted at the scope boundary, as
  task exit would abort it: a receiver or a completion it created or was
  handed inside the scope. Without mutation, those capabilities were reachable
  only from values the cancelled code produced, so nothing can use them
  again.
- A capability the task held before the scope, and a child's result handed
  back to the task inside the scope from a child started before it, are not
  aborted.
- A scope that ends normally keeps everything, as before.

## Handle escape

The plan asked that the raw handle not escape `use`. This contract does not
check that statically: a handle can be returned from `use` or captured in a
closure, and using it after release is a bug of the program, as with a closed
file in Go or Python. A static rule would need ownership or region types,
which the language does not have and which no forcing program has needed yet.
Libraries SHOULD hand `use` a value whose operations fail cleanly once the
resource is released (a closed stream answers `Closed`).

## Diagnostics

`scoped` adds no diagnostics. Arity and type mismatches are the ordinary
call diagnostics. A defect inside `acquire`, `use`, or `release` is reported
with that call's span.

## Conformance

| Rule | Evidence |
| --- | --- |
| Value, use failure, release failure replacing value and failure, failed acquire | `std-tests/resource/outcomes.chimi` (both backends) |
| A defect in `use` stays the outcome when `release` fails | `std-tests/resource/use_defect_stays_primary.chimi` |
| A defect in `release` replaces a use failure | `std-tests/resource/release_defect_replaces_failure.chimi` |
| Release runs masked after a timeout fires in use | `tests/resources.rs` `release_runs_masked_after_a_timeout_fires_in_use` |
| `Task.cancel` runs the release of a parked task | `task_cancel_runs_the_release_of_a_task_parked_in_use` |
| Cancel during a release waits for it | `cancel_while_masked_waits_for_the_release` |
| A timeout inside a release still fires | `a_timeout_inside_a_release_still_fires` |
| A release's helper task survives the outer timeout | `a_release_helper_task_survives_the_outer_timeout` |
| Nested releases run innermost first under cancel | `nested_releases_run_innermost_first_under_cancel` |
| A release waiting on its helper is not cut short | `a_release_waiting_on_its_helper_is_not_cut_short_by_cancel` |
| A release that never returns ends in the deadlock defect | `a_release_that_never_returns_ends_in_the_deadlock_defect` |
| A child's defect runs the parent's release, which can still use an earlier sibling, and stays the outcome | `a_child_defect_runs_the_parent_release_and_stays_the_outcome` |
| A timed-out scope aborts channels it created | `a_timed_out_scope_aborts_channels_it_created` |
| A child result handed back inside a timed-out scope stays open | `a_child_result_handed_back_inside_a_timed_out_scope_stays_open` |

## Implementation notes

- **Interpreter** (`execution/resource_operations.rs`). The interpreter has
  no scheduler, so there is nothing to mask; it applies the outcome order.
- **Native runtime** (`runtime.c` `resource_scoped`). It issues
  `REQUEST_RESOURCE_ENTER` before `acquire`, `REQUEST_RESOURCE_RELEASE`
  before `release`, and `REQUEST_RESOURCE_EXIT` after it. The exit reply says
  whether a cancellation is waiting, which `scoped` then returns unless the
  outcome is a defect. A fiber resumed as terminal issues none of them.
- **Scheduler** (`runtime/chimi-fiber/src/scheduler/scope.rs`). A resource is
  an entry on the task's scope stack. Release masks it; only scopes above the
  innermost mask are deliverable. Cancelling a task that holds a resource
  inserts a fired *root* entry at the bottom of the stack and records the
  outcome in `terminating`, which `begin_terminal` applies once the fiber
  finishes. Forced paths go through `force_terminal`, which keeps the old
  synchronous teardown.
- **Arrival stamps** (`channel.rs`). The channel registry keeps a counter.
  Each receiver and completion records when its current owner gained it, and
  every scope entry records the counter when it was pushed. A cancelled
  `REQUEST_SCOPE_EXIT` (its `value` is 1) aborts the capabilities stamped
  after the scope's entry. A child's result handed back to its parent is
  stamped with the child's start, so it counts as gained when the child was
  started.
- **ABI.** `CHIMI_FIBER_ABI_VERSION` is 10 and the native runtime ABI is 26.
- **Not covered yet.** A supervisor started inside a scope is not stopped
  when the scope is cancelled (follow-up ticket). `Process.send` does not
  transfer capabilities (CHI-213), so a capability sent to a process is
  outside this rule.
