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
marks an exclusive binding as moved when the payload expression names it
directly: the payload is the binding, or the binding appears as an element of a
list or tuple literal, a field of a record literal, or an argument of a struct
or enum constructor call written in the payload, nested in any of these and in
parentheses at any depth. Nothing else moves a binding. A payload bound to a
`let` before the send, or returned by a function call such as `wrap(r)`, leaves
`r` usable after the send; that aliasing hole is `ts-184972` and is not closed
by this rule. If close or abort
wins before acceptance, `Channel.SendError.Closed(value)` or
`Channel.SendError.Aborted(value)` returns the unaccepted value through the
ordinary failure channel. Cancellation before acceptance removes the waiter
and transfers no value.

Capacity, FIFO admission, graceful drain, abort, and cancellation
linearization follow [bounded channel semantics](../research/channel-semantics.md).
Channel operations add no effect row. `send` and `receive` may park and tooling
may expose that compiler-private execution property.

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
