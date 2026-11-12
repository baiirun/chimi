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

An exclusive value has one owner in its task as well as across tasks: a value
built from it takes it, so the place it came from cannot stay usable while the
value crosses a task boundary. The compiler follows this through a function
body, and through calls by per-function summaries (`ts-184972`, `ts-05c0da`,
`ts-a2fc84`). A **place** is a binding or a part of one: a field read out of a
place (`channel.receiver`), or a pattern binding over a part of a `case`
subject that is a place. Moves are tracked for exclusive values and for values
whose type holds a type parameter (`x: T`, `Option<T>`), since `T` may be
exclusive; a function or interface value is not tracked, so it may be called
twice.

- A value **carries** a place when it is read out of that place, or is built
  from a value that carries it: an element of a list or tuple literal, a field
  of a record literal or update, an argument of a constructor call, the value
  of a block, of any `case` arm, or of a `break` out of a loop, nested at any
  depth and in parentheses. A field read out of a value that is not a place,
  such as `boxed(r).inner`, carries what that value carries. A value built
  from two parts that carry one place, such as `(r, r)` or
  `Pair(channel, channel.receiver)`, is `T4241`.
- A call's result carries what an argument carries when the result type is not
  always shareable and the argument's type occurs in it: as the result itself,
  or inside a field, variant field, element, or type argument. A function type,
  an interface, or an open type parameter in the result may hold any value. The
  result type decides, not the called function's body, so `wrap(r) -> Hand`,
  where `Hand` has a `Receiver<Int>` field, is assumed to embed
  `r: Receiver<Int>`, while `next_hand(rs: Receiver<Hand>) -> Hand` does not
  embed `rs`, since `Receiver<Hand>` does not occur in `Hand`. Calling a
  one-shot closure whose result is not always shareable also carries the
  closure.
- Binding a value with `let`, or as a loop's initial state, moves every place
  the value carries into the new binding, which becomes the only owner. A
  destructuring `let`, or a `case` whose subject is not a place, moves what the
  subject carries into its patterns. Moving a place moves its parts, and a
  place one of whose parts moved can no longer be used whole: a later use of a
  moved place, or of a place it is part of, is `T4241`, and its other parts
  stay usable. So the record that `Channel.bounded` returns splits into its
  three capabilities by three field reads, and a second
  `let b = channel.receiver` is `T4241`.
- A place **escapes** when it reaches a transfer boundary: `Channel.send`,
  `Process.send`, `Task.start`, a `using` provider, `Supervisor.remove_process`
  or `remove_supervisor`, a closure that captures it, or a call that transfers
  it. A place moved into a binding escapes when that binding, or a part of it,
  escapes. A move that goes no further than a local binding leaves the value
  in the task.
- A call of a function whose body makes a parameter, or a part of it, escape
  moves the same part of the argument (or, for a built argument, what it
  carries). Each named function has a summary of the parameter parts that
  escape, and of the parts it reads, computed to a fixed point over the
  project, so the order of declarations does not matter; a closure the
  compiler sees built has a summary of what escapes too. A function that only
  moves its parameter into a local, such as `fn next(r) { let q = r; ... }`,
  transfers nothing. Passing an argument that only a returned value embeds
  moves nothing at the call; the result carries it as above.
- An argument passed to a named function is checked only at the parts the
  function reads: a record whose `receiver` moved may go to a function that
  reads only its `sender`, and passing it to one that reads `receiver` is
  `T4241`. A `case` subject is checked only for a move of the place itself or
  of a place it lies in; its pattern bindings check the parts they read.
- A move inside a loop of a place bound outside it is `T4250` when the place
  escapes during the pass, since the next pass would take it again. A
  `let r = channel.receiver` inside the loop that goes no further is not.
- Reading a place moves nothing: `Channel.receive(r)`,
  `Channel.receive(channel.receiver)`, or a helper `next(r) -> Int` that only
  reads its parameter may run on every pass of a loop. A value that is built
  and then dropped, never bound or transferred, moves nothing. Matching a place
  in a `case` moves nothing until a pattern binding is taken, which moves that
  part of the subject.
- Parts are named by field (`h.receiver`, and the `receiver` field of
  `Hand.Over(x)`), by tuple position, and for list patterns by position from
  the front or back and the rest. Two list parts are apart when no list length
  that both patterns match puts them on a common element, so the `r` and
  `rest` of `[r, ..rest]` are apart, while `[a, ..]` and `[.., z]` matched on
  one list may be the same element. Paths deeper than four steps are cut to
  four, which names a larger part, so summaries over recursive data stay
  finite.
- `Channel.Receive<T>` is what `T` is: a received exclusive value is exclusive
  before it is matched out, so one `Received(receiver)` cannot be captured by
  two children (`ts-bb95dc`).

Two flows are outside this rule, and the runtime owner token is their only
backstop. A call through a function value the compiler did not see built (a
function parameter, an interface method, or a closure read out of a received or
returned value) does not move its argument when that function transfers its
parameter. And the failure value of `Channel.send`, whose `Closed(value)` and
`Aborted(value)` hand the payload back, is not exclusive whatever the payload
is, so it can be captured by two children before it is matched out.

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
