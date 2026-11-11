# Define Chimi's First Concurrency Kernel

The canonical Wayfinder map is the `prog` epic `ep-167ecf`. Use
`prog show ep-167ecf` for the destination, standing scope, decisions, and fog;
use `prog graph -p chimi` for the live dependency graph.

## Destination

Define and validate Chimi's first user-visible concurrency model without
`async`/`await` syntax. The result must connect static contracts, executable
runtime semantics, structured observability, and measured native behavior.

This map explicitly carries decisions through focused prototypes and the
smallest connected compiler/runtime implementation needed to test them.
Production durability follows later.

## Testing Shape

The forcing artifact is a **Concurrency Conformance Lab**, not one application
that tries to test every concern at once:

| Evidence | Artifact |
| --- | --- |
| Static semantics | Focused compile-pass and compile-fail fixtures |
| Runtime semantics | Deterministic scenarios with controlled time and IO |
| Operational primitives | Small executable probes for each lifecycle or communication primitive |
| Host integration | Adapter-specific tests for clocks, signals, networking, and shutdown |
| Performance | Synthetic workloads with explicit latency, allocation, and tracing budgets |
| Composition | One modest smoke program after the individual contracts hold |

Runtime scenarios also assert structured events, task identities, causal
relationships, cancellation causes, and terminal outcomes.

## Current Frontier

- [Compare async and concurrency primitive models](../research/async-concurrency-primitive-models.md)
  (`prog show ts-7d607f`) — Existing runtimes bundle orthogonal concepts.
  Suspension, concurrency units, structured lifetime, communication,
  isolation, cancellation, failure propagation, supervision, backpressure,
  observability, and scheduling must be chosen separately. Structured tasks,
  bounded channels, and supervised processes solve complementary problems;
  stackful fibers are the first implementation candidate while task semantics
  and the compiler/runtime boundary remain representation-independent.

- [Choose and implement bounded channel semantics](../research/channel-semantics.md)
  (`prog show ts-f566a8`) — The
  first channel is bounded MPSC with capacity-zero rendezvous, FIFO value and
  waiter ordering, explicit coordinated completion, destructive consumer
  abort, atomic cancellation races, recoverable unsent values, and an explicit
  `Received`/`Drained` result. The runtime state machine is connected to finite
  task parking, cancellation, capability cleanup, and causal trace modes.
  Lossy, unbounded, broadcast, and selection surfaces remain separate future
  abstractions.

- [Run the connected concurrency conformance lab](../performance/concurrency-kernel-conformance-baseline.md)
  (`prog show ts-7c4508`) — Compiler fixtures, deterministic native Task/Channel
  scenarios, causal trace modes, performance gates, and one finite worker
  pipeline now run independently or through one composite executable. The lab
  keeps its evidence boundary explicit: source-level Task and Channel calls are
  not yet connected to native scheduling.

- **Choose the first concurrency primitive layers** — `prog show ts-df54c5`.
  The aligned baseline is direct suspension inside the current task,
  parent-owned finite tasks started from thunks, bounded typed channels for
  repeated values and backpressure, and supervised processes for long-lived
  isolated state and lifecycle. Task graphs are relationships derived from
  task composition and traces, not another language primitive. The smallest
  operation surface below is accepted; the current frontier is connecting
  verified Task and Channel calls to native execution (`prog show ts-4e0757`).

### First operation surface

The operation names remain provisional source spelling. Module-qualified
functions keep the runtime model visible while thunks and ordinary values
remain the composition mechanism. The normative ownership, transfer, and
outcome rules are in
[Task and Channel capability contracts](../specification/task-and-channel-capabilities.md).

| Layer | Smallest candidate operations | Meaning |
| --- | --- | --- |
| Direct execution | ordinary function call | Runs in the current task and may park; it creates no concurrency. |
| Finite task | `Task.start(thunk)`, `Task.result(task)`, `Task.cancel(task)` | Starts work automatically owned by the current task, obtains its result through ordinary success/failure propagation, or requests cancellation. A task graph is derived from these relationships. |
| Communication | `Channel.bounded(capacity)`, `Channel.send(sender, value)`, `Channel.receive(receiver)`, `Channel.close(completion)`, `Channel.abort(receiver)` | Creates typed sender, receiver, and producer-completion capabilities backed by one finite queue. Send and receive may park; close drains accepted values, while abort discards unread values. |
| Long-lived process | `Process.name`, `Process.start_named`, `Process.send`, `Process.receive`, `Process.stop` | Runs an isolated state loop with a typed inbox. A reference identifies one incarnation; an optional registered name can route to a replacement. |
| Containment | `Supervisor.process`, `Supervisor.supervisor`, `Supervisor.restart_on_defect`, `Supervisor.policy`, `Supervisor.start`, `Supervisor.start_process`, `Supervisor.start_supervisor`, `Supervisor.remove_process`, `Supervisor.remove_supervisor`, `Supervisor.stop`, `Supervisor.wait` | Uses distinct inert specifications and management handles for processes and nested supervisors. Restart behavior composes explicitly over either specification. Only a root start or an owning supervisor creates runtime work. |

The initial surface deliberately omits detached tasks, unbounded channels,
multi-channel selection, request/reply calls, shared mutable cells, locks, STM,
distributed process names, persistence, and durability. Task combinators such
as bounded parallel traversal, collect-all, fail-fast, and first-success are
ordinary functions built above the task operations and may hide an internal
owned child set.

Task ownership does not require a source-level `Scope` or `TaskGroup` value.
`Task.start` creates an explicit task handle and the runtime attaches the child
to the current task. Results remain explicitly joined or composed. Discarding
a handle never detaches its work; tooling diagnoses unobserved results, and a
parent cannot terminate until remaining children have been cancelled and
drained. Work intended to outlive its caller must instead be a supervised
process.

Ordinary calls are sequential by default. A suspending operation parks only
the current lightweight task; it does not block the runtime scheduler, host
event loop, carrier thread, or other tasks and processes. Concurrency begins
only through an explicit operation such as Task.start. The runtime root runs
inside the same task model, so a parked main task leaves the runtime able to
service host events and other runnable work.

The first channel topology is many-producer, single-consumer. Construction
creates separate `Channel.Sender<T>`, `Channel.Receiver<T>`, and logical
producer-completion capabilities. The sender may be shared with producer tasks,
the receiver remains with one consumer, and the coordinator retains completion
authority so an ordinary producer cannot end the shared stream. These
distinctions are semantic even if the handles are zero-cost references to one
runtime allocation. Exact construction/destructuring syntax and the final name
of the completion capability remain open until Chimi's ownership model is
defined.

The bounded channel is a FIFO queue with suspending backpressure. Capacity zero
is a rendezvous: send and receive must pair and no value is buffered. Positive
capacity is the exact maximum number of accepted but unreceived values. A send
is accepted immediately when a receiver is waiting or buffer capacity remains;
otherwise only the sending task parks. A receive consumes the oldest accepted
value, atomically reserves freed capacity for the oldest waiting send, and then
makes that sender runnable. A newer sender cannot barge into that reservation.
Concurrent send order is the order in which sends linearize at the channel. The
default never drops, overwrites, or conflates values. Dropping, conflation,
broadcast, multiple consumers, and unbounded buffering require distinct future
abstractions.

Channel termination has two intentionally different operations. Producer-side
`close` is an explicit, idempotent global transition performed through the
coordinator's completion capability. It rejects blocked and future sends but
preserves accepted values. A blocked or future send fails with
`Channel.SendError.Closed(value)`, retaining ownership of the unaccepted value.
After accepted values drain, receive returns the dedicated
`Channel.Receive.Drained` result. Ordinary delivery is
`Channel.Receive.Received(value)`, so a channel carrying `Option<T>` remains
unambiguous.

Consumer-side `abort` is an idempotent destructive transition. It rejects
blocked and future sends with `Channel.SendError.Aborted(value)`, discards
accepted unread values, and makes parked and future receives fail with
`Channel.Aborted`. Abort after close upgrades the remaining drain to an abort;
close after abort is a no-op. Abort does not cancel unrelated tasks or affect a
value already returned to consumer code. `abort` is preferred over `cancel` so
cooperative task cancellation and destructive channel termination remain
distinct in source and traces.

Send, receive, close, abort, and task cancellation each have an atomic
linearization point. Whichever competing transition linearizes first wins. If
cancellation wins before send acceptance, the value was not delivered and the
waiter is removed. If acceptance wins, later cancellation cannot revoke the
send. The equivalent rule for receive prevents a cancelled waiter from taking
a value while also preventing a delivered value from silently returning to the
channel. A failed send therefore never means "the value might also have been
delivered."

Completion never follows from one producer finishing or from nondeterministic
garbage collection. The coordinator closes only after all producers terminate.
The static and cleanup contract is specified by
[task and channel capabilities](../specification/task-and-channel-capabilities.md):
senders are shareable, receiver and completion authority are exclusive and
transferable, task handles remain parent-local, and abandoning an open
exclusive endpoint aborts rather than leaving peers parked or pretending the
stream completed gracefully.

Supervision configuration is separate from process execution.
`Supervisor.process` is the constructor for an inert process
specification: it records a start thunk and shutdown policy but starts no work.
This replaces the generic candidate name `Supervisor.child`; Chimi does not
introduce a general source-level "child" concept. A supervisor consumes process
specifications and owns the resulting runtime processes.

Restart policy is a source-visible transformation rather than an implicit
property of every specification:

```chimi
let counter =
  fn() { Counter.start() }
  |> Supervisor.process(
    Duration.seconds(5),
  )
  |> Supervisor.restart_on_defect
```

The same operation composes over a nested-supervisor specification. The piped
specification determines the restart target: a process specification recreates
that process, while a supervisor specification reconstructs that supervisor
and its declared subtree. A specification without this transformation does not
restart: after it terminates, its owner continues, its capacity slot is
released, and no restart budget is consumed.

Restart remains supervisor-owned composition. An ordinary retry transformer can
retry expected failures while starting or handling work, but it cannot observe
a later process defect without a blocking lifecycle operation that reifies the
defect. Applying retry independently to those lifecycle operations also creates
per-member budgets. Giving the transformer the owning supervisor's shared
budget, membership serialization, removal state, and shutdown state merely
recreates supervisor-specific restart behind a generic name. The supervisor
therefore owns replacement and accounting; `Supervisor.restart_on_defect`
remains provisional surface spelling for the inert-specification
transformation.

Process restart follows the BEAM boundary rather than implying transparent
durability. Each running incarnation has a unique reference and owns its local
state and mailbox. A defect destroys that reference, state, the in-flight
message, and unread mailbox messages. A supervisor may run the start thunk to
create a new incarnation. An optional typed process name can be registered by
the replacement, so future sends resolve to it; a direct reference never
retargets, and sends during the registration gap fail as unavailable.

State survival is separately composed. A process may reconstruct initial state
from a longer-lived in-memory owner, persistent storage, or an event log, but
the process and supervisor APIs do not checkpoint state or promise delivery.
Storage must record state before acknowledgement or by another explicit
consistency protocol; cleanup on defect is not a persistence boundary. Durable
mailboxes, replay, acknowledgement, and redelivery belong to later
abstractions built above the concurrency kernel.

The first supervisor has one explicit restart transformation rather than
configurable OTP-wide strategies. `Supervisor.restart_on_defect` restarts its
piped process or nested-supervisor specification only after a defect. Normal
return, expected failure, cancellation, and explicit stop do not restart. Each
supervisor owns a finite rolling restart limit shared by all of its directly
owned restartable specifications. Every attempted replacement counts,
including failed initialization. Exceeding the limit terminates the
supervisor's remaining processes and nested supervisors, and the supervisor defects with a causal
`Supervisor.RestartLimitExceeded`; a parent supervisor may then replace that
supervisor and its subtree only when that nested-supervisor specification was
also transformed with `Supervisor.restart_on_defect`. An uncontained root
escalation terminates the program. The runtime emits the process defect,
restart attempts, exhausted limit, and supervisor escalation as linked
structured events.

Defect replacement is immediate and has no supervisor-level delay or backoff
configuration. The owner charges the restart attempt before invoking the start
thunk. It does not begin replacement until the terminated incarnation has
released registered routing and its structured child tasks have been cancelled
and drained, so old and new incarnations never overlap. The rolling restart
limit bounds defect loops. Expected transient failures and their delays remain
ordinary composition above this mechanism rather than being reclassified as
defects.

Supervisor shutdown is out-of-band lifecycle control, not an ordinary mailbox
message. `Supervisor.stop` first removes registered routing, requests shutdown
in reverse process declaration order, cancels and drains each process's
structured child tasks, and permits cleanup for the process specification's
finite `shutdown_within` duration. It forcefully terminates a process that
exceeds that duration and returns only after the complete subtree has stopped.
Graceful shutdown means cleanup-bounded, not mailbox-draining: unread messages
are discarded unless the process protocol explicitly drains them before
returning. Requested and forced shutdown are not defects and do not trigger
restart. The runtime must support immediate destructive termination, while a
public `Supervisor.abort` operation remains deferred until a source-level
forcing case requires it.

Supervision membership is dynamic in the first kernel rather than fixed at
`Supervisor.start`. A running supervisor can synchronously accept a process
specification, start and assume ownership of its process, and return a stable
supervisor-management identity distinct from both the process incarnation
reference and any registered routing name. It can also shut down and remove
that managed process without triggering restart. Membership changes are
serialized by the supervisor, count toward the same restart and shutdown
policies, and have a configured finite capacity so dynamic creation cannot
silently become unbounded. `start_process` and `start_supervisor` return
distinct `ManagedProcess` and `ManagedSupervisor` lifecycle handles;
`remove_process` and `remove_supervisor` consume the corresponding ownership.

Removal is a synchronous terminal revocation of that membership. After
`remove_process` or `remove_supervisor` returns, no incarnation owned by that
membership is running and no pending replacement may create another one. The
supervisor serializes removal with defect handling: removal ordered before a
restart prevents it; removal ordered after initialization has begun waits for
or interrupts that initialization and then stops the replacement. An attempt
that had already begun still consumes restart budget.

Admission at that capacity is fail-fast rather than queued.
Starting a dynamic process reserves a slot before initialization; failed
initialization releases it, restart retains it, and removal or non-restarting
termination releases it. Concurrent requests are serialized and excess
requests return the expected `Supervisor.CapacityReached` failure. Any waiting,
fairness, retry, or overload policy is explicitly composed above the supervisor
with bounded channels, pools, or ordinary retry functions. Rejection is emitted
as a structured admission event.

Dynamic membership is supervisor-local state and does not survive replacement
of the supervisor itself. A replacement supervisor reconstructs only its
declared initial blueprint; previous dynamic management handles become stopped.
Applications that must reconstruct dynamic membership persist the necessary
domain data externally and deliberately reapply it during initialization. The
supervisor does not make dynamic topology durable implicitly.

Nested supervisors use a distinct inert `Supervisor.supervisor` specification
alongside `Supervisor.process`; Chimi does not expose a generic child
constructor. A policy declares separate initial `processes` and `supervisors`
collections. Its required `capacity` bounds their combined direct
membership, while every nested supervisor independently bounds its own direct
membership. A nested supervisor exhausting its restart limit defects to its
parent. If its specification was transformed with
`Supervisor.restart_on_defect`, replacing that whole supervisor counts as one
parent-level restart and reconstructs only the nested supervisor's initial
blueprint; otherwise it remains stopped.

Static topology is batch data rather than a builder chain: `Supervisor.policy`
accepts the process and supervisor collections, capacity, and restart limit.
Composition stays at meaningful boundaries. A start thunk can pipe through
`Supervisor.process`, either inert specification can pipe through
`Supervisor.restart_on_defect`, a nested policy can pipe through
`Supervisor.supervisor`, and a root policy can pipe through `Supervisor.start`.
Runtime additions and removals compose through the dynamic supervisor
operations. Repeated `manage_process` or `manage_supervisor` transformations
are deliberately absent.

### Initial execution representation

The first runtime should back each task with a stackful fiber so an
ordinary call can park without rewriting its transitive callers into public or
ABI-visible state machines. `Fiber` is runtime vocabulary, not a Chimi source
concept: programs create and observe tasks.

The accepted bootstrap mechanism is a statically linked Rust runtime component
behind a narrow C ABI, initially using `corosensei`. Generated code publishes
exact live-root frames before approved park calls. Cancellation resumes the
fiber with an explicit command so generated C cleanup returns normally; neither
cancellation nor defects use Rust unwinding across C. Fixed guarded stacks,
platform overflow-to-defect handling, unwind metadata, and collector integration
are runtime implementation obligations, not source semantics.

Core and persisted execution must not expose raw fiber stacks or depend on this
representation. The mechanism may be replaced behind the runtime ABI during
self-hosting. The measured evidence and remaining implementation limits live in
[`prototypes/stackful-task-runtime`](../../prototypes/stackful-task-runtime/RESULTS.md).

- [Evaluate synchronization STM reactive streams and effect handlers](../research/synchronization-stm-reactive-effects.md)
  (`prog show ts-c3aa4f`) — Scope-bound suspension-aware synchronization and
  direct-style streams over bounded channels fit as narrow official
  primitives. OCaml-style one-shot effects are a strong internal suspension
  candidate. STM and public effect handlers should wait for memory-model and
  continuation-ownership evidence.

## Scope Boundary

Persistent queues, poison-job policy, recovery, distributed process location,
and hot code upgrades are outside this map. They must build on the concurrency
kernel rather than determine it prematurely.

The finite Task and Channel kernel is complete. Continue with
[Define Chimi's First Supervised-Process Kernel](first-supervised-process-kernel.md)
for the long-lived process and supervision layer.
