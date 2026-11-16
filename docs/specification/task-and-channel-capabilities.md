# Task and Channel Capability Contracts

## Status

This is the normative first-kernel static contract for finite tasks and bounded
channels. Source spelling remains provisional; the ownership and outcome rules
do not depend on stackful fibers, a scheduler, or a particular memory manager.

## Goals

The contract must make the blessed concurrent path safe and inspectable without
adding `async`/`await`, a public effect row, a universal `Scope` value, or a
Rust-style trait that callers must know to import. It must also preserve a
future parallel scheduler rather than making safety depend on all tasks sharing
one carrier thread.

## Concurrent value properties

The compiler derives two properties independently of callable type channels:

- A **transferable value** may move from one task to another without leaving an
  unsafe alias behind.
- A **shareable value** may be referenced by more than one task concurrently.
  Every shareable value is transferable; not every transferable value is
  shareable.

These are structural execution properties, not interfaces, imports, generic
rows, or runtime effects. Tooling exposes them and diagnostics identify the
field, capture, provider, or opaque type that prevents derivation.

Primitive immutable values are shareable. A struct, enum, record, list, map, or
function value is shareable when every reachable field or capture is shareable.
A value with exclusive mutable state or a unique runtime capability may be
transferable without being shareable. An opaque type must publish enough module
metadata for the compiler to know its property; private representation does not
silently grant shareability.

The first kernel exposes no general shared mutable cell. Controlled mutation
remains task-local, process-owned, runtime-internal, or behind an official
synchronization abstraction whose contract establishes safe sharing. This lets
ordinary immutable Chimi data cross tasks without ceremony while keeping the
parallelism ceiling open.

## Task contract

### Starting work

`Task.start` accepts an unexecuted zero-argument function value and starts it as
a child automatically owned by the current task. The operation transfers the
thunk and any exclusive captures into the child. A reusable function with only
shareable captures remains reusable; a closure with an exclusive capture is a
one-shot transferable value and cannot be started twice.

Conceptually:

```text
Task.start:
  fn<T, F, R>(fn() -> T errors F needs R)
    -> Task<T, F>
    needs R
```

This is a semantic shape, not accepted generic syntax. Starting resolves the
thunk's remaining needs from the current execution environment and snapshots
those provider implementations for the child. A provision already attached by
`using` remains attached and takes precedence. Consequently:

- open needs appear at the `Task.start` call and therefore in its caller;
- the running task and `Task<T, F>` handle do not retain an open needs row;
- a provider captured for concurrent work must be shareable;
- leaving a `using` boundary after start does not remove the child's captured
  provider.

The returned task handle is a **parent-local capability**. It may be stored and
composed by its owning task, but it cannot be returned beyond that task, sent
through a channel, captured by a sibling, stored in a process, or used to detach
the child. Higher-level task combinators execute in the owner and may manage
collections of its handles.

### Observing and cancelling

`Task.result(task)` may park the owner until the child terminates. Success
returns the success value; an expected failure bubbles through the ordinary
failure channel. A child expected failure does not otherwise cancel its owner
or siblings.

A child defect is not an ignorable value. It immediately begins unwinding its
task containment: the owner cancels and drains its other children, then defects
with the linked causal path. Only an uncontained defect reaching the runtime
root terminates the program. `Task.result` therefore does not add defects to the
expected failure row.

Cancellation is also a runtime terminal cause rather than an expected failure.
`Task.cancel(task)` is idempotent: it requests cancellation and does not return
until the child and its descendants have terminated and completed cleanup. The
explicit cancel operation treats the requested cancellation as successful
`Void`; a defect that wins the race still follows defect containment. Calling
`Task.result` on a child cancelled by another cause propagates cancellation to
the waiting task.

Task completion follows these rules regardless of whether its handle was
observed:

- normal return or expected failure cancels and drains any remaining children
  before becoming terminal;
- cancellation recursively cancels and drains children;
- defect performs the same cleanup while preserving the defect as primary;
- losing a handle never detaches work;
- tooling diagnoses statically obvious discarded handles and traces every
  unobserved terminal outcome.

## Channel capability contract

`Channel.bounded` creates one runtime queue and three typed logical
capabilities:

| Capability | Property | Authority |
| --- | --- | --- |
| `Channel.Sender<T>` | shareable | submit transferable `T` values |
| `Channel.Receiver<T>` | exclusive and transferable | receive values or abort the channel |
| `Channel.Completion<T>` | exclusive and transferable | gracefully close the producer side |

Construction returns the structural record
`{ sender: Sender<T>, receiver: Receiver<T>, completion: Completion<T> }`.
Completion is distinct from `Sender<T>` so an ordinary producer cannot
terminate a shared stream.

Receiver and completion capabilities have one owning task at a time. They may
move into a child thunk, including as an exclusive closure capture, but cannot
be captured by two children or used by the old owner after transfer. Repeated
sequential operations by the current owner do not consume the receiver. The
compiler diagnoses provable aliasing and use-after-transfer.

`Channel.send(sender, value)` is a transfer boundary. Acceptance moves the
value into the channel; receive moves it to the receiver task. The compiler
marks every exclusive binding the payload carries as moved
([moving exclusive values](#moving-exclusive-values)). If close or abort
wins before acceptance, `Channel.SendError.Closed(value)` or
`Channel.SendError.Aborted(value)` returns the unaccepted value through the
ordinary failure channel. Cancellation before acceptance removes the waiter
and transfers no value.

### Moving exclusive values

An exclusive binding has one owner in its task as well as across tasks: a value
built from it takes it, so the binding cannot stay usable while the value
crosses a task boundary. The compiler follows this through a function body
(`ts-184972`):

- A value **carries** an exclusive binding when it is that binding, or is built
  from a value that carries it: an element of a list or tuple literal, a field
  of a record literal or update, an argument of a constructor call, the value
  of a block, of any `case` arm, or of a `break` out of a loop, nested at any
  depth and in parentheses. A field read out of a value that is not a binding,
  such as `boxed(r).inner`, carries what that value carries.
- A call's result carries what an argument carries when the result type is not
  always shareable and the argument's type occurs in it: as the result itself,
  or inside a field, variant field, element, or type argument. A function type,
  an interface, or an open type parameter in the result may hold any value. The
  compiler does not look inside the called function, so `wrap(r) -> Hand`, where
  `Hand` has a `Receiver<Int>` field, is assumed to embed `r: Receiver<Int>`,
  while `next_hand(rs: Receiver<Hand>) -> Hand` does not embed `rs`, since
  `Receiver<Hand>` does not occur in `Hand`. Calling a one-shot closure whose
  result is not always shareable also carries the closure.
- Binding a value with `let`, or as a loop's initial state, moves every binding
  the value carries into the new binding, which becomes the only owner. A
  destructuring `let`, or a `case` whose subject is not a binding, moves what
  the subject carries into its patterns. A later use of a moved binding is
  `T4241`; moving a binding bound outside a loop from inside the loop is
  `T4250`.
- `Channel.send`, `Process.send`, `Supervisor.start_handoff`'s argument,
  `Task.start`, a `using` provider, and `Supervisor.remove_process` and
  `remove_supervisor` transfer every binding their operand carries.
- Passing a binding to a call whose result cannot embed it does not move it.
  `Channel.receive(r)`, or a helper `next(r) -> Int`, may run on every pass of a
  loop. A value that is built and then dropped, never bound or transferred,
  moves nothing.
- Reading a field of a binding (`channel.receiver`), or matching a binding in a
  `case`, gives a new, independent binding and moves nothing, so the record that
  `Channel.bounded` returns can be split into its three capabilities.

- `Channel.Receive<T>` has the concurrent properties of `T`, as
  `Process.SendError<M>` has those of `M`: a received `Receive<Receiver<Int>>`
  is exclusive while it is still wrapped, so two children cannot capture it,
  and a received `Receive<Int>` stays shareable (`ts-bb95dc`).

This rule stops at the function boundary. The flows it does not follow are
checked at run time; [where each rule is checked](#where-each-rule-is-checked)
lists them.

Capacity, FIFO admission, graceful drain, abort, and cancellation
linearization follow [bounded channel semantics](../research/channel-semantics.md).
Channel operations add no effect row. `send` and `receive` may park and tooling
may expose that compiler-private execution property.

### Handing a capability back

Ownership of a Receiver or Completion follows the value that holds it, on the
native runtime (ts-f4e749):

- **Sent through a channel.** When the channel accepts or parks the send, every
  exclusive capability in the message is in flight: no task owns it, and the
  sender's own copies are stale. The task that receives the message owns them.
  A refused send (`SendError.Closed(value)` or `Aborted(value)`) gives them
  back to the sender, whose copies work again. A message discarded unread (its
  channel aborted or released while it was queued, or its parked send
  cancelled) ends them as an owner's exit would: an open Receiver or
  Completion aborts its channel.
- **Returned as `Task.result`.** A child's success or failure value hands its
  exclusive capabilities to the task that started it before the child ends,
  so the child's cleanup leaves them open. Only that task can read the result,
  because a task handle is parent-local. A parent that never reads the result
  still owns them, and its own exit cleans them up.
- **Sent to a process.** A `Process.send` message follows the channel rule
  (CHI-213). An accepted message puts its capabilities in flight, and the
  incarnation whose receive takes it owns them. A refused send
  (`SendError.Full(message)` or `Unavailable(message)`) leaves them with the
  sender, and a message still queued when the incarnation ends is discarded:
  its open channels abort.
- **Handed to a new process.** `Supervisor.start_handoff`'s argument travels
  the same way. An accepted start puts its capabilities in flight, and the new
  incarnation owns them once its body starts. A refused start
  (`HandoffError.CapacityReached(argument)` or `Stopped(argument)`) leaves them
  with the caller, and an incarnation that ends before its body starts
  discards the argument.

Only Receivers and Completions carry an owner token. A `ManagedProcess` or
`ManagedSupervisor` inside a sent value moves by the static rule alone: the
runtime does not track which task holds the handle, and a task that ends
holding one leaves the membership running.

`Channel.SendError<T>` has the concurrent properties of `T`, as
`Channel.Receive<T>` does, so the checker treats a refused exclusive value as
exclusive (ts-f704d3).

## Channel variants

`Channel.bounded` gives one queue with one receiver. Services also need a
queue that several workers take from, a stream that every subscriber sees,
and a latest value that subscribers follow. These are three variants (G29).
They reuse the channel vocabulary: `Channel.Receive<T>`,
`Channel.SendError<T>`, `ChannelError`, `Channel.Completion<T>`, and
`Channel.close`. A variant that can lose values has its own module and types,
so a reader sees the loss where the value is sent and where it is received
([bounded channel semantics](../research/channel-semantics.md#deferred-surfaces)).

```text
Channel.add_receiver: fn<T>(Receiver<T>) -> Receiver<T>

Broadcast.bounded:    fn<T>(Int) -> { publisher: Broadcast.Publisher<T>,
                                      completion: Channel.Completion<T> }
Broadcast.publish:    fn<T>(Broadcast.Publisher<T>, T) -> Int errors Channel.SendError<T>
Broadcast.subscribe:  fn<T>(Broadcast.Publisher<T>) -> Channel.Receiver<T>

Watch.starting_with:  fn<T>(T) -> { publisher: Watch.Publisher<T>,
                                    completion: Channel.Completion<T> }
Watch.publish:        fn<T>(Watch.Publisher<T>, T) errors Channel.SendError<T>
Watch.subscribe:      fn<T>(Watch.Publisher<T>) -> Watch.Subscription<T>
Watch.receive:        fn<T>(Watch.Subscription<T>) -> Channel.Receive<T> errors ChannelError

Select.watch:         fn<T, A, F, R>(Watch.Subscription<T>, fn(Receive<T>) -> A errors F needs R)
                        -> Event<A, F | Channel.Aborted> needs R
```

| Capability | Property | Authority |
| --- | --- | --- |
| `Broadcast.Publisher<T>` | shareable | publish, subscribe |
| `Watch.Publisher<T>` | shareable | publish, subscribe |
| `Watch.Subscription<T>` | exclusive and transferable | receive the latest value |

A broadcast subscription is an ordinary `Channel.Receiver<T>`, and a watch
subscription follows every Receiver rule: one owning task, moved by a send,
`Task.start` or a `using` provider, handed back as a `Task.result`, ended by
its owner's exit.

### Several receivers

`Channel.add_receiver(receiver)` returns another receiver of the same queue,
owned by the calling task. The argument is not moved: the result is a new
capability, not the argument, so the
[carry rule](#moving-exclusive-values) does not apply to this call. A pool
adds one receiver per worker and moves each into its worker:

```text
loop count = 0 {
  if count == workers { break count }
  let mine = Channel.add_receiver(jobs.receiver)
  let _ = Task.start(fn() { work(mine) })
  count + 1
}
```

- Each accepted value goes to exactly one receiver. Receivers parked in
  `Channel.receive` are served in the order they parked. A receiver waiting
  in `Select.first` is woken and competes when it polls again; a parked
  plain receive is served before any select. There is no fairness between
  receivers beyond that.
- `Channel.abort(receiver)` ends that receiver. Its later receives fail with
  `Channel.Aborted`, and so does a select on it. An owner's exit ends the
  receivers it owns, and so does discarding a message that carries one.
- The queue is aborted when its last receiver ends, by any of these routes.
  A receiver in flight inside a message counts as live. For a channel that
  never calls `add_receiver` this is exactly the one-receiver rule: aborting
  or abandoning the receiver aborts the channel.
- A receiver added to an aborted queue, or through a receiver that has
  ended, is ended at once.
- A value handed to a parked receiver belongs to that receiver, as on a
  one-receiver channel. If its task is cancelled before it resumes, the value
  is dropped, not requeued: the delivery linearized first.

### Broadcast

`Broadcast.bounded(capacity)` creates a broadcast whose subscriptions each
buffer up to `capacity` values. The capacity MUST be positive; zero or a
negative capacity defects.

- `Broadcast.subscribe(publisher)` returns a new receiver. It receives every
  value whose publish is accepted after it subscribed, in publish order, and
  no value accepted before.
- `Broadcast.publish(publisher, value)` is accepted when every current
  subscription has room and no earlier publish is parked. At acceptance each
  subscription receives its own copy, and the call returns how many
  subscriptions it reached. Otherwise the publish parks; parked publishes are
  accepted in the order they parked. A slow subscriber therefore slows every
  publisher: nothing is dropped.
- A publish with no subscriptions is accepted and returns `0`. No subscriber
  is promised a value published before it subscribed, so this loses nothing
  a subscriber was owed, and the count keeps it visible.
- Cancelling a parked publish removes it. Nothing was delivered.
- A subscription ends when its last receiver ends (`Channel.abort`, owner
  exit, or `Channel.add_receiver` receivers all ending). It leaves the
  broadcast, and a parked publish is checked again, so a publish that
  waited only on that subscription is accepted.
- `Channel.close(completion)` closes the broadcast. Parked and later
  publishes fail with `SendError.Closed(value)`. Each subscription drains
  its buffered values, then answers `Drained`. A subscription made after the
  close answers `Drained` at once.
- An abandoned completion (its owner ends while the broadcast is open, or
  every publisher and the completion are dropped) aborts the broadcast.
  Parked and later publishes fail with `SendError.Aborted(value)`, every
  subscription is aborted, and a later subscription is aborted at once.
- A task that publishes while its own subscription is full parks until
  someone else receives from it. If nobody does, the program deadlocks, and
  the deadlock report shows the task waiting in a channel send.

### Watch

`Watch.starting_with(initial)` creates a watch whose current value is
`initial`.

- `Watch.publish(publisher, value)` never parks. It makes `value` the current
  value. A subscription that has not yet received the previous current value
  will now receive `value` instead: older unseen values are replaced, never
  queued.
- `Watch.subscribe(publisher)` returns a subscription that has not seen the
  current value, so its first receive returns it at once.
- `Watch.receive(subscription)` returns `Received(value)` with the newest
  value the subscription has not seen. It parks when the subscription has
  seen the current value, until the next publish. After the watch is closed
  it returns the final value if unseen, and then `Drained`. It fails with
  `Channel.Aborted` once the watch is aborted.
- `Channel.close(completion)` closes the watch; later publishes fail with
  `SendError.Closed(value)`. An abandoned completion aborts it, as for a
  broadcast; later publishes fail with `SendError.Aborted(value)`.
- A subscription ends with its owner, like a receiver. There is no
  `Watch.abort`: ending a subscription affects no one else.

### Values

Every subscriber of a broadcast or watch gets its own copy of each value, so
`T` MUST be shareable. `Broadcast.publish`, `Watch.publish` and
`Watch.starting_with` report `T4244` at the value when its type is never
shareable (it holds a Receiver, a Completion, a subscription, a task handle,
or an inbox). When `T` is a generic parameter the checker cannot decide, and
the runtime backstop applies: publishing a value that holds a Receiver,
Completion, subscription, or managed process or supervisor handle defects the
publisher as a channel capability violation. A managed handle sent on an
ordinary channel still moves, since only one receiver gets it. A generic
function can still copy a managed handle within one task, which neither check
sees; removing a member through the second copy then defects and says the
handle was removed already, through it or a copy (CHI-240).

### Choosing among variants

`Select.receive` accepts any Receiver, including a broadcast subscription and
a receiver from `Channel.add_receiver`. `Select.watch` is the watch
subscription's event. Readiness is the same as for a receive: a value,
`Drained`, or `Aborted` makes the event ready. Several tasks may select on
receivers of one queue; each value still goes to one of them.

### Variants as implemented (CHI-231)

- **Native only**, like the rest of the channel runtime. The interpreter
  reports each new function with "requires the bounded Channel runtime".
- **Receivers.** Every receiver is its own registry entry with its own owner
  and token, so transfer, hand-back, parcels and owner cleanup are the
  one-receiver code. A receiver from `add_receiver` points at the queue's
  entry; a broadcast or watch subscription is a queue of its own, fed by
  the broadcast or watch entry.
- **Copies.** The scheduler copies a value for every subscription past the
  first through a new host hook, `copy_channel_value`, so each subscription
  owns its value as an ordinary queued message.
- **Watch subscriptions** are conflating queues of capacity one: a publish
  replaces an unread value and releases the old one.
- **Select commit.** A select still consumes nothing. After it answers with
  a ready receiver, the host's receive is served before any other task runs
  (the reply puts the task at the front of the run queue), so another
  receiver of the same queue cannot take the value in between. This depends
  on nothing between the reply and the receive request letting another task
  run: today the host does no allocation there, and so flushes no channel
  release, whose reply would put the task at the back of the queue.
- **Events borrow their receiver.** A `Select.receive` or `Select.watch`
  event reads its receiver without holding it, so building one moves
  nothing, but using the event after the receiver moved is `T4241` at the
  event (CHI-224), including when the receiver is read from a field such as
  `jobs.receiver`. Moving the receiver into another binding of the same task
  leaves the event valid. A select's answer borrows nothing. An event built
  in a helper function, carried out of a loop by `break`, or taken out of a
  tuple by a pattern loses the link; those cases keep the run-time
  owner-token check (CHI-241).
- **ABI.** Fiber ABI 15 adds requests 36 to 39 (add receiver, broadcast,
  watch, subscribe), capability violations 7 (a published value holds a
  capability) and 8 (an operation on the wrong kind of channel), and the
  `copy_channel_value` hook at the end of `ChimiSchedulerConfig`. Native
  runtime ABI 33 adds the nine intrinsics after `Select.first`.

## Owner exit and automatic cleanup

The runtime tracks children and exclusive capabilities as part of task
containment. Transferring an exclusive capability transfers this cleanup
responsibility.

An open receiver or completion capability that reaches owner unwind without an
explicit terminal operation aborts its channel. Automatic abort is deliberately
destructive: silently converting an abandoned producer or consumer into a
graceful completed stream would claim that all intended values were delivered.
Abort wakes peers, returns unaccepted values to live senders, discards accepted
unread values, and emits the owner terminal cause.

Explicit graceful close remains closed during later owner cleanup and is not
upgraded merely because the completion capability leaves scope. If the
receiver's owner exits before draining, its receiver cleanup still aborts the
remaining channel. Releasing the final sender never implies graceful close;
completion does not depend on garbage collection or destructor timing.

Cleanup ordering for an exiting task is:

1. establish the task's primary terminal cause;
2. request cancellation of live child tasks;
3. abort open exclusive channel capabilities still owned by the task;
4. drain child cleanup and channel wakeups;
5. publish the terminal task outcome.

Cleanup failures are attached as suppressed causal context unless a separately
specified resource contract gives them precedence. Runtime cleanup is bounded
by the host operation and forced-termination rules; it cannot abandon a parked
peer silently.

## Static enforcement and runtime backstop

The compiler must diagnose:

- starting or sending a non-transferable value;
- capturing a non-shareable provider for concurrent work;
- starting the same one-shot thunk twice;
- capturing one exclusive capability into multiple children;
- using an exclusive capability after transfer;
- returning, sending, or otherwise escaping a parent-local task handle;
- concurrent receiver operations or completion attempts that are provably
  aliased;
- treating a sender as graceful completion authority.

When indirect flow or erased foreign code prevents a static proof, each
exclusive runtime capability carries an owner token. An operation with the
wrong owner or an overlapping exclusive operation defects with a precise
capability-violation cause and trace event. It is a programmer/runtime contract
violation, not an expected channel failure. Chimi exposes no unsafe escape hatch
that disables these checks.

### Where each rule is checked

Decision (Byron, 2026-11-12): the compiler checks exclusive moves within one
function body and does not summarize what a called function does with its
parameters. Aliasing that crosses a function boundary is caught at run time by
the owner token.

Rationale: a cross-function rule needs a transfer summary for every function,
iterated to a fixed point with recursion, generics, and function values. The
attempt on `archive/capability-places-summaries` did not converge: its fixed
point hung on a recursive struct walk, it rejected a plain binary-tree walk,
and it still missed calls through a function value or a generic `dup`. Each
fix exposed the next case. The owner token already decides every one of these
programs at the operation that breaks ownership, so a second, partial static
proof would add rejected programs without closing the class.

Checked at compile time (within the function that holds the binding):

- every rule in [moving exclusive values](#moving-exclusive-values): a value
  built from an exclusive binding takes it; a `let`, loop state, or pattern on
  a built value moves it; a send, `Task.start`, `using` provider, or removal
  transfers what its operand carries; a call's result carries an argument whose
  type can occur in the result type;
- capturing one exclusive capability into two children, or using it after a
  transfer, in the same function (`T4241`), and moving an outer binding from
  inside a loop (`T4250`);
- the concurrent properties of every type, including `Channel.Receive<T>`
  following `T`.

Checked at run time (native, by the owner token):

- a callee that hands its parameter to a task while the caller keeps using the
  argument: the caller's next operation on it defects;
- a capability duplicated by code the checker does not follow: a generic
  `dup<T>` reached through a function value (`let f = dup; f(r)`), a
  higher-order `twice(x, f)`, or any other call through a function parameter;
  the second `Task.start` that captures it defects;
- one capability read out of the same binding twice (two reads of
  `channel.receiver`, or a pattern binding beside the binding it matched) and
  handed to two tasks: the second hand-off defects;
- any of these after the task that was handed the capability has ended: its
  cleanup aborted the channel and dropped the capability, so the stale copy
  still defects, and the message says the owner ended instead of naming one;
- a stale copy sent in a message (a duplicate sent to the child that captured
  the other copy): the send defects the sender, naming the owner, because a
  sent capability travels with its message (see
  [hand-back](#handing-a-capability-back));
- a stale copy used by the task that owns the capability: the use defects,
  and the message says this copy is stale and that task owns the capability
  (it may already have closed or aborted the current copy);
- the sender's own copy of a capability it sent, while the message waits to
  be received: the use defects, and the message says the capability was sent
  in a message no task has received yet.

The defect names the capability, the task that broke the contract, and the task
that owns the capability, for example
`channel capability violation: task 1 cannot hand the Receiver of channel 1 to
task 3: task 2 owns it`, or
`channel capability violation: task 1 cannot use the Receiver of channel 1 in
Channel.receive: task 2 owns it`. A child's violation defects its owner, so the
program ends with the child's message. `tests/cli.rs` `native_backstop_*`
build and run one program per route.

Not yet covered:

- the interpreter has no bounded-channel runtime (`Channel.bounded` defects with
  "requires the bounded Channel runtime"), so these routes run natively only;
- a violation emits no dedicated trace event; the trace shows only the
  defected task's cancellation and terminal events.

## Conformance matrix

| Kind | Required evidence |
| --- | --- |
| Type shape | `Task.start` captures remaining needs; `Task<T, F>` carries only terminal success/failure types; no effect row appears. |
| Immutable sharing | Nested immutable values and capture-only functions cross task boundaries without annotations. |
| Provider safety | A shareable provider starts successfully; a task-local provider diagnostic identifies the captured requirement. |
| Exclusive transfer | Receiver moves into exactly one consumer thunk; old-owner use and two-child capture fail compilation. |
| Parent-local handle | Local result/cancel composition passes; returning or channel-sending a task handle fails compilation. |
| Value transfer | Accepted send invalidates an exclusive value at the sender; close/abort failure exposes it only through the failure pattern. |
| Dependency lifetime | A task started inside `using` continues with the captured provider after that expression boundary ends. |
| Expected failure | Child failure remains stored until `Task.result` observes and bubbles it; siblings continue unless a combinator cancels them. |
| Defect | Child defect immediately unwinds its owner containment, cancels siblings, and retains its causal path. |
| Cancellation | Explicit cancel waits for descendant and capability cleanup without becoming an expected failure. |
| Owner cleanup | Abandoned receiver/completion capabilities abort and wake every parked peer; final sender release does not close. |
| Runtime backstop | Dynamically obscured double ownership defects deterministically with capability and task identity. |
| Observability | Events link start, park, transfer, wake, close/abort, cancellation, cleanup, and terminal outcomes without payloads by default. |

Compiler fixtures and runtime conformance scenarios must enforce this matrix
without exposing fiber stacks, owner tokens, or execution properties in the
source language or public callable identity. The accepted private execution
representation is recorded in the
[first concurrency kernel map](../wayfinding/first-concurrency-kernel.md#initial-execution-representation).
