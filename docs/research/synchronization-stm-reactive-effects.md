# Synchronization, STM, Reactive Streams, and Effect Handlers

## Question

What should Chimi take from shared-memory synchronization, software
transactional memory, Reactive Streams, and OCaml 5 effect handlers when
designing its async model?

This note treats them as orthogonal families rather than competing answers to
one question. It evaluates them against Chimi's current constraints:

- functions and thunks are values;
- function shapes infer success, failure, and `needs`, with no user-facing
  effect row;
- expected errors are values rather than thrown exceptions;
- direct-style suspension is preferred over `async`/`await`;
- the compiler emits native AOT programs with a linked runtime and should
  retain a high performance ceiling;
- values are immutable by default, while controlled mutation must remain
  available for performance;
- runtime tracing is structured but optional;
- one binary should contain the program and the runtime it needs;
- the language should make the recommended path obvious to coding agents.

No syntax is selected here.

## The models answer different questions

| Family | Primary question | What it does not inherently provide |
| --- | --- | --- |
| Shared-memory synchronization | How can concurrent work safely share or coordinate through mutable state? | Task lifetime, restart policy, stream protocols |
| Software transactional memory | How can several shared-state reads, writes, waits, and alternatives compose atomically? | I/O rollback, task lifetime, a stream abstraction |
| Reactive Streams | How does a consumer regulate an asynchronous sequence across boundaries? | General mutual exclusion, task ownership, isolation |
| Effect handlers | How can a computation suspend and transfer control to an enclosing interpreter? | A scheduling policy, backpressure, supervision |

The existing candidate division—direct suspension, scope-owned tasks, bounded
channels, and supervised processes—can therefore absorb selected parts of all
four families without collapsing them into a single abstraction.

## 1. Shared-memory synchronization

### What each primitive guarantees

A mutex protects a mutable value by granting one holder exclusive access. Rust
ties the protected value to `Mutex<T>` and exposes it only through a guard; the
guard releases the lock when its scope ends. This makes the critical section a
resource lifetime rather than a manual lock/unlock convention
([Rust `Mutex`](https://doc.rust-lang.org/stable/std/sync/struct.Mutex.html)).

An RW lock permits either several readers or one writer. It improves concurrency
only when the workload and implementation make concurrent reads worth the
extra bookkeeping; it does not make a compound protocol atomic by itself
([Rust `RwLock`](https://doc.rust-lang.org/stable/std/sync/struct.RwLock.html)).

A condition variable coordinates around a predicate over protected state. A
wait atomically releases the associated mutex and blocks, then reacquires the
mutex before returning. Wakeups may be spurious, so the predicate must be
rechecked in a loop. A notification means "the predicate may have changed," not
"the condition is now true"
([Rust `Condvar`](https://doc.rust-lang.org/stable/std/sync/struct.Condvar.html),
[Java `Condition`](https://docs.oracle.com/en/java/javase/26/docs/api/java.base/java/util/concurrent/locks/Condition.html)).

A counting semaphore controls admission rather than protecting the consistency
of a particular value. A caller takes a permit before entering a bounded region
and returns it afterward. Java explicitly separates the semaphore that limits
access to a pool from the synchronization that protects the pool's internal
state
([Java `Semaphore`](https://docs.oracle.com/en/java/javase/26/docs/api/java.base/java/util/concurrent/Semaphore.html)).

A barrier is a phase rendezvous: all participants wait until the configured
number arrives, then all may continue. An atomic exposes operations on one
location with explicit memory-ordering guarantees; weaker ordering reduces
constraints but is substantially easier to misuse than a lock
([Rust `Barrier`](https://doc.rust-lang.org/std/sync/struct.Barrier.html),
[Rust atomic ordering](https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html)).

These primitives divide into three concerns:

| Concern | Representative primitives |
| --- | --- |
| State safety | Mutex, RW lock, atomics |
| State-dependent coordination | Condition variable, notification, barrier |
| Admission and capacity | Semaphore, pool, bounded queue |

That distinction matters for Chimi. A semaphore is a natural complement to a
scope-owned task API because it bounds concurrent admission. It should not be
presented as the way to make shared state correct. A condition variable is a
low-level mechanism from which channels and other waitable state can be built;
it is rarely the best application-facing convention.

### Blocking, suspension, and scopes

OS-thread locks block the calling thread. A task-runtime lock instead suspends
the task and allows its runtime worker to run other work. Tokio's async mutex is
explicitly designed to be held across suspension and uses FIFO acquisition,
while Tokio recommends the cheaper standard mutex when the guard does not cross
a suspension point. Tokio also recommends task ownership plus message passing
for many shared I/O resources
([Tokio `Mutex`](https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html)).

Eio makes the same distinction in direct style: its mutex suspends the fiber
rather than the whole domain. Its high-level `use` operation scopes acquisition
and release, can protect a critical section from cancellation, and can disable
the mutex if the critical section raises, while its low-level manual API warns
about deadlock and cancellation hazards
([Eio `Mutex`](https://ocaml-multicore.github.io/eio/eio/Eio/Mutex/index.html)).

For Chimi, a structured lock operation is more important than a guard syntax.
It can accept a thunk, acquire a resource, run the thunk, and release on every
exit path. Because thunks are values, this composes naturally with piping and
does not require a special statement form. A guard value may still be useful
inside the runtime or unsafe/performance-oriented library, but allowing a guard
to escape or cross arbitrary suspension points would complicate ownership,
cancellation, and deadlock analysis.

### Failure, cancellation, fairness, and deadlock

Lock poisoning is a policy, not a universal property. Rust's standard mutex
marks a lock poisoned when it observes a panic while held, but poisoning is
advisory and can be bypassed for recovery. Tokio's async mutex does not poison;
if a panic is caught, protected data may therefore be inconsistent
([Rust `Mutex` poisoning](https://doc.rust-lang.org/stable/std/sync/struct.Mutex.html#poisoning),
[Tokio `Mutex`](https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html)).

Chimi should not turn an expected value-level failure inside a critical section
into poison automatically. A returned failure is ordinary control flow, and a
library can preserve or replace the state before returning it. Poisoning is
relevant to defects, forced cancellation at an unsafe point, or an invariant
breach. The policy must say whether the resource is disabled, repaired, or
released as-is.

Cancellation while waiting should remove the waiter without acquiring the
resource. Cancellation while holding a resource must run release logic, but it
cannot prove that partially mutated state is valid. Tokio documents that
cancelling a queued mutex, RW-lock, or semaphore acquisition loses its queue
position; Eio can protect a critical section from cancellation but still allows
cancellation while waiting
([Tokio cancellation safety](https://docs.rs/tokio/latest/tokio/macro.select.html#cancellation-safety),
[Eio `Mutex`](https://ocaml-multicore.github.io/eio/eio/Eio/Mutex/index.html)).

Fairness is observable under contention and has a throughput cost. Tokio's
mutex and semaphore use FIFO queues; Java semaphores allow fair or non-fair
operation and document barging and throughput tradeoffs. Multi-permit FIFO
acquisition can also cause head-of-line blocking
([Tokio `Semaphore`](https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html),
[Java `Semaphore`](https://docs.oracle.com/en/java/javase/26/docs/api/java.base/java/util/concurrent/Semaphore.html)).

None of these primitives prevents deadlock. Scoped release prevents forgotten
unlock, but lock cycles, self-locking, barrier participant loss, and a condition
whose notifier cannot run remain possible. Rust even leaves recursive locking
of its non-reentrant mutex unspecified between panic and deadlock
([Rust `Mutex::lock`](https://doc.rust-lang.org/stable/std/sync/struct.Mutex.html#method.lock)).

### Observability and native cost

A tracing runtime can record stable resource identity, acquisition request,
suspension, acquisition, release, queue depth, wait duration, and cancellation.
It must not make fairness or scheduler order part of language semantics merely
because the trace exposes them. Deadlock diagnostics become materially better
if the runtime can snapshot `task -> waits for resource -> held by task` edges.

Native implementations of mutexes, conditions, semaphores, and atomics are
compatible with a single linked binary. Their fast paths can be small atomic
operations; contended paths require waiter queues and scheduler integration.
Suspension-aware primitives cost more state than raw OS locks but avoid pinning
a runtime worker. Atomics can be the cheapest mechanism and the most expensive
to verify. These are implementation tradeoffs to benchmark rather than language
semantics.

### Chimi classification

| Candidate | Classification | Initial disposition |
| --- | --- | --- |
| No data races in safe Chimi | Language semantic | Required before parallel tasks |
| Scope-bound resource acquisition | Language/library contract | Blessed convention |
| Suspension-aware mutex and semaphore | Official runtime/library primitive | Prototype |
| RW lock, condition, barrier | Official library primitive | Add only with demonstrated use |
| Atomics and explicit memory order | Low-level/unsafe library primitive | Defer until memory model is explicit |
| Wait queues, parking, wakeups | Internal runtime mechanism | Required by higher primitives |
| Poisoning policy | Runtime/library contract | Decide for defects, not expected failures |

## 2. Software transactional memory

### Haskell STM

Haskell separates transactional computations from I/O in the type system.
`TVar` holds transactional state; `atomically` executes an `STM a` computation
as an `IO a`. `retry` abandons the current attempt and may block until a `TVar`
read by that attempt changes. `orElse` runs an alternative when its first
transaction retries, and retries the whole choice only if both alternatives do
([GHC STM guide](https://ghc.gitlab.haskell.org/ghc/doc/users_guide/exts/stm.html),
[GHC `Control.Monad.STM`](https://downloads.haskell.org/ghc/9.6.6/docs/libraries/stm-2.5.1.0/Control-Monad-STM.html)).

The important property is composability. A bounded queue operation can use
`retry`; two queue operations can be combined atomically; `orElse` can wait for
whichever of several transactional protocols becomes possible. The original
model was designed specifically to compose blocking and choice without exposing
lock ordering
([Composable Memory Transactions](https://www.microsoft.com/en-us/research/publication/composable-memory-transactions/)).

Conflicting attempts may execute again automatically, so externally visible
effects inside an attempt are unsafe unless they are themselves transactional
or idempotent. The Haskell API keeps ordinary I/O outside `STM`, warns that
`unsafeIOToSTM` can break transaction guarantees, and rolls the transaction
back when an uncaught transactional exception escapes `atomically`
([GHC `Control.Monad.STM`](https://downloads.haskell.org/ghc/9.6.6/docs/libraries/stm-2.5.1.0/Control-Monad-STM.html)).

STM prevents lock-order deadlock inside its transactional domain, but does not
guarantee fairness or freedom from starvation. GHC's transactional semaphore
explicitly provides no waiter ordering and wakes contenders to compete; its
documentation warns against high contention
([GHC `TSem`](https://hackage.haskell.org/package/stm/docs/Control-Concurrent-STM-TSem.html)).
Retries also mean that transaction cost depends on contention, read/write-set
size, validation, allocation during speculative attempts, and retry wakeups.
GHC implements these behaviors in its runtime rather than compiling a
transaction into an ordinary critical section
([GHC STM runtime source](https://gitlab.haskell.org/ghc/ghc/-/blob/ede94e9d4b326ee0cdb4b5bce3d8eba9e81601fc/rts/STM.c)).

### Clojure refs

Clojure's refs provide coordinated mutable locations changed only inside
transactions. `dosync` gives atomic, consistent, isolated updates and retries
conflicting transactions automatically. Its implementation uses multiversion
concurrency control and snapshot isolation; `alter`, `ref-set`, `ensure`, and
`commute` express different conflict rules. Clojure explicitly says to avoid
I/O and other side effects in transactions because attempts may be retried, and
recommends immutable persistent values so speculative copies and modifications
are cheap
([Clojure refs and transactions](https://clojure.org/reference/refs)).

Haskell gives the transaction boundary and permitted operations a dedicated
`STM` type. Clojure makes the boundary a dynamic `dosync` region around refs and
uses runtime restrictions/conventions for side effects. Chimi's inferred effect
rows make the Haskell separation more natural: a transaction could allow only
transactional-state operations, value-level computation, and explicitly safe
effects. That is a language-design possibility, not a recommendation to add
STM now.

### Invariants, errors, and cancellation

STM is strongest when an invariant spans several mutable locations and all
participating operations stay within the transaction. It is weaker when the
invariant crosses a socket, file, subprocess, external database, process
mailbox, or nontransactional runtime structure. Those operations cannot simply
be undone when an attempt conflicts.

For Chimi, a value-level failure returned by an attempt needs an explicit rule:
commit the writes together with the failure value, or abort them. Inferring the
answer from the fact that the value happens to inhabit a failure channel would
be surprising. Likewise, cancellation must abort speculative writes and leave
no transaction-owned wait registrations behind. Cancellation itself should not
be an expected business failure unless a caller deliberately maps it into one.

Automatic retry must remain distinct from Chimi's ordinary functional retry
composition. Functional retry reruns a whole effectful thunk according to a
caller-selected policy. STM conflict retry is an internal attempt to commit one
atomic memory transition and must not repeat arbitrary external effects.

### Composition with the current async candidate

STM can implement composable queues, promises, semaphores, and selective waits.
It therefore overlaps with channel internals but not with channel semantics.
Tasks still need structured ownership; processes still need isolation and
supervision; direct suspension still needs a continuation/runtime mechanism.

The design cost is large for native AOT: transaction logs or versioned cells,
read/write sets, validation, retry dependency tracking, wakeups, cancellation,
GC/root integration, and tracing all enter the linked runtime. Default
immutability helps reduce accidental shared mutation, but it does not make an
STM implementation cheap. The performance ceiling should be established with
contention benchmarks before committing to a language-level transaction model.

### Chimi classification

| Candidate | Classification | Initial disposition |
| --- | --- | --- |
| Atomic multi-location transaction | Possible language semantic | Defer pending memory model and evidence |
| Transaction-only effect capability | Possible effect-row rule | Prototype only if STM advances |
| Transactional cells and `retry`/choice | Official runtime/library primitive | Experimental prototype |
| Conflict detection, validation, retry wait sets | Internal runtime mechanism | Required only for STM |
| Arbitrary I/O inside transactions | Invalid design | Exclude |
| STM as implementation of every channel/task | Internal architecture choice | Avoid without benchmark evidence |

## 3. Reactive Streams and dataflow

### The protocol

Reactive Streams standardizes asynchronous sequence exchange with non-blocking
backpressure. Its four interfaces are `Publisher`, `Subscriber`, `Subscription`,
and `Processor`. After subscription, a subscriber receives zero or more
requested elements followed by at most one terminal error or completion signal.
A publisher may not send more elements than requested, and signals to one
subscriber must be serial
([Reactive Streams specification](https://github.com/reactive-streams/reactive-streams-jvm/blob/v1.0.4/README.md#api-components)).

Demand is a counter communicated through `Subscription.request(n)`. It is not a
guarantee that every implementation buffer is bounded; it is the protocol that
allows implementations to keep asynchronous-boundary queues bounded. The
subscriber must cancel a subscription it no longer needs, and cancellation may
take time to propagate, so some already-requested elements can still arrive
([Reactive Streams specification](https://github.com/reactive-streams/reactive-streams-jvm/blob/v1.0.4/README.md#specification)).

Java's `Flow` API mirrors these interfaces and semantics in the standard
library, showing that the protocol can remain a library contract rather than a
language construct
([Java `Flow`](https://docs.oracle.com/en/java/javase/26/docs/api/java.base/java/util/concurrent/Flow.html)).

### Hot, cold, ordering, and terminal outcomes

Reactive Streams does not itself specify whether a publisher is cold or hot,
or whether multiple subscribers receive independent, replayed, or shared data.
Reactor makes that policy explicit: a cold publisher generates data anew for
each subscription, while a hot publisher can produce independently of
subscribers and late subscribers may see only later elements
([Reactor hot versus cold](https://projectreactor.io/docs/core/snapshot/reference/advancedFeatures/reactor-hotCold.html)).

The protocol preserves serial signals per subscription but operators determine
semantic ordering. A concurrent flattening operator may interleave inner
publishers, while a concatenating variant preserves their order. Buffer size,
prefetch, concurrency, and consumption order can interact strongly enough to
stall an operator graph even though every component honors backpressure
([Reactor batching and backpressure](https://projectreactor.io/docs/core/release/reference/advancedFeatures/advanced-three-sorts-batching.html),
[Reactor `Flux` operators](https://projectreactor.io/docs/core/release/api/reactor/core/publisher/Flux.html)).

Reactive Streams gives terminal `onError` a control role: no later elements may
follow it. Reactor consequently treats an error as terminating the upstream
sequence and uses operators to replace, transform, or recover with another
sequence
([Reactor error handling](https://projectreactor.io/docs/core/release/reference/coreFeatures/error-handling.html)).
That is not automatically compatible with Chimi's expected errors as values.
A Chimi stream may carry values whose type includes expected failures and still
continue. A terminal stream failure should instead mean the producer cannot
continue, the protocol was violated, or the scope was cancelled. The type and
library contract must keep per-element outcomes separate from terminal causes.

### Operator graphs, fusion, and observability

Production reactive libraries build graphs of publishers/operators and often
optimize them. Reactor has a queue-fusion contract that can remove request
accounting or asynchronous-boundary overhead in eligible chains. Fusion is an
implementation optimization; source-level operator meaning cannot depend on
whether fusion occurs
([Reactor fuseable contracts](https://projectreactor.io/docs/core/release/api/reactor/core/package-summary.html)).

This representation creates both an opportunity and a tooling burden. Reactor
can inspect parents, downstream consumers, inner publishers, backlog, capacity,
prefetch, and run style through `Scannable`, while its debugging support can
capture assembly information and checkpoints. The state is explicitly
best-effort rather than a strongly consistent snapshot
([Reactor `Scannable`](https://projectreactor.io/docs/core/release/api/reactor/core/Scannable.html),
[Reactor debugging](https://projectreactor.io/docs/core/snapshot/reference/debugging.html)).

Chimi's optional trace should distinguish graph assembly from runtime signals.
Useful event data includes stream/subscription identity, producer and consumer
task identity, requested and delivered counts, buffer occupancy, operator
identity, terminal outcome, and cancellation cause. Full per-element tracing
must remain optional because it can dominate allocation and I/O. Aggregate
demand and occupancy metrics can offer a lower-cost mode.

### API and type complexity

A rich reactive API multiplies concepts: one-versus-many cardinality, hot versus
cold ownership, subscription lifetime, demand, buffers, scheduler boundaries,
merge ordering, terminal errors, retries, sharing, and replay. Fluent operator
syntax can make a graph concise while hiding where work runs or which operator
buffers. This is a pit-of-success risk for agents unless defaults are narrow and
the type checker rejects ambiguous lifetime and failure behavior.

Chimi already has the smaller ingredients for a stream library: thunks,
scope-owned tasks, bounded channels, value-level errors, and direct suspension.
A pull-like consumer that directly suspends for the next value can compile to a
demand protocol internally without exposing callbacks or `Subscriber` methods.
Processes can own hot stream lifetimes; lexical task scopes can own cold finite
pipelines. These policies should be explicit in types or constructors even if
the low-level demand signals remain internal.

### Chimi classification

| Candidate | Classification | Initial disposition |
| --- | --- | --- |
| Demand never permits overproduction | Stream library semantic | Prototype |
| Hot/cold and ordered/unordered policy | Stream type/library contract | Must be explicit if streams advance |
| Per-element expected errors as values | Language/type interaction | Preserve Chimi model |
| Terminal completion/failure/cancellation | Stream runtime contract | Prototype |
| Publisher/subscriber callbacks | Interop/internal mechanism | Do not bless as source API |
| Operator graph and fusion | Library/compiler/runtime optimization | Defer until semantics stabilize |
| Demand, buffer, and operator trace events | Runtime observability | Prototype sampled/aggregate modes |

## 4. OCaml 5 effects and Eio

### Effect handlers as a suspension mechanism

OCaml effects are synchronous operations handled by a dynamically enclosing
handler. Performing an effect captures the delimited continuation up to that
handler; the handler may resume it later, which is enough to implement
lightweight tasks, generators, coroutines, and asynchronous I/O in direct style
([OCaml effect handlers](https://ocaml.org/manual/5.2/effects.html)).

The continuation is one-shot. Resuming it more than once raises
`Continuation_already_resumed`; failing to resume or discontinue it can leak the
fiber stack and resources held by the suspended computation. OCaml chose
one-shot continuations because concurrent programming normally needs them,
they avoid copying stack frames, and they fit linear resources such as sockets
([OCaml effect-handler semantics](https://ocaml.org/manual/5.2/effects.html#ss:effect-semantics)).

This model separates direct-style source code from the runtime protocol. Eio's
private suspension effect is performed when a fiber must wait; its event loop
handles that effect and resumes the fiber later
([Eio private effects](https://ocaml-multicore.github.io/eio/eio/Eio/Private/Effects/index.html)).
The application uses ordinary calls rather than constructing visible futures
or writing `await` at each suspension.

### Handler delimitation and effect typing

Deep handlers reinstall themselves when a continuation resumes; shallow
handlers handle one effect and require the resumer to provide the next handler.
Effects not handled by the current handler are forwarded outward. OCaml does
not statically track which effects a computation may perform or prove that all
are handled; an unhandled effect raises `Effect.Unhandled`
([OCaml effect-handler semantics](https://ocaml.org/manual/5.2/effects.html#ss:effect-semantics)).

Chimi should not copy that typing choice or add a public effect row. The initial
runtime may use handlers internally while exposing no general handler API, and
resolved Core may derive a conservative compiler-private `may_park` property
for a concrete backend or tooling consumer. Expected Chimi failures remain
values; continuation discontinuation is a runtime cancellation/defect path,
not a replacement for the failure row.

### Structured lifetime and cancellation in Eio

An Eio `Switch` owns fibers and resources. It does not return until its attached
fibers finish, then releases resources in LIFO order. A resource cannot outlive
its switch. Failing the switch records the error, cancels its fibers, waits for
shutdown, and reports after cleanup
([Eio `Switch`](https://ocaml-multicore.github.io/eio/eio/Eio/Switch/index.html)).

Fibers forked on a switch inherit its cancellation context. Cancellation
contexts form a tree and cancellation recursively reaches children unless a
subcontext is protected. Cancellation is cooperative through registered
operations and checks; Eio explicitly distinguishes cancellation used to stop
work from the error that caused a switch to fail
([Eio `Fiber`](https://ocaml-multicore.github.io/eio/eio/Eio/Fiber/index.html),
[Eio `Cancel`](https://ocaml-multicore.github.io/eio/eio/Eio/Cancel/index.html)).

This is close to Chimi's desired division. A scope can own task and resource
lifetime; cancellation can be a distinct runtime cause; value-level failures
can flow normally until a scope policy decides whether one should cancel
siblings. The handler mechanism supplies suspension but does not dictate that
policy.

### Native AOT representation and performance implications

OCaml implements handled computations with runtime-managed, dynamically growing
stack segments called fibers. A captured continuation points to suspended stack
segments on the heap; capture does not copy frames, and resumption relinks the
segment into the active stack
([OCaml effect-handler fibers](https://ocaml.org/manual/5.2/effects.html#ss:effect-fibers)).
The OCaml implementation paper reports about 1% mean overhead on macrobenchmarks
that do not use effects and preserves compatibility with tools that inspect the
stack, demonstrating that native AOT and efficient effect handlers can coexist
([Retrofitting Effect Handlers onto OCaml](https://arxiv.org/abs/2104.00250)).

It does not make the representation free for Chimi. Chimi's runtime must define:

- stack-segment allocation, growth, reuse, and per-task memory bounds;
- which registers and stack slots are GC roots while a continuation is parked;
- unwinding and cleanup when a task returns, defects, or is cancelled;
- backtraces across handler and suspension boundaries;
- debugger/profiler representation for detached stacks;
- foreign-call boundaries across which suspension is forbidden or adapted;
- trace identity that survives suspension, migration, and resumption.

OCaml itself forbids effects from crossing its C-to-OCaml callback boundary,
showing that FFI behavior is part of the semantic contract rather than an
afterthought
([OCaml effect-handler limitations](https://ocaml.org/manual/5.2/effects.html#ss:effect-limitations)).

The alternative is stackless lowering: compile every suspending function into
a state machine or continuation-passing form. That can make parked state and GC
roots explicit but colors the compiler's intermediate representation and may
increase code size. An OCaml-style one-shot stackful implementation can preserve
ordinary call-shaped native code but deepens runtime, unwind, and toolchain
integration. The conformance lab should test shared semantics against both
representations before the compiler commits to one.

### User-defined versus runtime-internal handlers

General user-defined handlers would let libraries interpret effects, implement
test runtimes, and define control operators. They would also expose
continuations, dynamic handler scope, effect-row subtraction, resource
linearity, and interactions with cancellation and defects. That is a separate
language feature from using a private one-shot suspension effect inside the
runtime.

For an agent-friendly first kernel, the smaller surface is preferable: direct
suspending calls and scope-owned tasks as language semantics, a private runtime
suspension protocol, and explicit official primitives for channels and
synchronization. General handlers should wait until a concrete use case and
resource model can make invalid handler compositions unrepresentable.

### Chimi classification

| Candidate | Classification | Initial disposition |
| --- | --- | --- |
| Suspension appears in public effect rows | Language semantic | Reject |
| Compiler derives private `may_park` where consumed | Compiler property | Prototype with its first consumer |
| Direct calls may suspend without `await` | Language semantic | Prototype first |
| Scope owns tasks and resources | Language/runtime semantic | Prototype first |
| One-shot continuation/fiber | Internal runtime mechanism | Strong candidate, benchmark |
| Runtime-private suspension handlers | Internal runtime mechanism | Strong candidate |
| General user-defined handlers | Language feature | Defer |
| Cancellation as continuation discontinuation/unwind | Runtime mechanism | Prototype with cleanup tests |

## Decision matrix

Ratings describe fitness for the specific concern, not overall desirability.

| Criterion | Shared synchronization | STM | Reactive Streams | OCaml-style effects |
| --- | --- | --- | --- | --- |
| Protect one mutable invariant | Strong | Strong but heavier | Not applicable | Not applicable |
| Compose several mutable invariants | Manual lock discipline | Strong | Not applicable | Only by interpreting state effects |
| State-dependent waiting/choice | Low-level conditions | Strong `retry`/choice | Demand only | Can suspend, but needs a protocol |
| Bound concurrent admission | Strong semaphore | Can model it | Indirect through demand | Not by itself |
| Bound asynchronous sequences | Bounded queue/channel | Can implement queues | Primary purpose | Not by itself |
| Preserve direct source style | Yes | Yes inside transaction boundary | Often graph/callback style | Strong |
| Structured task/resource lifetime | Only scoped guards | No | Subscription cancellation only | Strong with an Eio-like switch |
| Expected errors as values | Natural | Needs commit/abort rule | Natural as elements; terminal error differs | Orthogonal |
| Cancellation semantics | Primitive-specific | Must abort attempt | Eventual protocol signal | Natural through discontinued continuations |
| Deadlock resistance | Weak | Strong inside transaction domain | Backpressure graphs can still stall | Depends on scheduler/protocol |
| Runtime implementation size | Low to medium | High | Medium to high | High |
| Native AOT compatibility | Straightforward | Feasible, runtime-heavy | Feasible, library/runtime-heavy | Demonstrated feasible, toolchain-heavy |
| Optional trace value | Lock/wait graph | Attempts/conflicts/retries | Demand/operator graph | Task/continuation causality |
| Best role in first Chimi kernel | Minimal official primitives | Experimental/deferred | Library prototype | Suspension implementation candidate |

## Conformance-lab scenarios

Each scenario should assert semantic results and structured events while making
schedule order deterministic only inside the lab.

### Shared synchronization scenarios

1. **Scoped mutex release:** success, value-level failure, defect, and
   cancellation all leave the mutex in the specified usable or poisoned state.
2. **Suspension-aware contention:** one task waits on a lock while unrelated
   tasks continue on the same runtime worker.
3. **Condition predicate loop:** notification before wait, spurious wakeup, and
   broadcast never let a consumer observe a false predicate.
4. **Semaphore admission:** at most `N` thunks enter; queued cancellation removes
   the waiter and never leaks a permit.
5. **Fairness modes:** FIFO and throughput-oriented modes produce their promised
   acquisition constraints without making a global scheduling guarantee.
6. **Deadlock diagnostics:** two resources in a cycle produce a traceable wait
   graph and deterministic lab timeout.
7. **Barrier participant cancellation:** define whether the phase breaks,
   shrinks, or waits forever; do not leave this implicit.

### STM scenarios

1. **Atomic invariant:** concurrent transfers preserve a total across several
   transactional cells.
2. **Conflict retry:** two writes conflict; only one visible commit occurs and
   the trace distinguishes an attempt from a committed transaction.
3. **State-dependent wait:** `retry` sleeps until a read dependency changes
   rather than polling.
4. **Alternative choice:** two blocked alternatives wake correctly and commit
   exactly one branch.
5. **Expected failure policy:** explicitly test commit-on-failure and
   abort-on-failure candidates.
6. **Forbidden effect:** network, file, process message, and ordinary mutable
   writes are rejected inside a retryable transaction.
7. **Cancellation:** cancelling a running or waiting transaction rolls back
   speculative state and removes dependency registrations.
8. **Contention gate:** measure attempts per commit, tail latency, allocation,
   and starvation under skewed hot-cell workloads.

### Reactive/dataflow scenarios

1. **Demand accounting:** producer delivery never exceeds cumulative demand,
   including reentrant requests.
2. **Bounded slow consumer:** buffer occupancy never exceeds capacity and the
   producer suspends or applies an explicit overflow policy.
3. **Cancellation race:** already-requested in-flight values and eventual stop
   follow one documented rule.
4. **Terminal states:** completion and terminal failure occur once; expected
   failure values can be followed by later elements.
5. **Cold versus hot:** a second consumer either causes a new producer or joins
   a shared lifetime according to an explicit policy.
6. **Ordering:** concurrent merge and ordered concatenation have distinct,
   asserted output guarantees.
7. **Graph stall:** bounded buffers plus a consumption-order cycle produce a
   diagnosable wait/demand graph.
8. **Fusion equivalence:** fused and unfused execution produce identical
   values, terminal outcomes, cancellation, and logical trace relationships.
9. **Trace overhead:** off, aggregate, and per-element modes have separate
   allocation and throughput gates.

### Effect-handler/runtime scenarios

1. **Direct suspension:** nested ordinary calls suspend and resume with no
   source-visible future or await marker.
2. **One-shot enforcement:** double resume is impossible or deterministically
   diagnosed; abandoned continuations are cleaned up.
3. **Handler delimitation:** nested runtime handlers forward and intercept only
   their declared operations.
4. **Scope exit:** no child continuation or attached resource outlives its
   scope.
5. **Sibling failure:** a configured scope policy cancels siblings, waits for
   cleanup, and preserves the original value-level or defect cause.
6. **Cancellation while parked:** discontinuation runs cleanup once and removes
   the continuation from all waiter queues.
7. **Stack roots:** values live only in parked frames survive GC and resume
   correctly.
8. **Backtrace continuity:** diagnostics connect caller frames across suspend,
   migration, and resume boundaries.
9. **FFI boundary:** an attempted suspension across a forbidden native callback
   is statically rejected or deterministically trapped.
10. **Representation benchmark:** stackful and stackless implementations run the
    same fan-out, deep-stack, high-suspension, and cancellation workloads with
    code-size, allocation, RSS, and latency gates.

## Major risks

1. **Blessing too many peer abstractions.** Agents will choose inconsistently if
   mutexes, STM, channels, streams, and processes all look equally canonical.
2. **Conflating expected failure and runtime termination.** STM abort,
   stream-terminal error, task defect, cancellation, and a returned failure
   value require distinct rules.
3. **Hidden unboundedness.** Demand protocols, mailboxes, task creation, parked
   continuations, retry wait sets, and trace buffers can each grow independently.
4. **Cancellation-unsound critical regions.** Releasing a lock is not enough if
   cancellation exposes partially updated state.
5. **Effect-handler surface area.** Public handlers would force early decisions
   on effect-row subtraction, continuation ownership, handler forwarding, and
   resource linearity.
6. **Runtime lock-in before measurement.** Stackful fibers, stackless state
   machines, STM logs, and reactive operator graphs have different code-size,
   GC, and observability costs.
7. **Tracing changes semantics or performance.** Instrumentation must not add
   synchronization that hides races or makes a fairness guarantee, and trace-off
   mode must avoid allocating event payloads.
8. **Parallelism arrives before a memory model.** Safe sharing, ownership, send
   eligibility, atomics, and foreign memory must be settled before tasks run in
   parallel.

## Recommended next decision order

1. **Specify task and resource lifetime independently of scheduling.** Decide
   scope exit, child completion, value-level failure, defect, and cancellation
   behavior first.
2. **Specify direct suspension as runtime behavior.** Keep it out of public
   callable types; derive a conservative compiler-private property only where a
   backend, restricted boundary, or tool consumes it.
3. **Prototype the runtime representation behind the same semantics.** Compare
   one-shot stackful continuations with stackless lowering using the handler
   conformance scenarios and native AOT performance gates.
4. **Choose the minimum shared-state library.** Start with a scope-bound,
   suspension-aware mutex and semaphore. Keep atomics, condition variables,
   barriers, and RW locks secondary until a forcing scenario needs them.
5. **Specify bounded-channel semantics.** Capacity, close, cancellation,
   fairness, and trace identity should stabilize before higher dataflow APIs.
6. **Prototype a small direct-style stream library over tasks and bounded
   channels.** Test demand, hot/cold lifetime, ordering, terminal causes, and
   optional tracing without exposing the Reactive Streams callback protocol as
   Chimi's blessed source API.
7. **Evaluate supervised processes above the task kernel.** Processes can own
   long-lived state and hot streams; they should not become the implementation
   of every mutex, stream, or finite task.
8. **Revisit STM only after the memory model and benchmarks exist.** A small
   experimental transactional-cell runtime can answer whether composable
   `retry` and choice justify their implementation and typing cost.
9. **Revisit public effect handlers last.** Runtime-private handlers can enable
   direct suspension without committing the language to general continuation
   programming.

This order keeps the blessed path narrow: direct calls suspend, scopes own
finite tasks and resources, bounded channels communicate, and supervised
processes own long-lived state. Synchronization supports controlled shared
state; streams add demand semantics; STM and public handlers remain candidates
whose costs must earn their surface area.
