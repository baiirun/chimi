# Chimi Observable Runner Prototype

> PROTOTYPE: this is a disposable interpreter for executable semantics, not
> Chimi's production runtime, compiler backend, scheduler, or durability model.

## Question

Can a small typed executable core define deterministic evaluation order,
expected-failure short-circuiting and handling, dependency calls, retry,
defects, cancellation, and a structured observable event stream without
committing Chimi to a permanent backend or runtime architecture?

The pure `runtime` package interprets in-memory executable nodes against
scripted providers. The terminal shell displays source-shaped scenarios,
outcomes, parent/child spans, and complete structured events. No database,
network, real clock, or persistence is involved.

## Run

From the repository root:

```sh
go -C prototypes/runner run .
```

Check every scenario:

```sh
go -C prototypes/runner run . --check
```

Export one trace as JSONL:

```sh
go -C prototypes/runner run . --json "retry succeeds"
```

## Working semantics

- Checked source lowers to a small executable expression tree. This is an
  implementation representation, not a user-visible `computation` block.
- Blocks and calls evaluate left to right. The final block expression supplies
  the success value.
- Expected failures are ordinary enum values in a separate outcome channel.
  An unhandled failure short-circuits the remaining sequence.
- A matching failure handler may recover to success or produce another
  expected failure. Unmatched variants remain failures.
- Retry reruns its input only for expected-failure variants selected by its
  policy. The final failure value is preserved when retry is skipped or
  exhausted. Defects and cancellation never retry.
- Defects and cancellation are observable runtime outcomes outside callable
  expected-failure contracts.
- The runner defensively checks provider configuration before starting, even
  though the type checker should already reject open runtime roots.
- Every executed semantic node emits structured events in `all` tracing mode.
  Events carry deterministic ordering, trace/span parentage, source spans,
  root computation name, node identity, and structured attributes; the same
  events can be rendered locally or exported as JSONL. Root computation name is
  repeated as canonical context on every event so individual events remain
  independently queryable without reconstructing their parent chain.
- `computation` is a stable, bounded-cardinality kind such as
  `Checkout.submit`; `trace_id` identifies one execution instance. Dynamic
  identifiers such as order IDs belong in structured attributes rather than
  the computation name.
- Process or task labels are optional debugging identity, not the source of
  computation context. The runtime propagates structured computation context
  across child work, retries, and dependency calls; exporters decide how to
  encode that context in logs or traces.
- `using` and root configuration choose providers before interpretation. Each
  dependency event records whether its provider came from an intermediate
  `using` boundary or the runtime configuration.

## Deliberately deferred

- bytecode, native compilation, JITs, or a permanent interpreter;
- production scheduling, actors, supervision, durability, or recovery;
- real asynchronous IO, clock behavior, backpressure, and resource scopes;
- hot code upgrades or active-state migration;
- OpenTelemetry wire formats and exporter lifecycle.

## Runtime precedents

[Runtime computation context precedents](../../docs/research/runtime-computation-event-context.md)
compares Go runtime trace tasks, Java Flight Recorder contextual fields, .NET
activities, and BEAM process labels/tracing. These support runtime-owned
propagation while showing that efficient trace storage may encode context
changes rather than physically duplicating strings on every record.
