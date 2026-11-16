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
which builds and runs with this change, with its contracts as `chimi inspect` prints
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

<!-- chimi: fragment -->
```chimi
enum Wake {
  Next(next: Receive<Order>)
  Changed(next: Receive<Settings>)
  Beat
}

  loop state = (False, True, 0) {
    let (paused, following, handled) = state
    let watching = case following {
      True -> [Select.watch(settings, fn(next) { Wake.Changed(next) })]
      False -> []
    }
    let taking = case paused {
      True -> []
      False -> [Select.receive(jobs, fn(next) { Wake.Next(next) })]
    }
    let beat = Select.after(Duration.milliseconds(heartbeat_ms), Wake.Beat)
    let events_now = List.concat([watching, taking, [beat]])
    let wake = Select.first(events_now)
    case wake {
      Wake.Changed(Receive.Received(changed)) -> (changed.paused, following, handled)
      Wake.Changed(Receive.Drained) -> (paused, False, handled)
      Wake.Next(Receive.Received(order)) -> {
        handle(name, order, outcomes, events)
        (paused, following, handled + 1)
      }
      Wake.Next(Receive.Drained) -> break handled
      Wake.Beat -> {
        report(events, Event.Heartbeat(name))
        (paused, following, handled)
      }
    }
  }
```

The program has one select site, this one, so every point below is one
example, not a pattern.

### What this shows

- **The naming cost is a wake enum and one mapping function per source.**
  The enum is five whole lines. The two lambdas, `fn(next) { Wake.Changed(next) }`,
  only rewrap, and the `case` unwraps twice: `Wake.Next(Receive.Received(order))`.
  Those sit on lines any form would keep, so they cost width rather than
  lines.
- **The larger cost is choosing which events are live.** About eight lines
  build the list from state. Two conditions decide it: a paused worker stops
  taking orders, and a drained watch must leave the list.
- **The drained watch was a real bug in the first draft.** The first draft
  always put the settings watch first. Once the watch drains it is always
  ready, and list priority means it then wins every pass, so the worker spun
  forever without taking another order or reaching its own `Drained`. The
  run hid it at first, because `main` closed the settings only after the
  workers had finished. Closing the settings first (the order `main` now
  uses) made the program hang under the virtual clock. The spec's priority
  rule says this ("an event early in the list that is always ready starves
  later ones"), but it is easy to miss. Any form of choice has to handle it:
  Go sets the channel variable to `nil`, and Rust's `tokio::select!` has a
  per-branch `, if cond` guard and disables a branch whose pattern fails.
  Here the library form handled it with ordinary list code; a block form
  would need guards, or would need to drop a drained source by itself.
- **The mapping functions did no real work.** Each one is a constructor. In
  this program the generality bought nothing.
- **`Select.after` measures from each call.** The heartbeat therefore means
  "after one idle second", not "every second". That suited this worker; a
  fixed period needs `Select.at` with the next instant carried in the loop
  state, as the spec example does.
- **Priority by list order was natural** for putting settings first, so that
  a pause takes effect before the next order.

For comparison only, and not proposed syntax, a block form of the same loop
might read:

```text
loop state = (False, True, 0) {
  let (paused, following, handled) = state
  select {
    Watch.receive(settings) if following {
      Received(changed) -> (changed.paused, following, handled)
      Drained -> (paused, False, handled)
    }
    Channel.receive(jobs) if !paused {
      Received(order) -> { ... }
      Drained -> break handled
    }
    after Duration.milliseconds(heartbeat_ms) -> { ... }
  }
}
```

It drops the enum, the lambdas and the list building, about fifteen lines,
and needs guards, nested arms and an `after` form as new syntax.

### Summary for question 1

In one realistic three-source loop, the library form cost a five-line enum,
two rewrapping lambdas, and about eight lines of list building. The list
building handled the one hard case the program met, a drained source that
would otherwise starve the rest, with code the language already has. A block
form would be shorter here and would have to answer the same case with
guards. A service with many select loops would multiply the enum cost; this
program has one.

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

The program has about 40 call sites that can fail (about 20 in the checkout,
4 in the worker, 16 in `main`, counting console writes and `Task.result`).
Two of them handle the failure with a `case`; the rest propagate, and none
carries propagation syntax. Rust would write `?` at each one, Swift `try`, and Go an
`if err != nil` block. So `?` in Rust's sense, "unwrap or return the
failure", would have nothing to do here. Its only candidate is
`Result.get`, which turns a stored `Err` back into a live failure, and the
program calls it only in tests (two sites).

### What the cost actually is

1. **Fallible calls look like infallible ones.** In `checkout`:

   <!-- chimi: fragment -->
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
   `handle`, where one `Channel.send` adds `SendError<Outcome>` to the
   worker's row. The signatures do not show it either: `handle` and `work`
   are written `-> Void` and `-> Int` with no `errors` clause, because rows
   are inferred, and `Failure(FraudError.Flagged(score))` in `checks` raises
   while reading like a value. The inferred contract shows all of it, through
   `chimi inspect` or an editor. Whether that is visible enough is the
   question "explicit local reasoning" in the vision raises.

2. **Handling one failure is short, and the rest pass on.**

   <!-- chimi: fragment -->
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

   <!-- chimi: fragment -->
   ```chimi
   case Broadcast.publish(events, event) {
     Success(_) -> Void
     Failure(_) -> Void
   }
   ```

4. **A fallible call cannot be the subject of a value `case`.** The compiler
   requires `Success`/`Failure` arms there (`T4222`), so matching on the
   value needs a binding first:

   <!-- chimi: fragment -->
   ```chimi
   let both = Task.all([reserve, score])
   case both {
     [Check.Reserved(reservation), Check.Scored(score)] -> ...
     _ -> Debug.crash("Task.all returns its results in input order")
   }
   ```

5. **Wrappers take thunks, and thunks nest.** Every combinator takes
   `fn() { ... }`, so a deadline inside a capture is a thunk inside a thunk:

   <!-- chimi: fragment -->
   ```chimi
   let bounded = fn() {
     fn() { checkout(order) }
     |> Task.deadline(deadline)
   }
   Result.capture(bounded)
   ```

   and a retried, timed call is two named thunks (`call` and `attempt`) and
   a policy constant. This is the noisiest error-handling code in the
   program, and neither `try` nor `?` would remove it.

6. **Reading a stored outcome needs a `case`.** `notify` counts deliveries
   with `case outcome { Result.Ok(_) -> True  Result.Err(_) -> False }`;
   `Result.is_ok` is planned but not written yet.

7. **A failure set is written out in full each time.** The five-member set
   appears three times (the `Outcome` field, its accessor, and
   `describe_failure`). There are no aliases.

### Summary for question 2

The program found no place where writing propagation was noisy, because
Chimi already propagates without syntax. The program stores a `Result` at
one boundary only, so it says little about an unwrap operator like Rust's
`?`, whose only candidate here is `Result.get` (two test sites). The
friction it did find sits elsewhere: fallible calls and raising
`Failure(...)` look like ordinary expressions, discarding a failure takes a
four-line `case`, and wrappers nest thunks.

These bear on the options in different ways:

- **Keep propagation implicit.** Nothing to add; contracts stay visible
  through inspection and tooling (principle 5), not at the call site.
- **A call-site marker, as Swift's `try`.** It would make the roughly 40
  fallible sites visible, at the cost of a token at each, several of them
  inside the thunks that are already the noisiest code.
- **Written `errors` clauses on some signatures,** for example public
  functions, so the row shows where a reader looks first, without marking
  every call.
- **An unwrap operator, as Rust's `?`.** This program gives it almost
  nothing to do.

The thunk nesting and the discard `case` are separate problems that a std
helper or a block-taking combinator form could address without new failure
syntax.

## Other findings

| Finding | What happened | Where it went |
| --- | --- | --- |
| A `Result` received from a channel in a loop was rejected (`T4250`) | `Ok` and `Err` are typed in the runtime's own generics, which `Result`'s arguments did not bind, so the checker thought every received result could hold the receiver. The same gap hid a `Result` holding a Receiver from `T4244`. | Fixed in this change, with a test for each |
| A record does not fit an interface type as a parameter or return type | Fake constructors had to leave off `-> Payments` | CHI-244 |
| `Task.race` returns the first failure as readily as the first success | A hedged carrier quote fails when the fast carrier fails fast | CHI-245 |
| A timed-out checkout's cause has no frames | Order 5's cause is only `Timeout.TimedOut(800ms)`; order 4's shows `charging order 4` | Added to CHI-226 |
| `Err(FraudError.Flagged(95))` does not fit the narrowed set | A constructor has its whole enum's type | Known, CHI-218; the test matches instead |
| A drained watch first in a `Select.first` list starves the rest | The first draft's worker spun forever once its settings watch drained | Fixed in the program; discussed under question 1 |
| `Task.all` takes one success type | Two checks with different results share a wrapper enum and an impossible `case` arm | Evidence only |
| Struct fields are private across modules | Fakes and workers read orders through accessor functions; the main module has 19 imports | Evidence for the open import-noise and large-struct questions |
