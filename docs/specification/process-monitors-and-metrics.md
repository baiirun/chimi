# Process Monitors, Own Address, and Mailbox Metrics

## Status

Contract for G27 in the [language completion plan](../plans/language-completion-plan.md)
(Linear CHI-220). It extends the first supervised-process kernel:
[process execution and mailboxes](process-execution-and-mailboxes.md) and
[process identity and routing](process-identity-and-routing.md). Decided from
the vision docs and recorded in [decisions.md](../decisions.md); source
spelling stays provisional like the rest of `std/process`.

Links are out of scope (decided 2026-11-15, see "Links" below).

## Goals

A process needs three things the first kernel left out:

- its **own address**, so it can hand a reply target to a peer without
  building a channel;
- **monitors**, so a process can learn that another process ended, and how,
  through its one ordered mailbox;
- **mailbox metrics**, so a busy process can see its own backlog and shed load
  before senders start getting `Full`.

Each must keep the kernel's guarantees: bounded mailboxes, no selective
receive, sends that never park, and no message a process can skip.

## Example

A process that hands its own address to a peer and sheds load when it falls
behind:

<!-- chimi: check -->
```chimi
import std/process as Process
import std/process.{Address, Inbox}

pub enum Request {
  Ping(reply: Address<Reply>)
  Work(amount: Int)
}

pub enum Reply {
  Pong(from: Address<Request>)
}

fn server(inbox: Inbox<Request>) -> Void {
  loop handled = 0 {
    case Process.receive(inbox) {
      Request.Ping(reply) -> {
        Process.send(reply, Reply.Pong(Process.address(inbox)))
        handled
      }
      Request.Work(amount) -> case falling_behind(inbox) {
        True -> handled
        False -> handled + amount
      }
    }
  }
}

fn falling_behind(inbox: Inbox<Request>) -> Bool {
  Process.queued(inbox) * 2 > Process.capacity(inbox)
}
```

A process that watches workers and stops at the first defect:

<!-- chimi: check -->
```chimi
import std/process as Process
import std/process.{Address, Down, Inbox, Monitor}

pub enum Message {
  Watch(worker: Address<Int>)
  WorkerDown(down: Down)
}

fn watcher(inbox: Inbox<Message>) -> Void {
  loop downs = 0 {
    case Process.receive(inbox) {
      Message.Watch(worker) -> {
        let _monitor: Monitor = Process.monitor(inbox, worker, fn(down) { Message.WorkerDown(down) })
        downs
      }
      Message.WorkerDown(down) -> case down {
        Down.Defected -> break
        Down.Returned -> downs + 1
        Down.Failed -> downs + 1
        Down.Stopped -> downs + 1
        Down.Unavailable -> downs + 1
      }
    }
  }
}
```

## Own address

| Operation | Parks? | Result |
| --- | --- | --- |
| `Process.address(inbox)` | No | The direct `Address<Message>` of the incarnation that owns `inbox`. |

- The address is direct: it is pinned to this incarnation and never
  retargets to a replacement, exactly like `Supervisor.address(managed)`.
  A process that wants restart-transparent routing registers a name.
- It takes the inbox, not an ambient "self", so only a process body can ask,
  and the call site shows whose address it is.
- Sending to your own address is an ordinary send: it never parks and returns
  `Full` when your own mailbox is at capacity.

## Mailbox metrics

| Operation | Parks? | Result |
| --- | --- | --- |
| `Process.queued(inbox)` | No | Entries waiting in the mailbox: accepted messages plus queued monitor notices. |
| `Process.capacity(inbox)` | No | The spec's mailbox capacity. |

- Both read the owner's own mailbox at the moment of the call. They are for
  load decisions inside the body ("am I falling behind?"). Another process
  cannot read them: the answer would be stale before it arrived.
- `queued` can exceed `capacity` by the number of queued monitor notices (see
  "Bound" below). A sender sees `Full` only when accepted messages reach the
  capacity.

## Monitors

| Operation | Parks? | Result |
| --- | --- | --- |
| `Process.monitor(inbox, target, notice)` | No | A `Monitor`. When the watched incarnation ends, `notice(down)` is delivered to this inbox once. |
| `Process.unmonitor(inbox, monitor)` | No | `Void`. After it returns, this inbox never receives that monitor's notice. |

`notice` has the type `fn(Down) -> Message`: it turns the end into the
watcher's own message type, so the notice arrives through the same exhaustive
`case` as every other message. It may not fail or need providers. The monitor
keeps the notice until it delivers it, so passing a notice moves it, and the
values it captures, into the monitor, as `Process.send` moves a message.

`Process.Down` says how the watched incarnation ended:

| Variant | The watched incarnation |
| --- | --- |
| `Down.Returned` | returned `Void` |
| `Down.Failed` | returned `Failure(E)` |
| `Down.Defected` | defected (including a stack overflow, or a child task's defect) |
| `Down.Stopped` | was stopped or removed by its supervisor (cleanup finished or forced) |
| `Down.Unavailable` | did not exist when the monitor was created |

### What a monitor watches

- `target` is resolved once, when `monitor` is called. A named address
  resolves to the incarnation registered under the name at that moment, and
  the monitor watches that incarnation, not the name. A replacement started
  later is not watched; monitor it again after the notice.
- If there is no incarnation to watch (the direct incarnation has ended, or
  the name has none right now), the notice `Down.Unavailable` is queued
  immediately.
- A direct address to an incarnation that is shutting down still names it: the
  monitor waits for its end and reports `Down.Stopped` (or `Down.Defected` if
  cleanup defects). A name stops routing when shutdown begins, so a monitor
  set through the name from then on gets `Down.Unavailable`.
- `Down` carries no failure value or defect text. The target's error type is
  not part of `Address<Message>`, and the defect text belongs in the trace and
  the supervisor's reports. The watcher learns how, not why.
- A process may monitor its own address. The notice is never delivered,
  because the mailbox ends with the process.

### Delivery

- The notice is queued when the watched incarnation's end is recorded, after
  every message the watcher's mailbox already accepted, in the same FIFO
  order as messages. A peer's message sent before it ended can therefore
  arrive before or after its notice only according to acceptance order, which
  is the order the kernel saw them.
- `notice` runs inside the watcher, when `Process.receive` takes the notice
  from the mailbox, and its result is what `receive` returns. A defect in
  `notice` is a defect of the watcher at that `receive`.
- Each monitor delivers at most one notice and is then gone. Monitoring the
  same target twice creates two monitors and two notices.
- From the moment the watcher's shutdown begins, its mailbox accepts no new
  messages, and no new notices either; a notice that would arrive then is
  dropped, even while the watcher's drain still runs handlers (lab MB22).
  Notices already queued drain like messages.
- When the watcher ends by any route, its monitors end with it, and queued
  notices are discarded unread like messages.

### Unmonitor

- `unmonitor` removes the monitor. If its notice is already queued and not yet
  received, it is removed from the mailbox too, so the call leaves nothing
  behind to handle.
- A monitor that has already delivered its notice, was already removed, or was
  created by another incarnation is ignored: this inbox will not receive its
  notice in any case.
- A `Monitor` is opaque, like an `Address`: it may be stored or sent, but it
  has an effect only in the incarnation that created it.

### Bound

A notice does not count against the mailbox capacity, so a full mailbox never
loses one and a peer's end is never reported as `Full`. Each monitor queues at
most one notice, so a mailbox holds at most `capacity` messages plus one
notice per live monitor of its owner. A process that keeps monitoring a
long-lived peer without `unmonitor` grows its monitor table, as on the BEAM.

## Observability

- The watched incarnation's end is already a `process_terminated` event in
  every traced mode. Under `--trace=full`, the receive that returns a notice
  and the notice function's call are traced like any other Core operations.
- `monitor`, `unmonitor`, `address`, `queued`, and `capacity` emit no process
  events of their own. A dedicated `process_notice` event can follow if a
  program needs to see dropped notices.

## Links

Chimi does not add BEAM-style links in this layer (decided 2026-11-15 from the
vision docs: real problems over speculative ones, and explicit local
reasoning). A link ends one process when another crashes, at a distance and
invisibly in the supervision tree. The cases it covers are already covered:

- a process's child tasks end with it, and a child defect is the process's
  defect;
- a process that must react to a peer's end monitors it;
- processes that only make sense together belong under one supervisor.

When a forcing program needs a group of members to restart together, the
answer is a supervisor restart policy that shows the group in the tree, not
links.

## Out of scope

Links, monitoring supervisors or tasks, reading another process's mailbox
metrics, mailbox high-water marks in traces or `std/metrics` (Phase 7c),
demonitor options other than "flush", and request/reply calls.
