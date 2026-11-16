# Process and Supervisor Static Contracts

## Status

Contract for `prog show ts-5961ee`, inside the
[first supervised-process kernel](../wayfinding/first-supervised-process-kernel.md).
It states how the process and supervisor API appears in callable contracts and
verified Core, and which programs the compiler rejects before execution.
Runtime behavior is specified in
[process execution and mailboxes](process-execution-and-mailboxes.md),
[process identity and routing](process-identity-and-routing.md), and
[supervisor policy and lifecycle](supervisor-policy-and-lifecycle.md). The
concurrent value properties and transfer rules come from
[task and channel capabilities](task-and-channel-capabilities.md).

Decisions logged on the ticket:

- Chimi has no argument labels. Every call and construction is positional
  (`prog show ts-835fc0`), so the API is ordinary functions.
- `Supervisor.process` and `Supervisor.supervisor` both produce one
  `Supervisor.Member` type that does not carry a message type, so one policy can
  list processes with different message types and nested supervisors together.
- Restart is a `Supervisor.Restart` argument, not a transform, because Chimi has
  no overloading.

## Example

<!-- chimi: pending ts-6ccbc9 ts-f31a20 -->
```chimi
import std/duration as Duration
import std/process as Process
import std/process.{Inbox}
import std/supervisor as Supervisor

pub process name counter: CounterMessage

fn counter_loop(inbox: Inbox<CounterMessage>) -> Void {
  loop count = 0 {
    case Process.receive(inbox) {
      CounterMessage.Add(amount) -> count + amount
      CounterMessage.Finish -> break
    }
  }
}

const counting = Process.spec(counter_loop, 64, Duration.seconds(5))
  |> Process.register(counter)
  |> Supervisor.process(Supervisor.Restart.OnDefect)

const caching = Process.spec(cache_loop, 256, Duration.seconds(5))
  |> Supervisor.process(Supervisor.Restart.Never)

const workers = Supervisor.policy(
  [counting, caching],
  16,
  Supervisor.restarts(2, Duration.seconds(30)),
)

pub fn main() {
  let root = Supervisor.start(workers)
  Supervisor.wait(root)
}
```

## Declarations

```text
process name <identifier>: <Message>        optionally `pub`
```

- The identifier is a module-level value of type `Process.Address<Message>`
  with named routing. It is not a binding: it has no initializer and is never
  evaluated, so it needs no initialization order.
- `Message` MUST be transferable (`T4261`).
- Two declarations with the same identifier in one module are an ordinary
  duplicate-declaration error.

## Types

All types are opaque except the enums. Values are built only through the
functions below.

| Type | Concurrent property |
| --- | --- |
| `Process.Address<M>` | shareable |
| `Process.Inbox<M>` | task-local |
| `Process.Spec<M, E>` | shareable, enforced by `Process.spec` |
| `Process.HandoffSpec<A, M, E>` | shareable, enforced by `Process.handoff_spec` |
| `Process.SendError<M>`: `Full(message: M)`, `Unavailable(message: M)` | that of `M` |
| `Supervisor.Member` | shareable |
| `Supervisor.Policy` | shareable |
| `Supervisor.Restarts` | shareable |
| `Supervisor.Restart`: `Never`, `OnDefect` | shareable |
| `Supervisor.Running` | shareable |
| `Supervisor.ManagedProcess<M>` | exclusive, transferable |
| `Supervisor.ManagedSupervisor` | exclusive, transferable |
| `Supervisor.StartError`: `CapacityReached`, `NameTaken(name: Text)`, `Stopped` | shareable |
| `Supervisor.HandoffError<A>`: `CapacityReached(argument: A)`, `Stopped(argument: A)` | that of `A` |

- **Task-local** is the existing parent-local capability class that
  `Task<T, F>` uses: the value may be passed to ordinary calls within the
  owning task but cannot be returned beyond it, sent, captured by a started
  task, or stored in a spec.
- Payload fields are named, as the
  [standard-library conventions](standard-library-conventions.md#payloads)
  require. `Full` and `Unavailable` hand the transferred message back; that
  is not a copy of input (the conventions' transferred-input rule).
- The variants are written type-qualified (`SendError.Full`,
  `Supervisor.Restart.Never`, `Supervisor.StartError.Stopped`); none is a
  module member.
- `M` and `E` stay in `Process.Spec` so a dynamic start can return a typed
  `ManagedProcess<M>`. `Supervisor.process` drops both: a declared member is
  reached through its `process name`, and its terminal failure is observed only
  through lifecycle events.

## Functions

Shapes use the same informal notation as `Task.start`; they are not accepted
generic syntax. Whether a call parks is an execution property and does not
appear in the type
([ADR 0004](../adr/0004-keep-execution-properties-out-of-callable-types.md)).

```text
Process.spec:
  fn<M, E, R>(fn(Inbox<M>) -> Void errors E needs R, Int, Duration)
    -> Process.Spec<M, E>
    needs R
Process.handoff_spec:
  fn<A, M, E, R>(fn(Inbox<M>, A) -> Void errors E needs R, Int, Duration)
    -> Process.HandoffSpec<A, M, E>
    needs R
Process.register:    fn<M, E>(Process.Spec<M, E>, Address<M>) -> Process.Spec<M, E>
Process.receive:     fn<M>(Inbox<M>) -> M
Process.send:        fn<M>(Address<M>, M) -> Void errors SendError<M>

Supervisor.process:    fn<M, E>(Process.Spec<M, E>, Restart) -> Member
Supervisor.supervisor: fn(Policy, Restart) -> Member
Supervisor.policy:     fn(List<Member>, Int, Restarts) -> Policy
Supervisor.restarts:   fn(Int, Duration) -> Restarts

Supervisor.start:    fn(Policy) -> Running errors StartError.NameTaken
Supervisor.start_process:
  fn<M, E>(Running, Process.Spec<M, E>, Restart) -> ManagedProcess<M>
    errors StartError.CapacityReached | StartError.NameTaken | StartError.Stopped
Supervisor.start_supervisor:
  fn(Running, Policy, Restart) -> ManagedSupervisor
    errors StartError.CapacityReached | StartError.NameTaken | StartError.Stopped
Supervisor.start_handoff:
  fn<A, M, E>(Running, Process.HandoffSpec<A, M, E>, A) -> ManagedProcess<M>
    errors HandoffError<A>
Supervisor.address:  fn<M>(ManagedProcess<M>) -> Address<M>
Supervisor.remove_process:    fn<M>(Running, ManagedProcess<M>) -> Void
Supervisor.remove_supervisor: fn(Running, ManagedSupervisor) -> Void
Supervisor.stop:     fn(Running) -> Void
Supervisor.wait:     fn(Running) -> Void
```

- `Process.spec` takes the body, the mailbox capacity, and the shutdown bound.
  Like `Task.start`, it resolves the body's needs from the calling
  environment and snapshots the providers into the spec, so needs appear at
  the `Process.spec` call and `Process.Spec` carries none. Every replacement
  incarnation uses the same providers, so they MUST be shareable.
- `Process.register`'s second argument MUST be a direct reference to a
  `process name` declaration. Registering a spec that already has a name is a
  defect.
- `Supervisor.policy` takes the members, the capacity, and the restart limit.
  `Supervisor.restarts` takes the limit and the window.
- Each failure row lists only the variants that operation can produce. A row
  that holds every variant of an enum is that enum, so contracts render
  `start_process` and `start_supervisor` as `errors StartError`.
- Process operations name themselves in an interpreter defect:
  `Process.send requires the supervised process runtime`.
- `Process.send` is a transfer boundary for its message, like `Channel.send`.
- `Supervisor.address` reads its argument without consuming it.
  `remove_process` and `remove_supervisor` consume theirs.
- `Supervisor.start` cannot fail with `CapacityReached`: declared members over
  capacity are a defect (see below).
- `NameTaken` carries the qualified identifier of the `process name`
  declaration, the same label traces use.
- `Stopped` is returned when the supervisor has stopped or been replaced.
  Removing from or stopping a stopped supervisor returns `Void`.
- `Process.handoff_spec` and `Supervisor.start_handoff` are described in
  [handoff starts](#handoff-starts).
- A process started from a policy has no `Running` handle of its own
  supervisor. Adding dynamic members to a nested supervisor is out of scope
  for the first kernel.

## Handoff starts

A handoff start begins one process that owns a value from the start, such as
an accepted connection's socket or a `Channel.Receiver` (G28, CHI-213). An
ordinary spec cannot carry one: its body must be shareable (`T4260`), because
a supervisor runs it again for every incarnation.

<!-- chimi: fragment -->
```chimi
const connection = Process.handoff_spec(serve, 16, Duration.seconds(5))

fn accept(connections: Running, socket: Socket) -> Void {
  case Supervisor.start_handoff(connections, connection, socket) {
    Success(_) -> Void
    Failure(HandoffError.CapacityReached(socket)) -> refuse(socket)
    Failure(HandoffError.Stopped(socket)) -> refuse(socket)
  }
}
```

- The body takes its inbox and then the start argument. The spec stays
  shareable, so a module constant may hold it and one spec may start many
  incarnations, each with its own argument. `T4260`, `T4261`, and `T4240`
  apply to the body as they do to a `Process.spec` body. `T4261` also covers
  the argument type `A`, which moves into another process.
- `A` may be any transferable type, shareable ones too, such as a
  per-connection configuration record. An exclusive `A` is the reason the
  operation exists.
- A handoff member is never restarted, because its argument is consumed. It
  behaves as `Restart.Never`: when it ends, its slot is free, so an acceptor
  does not call `remove_process` per connection. It has no `process name`:
  `Process.register` takes only a `Process.Spec`, so a handoff start never
  fails with `NameTaken`. A dynamic member does not survive its supervisor's
  replacement, so no other path runs the body twice.
- `start_handoff` transfers every exclusive binding its argument carries, as
  `Process.send` does for its message: a later use is `T4241`, and a start
  inside a loop of a binding bound outside it is `T4250`. A helper that calls
  `start_handoff` on its parameter is caught at run time by the owner token,
  as every flow across a function boundary is.
- A refused start hands the argument back in `HandoffError`, with ownership
  restored, the way `Process.SendError` hands back a message.
- At run time the argument travels like a `Process.send` message (see
  [handing a capability back](task-and-channel-capabilities.md#handing-a-capability-back)).
  The start is accepted or refused in one scheduler step, before the caller
  can be cancelled. On acceptance its Receivers and Completions are in
  flight, and the new incarnation owns them once its body starts, before the
  body runs any code. A successful start never hands the argument back. An
  incarnation stopped or removed before its body starts discards the
  argument, which aborts open channels as an owner's exit would. One that
  ends after taking it cleans the argument up as any owner does.

## Compile-time rejections

| Code | Rejected program |
| --- | --- |
| `T4260` | A `Process.spec` or `Process.handoff_spec` body that is not shareable: it captures an exclusive or task-local value, or needs a provider that is not shareable. The diagnostic names the capture or provider. |
| `T4261` | A `process name`, `Process.spec` body, or `Process.handoff_spec` body whose message type is not transferable, or a handoff body whose start argument type is not transferable. |
| `T4262` | `Process.register(spec, x)` where `x` is not a direct reference to a `process name` declaration, for example a local variable or `Supervisor.address(...)`. |
| `T4263` | A literal mailbox capacity below 1, a literal policy capacity below 1, a literal restart limit below 0, a literal negative `shutdown_within` duration, a literal restart window of zero or less, or a members list literal longer than a literal policy capacity. |
| `T4264` | One `Supervisor.policy` call whose members list literal registers the same `process name` twice. |
| `T4241` | Using a managed handle after `remove_process` or `remove_supervisor` consumed it, a message after `Process.send` transferred an exclusive value, or an argument after `Supervisor.start_handoff` transferred one. |
| `T4240`, `T4243` | Capturing an inbox into `Task.start`, or sending an inbox through `Channel.send` or `Process.send`. |
| `T4250` | A loop pass that removes a managed handle bound outside the loop. |
| type mismatch | Sending a message whose type differs from the address's `M`; registering a name whose message type differs from the body's `M`; putting a `Process.Spec` directly in the members list instead of `Supervisor.process(...)`. |

A literal duration is `Duration.zero` or a `Duration` constructor such as
`Duration.seconds` applied to an integer literal. An integer literal may have
a leading minus and parentheses. A module constant or an arithmetic
expression is not a literal, even when the compiler could fold it: such a
value is checked at run time by the call that receives it (see below).

`T4264` follows each members-list element through `Supervisor.process`,
`Process.register`, pipelines, and references to module constants, at any
depth. It does not follow function calls. A duplicate it cannot see is the
runtime's `NameTaken` when the policy starts.

Range defect texts follow the standard-library form
`Function: precondition, got value`:

- `Process.spec: mailbox capacity must be at least 1, got 0`
- `Process.spec: shutdown_within must be at least zero, got -1s`
- `Process.register: spec already has a name`
- `Supervisor.policy: capacity must be at least 1, got 0`
- `Supervisor.policy: members must not exceed capacity 2, got 3`
- `Supervisor.restarts: restart limit must be at least 0, got -1`
- `Supervisor.restarts: restart window must be positive, got 0s`

## Runtime checks

The compiler cannot prove these; the runtime reports them as defects with
capability and supervisor identity, never as expected failures:

- a non-literal capacity, limit, or duration out of range, at the call that
  receives it;
- a members list longer than the policy capacity, at `Supervisor.policy`;
- registering a spec that already has a name;
- removing a managed handle through a `Running` that does not own it;
- an exclusive handle reaching an operation from the wrong owner after
  dynamically obscured flow.

## Verified Core invariants

- A `process name` lowers to a constant address node that carries the
  declaration's symbol identity. Registration and trace labels use that
  identity, never text.
- In Core, the second argument of `Process.register` is always such a constant
  node.
- No Core node produces a `Process.Inbox` except a process body's parameter.
- Every `Process.spec` call in verified Core has a proven shareable body,
  shareable providers, and a transferable message type.
- Process and supervisor operations are intrinsic calls with standard symbol
  identity; ordinary calls cannot reach their runtime entry points.

## Conformance scenarios

| Scenario | Expected |
| --- | --- |
| The example above, with two members of different message types | Compiles |
| `Process.spec` inside `using { store: memory_store }` with a shareable provider | Compiles; the spec carries no needs |
| Body needs a provider that is not shareable | `T4260` |
| Body captures a `Channel.Receiver` | `T4260` |
| `process name` with a message containing a `Task` handle | `T4261` |
| `Process.register(spec, local_copy_of_counter)` | `T4262` |
| `Process.spec(body, 0, Duration.seconds(5))` | `T4263` |
| `Process.spec(body, 1, Duration.seconds(-1))` | `T4263` |
| `Supervisor.restarts(1, Duration.zero)` | `T4263` |
| Two members register `counter` in one policy literal | `T4264` |
| `Process.send(counter, CacheMessage.Clear)` | type mismatch |
| `Supervisor.address(managed)` after `remove_process(root, managed)` | `T4241` |
| `Task.start(fn() { Process.receive(inbox) })` inside a body | `T4240` |
| `Process.send(other, Message.Forward(inbox))` | `T4243` |
| `case Supervisor.start_process(root, spec, Restart.Never)` matching all three `StartError` variants | Compiles; missing one is non-exhaustive |
| `case Supervisor.start(policy)` matching `CapacityReached` | unreachable arm |
| `Running` sent to a process in a message and used there to start a sibling | Compiles |
| `ManagedProcess` sent through a channel to another task, removed there | Compiles |
| A module constant `HandoffSpec<Receiver<Int>, Message, ChannelError>`, and a `case` on `start_handoff` matching both `HandoffError` variants | Compiles |
| A `Receiver` used after `Supervisor.start_handoff(root, spec, receiver)` | `T4241` |
| A `Process.handoff_spec` body that captures a `Receiver` | `T4260` |
| A handoff body whose start argument is a `Task` | `T4261` |

## Implementation status

- U1 (ts-6ccbc9): the `process name` declaration (`process` and `name` are
  contextual words at module item position), `std/process`, `std/supervisor`,
  the `std/clock` `Clock` provider type and `Clock.sleep` signature, the
  contracts above, the value properties of the types table, and `T4261` to
  `T4264` (`compiler/chimi/src/typing/process.rs`). The data constructors
  (`Process.spec`, `register`, `Supervisor.process`, `supervisor`, `policy`,
  `restarts`) run at build time, so module constants may hold specs,
  members, and policies; every other operation is a run-time operation
  (`E5201` in a constant). The interpreter defects at the first run-time
  operation. The native build refused process programs until the process
  kernel landed (U5).
- U2 (ts-6ccbc9): capability checking for processes in
  `compiler/chimi/src/capability.rs`, `T4240` for a body failure type in
  `typing/process.rs`, and the Core verifier
  (`compiler/chimi/src/executable/verify.rs`), which `lower` runs in debug
  builds and tests and the conformance lab runs on hand-built Core. See
  "Capability rules as checked" below.
- U5 (ts-6ccbc9): the native runtime runs processes on the scheduler's
  process kernel (`compiler/chimi/runtime/native/runtime.c`, "std/process and
  std/supervisor"). The range defects above are the native texts too. Two
  more limits come from the scheduler's 32-bit fields: a mailbox capacity, a
  policy capacity, or a restart limit above 4,294,967,295 defects at
  `Supervisor.start` or `Supervisor.start_process`, for example
  `Supervisor.start: mailbox capacity must be at most 4294967295, got
  4294967296`. `Supervisor.wait` on a root that escalated defects with
  `Supervisor.RestartLimitExceeded`. A `SendError`, `Restart`, or
  `StartError` value is a closed enum in a `case`, so naming every variant is
  exhaustive.
- U6 (ts-6ccbc9): the non-literal range defects above are checked at the call
  that receives the value, before anything is started (lab MB10, SV11). A
  root escalation ends the program with `Supervisor.RestartLimitExceeded`
  whether main is parked (in `Supervisor.wait` or anywhere else) or
  runnable.

## Capability rules as checked

- `T4260`, capture: a `Process.spec` body that captures a value that is never
  shareable is reported at the body argument, naming the capture
  (``capture `receiver` (Receiver<Int>)``).
- `T4260`, provider: a provider that is not shareable is reported at the body
  argument when `using` binds it to the body
  (`Process.spec(serve using { store: p }, ...)`), and at the provider when a
  `using` binds it to a need that flowed out of a `Process.spec` call: around
  the call itself, or around a call of a named function whose open needs come
  from a `Process.spec` call in its body, at any depth. The message names the
  provider. Needs that flow out of a spec built inside a closure count for the
  enclosing function; a closure value is not followed through a later call.
  As with `Task.start` (`T4242`), shareability is judged from the provider's
  flow when it is a local binding and from its type otherwise.
- `Process.send` moves every exclusive binding its message carries, as
  `Channel.send` does
  ([moving exclusive values](task-and-channel-capabilities.md#moving-exclusive-values)):
  the message itself, a binding it was built from through literals,
  constructors, blocks, `case` arms, or a call whose result type can hold the
  binding's type, and through a `let` that bound such a value (`ts-184972`).
  A shareable value in a message (a `Running`, an `Address`) stays usable.
- `Supervisor.start_handoff` moves every exclusive binding its argument
  carries, by the same rule.
- `Supervisor.remove_process` and `remove_supervisor` consume a handle passed
  as a binding: a later use is `T4241`, and a removal inside a loop of a
  handle bound outside it is `T4250`.
- `T4240`, escape beyond the body: the inbox is task-local, so ending a body
  with `Failure(E)` must not return one. A `Process.spec` body whose failure
  type is not transferable is `T4240` at the body argument. Captures by
  `Task.start` are the existing `T4240`, and an inbox in a message is `T4243`.

The Core verifier checks, over every node: each `Process.register` call's
second argument is a reference to a `process name` symbol; each node typed
`Process.Inbox<_>` only reads an inbox (a local reference, a call of a project
function or closure, a standard call given an inbox-holding argument, or a
node forwarding a child's value), so every inbox starts at a parameter; and a
`Process.spec` body closure captures only shareable values, with shareable
providers.
