# Async and Concurrency Primitive Models

## Question

Which async and concurrency primitives should Chimi prototype, given its
preference for direct expression-oriented code rather than `async`/`await`, its
inferred success, failure, and dependency-needs channels, and its native AOT
runtime goals?

This note compares established language and runtime models. It ends with
prototype questions rather than a Chimi syntax decision.

## Separate the concepts first

Languages often present a concurrency "model" that bundles several independent
choices. Chimi should test these choices separately:

| Concept | Question it answers | Not the same as |
| --- | --- | --- |
| Suspension | How can one computation pause without occupying an OS thread? | Starting concurrent work |
| Concurrency unit | What independently scheduled thing runs code? | An OS thread or an isolated process |
| Structured lifetime | Which work must finish or be cancelled before a scope exits? | Supervision and restarting |
| Communication | How do concurrent units exchange values or coordinate? | Ownership or state isolation |
| Isolation | What prevents unsafe concurrent access to mutable state? | Message passing itself |
| Cancellation and deadlines | How is no-longer-needed work asked to stop? | Expected failure or a defect |
| Failure propagation | What happens to a parent or siblings when work fails? | Restart policy |
| Supervision | Which long-lived work is restarted after termination, and under what policy? | Short-lived structured concurrency |
| Backpressure | What bounds queued or in-flight work when producers outrun consumers? | Limiting scheduler parallelism |
| Observability | What stable identity and causal structure can tools inspect? | A particular logging API |

Swift makes the suspension/concurrency distinction explicit: calling an
asynchronous function remains sequential within the same task, while child
tasks introduce concurrency. Java makes a similar distinction from the other
direction: virtual threads let code block in direct style, while structured
task scopes introduce and coordinate concurrent subtasks
([Swift structured-concurrency proposal](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0304-structured-concurrency.md),
[Java virtual threads](https://docs.oracle.com/en/java/javase/26/core/virtual-threads.html),
[JEP 525](https://openjdk.org/jeps/525)).

This distinction is central for Chimi. A function may be able to suspend without
starting a sibling computation; a primitive that accepts a thunk may start
concurrent work without changing the thunk's success value.

## Representative designs

### Go: goroutines, channels, and explicit cancellation context

Go starts concurrent work by applying `go` to a function call. A goroutine is a
function executing concurrently in the same address space; goroutine stacks
start small and grow, and the runtime multiplexes goroutines over OS threads.
Ordinary calls remain direct and use ordinary return values, so the language
does not color function types or require `await`
([Effective Go: goroutines](https://go.dev/doc/effective_go#goroutines)).

Channels are typed first-class values. An unbuffered channel rendezvous makes
both communication and synchronization explicit. A buffered channel makes the
sender wait only when its capacity is full, so capacity can implement a
concurrency limit or backpressure. Go also warns that starting a goroutine for
every request before applying a channel-based limit can still consume unbounded
resources; bounding the queue is not equivalent to bounding task creation
([Effective Go: channels](https://go.dev/doc/effective_go#channels)).

Cancellation and deadlines are a separate convention. `context.Context`
carries a cancellation signal, deadline, and request-scoped values through API
boundaries; derived contexts form a cancellation tree, but functions must
explicitly accept and propagate the context, and callers must invoke returned
cancel functions to release associated resources
([Go `context`](https://pkg.go.dev/context)). Goroutine lifetime itself is not
lexically confined by the `go` statement, and goroutine completion is silent
unless the program establishes a channel, wait group, or another protocol
([Effective Go: goroutines](https://go.dev/doc/effective_go#goroutines)).

Go's runtime trace observes goroutine creation, blocking, unblocking, syscalls,
GC, and scheduler activity. Its annotation API separately models logical tasks
that may span multiple goroutines; task contexts establish parent-child
relationships and attach logs and regions
([Go `runtime/trace`](https://pkg.go.dev/runtime/trace)).

**Chimi lesson:** direct-style suspension, explicit concurrent thunk start,
typed channels, cancellation propagation, and trace identity can be independent
features. Go's weakly enforced goroutine lifetime and manual `Context`
propagation are useful negative evidence for a language that wants a blessed
pit of success.

### Erlang/OTP: isolated processes, mailboxes, failure signals, and supervision

An Erlang process is a lightweight runtime-scheduled unit with its own identity.
`spawn` starts it and returns a PID. Processes communicate by sending terms to
mailboxes, and `receive` uses pattern matching to select and remove a matching
message while leaving unmatched messages in place
([Erlang processes](https://www.erlang.org/doc/system/ref_man_processes.html),
[Erlang receive expressions](https://www.erlang.org/doc/system/expressions.html#receive)).
Each process owns a heap area that includes its stack and grows under the
runtime's garbage collector; ordinary messages are copied between process
heaps except for reference-counted binaries. This isolation has a concrete
allocation and copying model, not merely a mailbox API
([Erlang process efficiency guide](https://erlang.org/documentation/doc-15.0/doc/system/eff_guide_processes.html)).

Mailbox messaging is asynchronous rather than a built-in rendezvous: sending
to a PID does not wait for receiver capacity, and the runtime exposes current
`message_queue_len` for inspection. The ordinary process mailbox has no
declared capacity or sender backpressure policy, so bounded protocols must be
built above it
([Erlang send expressions](https://www.erlang.org/doc/system/expressions.html#send),
[Erlang `process_info`](https://www.erlang.org/doc/apps/erts/erlang.html#process_info-2)).

Process failure relationships are explicit. Links are bidirectional and send
exit signals when either participant terminates; monitors are unidirectional
and deliver a `'DOWN'` message without linking the observer's fate to the
observed process. OTP supervisors use these mechanisms to start, stop, monitor,
and restart children according to a declared strategy and restart intensity
([Erlang links and monitors](https://www.erlang.org/doc/system/ref_man_processes.html#links),
[OTP supervisor behavior](https://www.erlang.org/doc/system/sup_princ.html)).
This is a long-lived operational topology, not lexical child-task joining.

The runtime can inspect process identity, current function, links, monitors,
memory, reductions, and mailbox length. Standard Logger events automatically
include PID and time, and may merge process-scoped metadata into every event
from that process
([Erlang `process_info`](https://www.erlang.org/doc/apps/erts/erlang.html#process_info-2),
[Erlang Logger](https://www.erlang.org/doc/apps/kernel/logger.html)).

**Chimi lesson:** process identity, isolated state, mailbox protocols,
failure notification, and restart supervision form a coherent service model,
but they should not be mistaken for short-lived structured concurrency or
automatic backpressure. A Chimi process layer could be built over a smaller
task kernel rather than making every suspended function call a process.

### OCaml Eio: direct-style effect-based fibers and switches

OCaml 5 effect handlers provide the runtime mechanism Eio uses for direct-style
user-level concurrency. OCaml implements handled computations with
runtime-managed, dynamically growing stack segments called fibers; a scheduler
can handle operations by suspending the current continuation and resuming
another one
([OCaml effect handlers](https://ocaml.org/manual/5.2/effects.html)). Eio exposes
ordinary direct-style functions: within one domain a fiber runs until an I/O or
other switching operation suspends it, after which another runnable fiber may
run
([Eio `Fiber`](https://ocaml-multicore.github.io/eio/eio/Eio/Fiber/index.html)).

An Eio `Switch` is an explicit structured lifetime for fibers and resources.
`Switch.run` waits for the body and all attached fibers, then releases attached
resources. A fiber spawned with `Fiber.fork` must attach to a switch, inherits
its cancellation context, and failing an attached fiber fails the switch and
cancels its other fibers
([Eio switches](https://ocaml.org/p/eio/latest/doc/README.html#switches),
[Eio `Fiber.fork`](https://ocaml-multicore.github.io/eio/eio/Eio/Fiber/index.html#val-fork)).

Cancellation contexts form a tree and cancellation recursively reaches child
contexts unless protected. Cancellation is cooperative around cancellable
operations and explicit checks; Eio deliberately distinguishes cancellation,
whose purpose is to stop fibers quickly, from recording the error that caused a
switch to fail
([Eio `Cancel`](https://ocaml-multicore.github.io/eio/eio/Eio/Cancel/index.html)).
Eio supplies promises, streams, semaphores, mutexes, and resource pools as
separate primitives rather than treating the fiber as the communication model
([Eio API](https://ocaml-multicore.github.io/eio/eio/Eio/index.html)). Its
`Stream` queue has an explicit capacity: zero is a rendezvous, a full stream
blocks writers, and `max_int` is explicitly the effectively-unbounded option.
Timeout helpers run a thunk and cancel it after a duration, with both
result-valued and exception-valued forms
([Eio `Stream`](https://ocaml-multicore.github.io/eio/eio/Eio/Stream/index.html),
[Eio `Time`](https://ocaml-multicore.github.io/eio/eio/Eio/Time/index.html)).

**Chimi lesson:** direct syntax does not require blocking OS threads or
`async`/`await`. A runtime suspension mechanism and a structured scope can stay
behind ordinary calls. Eio also shows the value of making the lifetime of both
tasks and resources impossible to omit, though Chimi must adapt its exception-
based failure policy to value-level expected failures and distinct defects.

### Java: virtual threads plus structured task scopes

Java virtual threads retain the ordinary `Thread` API and direct blocking
style. When a virtual thread calls supported blocking I/O, the runtime suspends
it and releases the carrier OS thread; many virtual threads are mapped onto a
smaller number of OS threads. Oracle explicitly recommends straightforward
synchronous code and one virtual thread per concurrent task, and says virtual
threads improve throughput rather than making an individual operation faster
([Java virtual threads](https://docs.oracle.com/en/java/javase/26/core/virtual-threads.html)).

Structured concurrency is a separate API. `StructuredTaskScope` confines fork,
join, error handling, cancellation, and result composition to one lexical
scope. Its completion policy can cancel remaining subtasks on failure or after
the first successful result, and the task tree gives observability tools the
logical relationship that a flat pool of futures loses. As of JDK 26,
structured concurrency remains a preview feature
([JEP 525](https://openjdk.org/jeps/525),
[JDK 26 features](https://openjdk.org/projects/jdk/26/)).

Virtual threads otherwise use Java's shared-memory model and thread
interruption; JEP 525 explicitly does not define channels or a replacement
cancellation mechanism. This keeps scheduling representation, structured
lifetime, communication, and cancellation policy separable
([JEP 525](https://openjdk.org/jeps/525)).

**Chimi lesson:** direct blocking-looking code and one-task-per-operation can be
an ergonomic model, but a native Chimi implementation does not inherit the
JVM's continuation, GC, or thread tooling. The prototype must measure whether
stackful runtime tasks fit Chimi's linked native runtime and memory ceiling.

### Kotlin: suspend functions, coroutine scopes, channels, flows, and supervisors

Kotlin makes suspension part of a function's shape with `suspend`, but `async`
and `await` are library functions rather than language keywords. The compiler
lowers each suspending function to continuation-passing style with a generated
continuation/state machine; this is why the language must know which functions
may suspend
([Kotlin coroutine guide](https://kotlinlang.org/docs/coroutines-guide.html),
[Kotlin language specification](https://kotlinlang.org/spec/asynchronous-programming-with-coroutines.html)).

`CoroutineScope` owns a tree of jobs. A parent waits for children; parent
failure or cancellation recursively cancels children; dispatchers separately
choose the thread or pool on which continuations run. `supervisorScope` changes
failure propagation so a child failure does not automatically cancel siblings,
while still bounding all children to the scope
([Kotlin coroutine basics](https://kotlinlang.org/docs/coroutines-basics.html),
[Kotlin exception handling](https://kotlinlang.org/docs/exception-handling.html#supervision)).

Kotlin channels are closable communication values with suspending send and
receive. Their capacity policy ranges from rendezvous through finite buffering
to conflated, drop, and unlimited modes; the chosen overflow policy determines
whether a full channel suspends the sender. `Flow` is a separate stream
abstraction whose cold producer normally runs when collected, while hot shared
flows have a lifetime owned by their producing scope
([Kotlin `Channel`](https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-core/kotlinx.coroutines.channels/-channel/),
[Kotlin flows](https://kotlinlang.org/docs/flow.html)).

The optional debug probes track coroutine creation, suspension, resumption,
hierarchy, and stacks; creation stack capture has non-negligible overhead and is
recommended off for production monitoring
([Kotlin debug probes](https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-debug/)).

**Chimi lesson:** Kotlin is strong evidence that task containment, dispatcher,
communication, flow, and supervision policy should remain orthogonal. Chimi
does not add Kotlin's explicit `suspend` color or a public suspension effect,
but native AOT still needs an equivalent continuation or stackful runtime
representation. The compiler may derive a private `may_park` summary when a
concrete lowering or tooling consumer needs it.

### Swift: structured tasks, task groups, and actors

Swift puts suspension in `async` function types and marks potential suspension
at calls with `await`. All asynchronous code runs in a task; an ordinary async
call remains in the same task, while task groups create child tasks. A group
cannot return until its children finish, and leaving by error cancels unfinished
children before waiting for them
([Swift concurrency](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/concurrency/),
[SE-0304](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0304-structured-concurrency.md)).

Cancellation is cooperative: cancelling a parent marks child tasks cancelled,
but work must check the flag or encounter an operation that responds to it.
Tasks also carry priority and task-local values, while executors schedule the
jobs between suspension points and need not preserve submission order
([SE-0304](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0304-structured-concurrency.md)).

Actors are a separate isolation abstraction. Actor-isolated mutable state is
accessed through the actor's serial executor so only one job runs on that actor
at a time; serial execution does not imply FIFO ordering, and actors may be
reentrant across suspension points
([Swift actors](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0306-actors.md),
[Swift custom actor executors](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0392-custom-actor-executors.md)).

**Chimi lesson:** a task tree and an actor/process abstraction solve different
problems. Swift's explicit `async`/`await` is not a desired syntax precedent,
but its separation of sequential async calls, concurrent child tasks,
executor jobs, and actor isolation is useful for the Chimi core.

### Koka: typed effect rows and handlers, not a complete runtime model

Koka infers an effect row as part of each function type and distinguishes pure
from effectful functions. Effect handlers are ordinary scoped values that can
remove an effect from an action's row; resumable control operations can express
generators, exceptions, schedulers, and async abstractions without making each
one a dedicated language construct
([Koka language book](https://koka-lang.github.io/koka/doc/book.html#sec-effect-handlers)).

That result is about typed control abstraction, not proof of a production async
runtime. The Koka v3 documentation says its async libraries are still lacking,
and the project roadmap lists porting `std/async` with `libuv` as future work
([Koka language book](https://koka-lang.github.io/koka/doc/book.html),
[Koka repository](https://github.com/koka-lang/koka)).

**Chimi lesson:** Koka shows how typed effects can describe suspension without a
visible future wrapper, but Chimi does not adopt that callable channel. Chimi
instead keeps suspension out of public types and may derive a conservative
compiler-private `may_park` property for code generation and tooling. General
user-defined resumable handlers remain a separate, deferred language feature.

## Cross-model comparison

| Model | Suspension surface | Concurrent unit and lifetime | Communication and isolation | Cancellation and failure | Supervision and backpressure |
| --- | --- | --- | --- | --- | --- |
| Go | Direct calls; blocking handled by goroutine/runtime | Explicit `go`; lifetime unstructured by default | Shared memory plus typed channels | Explicit propagated `Context`; goroutine completion protocol is manual | No built-in restart tree; finite channels can backpressure, but task creation must also be bounded |
| Erlang/OTP | Direct process execution with blocking receive | Runtime process, normally long-lived | Isolated process state and async selective mailbox | Exit signals, links, and monitors | OTP restart trees; ordinary mailbox has no capacity contract |
| OCaml Eio | Direct calls suspend through runtime effects | Fiber attached to mandatory switch | Shared memory; promises, streams, locks, and pools are separate | Hierarchical cooperative cancellation; exception failure cancels a switch | Scope failure policy; bounds are explicit through streams, semaphores, pools, or server limits |
| Java | Direct blocking style on virtual threads | Virtual thread; optional structured task scope | Shared memory and `java.util.concurrent` primitives | Interruption and scope policy; exceptions propagate through join | Structured scopes are not restart supervisors and define no channels |
| Kotlin | `suspend` type color; no language `await` keyword | Coroutine `Job` in a scope tree | Shared memory, channels, and flows | Cooperative cancellation plus exception propagation; supervisor scopes alter sibling policy | Supervisor jobs do not restart; channel capacity/overflow is explicit |
| Swift | `async` function plus `await` call marker | Task and lexical task group | Shared memory constrained by `Sendable`; actors isolate mutable state | Cooperative cancellation; throwing task groups propagate errors | No OTP-style restart tree; task groups do not themselves define bounded queues |
| Koka | Inferred typed effects and handlers | Library-definable control abstraction | Chosen by the handler/library | Handler semantics can transform effects | Demonstrates typing, not a mature concurrency/supervision runtime |

## Constraints that follow for Chimi prototypes

Chimi already treats functions and thunks as values, infers expected-failure
and dependency-needs rows, keeps expected errors as values, exposes no
user-facing effect row, and distinguishes defects and cancellation as runtime
outcomes
([executable kernel](../specification/executable-kernel.md),
[execution properties](../adr/0004-keep-execution-properties-out-of-callable-types.md)). The
prototype should therefore not use exceptions as the semantic representation
of ordinary task failure, even if its Rust implementation uses an internal
unwind or status code.

Chimi's selected backend is native AOT linked with a versioned runtime ABI. The
runtime is intended to own scheduling, child work, cancellation safepoints, and
structured events, while generated code owns ordinary control flow. Trace mode
must remain caller-configured, and the off path must avoid event construction
and export
([native AOT decision](../adr/0002-use-native-aot-with-a-linked-runtime.md)).

These constraints leave at least two implementation families open:

1. **Stackless tasks:** lower possibly-suspending functions into explicit state
   machines/continuations, as Kotlin does. This makes task state explicit and
   compact but changes calling convention and compiler lowering for every
   potentially suspending call
   ([Kotlin coroutine specification](https://kotlinlang.org/spec/asynchronous-programming-with-coroutines.html)).
2. **Stackful fibers:** give each task a growable or segmented runtime stack, as
   Go goroutines and OCaml effect-handler fibers do. This preserves more direct
   generated control flow but makes stack management, safepoints, unwinding,
   GC/root discovery, and debugging runtime concerns
   ([Effective Go: goroutines](https://go.dev/doc/effective_go#goroutines),
   [OCaml fibers](https://ocaml.org/manual/5.2/effects.html#s:effect-fibers)).

Neither family requires visible `async`/`await`. Ordinary calls may park, while
explicit task creation remains an operation over a thunk. Suspension does not
appear in public callable shapes; a compiler-private `may_park` summary may be
derived only when a concrete lowering or tooling consumer needs it.

## Candidate combinations to prototype

These are combinations of orthogonal choices, not syntax proposals.

| Candidate | Short-lived work | Communication | Long-lived state | Main evidence to seek |
| --- | --- | --- | --- | --- |
| Structured tasks + bounded channels | Every child is scope-owned; join/race policies are values or library operations | Finite typed channels with explicit overflow/close policy | Added later as a task loop or supervisor-owned process | Whether one small kernel covers fan-out, pipelines, cancellation, and backpressure ergonomically |
| Structured direct-style fibers + isolated processes | Scope-owned fibers for request work | Promises/channels within a request; async mailbox between processes | Named process with mailbox, links/monitors, and explicit restart policy | Whether a two-layer model is worth the extra concepts and gives clearer service ownership |
| Structured tasks + actors | Scope-owned tasks and groups | Calls to actor-isolated behavior plus explicit channels for streams | Actor holds mutable service state; restart remains separate | Whether actor reentrancy and isolation rules are understandable without hiding queue growth |
| Process-first kernel | Every concurrent unit is a process with a mailbox | Selective typed messages | Same primitive, organized by supervisors | Whether uniformity outweighs weaker lexical result collection and default mailbox backpressure |

## Concrete conformance-lab questions

### Static semantics

1. Can resolved Core conservatively derive `may_park` without adding a public
   callable channel?
2. Can higher-order calls treat unknown callbacks conservatively while tooling
   still explains the transitive operation that may park?
3. What thunk type may be started concurrently, and how do its success,
   expected-failure, and needs channels relate to a task handle?
4. Can task handles or channels escape the scope that owns them? If so, what
   replaces lexical lifetime proof?
5. Which values may cross a concurrent boundary under the permanent memory
   model, and does Chimi need an actor/ownership/sendability restriction?

### Runtime semantics

6. Is child creation always attached to an explicit scope, or may a root or
   supervisor deliberately create detached long-lived work?
7. When one child returns an expected failure value, does the scope cancel
   siblings automatically, collect all outcomes, or apply an explicit join
   policy? The answer may differ for expected failure, defect, and cancellation.
8. Are race operations allowed to report more than one success when external
   operations complete concurrently, as Eio documents for `Fiber.first`
   ([Eio `Fiber.first`](https://ocaml-multicore.github.io/eio/eio/Eio/Fiber/index.html#val-first))?
9. Is cancellation a sticky task state checked at safepoints, a typed effect, or
   both? Can cleanup temporarily mask cancellation, and how is cleanup bounded?
10. Does a timeout mean installing an absolute deadline on a task subtree,
    racing a clock operation, or merely requesting cancellation after a
    duration?

### Communication and backpressure

11. Are channels rendezvous-only, finite-buffered, or configurable? Is an
    unlimited channel forbidden from the blessed API?
12. What are the value-level results of send, receive, close, receiver
    cancellation, and sender cancellation?
13. Is multi-channel selection necessary in the first kernel, or can task races
    plus scoped channel operations express the forcing scenarios clearly?
14. Which limit controls admitted tasks, executing tasks, buffered messages,
    and host-resource concurrency? The prototype must prove these are not
    accidentally collapsed into one number.

### Supervision and observability

15. Is an OTP-like supervised process a separate long-lived abstraction above
    tasks, or only a restart combinator applied to a thunk? What stable state and
    mailbox identity survive a restart?
16. Which terminations are restartable: expected failures, defects, explicit
    shutdown, cancellation, or resource exhaustion?
17. Does every task/process receive stable runtime identity, parent/scope
    identity, computation name, source origin, and cancellation cause even when
    trace export is off?
18. Can boundary/full tracing reconstruct spawn, suspend, resume, message,
    block, unblock, cancel, cleanup, and terminal causal edges without requiring
    the production scheduler to be deterministic?

### Runtime architecture and performance

19. For the same fan-out and bounded-pipeline workloads, how do stackless state
    machines compare with stackful fibers in executable size, cold start, per-
    task allocation, idle-task memory, context-switch cost, cancellation
    latency, native stack traces, and trace-off overhead?
20. Can blocking host integrations be identified and isolated so one accidental
    blocking syscall cannot stall the runtime's worker thread?
21. What scheduling behavior is semantic, and what remains an implementation
    choice? Deterministic test scheduling must not accidentally promise FIFO
    execution in production.

## Suggested forcing artifacts

The conformance lab should use several small artifacts rather than one
application that tests everything:

1. **Compile fixtures:** suspension-effect inference, task/thunk row
   propagation, illegal lifetime escapes, and concurrent value-transfer rules.
2. **Deterministic task scenarios:** spawn/join, first-success race,
   fail-fast/all-outcomes policies, hierarchical cancellation, deadlines, and
   cleanup, driven by a virtual clock.
3. **Bounded pipeline:** a fast producer, transform workers, and slow consumer;
   independently vary admitted tasks, parallel workers, and channel capacity.
4. **Supervised state loop:** one long-lived state owner that defects and is
   restarted under a finite policy while callers observe message and state
   semantics.
5. **Host adapters:** one real socket or subprocess scenario and one real signal
   shutdown scenario, kept separate from deterministic scheduler tests.
6. **Runtime benchmarks:** fan-out, many idle tasks, channel contention,
   cancellation storms, and `off`/`boundaries`/`full` tracing modes.
7. **Composite smoke program:** only after the pieces are decided, combine a
   bounded request pipeline with one supervised state owner to test whether the
   chosen primitives compose into readable Chimi.

The first implementation question is therefore not "channels or tasks?" Tasks
name independently executing work; channels name one communication and
backpressure mechanism. The conformance lab should determine the structured
task kernel first, test bounded channels as a separate value abstraction, and
test process/actor supervision as a possible layer above both without assuming
that all three must become language syntax.
