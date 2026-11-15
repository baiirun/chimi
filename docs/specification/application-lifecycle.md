# Application Lifecycle: Drain, Signals, and Exit Status

## Status

Contract for G22 in the [language completion plan](../plans/language-completion-plan.md):
drain before cancel, ordered shutdown, signals, and the exit status. It
changes the shutdown rules of
[supervisor policy and lifecycle](supervisor-policy-and-lifecycle.md) and
[process execution and mailboxes](process-execution-and-mailboxes.md), and
the exit status of [runtime output channels](runtime-output-channels.md).
Decided from the vision docs and recorded in [decisions.md](../decisions.md).
The lab scenarios are RR05, SD06, SD10, SD11, MB19, SG01 to SG03, and the
Phase 4 forcing program FP02 (`programs/service-pool`), in the
[conformance lab](supervised-process-conformance.md) at stage U12.

Custom signal handlers, SIGHUP, and a `std/signal` module stay in Phase 6c.

## Goals

A service must shut down gracefully: stop taking work, finish the work it
accepted, and then exit, within a bound the author chose. A background worker
must shut down without losing work it accepted. The operator must be able to
stop it the usual ways (Ctrl-C, `kill`, systemd, Kubernetes) and read the
usual exit status.

This is the rule mainstream servers follow: a Kubernetes grace period, Go's
`http.Server.Shutdown`, Erlang's shutdown timeout before `brutal_kill`, and
Tokio's graceful shutdown all stop intake, let in-flight work finish, and
stop hard at a deadline.

## Example

<!-- chimi: fragment -->
```chimi
pub fn main() -> Void needs { clock: Clock } {
  let root = Supervisor.start(policy)
  // ... submit work ...
  Supervisor.wait(root)
}
```

`main` waits on its root supervisor until SIGTERM or Ctrl-C. The signal
cancels `main`, the root stops, and each member finishes the messages it
already accepted within its `shutdown_within`. `programs/service-pool` is the
full program.

## Drain before cancel

Stopping a process (through `Supervisor.stop`, `remove_process`, an
escalation, or its supervisor's own stop) has two phases.

- **Drain**, from the stop request until `now + shutdown_within`:
  - the process is unrouted, so sends to its name or address return
    `Unavailable`, and its mailbox accepts nothing new, monitor notices
    included;
  - the body is **not** cancelled. `Clock.sleep`, child tasks,
    `Task.result`, and channel operations proceed as they would before the
    stop;
  - `Process.receive` returns the messages accepted before the stop, oldest
    first, and once the mailbox is empty it returns cancellation at once, so
    an ordinary receive loop ends as soon as its backlog and in-flight work
    are done. The body ends `cancelled`.
- **Forced**, at the deadline: a park still pending is answered with the
  forced reply, the process's child tasks are cancelled, and the body runs no
  more user code. The body ends `forced`.

Three cases skip the drain:

- a body that has not run yet is cancelled at once, since nothing is in
  flight;
- `shutdown_within = Duration.zero` forces at once;
- a supervisor operation the process (or one of its tasks) is parked in that
  waits on its own shutdown, such as `Supervisor.stop` of its own root, is
  answered with cancellation, since it could never complete (SD08).

A `Task.timeout` or `Task.deadline` scope inside a draining body still fires;
only forcing stops scopes from running.

A process's end classification does not change: any end during the drain is
`cancelled`, a defect during the drain stays a defect (and is never
restarted), and an end at the deadline is `forced`.

A body that never calls `Process.receive`, such as a ticker looping on
`Clock.sleep`, learns of the stop only at its deadline. It will be able to
wait on its inbox and a timer together once `Select` (CHI-203) lands; until
then its author keeps `shutdown_within` short.

## Ordered shutdown

`Supervisor.stop` closes the supervisor to new members, then stops its
members one at a time: dynamic members newest first, then declared members
in reverse order. Each member is unrouted only when its own turn comes, so a
member that drains can still send to an older member, which stops after it
(SD06). A nested supervisor's turn stops its own members the same way.

Each member drains within its own `shutdown_within`, one after another, so
the longest a stop can take is the sum of its members' `shutdown_within`.

## Signals

The native runner handles SIGINT and SIGTERM.

- The **first** signal cancels `main`, the root task. That is the same
  cancellation as `Task.cancel`: `main`'s child tasks are cancelled, and
  every root supervisor `main` started stops, newest first, through the drain
  above. A `main` parked in `Supervisor.wait` therefore ends once its root
  has drained. The runner-wide cancellation that host operations observe is
  not set, so processes that drain keep their host access.
- A **second** SIGINT or SIGTERM ends the program at once with that signal's
  default action (press Ctrl-C twice).
- A program with no processes or tasks (no scheduler) is cancelled at its
  next semantic operation, and a blocking read of standard input is
  interrupted.
- A signal that arrives after `main` has ended changes nothing: the runner
  finishes stopping the remaining supervisors and reports `main`'s outcome.

Under the real clock, a program whose root is alive but has nothing to run
and no timer waits for a signal when a supervisor is running, because it is
a service waiting for work. Without a running supervisor the same stall is
still reported as a deadlock defect (exit 70), because nothing can wake it.

SIGPIPE keeps its default action. A later contract may change it for
programs that write to closed pipes.

### Injected signals

For deterministic tests the runner takes `--signal=INT@<duration>` or
`--signal=TERM@<duration>`, where the duration is an integer with a unit
(`ns`, `us`, `ms`, `s`, `m`, or `h`), as in `--signal=TERM@1500ms`. It needs
`--clock=virtual`; otherwise, or for any other spelling, the runner exits 64.
The signal is delivered at that virtual instant as a timer, so a program
that would otherwise be idle jumps to it. If `main` ends first, the signal is
never delivered and does not move virtual time (SG03).

With `--trace` on, delivery emits the boundary event
`clock_signal_received` with the signal number.

## Exit status

The statuses of [runtime output channels](runtime-output-channels.md) are
unchanged, with one addition. When the program ends with the cancellation
outcome because a signal cancelled `main`, the runner first writes its
outcome record and closes its sinks, then:

- for a real signal, restores the default action and raises the signal
  again, so the parent sees "terminated by SIGTERM" (a shell shows 143, or
  130 for SIGINT), as the GNU convention for cleanup handlers recommends and
  as systemd and Kubernetes read a clean stop. Where the signal cannot end
  the process (as PID 1 in a container), the runner exits `128 + signal`;
- for an injected signal, exits `128 + signal`: 143 for TERM, 130 for INT.

Any other outcome after a signal, such as a defect during cleanup, uses its
usual status. Cancellation without a signal still exits 130.

## Verification

- `compiler/chimi/tests/signals.rs` sends real signals to built programs: a
  service drains and dies by SIGTERM, an idle service waits for a signal
  instead of reporting a deadlock, a second signal ends at once, and SIGINT
  interrupts a program reading standard input.
- The lab scenarios listed under Status pin the virtual-clock transcripts.
- `runtime/chimi-fiber/src/scheduler/process_tests.rs` covers the scheduler
  rules: a sleeping handler finishes, a handler still parked at its deadline
  is forced, a draining body keeps its child task and channel, and a removal
  reply never reaches a later park.
