# Runtime Computation Context Precedents

## Question

Do programming languages or runtimes have a built-in concept analogous to Chimi's proposed computation name and context on runtime execution events?

## Finding

Yes, although no precedent exactly matches the whole proposal. Go has the closest established logical-operation model. Java 25 Flight Recorder has the closest automatic event-context model. .NET provides a mature ambient operation tree and event correlation. Erlang/BEAM supplies process labels, process-scoped logging metadata, and VM tracing as separate mechanisms.

Together they support making computation identity first-class in Chimi's runtime rather than treating it as an exporter convention.

## Go: named logical tasks in the runtime trace

Go's standard `runtime/trace` package defines a `Task` as a user-defined logical operation. `trace.NewTask(parent, taskType)` returns a `context.Context` carrying the task; a task may span multiple goroutines, and tasks created from that context become subtasks. Logs and regions emitted with the context are associated with the task by the runtime trace. The trace tool measures task latency and groups results by task type ([Go `runtime/trace`](https://pkg.go.dev/runtime/trace)). Go's runtime execution trace combines these annotations with scheduler, goroutine, garbage-collection, and blocking events ([Go execution-trace overview](https://go.dev/blog/execution-traces-2024)).

The important distinction is that `taskType`, such as `makeCappuccino`, is a bounded-cardinality classification. Per-execution data, such as an order ID, is logged separately. This maps well to:

```text
computation: Checkout.submit
execution_id: 01...
node/operation: Payment.charge
attributes: { order_id: ... }
```

**Status:** runtime and standard-library support, not a third-party tracing convention. Instrumentation is explicit, and context propagation follows Go's ordinary `context.Context` discipline rather than being language-automatic.

## Java: contextual fields attached to runtime events

Java Flight Recorder is a profiling and event system built into the JDK. Applications can define duration or instant events with structured fields and commit them directly into the same recording used for JVM and JDK events ([JFR overview](https://docs.oracle.com/en/java/java-components/jdk-mission-control/9/user-guide/using-jdk-flight-recorder.html), [`jdk.jfr.Event`](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/Event.html)).

Java 25 added `@Contextual`. A contextual event field represents state applying to every event on the same thread for the contextual event's lifetime. Oracle's example declares contextual trace ID and trace name fields; a later JVM `JavaMonitorEnter` event is displayed with `Trace.id`, `Trace.name`, and an order ID even though those fields do not belong to the monitor event itself ([`jdk.jfr.Contextual`](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/Contextual.html)).

This is a direct precedent for Chimi exporters being able to reconstruct or display root computation context on local runtime events without encoding the fields independently in every event record. Oracle cautions that parsers must track active context and that contextual fields should be used sparingly.

**Status:** JDK/runtime-supported event context. Its documented scope is same-thread temporal context, so it is weaker than Chimi's intended propagation across async tasks, retries, and dependency calls unless the application establishes new context at those boundaries.

## .NET: ambient named operations and correlated events

.NET represents units of traced work as `System.Diagnostics.Activity` objects. Activities form a parent-child tree, carry a stable operation name, unique trace/span IDs, tags, and timestamped events. `Activity.Current` flows through synchronous calls and follows `async` work across threads; built-in HTTP libraries can also propagate trace IDs across process boundaries ([.NET distributed-tracing concepts](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/distributed-tracing-concepts)). `ActivitySource` and `Activity` are runtime APIs; collectors such as OpenTelemetry are optional ([.NET instrumentation guide](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/distributed-tracing-instrumentation-walkthroughs)).

The older built-in `EventSource` system also attaches activity IDs to emitted events. Start/stop event pairs establish a unit of work, the ID follows `async`/`await`, and related activity IDs represent parent-child work trees ([EventSource activity IDs](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/eventsource-activity-ids)). Logging APIs can automatically capture the active Activity IDs, correlating logs with the distributed trace.

**Status:** runtime/standard-library primitives with some automatically instrumented framework operations. User-defined computation names still require instrumentation; arbitrary events do not automatically repeat the operation name as a physical field.

## Erlang/BEAM and Elixir: process identity plus metadata

OTP 27 added `proc_lib:set_label/1`, which assigns the current process an arbitrary descriptive term. Tools and crash reports use it to identify unregistered processes, and it need not be globally unique ([Erlang `proc_lib`](https://www.erlang.org/doc/apps/stdlib/proc_lib.html)). BEAM tracing itself is process-centric: runtime trace points cover calls, messages, process lifecycle, scheduling, and garbage collection; events identify their process, and tracing flags can inherit on spawn ([Erlang `trace`](https://www.erlang.org/doc/apps/kernel/trace.html)). The official trace documentation does not state that a process label is copied into each trace event.

Erlang's standard Logger separately supports process metadata that it automatically inserts in every log event produced by that process ([Erlang Logger](https://www.erlang.org/doc/apps/kernel/logger.html#set_process_metadata/1)). Elixir wraps this facility and permits per-process metadata while automatically adding data such as PID, initial call, and registered name when available ([Elixir Logger metadata](https://hexdocs.pm/logger/Logger.html#module-metadata)). Neither facility automatically propagates arbitrary metadata into newly spawned processes.

**Status:** VM/runtime process labels and tracing plus standard logging metadata, but no single cross-process logical-computation abstraction. This is evidence for durable labels and automatic local enrichment, not for complete computation-context propagation.

## Weaker adjacent precedents

- Python's standard `asyncio.Task` has `get_name` and `set_name`, with generated defaults when unnamed. The name is primarily visible in task representations; the runtime does not document it as automatically attached to all tracing or logging events ([Python asyncio tasks](https://docs.python.org/3/library/asyncio-task.html#asyncio.Task.get_name)).
- Elixir Telemetry, OpenTelemetry SDKs, Rust `tracing`, and similar systems offer spans and propagated metadata, but they are libraries rather than programming-language or VM semantics. They remain valuable exporter precedents, not evidence that a runtime itself owns the computation context.

## Implications for Chimi

Chimi should preserve four distinct concepts in its canonical event model:

| Field | Meaning | Cardinality |
| --- | --- | --- |
| `computation` | Stable logical kind, preferably derived from the qualified entry function or an explicit runtime name | Bounded |
| `execution_id` / `trace_id` | Unique execution instance | Unbounded identifier |
| `node_id` / `operation` | Local semantic node or dependency operation currently executing | Bounded within a compiled program |
| `attributes` | Structured instance data such as job, order, or retry attempt | Potentially high |

The canonical runtime context should carry all four internally. Every exported event should be queryable by computation and execution identity, but the in-memory representation need not physically duplicate strings on each event. Java JFR shows that exporters can materialize active contextual fields while an efficient recording stores context changes separately.

Propagation should be a runtime invariant across Chimi-owned boundaries: function sequencing, child computations, async work, retry attempts, dependency calls, cancellation, and defects. Exporters may filter or flatten data, but they should not be responsible for inventing or recovering computation identity.

Process or task labels remain a separate, optional debugging affordance. They
may help a human distinguish concurrent runtime workers, but they neither define
the logical computation nor substitute for its propagated structured context.

The root computation name should be stable and low-cardinality. Dynamic labels belong in typed attributes; otherwise grouping and aggregation become unreliable. A nested computation may add a local span/operation name, but it should not replace the root computation identity. This gives an isolated event enough context for log queries while retaining a causal trace tree.

## Recommendation

Adopt the proposal, with one refinement: define `computation` as a stable computation **kind**, not a unique human-generated instance name. Pair it with a generated execution/trace ID and a local node or operation identity on every canonical event. Implement propagation in the runtime, then let event storage choose between repeated fields and context-delta encoding.
