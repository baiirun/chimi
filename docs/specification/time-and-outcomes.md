# Time, Choice, and Storable Outcomes

## Status

This is the normative contract for time, timers, waiting on several events,
storable outcomes, failure-set types, failure context, and retry. It covers
gaps G13, G14, and G25 of the
[language completion plan](../plans/language-completion-plan.md) and the
`Duration` type of `prog show ts-f31a20`. The remaining work is tracked by
`prog show ts-80cbfb`. Only [`Duration`](#duration) is implemented: the
type, constructors, conversions, arithmetic, text form, parser, `Duration.zero`,
derived equality and order, debug rendering, module constants, `T4303` help,
and `T4348` to `T4350`, on both backends (see
[Implementation notes](#implementation-notes)). `T4263` for process
durations waits for `Process.spec`. Nothing else here is implemented yet.
Source spelling follows the current kernel and remains free to break.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative.

This contract builds on:

- [executable kernel](executable-kernel.md): failure rows, selective recovery,
  outcome patterns, failure-set generics, and retry as composition;
- [task and channel capabilities](task-and-channel-capabilities.md): task
  ownership, cancellation, and capability transfer;
- [bounded channel semantics](../research/channel-semantics.md): atomic
  linearization of channel operations against cancellation;
- [host capabilities and execution properties](host-capabilities-and-execution-properties.md)
  and [ADR 0004](../adr/0004-keep-execution-properties-out-of-callable-types.md):
  parking and host access stay out of callable types;
- [supervisor policy and lifecycle](supervisor-policy-and-lifecycle.md) and
  [process and supervisor static contracts](process-and-supervisor-static-contracts.md),
  which need `Duration` for `shutdown_within` and restart windows.

## Goals

- A program that waits, measures, or times out shows a `clock: Clock` need.
  Time is a dependency, not an ambient global.
- One test switch makes every timer deterministic, including timers inside
  supervisors, without a fake that has to hold state.
- A timeout never loses a value that a channel already delivered, and never
  turns an outer cancellation into an expected failure.
- Waiting on several channels, tasks, mailboxes, and timers commits to exactly
  one of them.
- An expected failure can be stored in a value, sent through a channel, and
  passed to a predicate, while its failure set stays visible in the type.
- A failure keeps its variant when code adds context, and keeps its cause when
  code converts it to another variant.
- Retry, `Task.all`, `Task.race`, and timeouts are ordinary std functions over
  thunks. None of them needs a dedicated Core operation.

Non-goals: calendars, time zones, and date formatting (`std/time`); send arms
in `Select`; jitter until `Random` and `Float` exist; ticker delivery into
process mailboxes (G27); drain deadlines for services (G22).

## Modules

| Module | Exports |
| --- | --- |
| `std/duration` | `Duration`, `ParseError`, constructors, arithmetic, text form |
| `std/clock` | `Clock` interface, `Instant`, `Timestamp`, `now`, `since`, `sleep`, `starting_at` |
| `std/task` | adds `timeout`, `deadline`, `all`, `results`, `race`, `Timeout` |
| `std/select` | `Event`, `first`, `receive`, `inbox`, `task`, `after`, `at` |
| `std/result` | `Result`, `capture`, `get` |
| `std/cause` | `Cause`, `context`, `of`, read functions, `to_text` |
| `std/schedule` | `Schedule`, `immediate`, `fixed`, `exponential`, `delay` |
| `std/retry` | `Policy`, `policy`, `run` |

## Example

A worker takes jobs from a channel, retries transient failures with backoff,
bounds each job by a timeout, and sends a heartbeat every ten seconds while it
is idle:

<!-- chimi: pending ts-80cbfb ts-975ca3 -->
```chimi
import std/cause as Cause
import std/channel as Channel
import std/channel.{Receive, Receiver, Sender}
import std/clock as Clock
import std/clock.{Instant}
import std/duration as Duration
import std/result as Result
import std/result.{Result}
import std/retry as Retry
import std/schedule as Schedule
import std/select as Select
import std/task as Task
import std/task.{Timeout}

enum JobError {
  Transient(reason: Text)
  Rejected(reason: Text)
}

interface Payments {
  charge: fn(Text) -> Void errors JobError
}

struct Job {
  id: Int,
  payload: Text,
}

enum Wake {
  Next(next: Receive<Job>)
  Heartbeat
}

fn retryable(error: JobError | Timeout) -> Bool {
  case error {
    JobError.Transient(_) -> True
    Timeout.TimedOut(_) -> True
    JobError.Rejected(_) -> False
  }
}

const backoff = Retry.policy(
  Schedule.exponential(5, Duration.milliseconds(100), Duration.seconds(5)),
  retryable,
)

fn process(job: Job) -> Void needs { payments: Payments, clock: Clock } {
  let charge = fn() { payments.charge(job.payload) }
  let attempt = fn() { charge |> Task.timeout(Duration.seconds(2)) }
  let retried = fn() { attempt |> Retry.run(backoff) }
  retried |> Cause.context("charging job ${job.id}")
}

fn worker(
  jobs: Receiver<Job>,
  outcomes: Sender<Result<Void, JobError | Timeout>>,
  beats: Sender<Instant>,
) -> Int needs { payments: Payments, clock: Clock } {
  let period = Duration.seconds(10)
  loop next_beat = Clock.add(Clock.now(), period) {
    let wake = Select.first([
      Select.receive(jobs, fn(next) { Wake.Next(next) }),
      Select.at(next_beat, Wake.Heartbeat),
    ])
    case wake {
      Wake.Next(Receive.Received(job)) -> {
        Channel.send(outcomes, Result.capture(fn() { process(job) }))
        Clock.add(Clock.now(), period)
      }
      Wake.Next(Receive.Drained) -> break 0
      Wake.Heartbeat -> {
        Channel.send(beats, Clock.now())
        Clock.add(next_beat, period)
      }
    }
  }
}
```

The inferred contract of `worker` lists `payments` and `clock` as needs. Its
expected failures are `Channel.Aborted` from the receive event and
`Channel.SendError` from the sends. `process` exposes `JobError | Timeout`,
which the `Result` type carries into the channel.

## Duration

### Representation

`Duration` is an opaque struct in `std/duration` with exactly one private
field: a signed 64-bit count of nanoseconds. Its range is about ±292 years.

- Derived equality, hashing, and ordering MUST equal numeric equality and
  order of the nanosecond count. The single field guarantees that.
- `Duration` is shareable.
- A negative duration is valid. It is the result of subtracting a later
  instant from an earlier one. Operations that wait treat a non-positive
  duration as a deadline that has already passed.
- The native backend SHOULD represent a `Duration` as an unboxed 64-bit
  integer.

Nanoseconds match the host precision of `CLOCK_MONOTONIC` and
`mach_continuous_time`, and 292 years covers every timeout, lease, and
restart window a service uses. A wider or floating representation would make
equality inexact.

### Constructors and conversions

| Function | Type | Notes |
| --- | --- | --- |
| `Duration.nanoseconds` | `fn(Int) -> Duration` | |
| `Duration.microseconds` | `fn(Int) -> Duration` | |
| `Duration.milliseconds` | `fn(Int) -> Duration` | |
| `Duration.seconds` | `fn(Int) -> Duration` | |
| `Duration.minutes` | `fn(Int) -> Duration` | |
| `Duration.hours` | `fn(Int) -> Duration` | |
| `Duration.zero` | `Duration` | Module constant (implemented as a registry constant equal to `Duration.nanoseconds(0)`). |
| `Duration.to_nanoseconds` | `fn(Duration) -> Int` | Exact |
| `Duration.to_microseconds` | `fn(Duration) -> Int` | Truncates toward zero |
| `Duration.to_milliseconds` | `fn(Duration) -> Int` | Truncates toward zero |
| `Duration.to_seconds` | `fn(Duration) -> Int` | Truncates toward zero |

There is no `days` constructor. A day of elapsed time is exactly 24 hours, but
a calendar day is not, and that distinction belongs to `std/time`.
Constructors take `Int` only. Fractional input is written in a smaller unit,
such as `Duration.milliseconds(1500)`.

### Arithmetic

| Function | Type |
| --- | --- |
| `Duration.add` | `fn(Duration, Duration) -> Duration` |
| `Duration.subtract` | `fn(Duration, Duration) -> Duration` |
| `Duration.multiply` | `fn(Duration, Int) -> Duration` |
| `Duration.divide` | `fn(Duration, Int) -> Duration` |
| `Duration.negate` | `fn(Duration) -> Duration` |
| `Duration.absolute` | `fn(Duration) -> Duration` |

- A constructor or arithmetic result outside the 64-bit range MUST defect,
  as `Int` arithmetic does. It MUST NOT wrap or saturate.
- `Duration.divide` truncates toward zero. A zero divisor MUST defect.
- `Duration.negate` and `Duration.absolute` of the minimum value MUST defect.
- The arithmetic operators `+ - * /` and unary `-` MUST NOT accept `Duration`,
  `Instant`, or `Timestamp`. The rejection is the general arithmetic-operand
  error `T4303`, with a help message that names the `Duration` or `Clock`
  function to use, such as `Duration.add`. Chimi has no operator overloading, and
  `Duration * Int` would be the only mixed-type operator.

Comparison uses the derived structural ordering. This spec's examples assume
G3 exposes that ordering through the comparison operators. If G3 instead
exposes it only through comparator values, `Duration` uses them unchanged and
needs no `Duration.less` or `Duration.greater`.

### Text form

`Duration.to_text(Duration) -> Text` produces the canonical text form:

- Zero is `0s`.
- A negative duration starts with `-`.
- Below one second, the text uses the largest of `ms`, `us`, or `ns` in which
  the value is at least 1, with the exact decimal fraction and no trailing
  zeros: `1.5ms`, `250us`, `7ns`.
- From one second up, the text uses `h`, `m`, and `s`, starting at the largest
  nonzero unit. A zero unit between two nonzero units is written. Trailing zero
  units are left out. Seconds carry the exact fraction with no trailing zeros:
  `1h`, `1h30m`, `1h0m30s`, `2m3.25s`.
- The text is ASCII. Microseconds are `us`, not `µs`, so that logs and greps
  stay ASCII.

`Duration.parse(Text) -> Duration errors ParseError` accepts the same grammar:
an optional `-`, then one or more `<decimal><unit>` parts with units in
strictly descending order and each unit at most once. A `<decimal>` is one or
more ASCII digits with an optional `.` and one or more fraction digits; a
fraction is allowed in every unit (`1.5h` is `1h30m`). The literal `0` is also
accepted; `-0` is not, but `-0s` is. Nothing else is: no `+`, whitespace,
`_`, or `µs`.

<!-- chimi: check -->
```chimi
enum ParseError {
  Empty
  Invalid
  OutOfRange
}
```

Following the [failure-type conventions](standard-library-conventions.md#payloads),
the variants carry no copy of the parsed text; a caller adds it with
`Cause.context`.

- The empty text is `Empty`.
- A grammar error anywhere in the text is `Invalid`, even when another part
  is out of range.

- `Duration.parse(Duration.to_text(d))` MUST return `d` for every `d`,
  including the minimum value.
- A fraction that is not an exact number of nanoseconds, such as `1.5ns`, is
  `Invalid`. Parsing MUST NOT round.
- A value outside the range is `OutOfRange`.
- When the argument is a `Text` literal or a module constant, the compiler
  parses it at build time, with the parser the interpreter runs. Text that would fail is a compile error (`T4349`).
  Valid text compiles and draws a warning (`T4350`) that suggests the
  equivalent constructor expression, because a constructor cannot fail and
  needs no failure handling. Neither check applies to a non-constant argument.
  These checks land with `Duration` itself (phase 3).
- Debug rendering (G10) of a `Duration` MUST use the canonical text form.

## Time sources

### Instant

`Instant` is an opaque, shareable point on the execution's monotonic time
source. It holds one signed 64-bit nanosecond count since an origin chosen
when the execution starts.

- `Instant` values never decrease within one execution, across all tasks and
  carrier threads.
- An `Instant` has no meaning outside the execution that produced it. It
  has no text form other than debug rendering.
- The native time source SHOULD include time the host spends suspended
  (`CLOCK_BOOTTIME` on Linux, `mach_continuous_time` on macOS), so that leases
  and deadlines expire after a laptop or VM resumes. Each target documents its
  choice.

| Function | Type |
| --- | --- |
| `Clock.add` | `fn(Instant, Duration) -> Instant` |
| `Clock.between` | `fn(Instant, Instant) -> Duration` |

`Clock.between(start, end)` is `end - start` and is negative when `end` is
earlier. Overflow in either function MUST defect.

### Timestamp

`Timestamp` is wall-clock time: an opaque, shareable signed 64-bit count of
nanoseconds since 1970-01-01T00:00:00Z, ignoring leap seconds. Its range is
the years 1678 to 2262.

- Wall time can jump backward or forward when the host adjusts its clock.
  Timers and timeouts MUST NOT use it. No function in this contract waits on a
  `Timestamp`.
- `Clock.timestamp_to_text(Timestamp) -> Text` produces RFC 3339 in UTC, with
  the fraction trimmed of trailing zeros: `2026-11-10T09:30:00.25Z`.
- `Clock.unix_nanoseconds(Timestamp) -> Int` and
  `Clock.from_unix_nanoseconds(Int) -> Timestamp` convert exactly.
- `Clock.timestamp_between(Timestamp, Timestamp) -> Duration` subtracts wall
  times. The result can be negative even when the arguments were read in
  order.

Calendar fields, time zones, and parsing belong to `std/time`.

### The Clock capability

`Clock` is a structural interface in `std/clock`:

<!-- chimi: fragment -->
```chimi
interface Clock {
  now: fn() -> Instant
  timestamp: fn() -> Timestamp
  sleep_until: fn(Instant) -> Void
}
```

Std functions that read or wait on time declare `needs { clock: Clock }` and
take no clock argument:

| Function | Type |
| --- | --- |
| `Clock.now` | `fn() -> Instant needs { clock: Clock }` |
| `Clock.timestamp` | `fn() -> Timestamp needs { clock: Clock }` |
| `Clock.since` | `fn(Instant) -> Duration needs { clock: Clock }` |
| `Clock.sleep` | `fn(Duration) -> Void needs { clock: Clock }` |
| `Clock.sleep_until` | `fn(Instant) -> Void needs { clock: Clock }` |

The need name is always `clock`. A program that names a different need
`clock` with another interface gets the existing incompatible-need
diagnostic. Code that declares the need may also call `clock.now()` directly.

Sleeping follows these rules:

- `Clock.sleep(d)` is `Clock.sleep_until(Clock.add(Clock.now(), d))`.
- A deadline at or before `now` returns without parking. Cancellation is still
  checked first.
- A sleeping task parks and does not hold a carrier thread.
- Cancellation while parked removes the timer and propagates cancellation.
  Cancellation and timer expiry linearize atomically, as channel operations
  do. Exactly one of them wins.

### Execution time source and providers

Every execution has one time source, chosen by the runner:

- `real`: the host monotonic and wall clocks. This is the default for
  `chimi run`.
- `virtual`: a deterministic source owned by the scheduler. This is the
  default for test runs (T1) and is available as `chimi run --clock virtual`.
  The runner accepts `--clock-start <RFC 3339>` for the initial `Timestamp`
  and defaults to `1970-01-01T00:00:00Z`.

The runner binds a root `clock: Clock` need to the execution clock provider,
which reads that source. Source code cannot construct the execution provider,
just as it cannot construct the native `FileSystem`. The time source is
execution configuration, like the trace mode. It is not a dependency, and it
does not appear in any callable type.

Programs MAY supply another `Clock` through `using`. That provider affects
only code that reaches time through the `clock` need. `std/clock` provides one
wrapper:

- `Clock.starting_at(Clock, Timestamp) -> Clock` returns a clock whose
  `timestamp` starts at the given value when the wrapper is created and then
  advances with the wrapped clock. `now` and `sleep_until` pass through.
  Tests use it to pin wall time for date logic.

Runtime-owned deadlines use the execution time source directly. These are
supervisor restart windows, process `shutdown_within` deadlines, and any later
runtime deadline. They are runtime behavior, not program dependencies, so they
do not go through a user-supplied `Clock`. A test under virtual time therefore
gets deterministic supervision without passing a clock to the supervisor.

### Virtual time

Under the virtual source:

- `now` starts at the origin, and `timestamp` starts at the runner's start
  value. Both advance only together.
- Time never advances while any task is runnable or any host operation is in
  flight. Computation takes zero virtual time.
- When no task is runnable, no host operation is in flight, and at least one
  timer is pending, the scheduler sets `now` to the earliest pending deadline
  and wakes every timer at that deadline.
- Timers with the same deadline wake in registration order. Woken tasks join
  the runnable queue in that order.
- When every live task is parked, no timer is pending, and no host operation
  is in flight, nothing can ever wake. The root MUST defect with a deadlock
  cause that lists each parked task and the operation it waits on.
- A loop that reads `Clock.now()` without parking never lets time advance.
  Such a loop spins forever, as it would on a stopped real clock.

Under the real source, an idle scheduler blocks on the host until the
earliest timer or a host completion. The real source does not detect
deadlock, because a host event may still arrive.

Two runs of the same program under the virtual source with the same inputs
MUST produce the same sequence of `Instant` and `Timestamp` values, the same
timer firing order, and the same trace event order.

## Timeouts and deadlines

### Contract

```text
Task.timeout:
  fn<T, F, R>(fn() -> T errors F needs R, Duration)
    -> T errors F | Timeout
    needs R + { clock: Clock }

Task.deadline:
  fn<T, F, R>(fn() -> T errors F needs R, Instant)
    -> T errors F | Timeout
    needs R + { clock: Clock }
```

<!-- chimi: fragment -->
```chimi
enum Timeout {
  TimedOut(limit: Duration)
}
```

`Task.timeout(operation, d)` is `Task.deadline(operation,
Clock.add(Clock.now(), d))` except that `TimedOut` carries `d`. `Task.deadline` carries the
duration from the call to the deadline.

### Semantics

`Task.timeout` runs the thunk in the calling task inside a **cancellation
scope**. It does not start a child task. Therefore the thunk may use
exclusive capabilities such as a channel receiver, and it needs no
transferable captures.

1. If the deadline has already passed, the operation does not start, and the
   result is `Failure(Timeout.TimedOut(limit))`.
2. Otherwise the runtime registers a timer and runs the operation.
3. If the operation terminates first, the runtime removes the timer and
   returns the operation's outcome unchanged: success, expected failure,
   defect, or cancellation.
4. If the timer fires first, the runtime requests cancellation of the scope.
   The current parked operation, if any, loses to cancellation under the
   atomic linearization rules. Children started inside the scope are
   cancelled and drained, and exclusive capabilities owned by the scope are
   cleaned up as for task exit.
5. When the scope's own cancellation reaches the scope boundary, the scope
   converts it to `Failure(Timeout.TimedOut(limit))`.

Rules:

- A timeout MUST convert only its own scope's cancellation. Cancellation of
  the task, of an enclosing timeout scope, or by supervisor shutdown MUST
  pass through unchanged.
- In nested timeouts, the outer deadline firing cancels the inner scope as
  well. The inner scope does not convert it. The outer scope returns its own
  `TimedOut`.
- A defect inside the scope remains a defect, even when the timer fired.
- Cancellation points are the existing ones: every park, every loop pass, and
  every host operation boundary. Straight-line code between two cancellation
  points runs to the next one, so a timeout can finish late by the length of
  that code. It is never early.
- If a parked receive linearizes before the timer, the value belongs to the
  operation. A pending cancellation then takes effect at the next
  cancellation point, and the received value is not lost: if the operation
  returns before that point, the operation's result stands.

### Argument order

`|>` inserts its left operand as the first argument of the call on its right
([executable kernel](executable-kernel.md#pipelines)), so every combinator
that wraps an operation takes that operation first and its configuration
after it:

- `M.x(operation, config...)` runs `operation` now and returns its outcome,
  so `operation |> M.x(config...)` reads in the order the work happens.
- A configured operation that runs later is an ordinary thunk:
  `fn() { operation |> M.x(config...) }`. Wrappers nest by wrapping that
  thunk again.

| Call | Pipeline |
| --- | --- |
| `Task.timeout(op, d)` | `op \|> Task.timeout(d)` |
| `Task.deadline(op, at)` | `op \|> Task.deadline(at)` |
| `Cause.context(op, text)` | `op \|> Cause.context(text)` |
| `Retry.run(op, policy)` | `op \|> Retry.run(policy)` |

Std MUST NOT define a second, function-returning form of a combinator (such as
`with_timeout` or `Retry.with`) only to make it pipeable. Earlier drafts did,
because the kernel then allowed only unary pipelines.

## Choosing among events

### Decision

Waiting on several sources is a std function, `Select.first`, over event
values. It is not syntax. A library form keeps the language small (principle
3), lets programs build the event list from data, and composes with `case`.
Its cost is one mapping function per event. The first open question asks
the user to confirm this.

`Select` cannot be built from `Task.race`. A race between two receiving tasks
can let the losing task receive a value and then be cancelled, which loses
that value. `Select.first` commits to exactly one event in one atomic step.

### Events

`Select.Event<A, F>` is an opaque value. It describes one wait that produces
an `A` or fails with `F`. Building an event does not park and consumes
nothing.

```text
Select.receive: fn<T, A, F, R>(Receiver<T>, fn(Receive<T>) -> A errors F needs R)
                  -> Event<A, F | Channel.Aborted> needs R
Select.inbox:   fn<M, A, F, R>(Inbox<M>, fn(M) -> A errors F needs R)
                  -> Event<A, F> needs R
Select.task:    fn<T, E, A, F, R>(Task<T, E>, fn(T) -> A errors F needs R)
                  -> Event<A, E | F> needs R
Select.after:   fn<A>(Duration, A) -> Event<A, Never> needs { clock: Clock }
Select.at:      fn<A>(Instant, A) -> Event<A, Never> needs { clock: Clock }
Select.first:   fn<A, F>(List<Event<A, F>>) -> A errors F
```

`Never` here means an empty failure set. Its spelling waits for L4.

- An event is task-local, like `Process.Inbox` and `Task` handles. It cannot
  be returned beyond its task, sent, captured by a started task, or stored in
  a process spec (`T4240`, `T4243`).
- An event constructor resolves its mapping function's needs when the event
  is built and snapshots the providers, as `Task.start` does. `Event` carries
  no needs.
- A receive event refers to its receiver and does not consume it. Using the
  event after the receiver was transferred is use-after-transfer (`T4241`),
  with the existing runtime owner-token check as the backstop.
- `Select.after(d, value)` measures `d` from the start of the
  `Select.first` call that waits on it, not from the time the event was built.

### Semantics of `Select.first`

1. The list is evaluated. An empty list literal is rejected (`T4345`). An
   empty list at run time MUST defect.
2. The runtime checks each event in list order. The first event that is
   ready commits. A receiver with a buffered value, `Drained`, or `Aborted`
   state is ready. So are a nonempty inbox, a terminated task, and a timer
   whose deadline has passed.
3. If no event is ready, the runtime registers a waiter on every event and
   parks the task.
4. The first event to linearize commits. In the same atomic step, the runtime
   removes every other registration. At most one channel or inbox delivers a
   value to the selecting task.
5. Cancellation while parked removes every registration and delivers nothing.
   Cancellation and commitment linearize atomically.
6. After commitment, the committed event's mapping function runs in the
   selecting task, and its result is the result of `Select.first`.
7. A committed receive on an aborted channel fails with `Channel.Aborted`
   without calling the mapping function. A committed task event whose task
   failed bubbles that failure. A child defect follows task containment as
   usual.

Priority is list order. Chimi does not pick randomly among ready events,
because reproducible behavior outweighs fairness by default. An event early
in the list that is always ready starves later ones. Put control events, such
as a stop channel, first. A program that needs rotation can rotate its list
each pass.

The same receiver MAY appear in two events. The earlier one wins.

A timer event on a user-supplied `Clock` behaves as if a child of the
selecting task ran `clock.sleep_until(deadline)`, and the event becomes ready
when that call returns. The runtime MAY register timers of the execution
clock provider directly on its timer queue, and SHOULD do so, because a hot
loop must not start a task for each pass.

Send events are deferred. A losing send must return its value to the sender,
and that ownership rule needs its own contract.

## Storable outcomes and failure-set types

### Failure-set types

A **failure-set type** is a finite set of failure members. Each member is one
of these:

- an enum type, which stands for all its variants;
- one variant, `StartError.NameTaken`;
- a nominal struct used as a failure value;
- a generic failure-set parameter;
- `Never`, the empty failure set, which adds no member.

Source writes a failure-set type with `|`, in a value position or after
`errors`:

<!-- chimi: fragment -->
```chimi
interface Starter {
  retryable: fn(HttpError | Timeout) -> Bool
  start: fn() -> Running errors StartError.CapacityReached | StartError.NameTaken
}
```

Rules:

- `|` is the one spelling for failure-set union in both positions. The `+`
  spelling in the host-capabilities contract is superseded. The formatter
  MUST rewrite `+` in a failure row to `|`.
- A member that is not one of the five kinds above, such as `Int | Text`, is
  rejected (`T4340`).
- Members are normalized: duplicates collapse, and a variant that is listed
  with its whole enum collapses into the enum. The order of members does not
  matter.
- In a value position, the type denotes the values built by exactly those
  variants and structs. A set is a subtype of every set that includes it, so a
  `HttpError` value passes where `HttpError | Timeout` is expected.
- `case` over a value of a closed failure-set type MUST be exhaustive over its
  variants. A set that contains a generic parameter is open and needs a
  catch-all arm.
- The same set in an `errors` position denotes the failure row, exactly as a
  generic failure set does in the kernel.

Aliases remain out of scope. A program that repeats a long set may declare its
own enum and convert into it.

### Projection into type arguments

The kernel already makes a generic used in both an ordinary and an `errors`
position one failure set with two projections. This contract makes the value
projection valid wherever an ordinary type is valid, including type arguments
of other generic types. This fixes the `T4203` failure the standard library
map records for `capture`:

<!-- chimi: pending ts-facbbe ts-80cbfb -->
```chimi
pub enum Result<T, E> {
  Ok(value: T)
  Err(error: E)
}

pub fn capture<T, E, R>(operation: fn() -> T errors E needs R) -> Result<T, E>
  needs R
{
  case operation() {
    Success(value) -> Ok(value)
    Failure(error) -> Err(error)
  }
}
```

In the `Failure(error)` arm, `error` has the value projection of `E`.
`Result<T, E>` instantiated from an operation with row
`PaymentError.Declined | Offline` has `E = PaymentError.Declined | Offline`.

### Joins

When two inferred types meet, as two `case` arms, two list elements, or two
arguments for one generic parameter do, the rules are:

- Two failure rows join by union. This is the existing composition rule.
- Two function types join when their inputs and success types match. Their
  failure rows join by union, and their needs rows join by union.
- Two failure-set types join by union. A type is a failure-set type when it
  is written with `|`, is the value projection of a generic failure set, or is
  the type of a binding from a `Failure(...)` pattern.
- A type argument joins by union only when it is a failure-set type and the
  generic parameter occurs only in covariant positions, which are fields and
  success types. The compiler infers that variance from the declaration. A
  parameter that also occurs as a function input does not join.
- Two different enum types that are not failure-set types do not join. The
  compiler reports `T4341` and suggests the annotation `A | B`.

So `[Result.capture(load_user), Result.capture(load_order)]` has type
`List<Result<Void, UserError | OrderError>>`. But
`[HttpError.Timeout, DbError.Lost]` with no annotation is rejected, because
an accidental mix of two enums is more often a mistake than a union.

When a value of a failure-set type flows into a position whose set lacks one
of its members, the compiler reports `T4346` and names the missing variants.
A retry predicate that accepts `HttpError` for an operation that can also
time out gets this diagnostic.

### Runtime representation

A value of a failure-set type can come from different enums, so every enum
value MUST carry its nominal enum identity as well as its variant tag at run
time. Matching on a failure-set value compares both. This adds no Core
operation. Lowering still uses ordinary outcome and enum matching, as the
kernel requires.

### `std/result`

| Function | Type |
| --- | --- |
| `Result.capture` | `fn<T, E, R>(fn() -> T errors E needs R) -> Result<T, E> needs R` |
| `Result.get` | `fn<T, E>(Result<T, E>) -> T errors E` |

- `Result.capture` stores an expected failure as `Err`. Defects and
  cancellation MUST pass through and are never stored.
- `Result.get` returns the `Ok` value, or turns `Err` back into a live
  failure with its original failure context.
- `Ok` and `Err` are ordinary constructors, distinct from the live outcome
  constructors `Success` and `Failure`, as the kernel requires.
- `Result<T, E>` derives equality, hashing, transferability, and shareability
  from `T` and `E`. It is ordered only when `T` is ordered and `E` is a single
  ordered type, such as one enum. A failure set of several members is
  equatable but not ordered, as
  [core data](core-data-and-operators.md#equatable-and-ordered) states.
  Failure context does not take part (see below).

The names follow Rust (`Result`, `Ok`, `Err`) and Swift
(`Result(catching:)`, `result.get()`). `raise` and `throw` were rejected,
because Chimi has no exceptions and the names would suggest stack unwinding.

### What failure-set types unblock

- **Retry predicates.** `fn(JobError | Timeout) -> Bool` has a writable type.
  A generic predicate `fn(E) -> Bool` relates to an operation's row `E`.
- **Channels that carry failures.** `Sender<Result<T, E>>` is an ordinary
  type. A result is transferable when `T` and `E` are.
- **`Task.all`, `Task.results`, and `Task.race`** over thunks whose rows
  differ (next section).
- **Decoders and config validation** that collect several failures into a
  `List<E>`.

## Task combinators

```text
Task.all:     fn<T, E, R>(List<fn() -> T errors E needs R>) -> List<T> errors E needs R
Task.results: fn<T, E, R>(List<fn() -> T errors E needs R>) -> List<Result<T, E>> needs R
Task.race:    fn<T, E, R>(List<fn() -> T errors E needs R>) -> T errors E needs R
```

Each combinator starts every thunk as a child of the calling task, with
`Task.start` rules for transfer and provider snapshots. It returns only after
every child it started has terminated. No child outlives the call.

- `Task.all` returns successes in input order. The first expected failure, in
  termination order, cancels and drains the other children and then bubbles.
  Failures of siblings that terminated before the cancellation took effect
  attach to it as suppressed causes with reason `Sibling`.
- `Task.results` waits for every child and returns each outcome in input
  order. It never cancels a sibling because of an expected failure.
- `Task.race` returns the outcome of the first child to terminate with
  success or expected failure. It cancels and drains the others. An empty
  list literal is rejected (`T4345`). An empty list at run time MUST defect.
  Use `Select.first` instead when the racing work receives from channels,
  because a cancelled loser may already have received a value.
- A child defect follows task containment in all three combinators. It
  cancels the siblings and unwinds the caller, whatever the combinator was
  waiting for.
- The list length is the concurrency bound. Bounded fan-out over large inputs
  belongs to `Task.map_bounded`, which this contract does not specify.

## Failure context

### Model

Every terminal failure carries a **cause**: runtime metadata that describes
where the failure came from. This covers expected failures, defects, and
cancellations. A cause has:

- a kind: expected failure, defect, or cancellation;
- a summary: the variant identity and the debug rendering of the value (G10)
  for an expected failure, the defect message and source origin for a defect,
  or the cancellation reason for a cancellation;
- the source span and trace span where the failure started;
- context frames, oldest first, each a `Text` with its source location;
- at most one `caused_by` cause;
- suppressed causes, each tagged with a reason.

<!-- chimi: check -->
```chimi
enum Suppression {
  Cleanup
  Sibling
  Displaced
}
```

`Cleanup` is a failure during unwinding or resource cleanup. `Sibling` is a
concurrent child's failure that lost to the primary. `Displaced` is an outcome
that a later failure replaced, as when a file close failure replaces a
pending success. That existing host rule now records its displaced outcome
this way.

A cause is not part of any failure value. Matching, equality, hashing,
ordering, failure rows, and selective recovery MUST ignore it. Code can read
it, but a program whose result depends on it is depending on diagnostic data.

### Cause chains

When a `Failure(...)` expression is evaluated in the body of an arm that
matched a live `Failure` pattern, the new failure's `caused_by` MUST be the
matched failure's cause:

<!-- chimi: fragment -->
```chimi
case payment.charge(total) {
  Success(receipt) -> receipt
  Failure(error) -> Failure(CheckoutError.PaymentFailed(error))
}
```

The `PaymentFailed` cause links to the cause of the original payment failure.
No wrapping syntax is needed.

If the arm re-raises the bound variable unchanged, as `Failure(error)` where
`error` is the variable bound by that arm's pattern, the runtime MUST keep the
original cause and add no chain link. `Result.get` re-raises the same way.

The compiler resolves the link statically. Lowering passes the matched
outcome as an explicit operand of the failure construction. The runtime does
not search for an enclosing handler.

### Adding context

```text
Cause.context: fn<T, E, R>(fn() -> T errors E needs R, Text) -> T errors E needs R
```

`Cause.context(operation, text)` runs the operation. On success it returns
the value. When the operation terminates with an expected failure, a defect,
or a cancellation, the runtime appends a frame to that outcome's cause and
passes the outcome on unchanged. The failure row, the variant, and the value
stay the same, so callers still match `Declined(reason)`.

The text is evaluated before the operation runs, on the success path too.
Keep it cheap. A lazy form can be added if profiles show that it matters.

### Reading causes

| Function | Type |
| --- | --- |
| `Cause.of` | `fn<T, E>(Result<T, E>) -> Option<Cause>` |
| `Cause.kind` | `fn(Cause) -> Cause.Kind` (`ExpectedFailure`, `Defect`, `Cancellation`) |
| `Cause.summary` | `fn(Cause) -> Text` |
| `Cause.frames` | `fn(Cause) -> List<Text>` |
| `Cause.caused_by` | `fn(Cause) -> Option<Cause>` |
| `Cause.suppressed` | `fn(Cause) -> List<Cause>` |
| `Cause.reason` | `fn(Cause) -> Option<Suppression>` |
| `Cause.to_text` | `fn(Cause) -> Text` |

A stored `Err` keeps the cause of the failure that `Result.capture` stored,
as metadata outside the value. `Cause.of` returns it, and `Cause.of` of `Ok`
is `None`. Sending a result through a channel moves its cause too. `Cause` is
shareable.

`Cause.to_text` renders the tree, for example:

```text
expected failure JobError.Transient("payments returned 503")
  context: charging job 42 (app/worker.chimi:41)
  caused by: expected failure HttpError.Status(503) (lib/payments.chimi:18)
  suppressed (cleanup): expected failure FileError.Close("receipts.log")
```

### Defects, cancellation, and traces

- A defect cause holds the defect message, Core node, source origin, and the
  parked-stack snapshot that the runtime already records. Context frames from
  `Cause.context` attach to defects and cancellations that pass through, but
  they never convert them.
- When several children defect in one containment, the first one in
  scheduler order is primary and the others are `Sibling` suppressed causes.
- A cancellation cause names its reason: explicit `Task.cancel`, a timeout
  scope and its limit, owner unwind, or supervisor shutdown.
- The root report prints the cause tree of a failed, defected, or cancelled
  root.
- Causes exist in every trace mode, because the root report and `Cause.of`
  need them. They are built only on failure paths. In `boundaries` and `full`
  mode, terminal-outcome events carry the cause identity and its variant
  identity. Payload renderings stay out of trace events by default, as
  channel payloads do.

### Bounds

A long-running retry or loop must not grow a cause without limit. The runtime
keeps at most 32 frames per cause, 16 suppressed causes per cause, and 64
`caused_by` links. When a limit is reached, it keeps the oldest and newest
entries, drops the middle, and records how many it dropped. The runner MAY
change these limits. They are execution configuration.

## Retry

### Schedules

A `Schedule` is pure data that describes when to try again. It is shared by
retry, polling, and later repeat helpers.

```text
Schedule.immediate:   fn(Int) -> Schedule
Schedule.fixed:       fn(Int, Duration) -> Schedule
Schedule.exponential: fn(Int, Duration, Duration) -> Schedule
Schedule.delay:       fn(Schedule, Int) -> Option<Duration>
```

- The first argument is the total number of attempts, including the first
  one. A value below 1 MUST defect at the call that receives it. The compiler
  rejects a literal below 1 (`T4347`).
- `Schedule.fixed(n, delay)` waits `delay` between attempts.
- `Schedule.exponential(n, first, limit)` waits `first` before the second
  attempt and doubles each time, never exceeding `limit`. The computation MUST
  stop doubling at `limit` and MUST NOT overflow.
- `Schedule.delay(schedule, attempt)` returns the wait before attempt
  `attempt + 1`, or `None` once `attempt` reached the total.
- `Schedule` is a closed enum with derived equality, so tests can compare
  policies.

Jitter MUST be added once `Random` and `Float` exist, as
`Schedule.jitter(Schedule, Int)` with a percentage and a
`needs { random: Random }` dependency at the point of use. Production retries
against shared services SHOULD use it.

### Policies

```text
Retry.policy: fn<E>(Schedule, fn(E) -> Bool) -> Retry.Policy<E>
Retry.run:    fn<T, E, R>(fn() -> T errors E needs R, Retry.Policy<E>)
                -> T errors E needs R + { clock: Clock }
```

A policy always has a predicate. Std provides no retry-everything policy,
because retrying validation, authorization, or business-rule failures cannot
change their result. A program that means to retry every failure writes
`fn(_) { True }`.

The operation's failure set MUST be a subset of the predicate's domain
(`T4346`). Failures outside the predicate cannot be classified, so they are a
type error rather than a silent rethrow.

`Retry.run` is ordinary Chimi:

<!-- chimi: fragment -->
```chimi
pub fn run<T, E, R>(operation: fn() -> T errors E needs R, policy: Policy<E>) {
  loop attempt = 1 {
    case operation() {
      Success(value) -> break value
      Failure(error) if policy.retryable(error) -> case Schedule.delay(policy.schedule, attempt) {
        Some(wait) -> {
          Clock.sleep(wait)
          attempt + 1
        }
        None -> Failure(error)
      }
    }
  }
}
```

- Only the listed failure variants reach the guard. Failures the predicate
  rejects bubble through the unmatched-failure rule, unchanged.
- Defects and cancellation bypass the arms and are never retried.
- Exhaustion re-raises the final failure with its own cause.
- Each attempt is a sibling span under the loop's span, so traces show every
  attempt and every backoff sleep.
- The guard calls `policy.retryable`, an ordinary function. A guard MUST have
  an empty failure row and an empty needs row, so a predicate with failures or
  needs is rejected at the guard, as any impure guard is.
- An operation that can run more than once SHOULD be idempotent or carry an
  idempotency key. Retry cannot check that, so its documentation says so.

Timeouts compose per attempt or over the whole retry:

<!-- chimi: fragment -->
```chimi
let post = fn() { client.post(url, body) }
let attempt = fn() { post |> Task.timeout(Duration.seconds(2)) }
let each_attempt = fn() { attempt |> Retry.run(policy) }

let overall = each_attempt |> Task.timeout(Duration.seconds(30))
```

`policy` here has type `Retry.Policy<HttpError | Timeout>`.

## Process and supervisor durations

This contract supplies the `Duration` that the process and supervisor
contracts already name:

- `Process.spec(body, capacity, shutdown_within)`: `shutdown_within` MUST be
  at least zero. Zero means forced termination right after the cancellation
  request.
- `Supervisor.restarts(limit, within)`: `within` MUST be positive.
- A value out of range MUST defect at the call that receives it, as the
  static contract already says for non-literal durations.
- Restart windows and shutdown deadlines use the execution time source, so
  they follow virtual time in tests.

## Diagnostics

| Code | Stage | Rejected program |
| --- | --- | --- |
| `T4340` | type | A failure-set type with a member that is not an enum, a variant, a nominal struct, a failure-set generic, or `Never`, such as `Int \| Text`. |
| `T4341` | type | Two different enum types meet in inference without a failure-set context. The diagnostic suggests `A \| B`. |
| `T4345` | type | An empty list literal passed to `Select.first` or `Task.race`. |
| `T4346` | type | A failure-set value flows into a set that lacks some of its members, such as a retry predicate that does not cover the operation's failures. The diagnostic lists the missing variants. |
| `T4347` | type | A literal attempt count below 1 in a `Schedule` constructor. |
| `T4348` | type | A `Duration` constructor whose constant argument overflows the range. The compiler SHOULD report it when it folds the constant, and MUST NOT report it for a non-constant argument. |
| `T4349` | type | `Duration.parse` applied to a `Text` literal or a module constant whose text the grammar above rejects, or whose value is out of range. A constant argument is known at build time, so the failure would be certain; the compiler MUST report it instead of compiling a call that always fails. It MUST NOT report it for a non-constant argument. |
| `T4350` | type (warning) | `Duration.parse` applied to a valid `Text` literal or module constant. The warning names the equivalent constructor expression, such as `Duration.seconds(5)` for `"5s"` or `Duration.add(Duration.hours(1), Duration.minutes(30))` for `"1h30m"`, because a constructor needs no failure handling. It does not block the build. |

Existing codes cover the remaining static checks: `T4240` and `T4243` for an
event that escapes its task, `T4241` for an event used after its receiver was
transferred, and the incompatible-need diagnostic for a `clock` need of a
different interface.

These are run-time defects, not diagnostics: duration and instant overflow,
division by zero, an empty list at `Select.first` or `Task.race`, an attempt
count below 1, an out-of-range `shutdown_within` or restart window, and
deadlock under virtual time.

## Conformance matrix

| Area | Required evidence |
| --- | --- |
| Duration range | Constructors at the boundary succeed. One more unit defects. `negate` of the minimum value defects. |
| Duration order | Derived `==` and ordering agree with `to_nanoseconds` for negative, zero, and positive values. |
| Text round trip | `parse(to_text(d)) == d` for a generated set that includes the minimum, maximum, sub-microsecond, and mixed-unit values. `1.5ns` and `1s1h` are `Invalid`. |
| Operators | `Duration.seconds(1) + Duration.seconds(2)` reports `T4303` with help that names `Duration.add`. |
| Clock need | A function that calls `Clock.sleep` shows `clock: Clock` in its inferred needs. The runner closes it at the root. |
| Virtual time | Two sleepers of 5 s and 2 s wake in deadline order after zero wall time. `Clock.since` reports exactly 5 s. The run repeats identically. |
| Virtual deadlock | A task parked on a receive with no other work defects the root with a deadlock cause naming that receive. |
| Starting timestamp | `Clock.starting_at` under `using` pins `Clock.timestamp` and leaves `Clock.now` unchanged. |
| Timeout fires | `Task.timeout(sleep 5s, 1s)` returns `TimedOut(1s)` at virtual 1 s, and child tasks inside the scope are drained. |
| Timeout passes | An operation that finishes first returns its own success, failure, or defect unchanged. |
| Timeout scope | An outer `Task.cancel` and an outer timeout pass through an inner timeout unconverted. |
| Receive and timeout | A receive that linearizes before the deadline keeps its value. A timeout that wins delivers nothing. |
| Non-positive timeout | `Task.timeout(op, Duration.zero)` never starts `op`. |
| Argument order | `op \|> Task.timeout(d)` and `Task.timeout(op, d)` have identical contracts and traces. No std module exports a `with_` twin of a combinator. |
| Constant parse | `Duration.parse("5x")` reports `T4349`. `Duration.parse("1h30m")` compiles, warns `T4350`, and suggests a constructor expression. `Duration.parse(name)` with a non-constant `name` reports neither. |
| Select commit | With two ready channels, only the first in list order delivers. The other keeps its value. |
| Select cancellation | Cancelling a parked `Select.first` leaves every channel unchanged. |
| Select timer | `Select.after` measures from the `Select.first` call. A user `Clock` timer works through its `sleep_until`. |
| Event locality | Sending an event or capturing it in `Task.start` is rejected. |
| Failure-set types | `fn(HttpError \| Timeout) -> Bool` accepts both kinds. `case` without a `Timeout` arm is non-exhaustive. `Int \| Text` reports `T4340`. |
| Projection | `Result.capture` compiles as written above. `T4203` no longer occurs. |
| Joins | A list of two captures with different rows infers the union. A list of two bare enums reports `T4341`. |
| Capture boundary | `Result.capture` stores expected failures only. A defect inside it still unwinds. |
| Combinators | `Task.all` cancels siblings on the first failure and attaches earlier sibling failures as `Sibling`. `Task.results` cancels nothing. `Task.race` drains losers before it returns. |
| Cause chain | Converting a failure in a failure arm links `caused_by`. Re-raising the bound variable adds no link. |
| Context | `Cause.context` adds a frame and leaves the variant matchable. It also annotates a passing defect without converting it. |
| Stored cause | `Cause.of(Result.capture(op))` returns the cause. Two `Err` values with equal errors and different causes are equal. |
| Bounds | A retry loop of 1000 attempts keeps causes within the configured limits and records the dropped count. |
| Retry | Selected failures retry with the scheduled delays under virtual time. Unselected failures, defects, and cancellation are never retried. Exhaustion returns the final failure. |
| Retry typing | A predicate over `HttpError` for an operation with `HttpError \| Timeout` reports `T4346`. |
| Supervisor time | A restart window and a `shutdown_within` deadline elapse under virtual time with no real waiting. |
| Tracing | Timer set, fire, and cancel, select park and commit with the committed index, timeout scope firing, virtual time advance, and terminal causes appear in `boundaries` mode without payloads. `off` mode constructs no events. |

## Implementation notes

### Duration as implemented (ts-f31a20)

`std/duration` is a built-in standard module until G1 lets it move to Chimi
source; its surface is the one this contract writes, so the move changes no
program.

- **Model.** `compiler/chimi/src/duration.rs` holds the arithmetic, the
  canonical text form, the parser, and the constructor expression for
  `T4350`. The interpreter (`execution/durations.rs`) and the type checker's
  constant checks (`typing/durations.rs`) both call it, so a build-time
  verdict cannot differ from the run-time one. The native runtime
  (`duration_*` in `runtime.c`) mirrors it value for value, text for text,
  and defect message for defect message. A language test runs a generated
  set of values and texts through both backends and compares them.
- **Representation.** The interpreter has a `Duration` runtime value and the
  native runtime a `VALUE_DURATION` whose payload is the `int64_t` count, in
  the same slot an `Int` uses. The native runtime boxes every value today,
  `Int` included, so a `Duration` is exactly as unboxed as an `Int`. Program
  data stores a constant `Duration` as one row (`CHIMI_CONSTANT_DURATION`).
- **Ordering and equality** compare the count, so `==`, the comparison
  operators, `Order.compare`, and `Map` keys follow numeric order.
- **`Duration.zero`** is a registry constant that lowers to
  `Duration.nanoseconds(0)`.
- **Defects** name the operation and its operands, such as
  ``Duration.add` overflows the Duration range: 2562047h47m16.854775807s + 1ns``
  or ``Duration.hours(2562048)` overflows the Duration range``.
- **Constant evaluator.** `typing::durations::ConstantEvaluator` folds
  literals, module constants, `Duration.zero`, `Int` arithmetic, and the
  `std/duration` constructors, conversions, and arithmetic, and reports a
  defect a folded call always raises. `T4348` uses it for constructors whose
  argument folds; process typing will use it for `T4263`.
- **Constant `parse`.** `T4349` and `T4350` apply when the argument is a
  `Text` literal or a reference to a module constant whose value folds to
  text. A local `let` bound to a literal is not a module constant and is not
  checked. The `T4350` suggestion builds one constructor per nonzero part, in
  that part's unit or the largest smaller unit that makes its count whole,
  and negates each count for a negative text, so it never overflows:
  `-1.5s` suggests `Duration.milliseconds(-1500)`.
- **Outcome records** encode a `Duration` as `{"duration":<nanoseconds>}`.
- **Interpolation** of a `Duration` stays `T4325`; write
  `${Duration.to_text(d)}` or `${Debug.render(d)}`.

### Compiler (`compiler/chimi/src`)

- **Syntax and formatter.** Parse `|` in type positions and after `errors`.
  The formatter rewrites `+` in failure rows to `|`.
- **Resolution.** Resolve failure-set members to enum, variant, struct, or
  generic identities, and reject others with `T4340`.
- **Typing (`typing.rs`).** Represent failure-set types as normalized variant
  sets with subset subtyping. Allow the value projection of failure-set
  generics as type arguments. Infer variance of data type parameters. Apply
  the join rules, give `T4303` its `Duration` help message, and emit `T4341`, `T4346`, `T4345`, `T4347`,
  `T4348`, `T4349`, and `T4350`. Check constant `Duration.parse` arguments with
  the same parser the runtime uses, so the two cannot disagree. Extend exhaustiveness to failure-set values.
- **Capability checking (`capability.rs`).** Classify `Select.Event` as
  task-local, and track receiver references held by events for `T4241`.
- **Lowering (`executable.rs`).** Lower `Failure(...)` inside a failure arm
  with the matched outcome as an explicit cause operand. Lower re-raise of the
  bound variable as a re-raise that keeps its cause. Lower `Cause.context` and
  timeout scopes as intrinsic calls with standard symbol identity. No new
  control-flow Core operation is needed.
- **Standard modules (`standard.rs`).** Add `std/duration`, `std/clock`,
  `std/select`, `std/result`, `std/cause`, `std/schedule`, and `std/retry`,
  and extend `std/task`. `Duration`, `Schedule`, `Result`, and `Retry` are
  ordinary Chimi once G1 exists. Clock reads, sleeping, timeout scopes,
  `Select`, causes, and the task combinators are primitives.

### Reference interpreter (`execution.rs`)

- Carry a cause on `ExpectedFailure`, `Defect`, and `Cancellation` outcomes.
  `ExecutionDefect` gains frames, `caused_by`, and suppressed entries.
- Model the execution time source, a timer queue ordered by deadline and then
  by registration sequence, virtual-time advance, and virtual deadlock.
- Model cancellation scopes with scope identity, so that a timeout converts
  only its own cancellation.
- Store enum identity with every enum value for failure-set matching.

### Native runtime (`compiler/chimi/runtime/native/runtime.c`)

- Read the monotonic source (`CLOCK_BOOTTIME` or `mach_continuous_time`) and
  wall time (`CLOCK_REALTIME`) at the host boundary.
- Represent `Duration`, `Instant`, and `Timestamp` as unboxed 64-bit
  integers where the backend supports it. Check overflow with
  `__builtin_*_overflow`, as `Int` arithmetic does.
- Lay out cause objects, allocated only on failure paths and bounded by the
  configured limits. Enum tags combine type identity and variant index.
- Print the cause tree in the root report.

### Scheduler (`runtime/chimi-fiber`)

- Add a timer heap keyed by deadline and registration sequence, with park on
  a timer and removal on cancellation.
- Add a time-source abstraction with a real and a virtual implementation. In
  virtual mode, `drive` advances to the earliest deadline when the runnable
  queue is empty and no host operation is in flight, and reports deadlock
  when nothing is pending.
- Add a multi-registration waiter for `Select.first`: one waiter record
  shared by all events, with a committed flag that the first linearizing
  event sets. The channel queue and inbox delivery paths must check that flag
  before they hand over a value.
- Add cancellation scopes inside a task: a scope identity on cancellation
  requests, and scope-limited draining of children started in the scope.
- Emit trace events for timers, select park and commit, scope firing, and
  virtual advance.

### Runner

- Add `--clock real|virtual` and `--clock-start`, and make the test runner
  default to virtual.
- Bind the root `clock` need to the execution clock provider.
- Expose cause limits as execution configuration.

## Changes to other specifications

These follow from this contract and are left to the owners of those files:

- [Host capabilities](host-capabilities-and-execution-properties.md): describe
  the close-failure rule through the `Displaced` and `Cleanup` suppression
  reasons.
- [Executable kernel](executable-kernel.md): record the value projection into
  type arguments.

## Open questions

1. **Should choice be syntax?** This contract makes `Select.first` a library
   function. A `select { ... }` form would remove the mapping functions and
   the intermediate enum, but it would add syntax that can only list a fixed
   set of events.
   *Recommendation:* keep the library form, and revisit it if the phase 3
   forcing program shows that the mapping noise hurts readability.
2. **May a stored `Err` carry hidden cause metadata?** The alternative is an
   explicit `Cause.Caught<E>` that holds `error` and `cause`, which every
   caller would have to unwrap.
   *Recommendation:* keep the metadata outside the value, excluded from
   equality. That treats a cause like a stack trace: present for diagnosis
   and never part of the value's meaning.
