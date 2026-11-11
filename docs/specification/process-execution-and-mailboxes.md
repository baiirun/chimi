# Process Execution and Mailboxes

## Status

Contract for `prog show ts-a20d02`, inside the
[first supervised-process kernel](../wayfinding/first-supervised-process-kernel.md).
Every decision below was resolved with the user and is logged on the ticket.
Source spelling
remains provisional. Addresses, registered names, and routing are specified in
[process identity and routing](process-identity-and-routing.md).

## Goals

A process is an isolated, long-lived state loop with a typed, bounded inbox.
Its contract must make overload visible instead of letting queues grow, must
never let two processes deadlock by sending to each other, and must keep
restart, shutdown, and expected failure distinct.

## Example

<!-- chimi: pending ts-6ccbc9 -->
```chimi
import std/channel as Channel
import std/channel.{Sender}
import std/process as Process
import std/process.{Address, Inbox, SendError}

enum CounterMessage {
  Add(amount: Int)
  Report(reply: Sender<Int>)
  Finish
}

fn counter(inbox: Inbox<CounterMessage>) -> Void {
  loop count = 0 {
    case Process.receive(inbox) {
      CounterMessage.Add(amount) -> count + amount
      CounterMessage.Report(reply) -> {
        Channel.send(reply, count)
        count
      }
      CounterMessage.Finish -> break
    }
  }
}

fn add(counter: Address<CounterMessage>, amount: Int) {
  case Process.send(counter, CounterMessage.Add(amount)) {
    Success(_) -> True
    Failure(error) -> case error {
      SendError.Full(_) -> False
      SendError.Unavailable(_) -> False
    }
  }
}
```

The process is declared as an inert specification and owned by a supervisor:

<!-- chimi: pending ts-6ccbc9 ts-f31a20 -->
```chimi
let counting = Process.spec(counter, 64, Duration.seconds(5))
  |> Supervisor.process(Supervisor.Restart.OnDefect)
```

## State ownership

- A process body has the shape
  `fn(Process.Inbox<Message>) -> Void errors E needs R`. `Process.spec`
  snapshots the providers for `R`, so every incarnation uses the same ones.
- State lives in the body's [`loop`](repetition.md) and belongs to
  one incarnation. A restart runs the body again from its initial state;
  nothing is checkpointed.
- The inbox is the process's exclusive receiving capability. It cannot be
  transferred, shared, or stored outside the body.
- Values the body captures must be shareable, because a restart
  evaluates the same body again in a new incarnation.

## Delivery

- Every mailbox has a required positive capacity: the exact
  maximum number of accepted but unreceived messages. Rendezvous delivery uses
  `Channel.bounded(0)` instead.
- A capacity below 1 is a programming error, not an expected condition. A
  literal is rejected at compile time; any other value is a defect when the
  specification is started.
- Delivery is strict FIFO in acceptance order across all senders.
- `Process.receive` returns the oldest accepted message. There is
  no selective receive; an exhaustive `case` over the single message type
  handles every variant, so no message can be skipped and left behind.
- Sending transfers the message to the process, under the same
  transferability rules as `Channel.send`
  ([task and channel capabilities](task-and-channel-capabilities.md)).

## Operations

| Operation | Parks? | Outcomes |
| --- | --- | --- |
| `Process.spec(body, mailbox_capacity, shutdown_within)` | No | An inert specification. No runtime work starts. |
| `Process.receive(inbox)` | Only while the mailbox is empty | The oldest accepted message. A stop request wakes a parked receive with cancellation. |
| `Process.send(address, message)` | Never | `Success`, `Failure(SendError.Full(message))` when the mailbox is at capacity, or `Failure(SendError.Unavailable(message))` once the incarnation has terminated. |

- `Process.send` never parks, so a full mailbox cannot block a
  sender and mutual senders cannot deadlock. Callers that want to wait compose
  retry, an admission queue, or a bounded channel explicitly.
- Acceptance is one atomic point. Once accepted, the message
  belongs to the process even if it defects immediately afterwards. A failed
  send always returns the unsent message.
- The first layer omits `Process.stop`. Stopping another process
  belongs to its owning supervisor (`Supervisor.remove_process` or
  `Supervisor.stop`); a process stops itself by finishing its body.

## Terminal outcomes

| How the body ends | Meaning | Restarted? |
| --- | --- | --- |
| Returns `Void` | Intentional completion | No |
| Returns `Failure(E)` | Expected terminal failure | No |
| Cancelled by its supervisor | Requested termination | No |
| Defects | Contained fault; state, the in-flight message, and unread messages are lost | Only under `Supervisor.Restart.OnDefect` |

- Terminal outcomes are observed through lifecycle events and the
  supervisor's management handle. They do not propagate into the supervising
  function's own failure channel.
- After termination by any route, sends fail with `Unavailable` and return the
  message.

## Shutdown

- Shutdown is out-of-band cancellation, not a mailbox message.
  It wakes a parked `Process.receive`, runs cleanup within the specification's
  `shutdown_within`, and discards unread messages unless the body drains them
  first. It never triggers defect restart.

## Out of scope

Durable mailboxes, acknowledgement, redelivery, request/reply calls, links,
monitors, registered names, and process discovery.
