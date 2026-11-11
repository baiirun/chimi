# Process Identity and Routing

## Status

Contract for `prog show ts-7aff55`, inside the
[first supervised-process kernel](../wayfinding/first-supervised-process-kernel.md).
Every decision below was resolved with the user and is logged on the ticket.
Source spelling remains provisional. Mailbox capacity, ordering, and send
outcomes are specified in
[process execution and mailboxes](process-execution-and-mailboxes.md).

## Goals

Senders need one way to reach a process that survives restarts, and a narrower
way to reach exactly one incarnation. Neither may imply persistence or
delivery guarantees, and a direct reference must never silently retarget.

## Example

<!-- chimi: pending ts-6ccbc9 ts-f31a20 -->
```chimi
import std/process as Process
import std/process.{Address}

pub process name counter: CounterMessage

let counting = Process.spec(counter_loop, 64, Duration.seconds(5))
  |> Process.register(counter)
  |> Supervisor.process(Supervisor.Restart.OnDefect)

fn add(amount: Int) {
  Process.send(counter, CounterMessage.Add(amount))
}

fn finish_exactly(managed: ManagedProcess<CounterMessage>) {
  let exact = Supervisor.address(managed)
  Process.send(exact, CounterMessage.Finish)
}
```

## Addresses

- `Process.Address<Message>` is the only send target. `Process.send(address,
  message)` accepts only `Message`.
- An address has a routing kind fixed when it is created:
  - A **named** address, from a `process name` declaration, resolves at each
    send to the incarnation currently registered under that name.
  - A **direct** address, from `Supervisor.address(managed)`, is pinned to the
    incarnation running when it was obtained.
- Addresses are shareable values. Any task or process may hold, copy, store, or
  send them. Holding an address grants only the right to send.
- Traces and tooling show the routing kind at every send.

## Names

- A name is a module-level declaration:
  `process name <identifier>: <Message>`, optionally `pub`. Its identity is the
  declaration, not its text, so two declarations never collide even with the
  same identifier in different modules, and a name's message type is fixed.
- The declared identifier is a value of type `Process.Address<Message>` with
  the named routing kind. Code shares a name by importing it, so names follow
  ordinary module visibility. Its qualified identifier is the trace label.
- Names are declarations rather than computed values because module bindings
  are neither exportable nor evaluated once (`prog show ts-f47931`); a declaration needs no program initialization order.
- `Process.register(spec, name)` is part of the process specification, so every replacement registers the same name. Its argument
  must be a reference to a `process name` declaration; passing a direct
  address is a compile-time error.
- A name is held by at most one running incarnation. Starting a process whose
  name is already held fails with the expected `Supervisor.NameTaken`, before
  initialization, in the same fail-fast style as `Supervisor.CapacityReached`.

## Routing transitions

| Moment | Named address | Direct address |
| --- | --- | --- |
| Incarnation created | Registered atomically with mailbox creation, before the body runs; early messages wait in the mailbox | Valid from the same moment |
| Shutdown or removal begins | Unregistered immediately; sends fail with `Unavailable` | Sends fail with `Unavailable` immediately, even during cleanup |
| Body ends by return, failure, or defect | Released at the same atomic point at which sends start failing | `Unavailable` forever |
| Gap before a replacement registers | `Unavailable` | Not applicable |
| Replacement starts | Routes to the new incarnation | The old address stays unavailable; obtain a new one through `Supervisor.address` |

- Every send is either accepted by exactly one incarnation or returned to the
  sender as `Full(message)` or `Unavailable(message)`. A message accepted just
  before a defect is lost with that incarnation's mailbox.
- Sends are rejected as soon as shutdown begins because cleanup accepts no new
  messages; it may drain messages already accepted, and the rest are
  discarded unread.
- Old and new incarnations never overlap: the owning supervisor releases the
  name before it starts a replacement.

## Trace identity

Each send event records:

- the address kind (`named` or `direct`);
- the name's label, for a named address;
- the incarnation that accepted the message, when one did;
- the outcome: `accepted`, `full`, or `unavailable`.

A trace can therefore show which incarnation each named send reached across
restarts.

## Out of scope

Lookup by text, names created at runtime, distributed or cluster-wide names, aliases or groups of names,
monitors and links, and request/reply calls.
