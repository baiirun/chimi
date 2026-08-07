# First Useful Chimi CLI

`prog show ep-0738db` tracks the first post-kernel milestone: implement a
maintained command-line program primarily in Chimi and ship it as one native
executable. The forcing program is `chimi-trace-summary`, a deterministic
summarizer for Chimi runtime JSONL.

## Purpose

The executable kernel proves that accepted syntax and semantics connect through
the compiler. It does not yet prove that Chimi can own an ordinary external
program boundary. `chimi-trace-summary` forces that boundary with a program
useful to Chimi development itself: it reads the runtime's structured output,
validates one execution, aggregates its event shape, and emits a stable report.

The program exists to expose missing general-purpose capabilities. Its domain
MUST NOT turn trace processing into privileged language syntax or make CLI
programs the permanent center of Chimi's design.

## Scope

The accepted program:

- accepts one input path, with `-` selecting standard input;
- consumes the JSONL produced by one ordinary Chimi execution;
- recognizes computation, operation, dependency-call, and terminal-outcome
  records;
- counts events, operations, dependency methods, and terminal outcomes;
- emits one deterministic JSON summary to application standard output;
- reports usage, file, UTF-8, JSON, and trace-shape failures as expected values;
- processes records incrementally rather than retaining the complete input;
- can run with Chimi tracing disabled or export its own telemetry to a sink
  separate from the report.

This milestone does not add duration analysis because current runtime events do
not carry timestamps. It also does not add async I/O, concurrency, durability,
queues, networking, package publishing, generalized optimization, or a public
compatibility promise for every standard-library API introduced along the way.

## Behavior Contract

The executable MUST accept exactly one application argument. No argument or
more than one argument is a usage failure. Runtime configuration MUST NOT be
mistaken for an application argument.

Each nonempty input line MUST be valid UTF-8 JSON and MUST decode as one of:

- a runtime event with `event`, `sequence`, and the currently applicable
  execution context;
- a terminal success, expected-failure, defect, or cancellation outcome.

Blank lines and unknown record shapes MUST be rejected with the one-based input
line number. The program MUST NOT silently skip data that could make a summary
look complete. External JSON `null` is represented inside the JSON model as an
explicit `Value.Null` variant; it MUST NOT introduce nullable Chimi values.

All runtime event records that carry execution identity MUST agree on one
`execution_id` and one computation name. Event sequence numbers MUST be
strictly increasing. Exactly one terminal outcome is required, and it MUST be
the final nonempty record. A trace consisting only of the terminal outcome is
valid because trace-off execution intentionally emits no events.

Successful output MUST be one JSON object followed by one newline. Object keys
and keyed count entries MUST have deterministic ordering. Optional source facts
MUST be omitted or represented through the JSON codec's explicit policy, not a
nullable Chimi domain value. The first report schema is:

```json
{
  "schema": "chimi.trace-summary.v1",
  "records": 7,
  "events": 6,
  "operations": {
    "call": 2,
    "constant": 2
  },
  "dependency_calls": {
    "payment.charge": 1
  },
  "terminal_outcomes": {
    "success": 1
  }
}
```

Execution and computation fields MAY be included when the input carries them,
but their absence in a trace-off outcome MUST remain distinguishable from an
empty string.

On success the process MUST exit zero. A usage or expected processing failure
MUST exit nonzero, emit a concise diagnostic to application standard error,
and emit no partial summary to application standard output. Defects and
cancellation retain their runtime meanings and MUST NOT be converted into
ordinary parse or I/O failures.

Peak live memory after fixed startup state MUST be bounded by the largest input
record, the number of distinct aggregation keys, and explicit buffering policy;
it MUST NOT grow with the total number of processed records. Implementations
MUST place a documented limit on record size or return a structured failure.

Application output, the terminal runner outcome, and runtime telemetry are
distinct channels. A tracing mode MUST NOT alter the summary or its exit status.
Trace-off execution MUST avoid constructing runtime events.

## Interfaces

The stable user surface is conceptually:

```text
chimi-trace-summary <path-or-dash>
```

The runner owns operational configuration such as trace mode, export sink,
execution identity, cancellation, and resource limits. The exact runner-option
spelling remains part of `prog show ts-eaae0e`; it MUST use a boundary that
cannot consume application arguments accidentally.

The input is the versioned runtime JSONL envelope defined by `prog show
ts-eaae0e`. The current unversioned event and outcome objects are evidence for
the fields the first version must preserve, not a permanent wire contract.

The program-visible boundary needs these conceptual capabilities without
prejudging their final module names:

- application arguments;
- a UTF-8 line source opened from a path or standard input;
- application standard output and standard error;
- JSON decode and encode;
- functional incremental processing and deterministic keyed aggregation.

Filesystem and console implementations are program dependencies. Whether their
operations add synchronous effect information to callable types is specified by
the accepted [synchronous effects and host capabilities](../specification/synchronous-effects-and-host-capabilities.md)
contract (`prog show ts-8cec2f`). Trace policy and execution identity remain
runner configuration rather than entries in a function's `needs` or `effects`
rows.

## Capability Inventory

| Layer | Current evidence | Missing contract or implementation | Ticket |
| --- | --- | --- | --- |
| Language | Functions are values; failures and `needs` compose independently; `using` provisions dependencies at expression boundaries. | Represent synchronous I/O and resource behavior without relabeling dependencies as effects or hiding operations. | `ts-8cec2f` |
| Compiler | Callable contracts carry success, failure, needs, and effect rows through typing, inspection, and executable metadata. | Implement no additional compiler channel for the first CLI. | `ts-5b0547` |
| Language and standard library | `Text`, `Int`, `Bool`, `List`, `Option`, structs, enums, records, case expressions, pipelines, and the accepted [trace data and processing](../specification/trace-data-and-processing.md) contract exist. | Implement the accepted JSON, immutable map, and bounded line-processing behavior. | `ts-2ac2de` |
| Runtime contract | Off, boundary, and full tracing preserve terminal outcomes and canonical event order. | Version the export and separate telemetry, runner outcomes, and application stdout/stderr. | `ts-eaae0e` |
| Program boundary | `main` is currently a closed zero-input root; native runtime flags consume every process argument. | Supply application arguments and synchronous host I/O while keeping runtime configuration external. | `ts-e2d9a9` |
| Standard library | JSON values, `Map<K, V>`, deterministic encoding, and `process_utf8_lines` are specified but not implemented. | Implement the smallest module-qualified trace-processing slice. | `ts-41796d` |
| Native memory | The current runtime allocates boxed values and environments for process lifetime. | Reclaim line-local state so peak live memory is independent of total input size. | `ts-da83af` |
| Application | `chimi build` produces one executable and reports linked system dependencies. | Write the maintained Chimi program and validate real trace inputs, failures, performance, and artifact shape. | `ts-65a567` |

Direct lowering beyond pure integer expressions is not a prerequisite. The
final performance evidence MAY graduate a measured hotspot into a separate
compiler ticket, but this milestone MUST NOT assume that JSON, maps, calls, or
matching need language-specific native shortcuts before profiling them.

## Implementation Model

The Chimi program owns trace decoding policy, trace-shape validation,
aggregation, and report construction. These remain ordinary functions whose
data, success, failures, dependencies, and effects are visible in their
callable contracts.

The official standard library owns general JSON, text, iteration, and keyed
collection behavior. It MUST expose module-qualified functions and MUST NOT add
trace-specific primitives. Usage from this program earns only the smallest API
surface required by at least one maintained call site.

The native runtime owns OS arguments, file descriptors, standard streams,
resource limits, terminal outcome mapping, and telemetry export. It MUST not
parse trace records or perform aggregation. The compiler owns type composition,
effect and need enforcement, executable sealing, and lowering of the resulting
ordinary program.

This placement keeps domain behavior in Chimi while trapping platform and
memory-management complexity behind a narrow runtime boundary. It also keeps
the source model viable for a later self-hosted compiler rather than exposing
Rust or C ownership types.

## Verification

The milestone is complete only when a clean checkout can:

1. build a representative Chimi program with boundary and full tracing;
2. capture each trace through the selected telemetry sink;
3. build `chimi-trace-summary` through the same compiler path;
4. summarize trace-off, boundary, full, expected-failure, defect, cancellation,
   and dependency-call fixtures with exact golden output;
5. reject invalid UTF-8, malformed JSON, mixed execution identities,
   non-monotonic sequences, duplicate outcomes, records after an outcome, and
   an oversized record without partial output;
6. demonstrate that changing the summarizer's own trace mode does not change
   its application output or exit status;
7. process a generated large input while measuring bounded peak live memory,
   throughput, startup, allocations, executable size, and dynamic system
   dependencies;
8. preserve the compiler's existing syntax, type, native-oracle, and strict
   lint gates.

Unit tests should cover pure decoding and aggregation. Integration tests should
use the real native executable and coarse filesystem/process boundaries rather
than mocks of compiler or runtime internals.

## Work Map

- `ts-1c0afb`: define this contract and capability inventory.
- `ts-8cec2f`: define synchronous effects and host capability ownership.
- `ts-5b0547`: implement inferred effect rows through compiler handoffs.
- `ts-2ac2de`: define the trace-processing data and iteration surface.
- `ts-eaae0e`: separate application output from telemetry and runner outcomes.
- `ts-e2d9a9`: implement CLI arguments and synchronous host I/O.
- `ts-41796d`: implement the JSONL aggregation standard-library slice.
- `ts-da83af`: bound native allocation for streaming workloads.
- `ts-65a567`: build and validate `chimi-trace-summary`.

The first three follow-up decisions may proceed independently after this
inventory. Implementation and final validation remain dependency-gated in
`prog`.
