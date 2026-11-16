# Phase 3 forcing program: evidence on choice syntax and error propagation

## Status

Evidence, not a decision. CHI-207 asked the Phase 3 forcing program to show
how noisy `Select.first` and stored outcomes are in a realistic service, so
that two open questions can be decided from real code:

1. Should choosing among events be syntax (a `select { ... }` form), or stay
   the library function `Select.first`? (time-and-outcomes.md, open question 1)
2. Should Chimi have an error-propagation shorthand (`try` or `?`)?

Neither question is decided here, and the program adds no sugar. Every
excerpt below is from
[`programs/checkout-service`](../../programs/checkout-service/README.md),
which builds and runs on main, with its contracts as `chimi inspect` prints
them.

## The program

A checkout service: about 620 lines of Chimi in four modules, plus an 85-line test module. Two workers
share one order queue (`Channel.add_receiver`). Each worker waits in
`Select.first` on the queue, a settings watch (`std/watch`), and a heartbeat
timer, and stops taking orders while the settings say it is paused. Each
checkout:

- reserves stock and scores fraud at once (`Task.all`), recovering one fraud
  failure and letting the others pass;
- charges the card with a 200 ms timeout per attempt (`Task.timeout`) and
  exponential backoff (`std/retry`, `std/schedule`), retrying only an
  unavailable service or a timeout;
- races two carriers for a quote (`Task.race`);
- sends an email and an SMS and counts what arrived (`Task.results`);
- runs under an 800 ms deadline (`Task.deadline`), with context frames
  (`Cause.context`), and is stored as a `Result` (`Result.capture`) with a
  failure-set type, sent back on a channel, described with an exhaustive
  `case`, and its cause printed with `Cause.to_text`.

Workers report on a broadcast (`std/broadcast`) that an audit task follows.
Fakes answer by rule under the virtual clock, so the run is exact. The
Phase 4 forcing program, `programs/service-pool`, covers processes,
supervisors and shutdown; this one uses tasks only and does not overlap it.

## Question 1: is `Select.first` too noisy?

### The worker loop

```chimi
enum Wake {
  Next(next: Receive<Order>)
  Changed(next: Receive<Settings>)
  Beat
}

  loop state = (False, 0) {
    let (paused, handled) = state
    let follow = Select.watch(settings, fn(next) { Wake.Changed(next) })
    let beat = Select.after(Duration.milliseconds(heartbeat_ms), Wake.Beat)
    let events_now = case paused {
      True -> [follow, beat]
      False -> [follow, Select.receive(jobs, fn(next) { Wake.Next(next) }), beat]
    }
    let wake = Select.first(events_now)
    case wake {
      Wake.Changed(Receive.Received(changed)) -> (changed.paused, handled)
      Wake.Changed(Receive.Drained) -> (paused, handled)
      Wake.Next(Receive.Received(order)) -> {
        handle(name, order, outcomes, events)
        (paused, handled + 1)
      }
      Wake.Next(Receive.Drained) -> break handled
      Wake.Beat -> {
        report(events, Event.Heartbeat(name))
        (paused, handled)
      }
    }
  }
```

### What this shows

- **The cost is a wake enum and one mapping function per source.** Here that
  is a five-line enum and two lambdas, `fn(next) { Wake.Changed(next) }`,
  which only rewrap. The `case` then unwraps twice:
  `Wake.Next(Receive.Received(order))`. Of the 23 lines above, about 9 exist
  only to name and unname the event (the enum, the two lambdas, and the
  `Wake.` prefixes).
- **The list form paid for itself once.** "Stop taking orders while paused"
  is an ordinary `case` that builds a shorter list. A fixed `select` block
  would need a per-arm guard for this, as Rust's `tokio::select!` has
  (`, if !paused`) and Go gets by setting the channel to `nil`.
- **The mapping functions did not do real work anywhere.** In every event of
  this program the mapping is a constructor. Nothing needed to transform the
  value at the event, so the generality bought nothing here.
- **One mismatch with intent showed up.** `Select.after` measures from the
  start of each `Select.first`, so the heartbeat means "after one idle
  second", not "every second". That was what this worker wanted. A fixed
  period needs `Select.at` with the next instant carried in the loop state,
  as the spec example does. Both are expressible; the difference is easy to
  miss.
- **Priority by list order was natural.** Settings come first so a pause
  takes effect before the next order.

For comparison only, and not proposed syntax, a block form of the same loop
would read roughly:

```text
loop state = (False, 0) {
  let (paused, handled) = state
  select {
    Watch.receive(settings) -> Received(changed) -> (changed.paused, handled)
                            -> Drained -> (paused, handled)
    Channel.receive(jobs) if !paused -> Received(order) -> { ... }
                                     -> Drained -> break handled
    after Duration.milliseconds(heartbeat_ms) -> { ... }
  }
}
```

It drops the enum and the lambdas (about 9 lines) and adds a guard and arm
syntax that exist nowhere else in the language.

### Summary for question 1

In one realistic loop with three sources, the library form cost about nine
lines of naming, and its one structural advantage (building the list from
state) was used once. The noise is real but local: it lives in one function,
and the rest of the service never sees it. This is one program, so it
measures one shape of loop; a service with many select loops would multiply
the enum cost.

## Question 2: would `try` or `?` help?

### Propagation already costs nothing to write

A fallible call propagates by itself, and the inferred contract carries the
set. These are the program's own contracts:

```text
checks:   fn(Order) -> Reservation errors FraudError.Flagged | StockError
charge:   fn(Order) -> Text errors PaymentError | Timeout
ship:     fn(Order) -> Text errors ShippingError
notify:   fn(Order) -> Int
checkout: fn(Order) -> Receipt errors FraudError.Flagged | PaymentError | ShippingError | StockError | Timeout
run:      fn(Order, Instant) -> Result<Receipt, FraudError.Flagged | PaymentError | ShippingError | StockError | Timeout>
work:     fn(...) -> Int errors ChannelError | SendError<Outcome>
main:     fn() -> Int errors ChannelError | ConsoleError | SendError
```

The program has about 30 call sites that can fail. None of them carries
propagation syntax. Rust would write `?` at each one, Swift `try`, and Go an
`if err != nil` block. So `?` in Rust's sense, "unwrap or return the
failure", would have nothing to do here. Its only candidate is
`Result.get`, which turns a stored `Err` back into a live failure, and the
program calls it only in tests (two sites).

### What the cost actually is

1. **Fallible calls look like infallible ones.** In `checkout`:

   ```chimi
   let reservation = fn() { checks(order) }
   |> Cause.context("checking order ${order.id}")
   let charged = fn() { charge(order) }
   |> Cause.context("charging order ${order.id}")
   let carrier = ship(order)
   Receipt(order.id, reservation.hold, charged, carrier, notify(order))
   ```

   `ship(order)` can fail and `notify(order)` cannot. Nothing on the line
   says which. The same holds in `main`, where seven `Channel.send` calls and
   two `Watch.publish` calls each put a failure in `main`'s row, and in
   `handle`, where one `Channel.send` makes the whole worker fallible with
   `SendError<Outcome>`. The contract shows it; the call site does not. This
   is the case for a Swift-style `try` marker (a visible, checked marker at
   each fallible call, with no change in meaning), and it pulls against
   "explicit local reasoning" in the vision.

2. **Handling one failure is short, and the rest pass on.**

   ```chimi
   fn score_or_review(order: Order) -> Int needs { fraud: Fraud } {
     case fraud.score(order) {
       Success(score) -> score
       Failure(FraudError.Unreachable) -> fraud_review_score
     }
   }
   ```

   The row narrows to `FraudError.Flagged`, and the narrowed member then
   appears in the stored type written in other modules. When `Unreachable`
   was added, the exhaustive `case` in `describe_failure` caught it at once.
   This part needed no shorthand.

3. **Discarding a failure costs four lines.**

   ```chimi
   case Broadcast.publish(events, event) {
     Success(_) -> Void
     Failure(_) -> Void
   }
   ```

4. **A fallible call cannot be the subject of a value `case`.** The compiler
   requires `Success`/`Failure` arms there (`T4222`), so matching on the
   value needs a binding first:

   ```chimi
   let both = Task.all([reserve, score])
   case both {
     [Check.Reserved(reservation), Check.Scored(score)] -> ...
     _ -> Debug.crash("Task.all returns its results in input order")
   }
   ```

5. **Wrappers take thunks, and thunks nest.** Every combinator takes
   `fn() { ... }`, so a deadline inside a capture is a thunk inside a thunk:

   ```chimi
   let bounded = fn() {
     fn() { checkout(order) }
     |> Task.deadline(deadline)
   }
   Result.capture(bounded)
   ```

   and a retried, timed call is three named thunks (`call`, `attempt`, and
   the policy). This is the noisiest error-handling code in the program, and
   neither `try` nor `?` touches it.

6. **Reading a stored outcome needs a `case`.** `notify` counts deliveries
   with `case outcome { Result.Ok(_) -> True  Result.Err(_) -> False }`;
   `Result.is_ok` is planned but not written yet.

7. **A failure set is written out in full each time.** The five-member set
   appears three times (the `Outcome` field, its accessor, and
   `describe_failure`). There are no aliases.

### Summary for question 2

The program found no place where propagation itself was noisy, because
Chimi already propagates without syntax. The friction sits at the
boundaries: making fallible calls visible, discarding, storing, adding
context, and wrapping work in thunks. If a shorthand is added, the evidence
points at a marker for visibility (`try` in Swift's sense) rather than an
unwrap operator (`?` in Rust's sense). The thunk nesting and the discard
`case` are separate problems that a std helper or a block-taking combinator
form could address without new failure syntax.

## Other findings

| Finding | What happened | Where it went |
| --- | --- | --- |
| A `Result` received from a channel in a loop was rejected (`T4250`) | `Ok` and `Err` are typed in the runtime's own generics, which `Result`'s arguments did not bind, so the checker thought every received result could hold the receiver. The same gap hid a `Result` holding a Receiver from `T4244`. | Fixed in this change, with a test for each |
| A record does not fit an interface type as a parameter or return type | Fake constructors had to leave off `-> Payments` | CHI-244 |
| `Task.race` returns the first failure as readily as the first success | A hedged carrier quote fails when the fast carrier fails fast | CHI-245 |
| A timed-out checkout's cause has no frames | Order 5's cause is only `Timeout.TimedOut(800ms)`; order 4's shows `charging order 4` | Added to CHI-226 |
| `Err(FraudError.Flagged(95))` does not fit the narrowed set | A constructor has its whole enum's type | Known, CHI-218; the test matches instead |
| `Task.all` takes one success type | Two checks with different results share a wrapper enum and an impossible `case` arm | Evidence only |
| Struct fields are private across modules | Fakes and workers read orders through accessor functions; the main module has 19 imports | Evidence for the open import-noise and large-struct questions |
