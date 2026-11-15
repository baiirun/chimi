# Chimi fiber runtime

This crate is the maintained, runtime-private stackful execution component for
native Chimi programs. It builds both a Rust library for compiler tests and a
static archive that `chimi build` links into the single output executable.
Fibers are not source or Core concepts.

The `scheduler` module owns the deterministic lifecycle of finite task trees
over those fibers. Its narrow interface starts roots and structured children,
drives runnable work, observes results, wakes parked work, and cancels owned
tasks. Run queues, descendant draining, owner-token validation, defect causes,
and terminal publication ordering stay inside the module. Expected Chimi
failures remain values inside a successful fiber result; only cancellation,
defects, and stack overflow are scheduler terminal causes.

The scheduler also owns bounded MPSC channels. Capacity zero rendezvous never
buffers; positive capacities bound accepted unread values exactly. Parked
senders are admitted FIFO without barging. Separate exclusive receiver and
completion capabilities provide destructive abort and graceful close/drain,
and owner exit aborts abandoned channels. A request that uses or hands on a
receiver or completion the requesting task does not own defects that task, and
the config's `capability_violation_report` receives the capability, the task,
and the current owner first, so the host can name them in the defect.
Cancellation and channel operations
have one deterministic winner, rejected sends retain their payload, and trace
events record identities, causes, depths, capacity, and discard counts without
recording payloads.

Terminal Task results and channel allocations remain reachable while their
language handles are reachable. The runtime-facing `release_task` and
`release_channel` operations reclaim them only after observation and terminal
cleanup; live-state release is rejected. Trace consumers export batches with
`drain_events`, which clears retained event storage while preserving the
monotonic sequence. Task cleanup consults an owner index rather than scanning
every channel in the runtime.

The scheduler also owns supervised processes (`scheduler/process.rs`). Each
incarnation runs on its own parentless task with a bounded FIFO mailbox of
opaque message handles; the runtime's release hook frees handles the
scheduler discards unread. Supervisors are scheduler records, not fibers:
admission, named routing, rolling restart accounting (charged before the
replacement starts), escalation, removal, and shutdown order run between fiber
resumes. Shutdown first cancels and drains the incarnation's child tasks and
withdraws whatever its current park waits on; the body is then cancelled at
that park, or at its next park that would block if it was running, while a
receive drains the mailbox first. Stops and removals are plans that wait for a
lingering incarnation's `shutdown_within` deadline and then force it. Nested
supervisors, declared or dynamic (`start_nested_supervisor`,
`remove_supervisor`), take one slot of their parent, escalate to it, and can
be stopped on their own; a plan that reaches a nested supervisor another plan
is already stopping waits for it to retire. A root supervisor started by a
task stops when that task ends, and the task stays draining, with its
terminal event withheld, until that stop has finished. Process requests
use `ChimiProcessRequest` (`CHIMI_REQUEST_PROCESS_*`,
`CHIMI_REQUEST_SUPERVISOR_*`), and lifecycle events, including
`supervisor_started`, use the process trace family. `docs/architecture/supervised-process-runtime.md` records the design.

The execution clock (`scheduler/clock.rs`) is real or virtual. Virtual time
moves only in `run_until_idle`, when nothing is runnable and no host
operation is in flight, and jumps to the earliest timer; a parked root with
nothing able to wake it is a deadlock defect, reported with each parked task
through the config's `deadlock_report`. The real clock never reports deadlock:
`chimi_scheduler_run` returns `CHIMI_FIBER_PARKED` for a root still waiting on
the host. `CHIMI_REQUEST_TIMER_NOW` and `CHIMI_REQUEST_TIMER_SLEEP_UNTIL`
expose the clock, and `chimi_scheduler_run` takes a `ChimiSchedulerConfig`
with the clock mode and release hooks.

Cancellation scopes (`scheduler/scope.rs`) back `Task.timeout` and
`Task.deadline`. `CHIMI_REQUEST_SCOPE_ENTER` pushes a scope with a deadline
timer onto the calling task, `CHIMI_REQUEST_SCOPE_EXIT` pops it (replying
whether that scope's own cancellation reached the task, and pending while its
cancelled children drain), and `CHIMI_REQUEST_SCOPE_CHECK` is the loop-pass
cancellation point. A firing scope withdraws the task's current park and
cancels the children started inside it, and emits `clock_scope_fired`.
`chimi_fiber_cancel_is_terminal()` and `chimi_fiber_scope_cancel_pending()`
tell the running fiber whether a cancel ends the whole task and whether a
scope cancel is waiting; `chimi_fiber_scope_polls()` tells a loop to poll
with `CHIMI_REQUEST_SCOPE_CHECK` now and then, because under the real clock
every request first fires the task's own due scope deadlines.
`chimi_scheduler_execute` keeps its earlier behavior.
`chimi_scheduler_execute_with_config` is `chimi_scheduler_execute` with three
release hooks: `release_task_value` for every value a task returned except
the outcome's, `release_channel_value` for every sent value the
scheduler discards (a cancelled send, an aborted or released queue, a ready
reply whose task is cancelled), and `release_task_context` for the context of
a started task that ends before its entry runs. The host issues
`CHIMI_REQUEST_CHANNEL_RELEASE` when it drops its last reference to a
channel's endpoints; the channel's queued values are released then rather than
when execution ends, and its id becomes invalid. `ChimiValueRelease` and
`ChimiTaskContextRelease` in `include/chimi_fiber.h` state the ownership
contract.

Generated code suspends through `chimi_fiber_request` (or the lower-level
`chimi_fiber_park`) with an optional linked `ChimiRootFrame`. Scheduler requests
are handled only after control returns to Rust, so a resumed fiber never
re-enters the scheduler through an aliased mutable reference. Every slot in a
root-frame chain is an exact live `ChimiValue` root
and remains writable while parked so a moving collector can update it. Foreign
callbacks may not park. Cancellation resumes generated frames with
`CHIMI_FIBER_CANCEL`; generated cleanup returns normally through C before the
fiber becomes terminal.

The public ABI is `include/chimi_fiber.h`. Its version, command values, outcome
values, stack bounds, and structure layouts are compatibility boundaries.
`capture_backtraces = 0` is the trace-off fast path and does not capture or
format a backtrace at park time.

On macOS and Linux arm64/x86-64, each scheduler thread installs an alternate
signal stack. A process handler translates a fault only when both the
interrupted stack pointer belongs to the active fiber and the fault address is
inside that fiber's guard page. That fault returns
`CHIMI_FIBER_STACK_OVERFLOW`; every unrelated fault is delegated to the host's
previous signal disposition.

Run the forcing fixture with:

    cargo test -p chimi-fiber

Run the connected compiler/runtime conformance lab with:

    cargo run --release --bin chimi-concurrency-lab -- all

Run the source-level forcing pipeline with:

    cargo run -p chimi -- build --root programs/concurrency-pipeline app/main -o target/concurrency-pipeline

The fixture compiles generated-style C with frame pointers and unwind tables.
It parks through nested and repeated ordinary calls, exposes and updates exact
live roots, resumes with locals intact across interleaved fibers, and verifies
task/channel lifecycles, cooperative cancellation, explicit defect outcomes,
and linked generated/runtime backtraces.
