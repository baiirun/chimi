# Bounded Channel Semantics

## Question

What should Chimi's first channel guarantee about capacity, ordering,
backpressure, cancellation, completion, abort, and observation?

This note selects a semantic contract. It does not settle constructor
destructuring syntax, the runtime queue representation, or the task scheduler.

## Decision

Chimi's first channel is a bounded, typed, many-producer/single-consumer
channel with separate capabilities for sending, receiving, and completing the
producer side.

- Capacity zero is a rendezvous. A send is accepted only when paired with a
  receive.
- Positive capacity is the exact maximum number of accepted but unreceived
  values.
- A full channel parks the sending task. An empty open channel parks the
  receiving task. Neither operation blocks the scheduler or a carrier thread.
- Accepted values are FIFO. Parked sends are admitted FIFO, with no barging by
  a newer sender when capacity becomes available.
- The default never drops, overwrites, or conflates values.
- Graceful `close` rejects unaccepted sends, drains accepted values, and then
  produces the explicit terminal result `Drained`.
- Destructive `abort` rejects sends, discards accepted unread values, and makes
  receives fail with `Channel.Aborted`.
- Cancellation and channel operations have atomic linearization points. A send
  that reports failure or loses to cancellation did not deliver its value.
- Payloads are not included in traces by default.

Unbounded queues, dropping buffers, conflated state, broadcast, multiple
consumers, channel selection, and channel-specific timeouts are separate future
abstractions rather than modes on this first primitive.

## Precedent

The common baseline is stronger than any one existing API:

| System | Useful precedent | Chimi choice |
| --- | --- | --- |
| Go | Capacity zero rendezvous, positive bounded buffers, blocking send/receive, FIFO values, explicit close and drain ([Go specification](https://go.dev/ref/spec#Channel_types), [close](https://go.dev/ref/spec#Close)) | Keep rendezvous, bounds, FIFO, and explicit graceful completion; use value-level failure instead of panic or a zero-value sentinel. |
| Rust `std::sync::mpsc` | Cloneable MPSC senders, one receiver, bounded backpressure, and capacity-zero rendezvous ([`sync_channel`](https://doc.rust-lang.org/std/sync/mpsc/fn.sync_channel.html)) | Keep the topology and rendezvous behavior, but park lightweight tasks rather than OS threads. |
| Tokio MPSC | All-sender completion, graceful receiver close/drain, FIFO waiter queue, and cancel-safe send where a cancelled send did not happen ([MPSC](https://docs.rs/tokio/latest/tokio/sync/mpsc/), [`Sender::send`](https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Sender.html#method.send), [`Receiver::close`](https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Receiver.html#method.close)) | Keep FIFO admission and unambiguous cancellation, but return the unsent value instead of dropping it and avoid making deterministic semantics depend on destructor timing. |
| Kotlin | Rendezvous and finite capacities; suspending backpressure is the default while dropping/conflation are explicit alternatives ([`Channel`](https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-core/kotlinx.coroutines.channels/-channel/)) | Keep suspension as the only first-kernel overflow behavior. Reject the prompt-cancellation rule under which an operation can change channel state and still report cancellation. |
| Crossbeam | Endpoint capabilities, zero-capacity rendezvous, drain after disconnect, and send errors that return the unsent value ([channel](https://docs.rs/crossbeam/latest/crossbeam/channel/), [`TrySendError`](https://docs.rs/crossbeam/latest/crossbeam/channel/enum.TrySendError.html)) | Preserve the unsent value in close and abort failures. Do not use reachability or drop as the only completion signal. |
| .NET | Bounded `Wait` is the default; dropping policies are explicit; a coordinator completes the writer after all producers finish ([channels](https://learn.microsoft.com/en-us/dotnet/core/extensions/channels)) | Keep explicit coordinated completion distinct from ordinary sender capabilities. |

Go's close panic and closed-channel zero value are poor fits for Chimi's
errors-as-values and non-nullable data model. Kotlin documents that cancellation
can win after a channel operation has already changed state, requiring an
`onUndeliveredElement` callback for resource transfer. Chimi instead chooses a
single atomic winner. Tokio's FIFO waiter queue and cancel-safe `send` are the
closest behavioral precedent, while Crossbeam supplies the useful rule that a
failed send retains the value.

## Capabilities and completion

Construction produces three logical capabilities:

- `Channel.Sender<T>` is shareable across producers and can only send.
- `Channel.Receiver<T>` belongs to one consumer and can receive or abort.
- a producer-completion capability is retained by the coordinator and can only
  close the producer side.

The third capability prevents any ordinary producer from accidentally ending a
shared stream. Its final source-level name and whether uniqueness is enforced
statically depend on Chimi's capability/ownership design. The semantic rule is
settled: completion is an explicit global transition performed by the
coordinator, not a consequence of one producer finishing and not a
garbage-collection event.

The normal structured pattern is:

1. The coordinator creates the channel and gives sender capabilities to
   producer tasks.
2. It retains the completion capability and receiver.
3. Once all producers have terminated, it closes the producer side.
4. The consumer receives accepted values until it observes `Drained`.

If endpoint-owning tasks terminate unexpectedly, runtime resource cleanup must
wake parked peers rather than leave them suspended forever. The exact cleanup
rule belongs to the capability-lifetime prototype because it depends on whether
Chimi adopts affine values, deterministic resource scopes, or another ownership
mechanism.

## Operation contracts

### Send

`send(sender, value)` completes only after the channel accepts the value into
its finite buffer or directly hands it to a waiting receiver.

Conceptually its expected failure channel is:

<!-- chimi: fragment -->
```chimi
enum Channel.SendError<T> {
  Closed(T)
  Aborted(T)
}
```

The error owns the value that was not accepted. This makes retry, rerouting, or
cleanup possible without cloning and establishes the invariant that an error
never means "the value may also have been delivered."

When the channel is full, a send enters the channel's FIFO waiter queue and its
task parks. Freeing capacity reserves the slot for the oldest waiter before the
waiter becomes runnable, so a newer send cannot take the slot.

### Receive

`receive(receiver)` returns an explicit channel result:

<!-- chimi: fragment -->
```chimi
enum Channel.Receive<T> {
  Received(T)
  Drained
}
```

`Drained` means both that the producer side is closed and that no accepted value
remains. It is not `None`: `Channel<Option<T>>` must be able to carry
`Received(None)` without ambiguity. Repeated receive after a graceful drain may
return `Drained` immediately.

The expected failure channel contains `Channel.Aborted`. Task cancellation is a
separate runtime terminal cause, not a declared channel error.

### Close

Graceful close is an idempotent, global producer-side transition:

- values accepted before close remain FIFO and receivable;
- parked sends have not been accepted and resume with `Closed(value)`;
- future sends immediately return `Closed(value)`;
- a parked receiver wakes with `Drained` only when no accepted value remains;
- close after close is a no-op;
- close after abort is a no-op.

The ordering boundary is the close linearization point, not the time a task is
woken. A send that linearized first is accepted; close winning first returns
the value as `Closed(value)`.

### Abort

Abort is an idempotent, global consumer-side destructive transition:

- accepted unread values are discarded;
- parked and future sends return `Aborted(value)`;
- a parked or future receive fails with `Channel.Aborted`;
- a value already returned by receive is not revoked;
- abort after close upgrades the remaining graceful drain to an abort;
- abort after abort is a no-op;
- abort does not cancel unrelated tasks.

Discarded resource-owning values follow Chimi's eventual deterministic cleanup
rules. The channel core does not run an arbitrary user callback while mutating
its queue.

## Cancellation races

Every send and receive has one atomic acceptance or delivery point. Cancellation
has its own atomic transition. The first transition to linearize wins.

| Race | Cancellation wins | Channel operation wins |
| --- | --- | --- |
| parked send versus capacity | waiter is removed, value is never accepted, task terminates cancelled | value is committed to the receiver/buffer and send completes before later cancellation |
| parked receive versus arriving value | waiter is removed and no value leaves the channel | ownership transfers to the receiving task and the value is not put back |
| send versus close | send retains its queue position only until one transition wins; close returns `Closed(value)` when first | accepted value drains normally |
| send versus abort | abort returns `Aborted(value)` when first | already delivered value is unaffected; accepted buffered value may then be discarded by abort |

This is intentionally stronger than scheduler timing. Waking a task makes it
runnable; it does not define which transition won.

## Ordering and fairness

The channel guarantees:

- FIFO order for accepted values;
- FIFO admission for parked send operations;
- no sender barging after a slot is assigned to a waiter;
- a deterministic channel linearization order even when producers race.

It does not guarantee that the scheduler immediately runs a woken task or that
unrelated runnable tasks receive strict global FIFO scheduling. With one
receiver, there is no competing-receiver fairness contract in the first
channel.

## Deferred surfaces

- Nonblocking `try_send` and `try_receive` should be added only for a concrete
  forcing case. If added, full/empty are ordinary attempt results while
  closed/aborted remain terminal channel states.
- Timeouts compose from timers and task races after cancellation races satisfy
  the atomic rules above; they are not channel-specific operations.
- Multi-channel selection needs its own fairness and cancellation contract and
  is not implied by this channel.
- Dropping, conflation, broadcast, and unbounded buffering need distinct names
  and types so callers and tooling can see their loss and memory behavior.
  Several receivers, broadcast and watch (a conflating latest value) are now
  specified this way, in `std/broadcast` and `std/watch`
  ([channel variants](../specification/task-and-channel-capabilities.md#channel-variants)).

## Observability

The runtime should make channel behavior visible without making tracing part of
the semantic result. Boundary/full trace modes may emit:

- channel creation with identity, topology, and capacity;
- send/receive park, admission, delivery, resumption, failure, and cancellation;
- close, drained, and abort transitions;
- queue depth, waiter depth, wait duration, terminal cause, and abort discard
  count.

Events carry channel identity, task/computation identity, and causal links.
Arbitrary payload values are omitted by default for privacy, allocation cost,
and stable schemas. A future explicit projector may opt selected payload data
into tracing.

## Conformance scenarios

The deterministic lab must establish these separately:

1. capacity-zero rendezvous never buffers;
2. positive capacity parks only the overflowing sender;
3. two parked senders resume FIFO without barging;
4. cancellation before send admission removes the value;
5. admission before later cancellation does not redeliver the value;
6. graceful close rejects parked sends, drains accepted values, then yields
   `Drained`;
7. abort discards accepted values, returns unaccepted values to senders, and
   fails receives;
8. close/abort and send/receive races have exactly one visible winner;
9. tracing contains identities, causal transitions, depths, and discard counts
   without payloads in the default mode.

The native runtime prototype later measures uncontended fast paths, rendezvous,
contention, cancellation storms, allocation, and off/boundaries/full tracing.
