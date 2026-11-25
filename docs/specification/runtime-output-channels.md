# Runtime Output Channels

## Purpose

A Chimi executable must let application output remain ordinary application
data. Runtime telemetry and the runner's terminal outcome are operational
protocols; writing either protocol to application stdout or stderr would make
program output ambiguous and would prevent one Chimi program from safely
consuming another execution's trace.

## Scope

This contract defines native-runner configuration, telemetry export, terminal
outcome export, and the first versioned runtime JSONL envelope. Application
arguments and the `Console` and `FileSystem` capabilities remain owned by
`ts-e2d9a9`. Telemetry transport beyond a local file, asynchronous export,
rotation, sampling, and durability are not part of this slice.

## Behavior Contract

- File descriptor 1 MUST remain application stdout and file descriptor 2 MUST
  remain application stderr after execution begins.
- Telemetry MUST be written only to the explicitly configured telemetry sink.
- The terminal runner outcome MUST be written only to the explicitly
  configured outcome sink.
- Telemetry and outcome sinks MUST be distinct files. The runtime MUST reject
  a configuration that names the same path for both.
- Ordinary telemetry and outcome files MUST be created exclusively so a run
  cannot overwrite an earlier export. `/dev/null` is the benchmark-only
  telemetry sink.
- Runner options MUST be recognized only inside the explicit runner boundary.
  Text outside that boundary is application data and MUST NOT be interpreted as
  tracing configuration.
- Trace-off execution MUST NOT construct or serialize runtime events. A
  configured telemetry file is therefore empty in trace-off mode.
- Boundary and full tracing MUST require a telemetry sink. The runtime MUST NOT
  silently discard explicitly requested telemetry.
- Every exported event and outcome record MUST carry the exact schema identity
  `chimi.runtime.v1`.
- Event order, identifiers, attributes, and terminal semantics MUST match the
  Rust execution oracle for the selected trace mode.
- Runner configuration and sink-open failures happen before application
  execution and MUST fail with a nonzero process status.

The default native execution policy is trace-off with no telemetry or outcome
export. This keeps the application streams clean and avoids observability work
unless the caller requests it.

## Interfaces

Runner configuration uses a reserved leading boundary and an ordinary `--`
terminator:

```text
program --chimi-runner \
  --trace=full \
  --telemetry-file=/path/to/trace.jsonl \
  --outcome-file=/path/to/outcome.jsonl \
  --memory-report-file=/path/to/memory.json \
  --profile-file=/path/to/profile.json \
  --execution-id=example \
  --max-line-bytes=1048576 \
  --log-level=info \
  --log-file=/path/to/app.log \
  --metrics-file=/path/to/metrics.json \
  -- \
  application arguments
```

Supported trace modes are `off`, `boundaries`, and `full`. `--log-level` and
`--log-file` configure the runner's `std/log` provider
([observability modules](observability.md#the-runners-provider)). Log lines
are application output: they go to application stderr, or to the log file,
which MUST be distinct from every report file above, and never to the
telemetry or outcome sinks. Unlike those sinks, the log file is appended to.
`--metrics-file` names where the runner's `std/metrics` provider writes its
aggregates when the program ends
([observability modules](observability.md#the-runners-aggregate)); it MUST be
distinct from every report file and from the log file, and is replaced on
each run. Cancellation,
benchmark configuration, and the maximum UTF-8 line size remain runner-owned
options inside the same boundary. Arguments after `--` are passed as the one
`List<Text>` input to `main`; without a runner marker, every process argument is
application data. A zero-input `main()` rejects nonempty application arguments
rather than silently discarding them.

An ordinary success exits zero, an expected failure exits 1, a defect exits 70,
and cancellation exits 130. Sink failures exit 74 and invalid runner
configuration exits 64. The terminal record remains available only through the
outcome sink; exit status does not copy it into an application stream.
A `main` that returns or fails with `std/cli`'s `Exit` ends with the status it
names instead, applied after the outcome record: `Code(n)` exits `n` (0 to
255, otherwise 70), `Help` writes its text to stdout and exits 0, and `Usage`
writes its message to stderr and exits 2
([system modules](system-modules.md#stdcli)).
A native program cancelled by SIGINT or SIGTERM writes its outcome, then dies
by that signal (a shell shows 130 or 143), and `--signal=INT@<duration>` or
`--signal=TERM@<duration>` under `--clock=virtual` injects one at a virtual
instant and exits `128 + signal`. [Application lifecycle](application-lifecycle.md)
gives the rules.
`chimi run` on the reference interpreter is a runner with the same contract
(CHI-315): the program's console writes go to stdout and stderr, and nothing
else does. It takes the native runner's sink options after its input and
before `--`: `--trace=off|boundaries|full` (default `off`),
`--telemetry-file=<path>` and `--outcome-file=<path>`, with the same rules:
tracing needs a telemetry file, the two files must differ (otherwise exit 64
before the program runs), and each file is created exclusively (an existing
one exits 74 before the program runs). It exits with the same status for the same outcome (0, 1, 70,
or 130), so a script sees one contract on both backends. The interpreter holds
the program's output until it finishes, then writes stdout and then stderr, so
the two are not interleaved as they were written. When the program ends with
a defect and no outcome file was given, `chimi run` adds one line to stderr,
`chimi: the program ended with a defect: <message>`, so a status of 70 is not
all there is to read. A program that does not
compile exits 1 with diagnostics on stderr and no outcome record.
`chimi run <path> -- <arguments>` (or `chimi run --root <dir> <entry> --
<arguments>`) passes every argument after the first `--` to `main` exactly as
a native runner does: later `--` and option-shaped words are application data,
and a zero-input `main` given arguments exits 64 with `program entry does not
accept application arguments` on stderr and no outcome record. Without `--`,
`chimi run` takes no arguments after its input. Arguments must be valid UTF-8;
otherwise `chimi run` exits 2 with a usage error.

`--memory-report-file` is an optional runner-owned diagnostic sink. It writes
one `chimi.runtime-memory.v1` record after execution and MUST be distinct from
the telemetry and outcome paths. Its scoped allocation counters are neither
application output nor semantic trace events. Omitting it avoids the report
open, serialization, and write entirely.

The record reports total scoped allocation, peak live bytes, retained bytes,
collection count, and a `scoped_allocated_bytes_by_kind` breakdown. The
breakdown accounts for raw byte buffers, values, controls, environments, value
pointer tables, fields, callables, and persistent map nodes; its values sum to
the reported scoped total. `unscoped_allocated_bytes` separately reports the
bytes allocated outside every reclaiming scope, which nothing frees before the
process exits.

`--profile-file` is an optional runner-owned diagnostic sink for the native
runner (T6). It writes one `chimi.runtime-profile.v1` object after execution and
MUST be distinct from the telemetry, outcome and memory report paths; an
existing file exits 74. Its `operations` array has one row per Core node that
ran:

```json
{"core_node_id":5,"origin_id":8,"file_id":0,"source_start":98,"source_end":107,
 "operation":"binary","evaluations":200000,"allocations":400000,"allocated_bytes":16000000}
```

`evaluations` counts every time the node ran. `allocations` and
`allocated_bytes` count what the node allocated itself, not what the nodes it
ran allocated; a task's allocations are charged to that task's nodes. Rows are
ordered by `allocated_bytes`, then `evaluations` (both descending), then
`core_node_id`, so one program always writes the same profile. The ids and
source range are the ones telemetry events carry. Profiling is independent of
the trace mode, and it turns off the direct integer paths (direct roots and
direct loops), so every node is counted. It cannot be combined with
`--benchmark-repeats`, which would add up the counts of every repeat; that
combination exits 64. It holds no timings, which would make
it nondeterministic and add overhead to every node; wall time per operation
belongs to tracing.

Telemetry contains event records only:

```json
{"schema":"chimi.runtime.v1","event":"computation_started","sequence":0}
```

### Event times

Every semantic event (`computation_started`, `operation_started`,
`operation_completed`, `dependency_called`, `process_body_started`,
`process_body_completed` and `computation_completed`)
carries `attributes.time_ns`. It is the execution clock's reading when the
event happened, in nanoseconds since the clock's origin, with the same
meaning as the `time_ns` of process and clock events. `computation_started`
is at the origin, 0, and also carries `attributes.start_unix_ns`, the Unix
time in nanoseconds at that origin, so `start_unix_ns + time_ns` is an
event's wall time:

```json
{"schema":"chimi.runtime.v1","event":"computation_started","sequence":0,"attributes":{"time_ns":0,"start_unix_ns":0}}
```

- Under the virtual clock both fields MUST be determined by the program and
  `--clock-start` alone: `start_unix_ns` is the `--clock-start` time (0
  without it), and an event's `time_ns` is the virtual time it ran at. A
  program that never waits writes 0 throughout.
- Under the real clock `start_unix_ns` is the host's wall time when the
  computation starts, and `time_ns` comes from the host's monotonic clock. A
  program that needs no scheduler reads no clock, so its events are at 0.
- Times MUST NOT decrease along one span's events: an operation completes no
  earlier than it started.
- Reading the time for an event MUST NOT be a scheduler request. It parks
  nothing and consumes no cancellation, so tracing cannot change what a
  program does. The native runtime reads it with `chimi_fiber_now()`.
- The interpreter has no execution clock, so it writes 0 for both fields.
  That is what the native runtime writes under the virtual clock for every
  program the interpreter can run.
- An event's existing attributes come first. `time_ns` follows them, then
  `start_unix_ns`, then `modules` ("Source modules").

### Process bodies

Each process incarnation's body is a span of its own (CHI-312), so a reader
can tie the body's operations to the incarnation's `process_started` and
`process_terminated`. Under `--trace=boundaries` and `--trace=full` the
native runtime writes `process_body_started` when the body begins and
`process_body_completed` when it ends:

```json
{"schema":"chimi.runtime.v1","event":"process_body_started","sequence":214,"computation":"main","execution_id":"e","span_id":96,"parent_span_id":null,"incarnation_id":1,"operation":"process_body","attributes":{"time_ns":0}}
{"schema":"chimi.runtime.v1","event":"process_body_completed","sequence":366,"computation":"main","execution_id":"e","span_id":96,"parent_span_id":null,"incarnation_id":1,"operation":"process_body","attributes":{"outcome":"defect","time_ns":1000000}}
```

- `span_id` is the parent span of the body's top operations, and
  `parent_span_id` is null: a body is not inside any operation of the
  computation.
- `incarnation_id` is the incarnation's id, as on its process events. The
  runtime reads it with `chimi_fiber_current_incarnation()`, which is not a
  scheduler request, so tracing cannot change what the body does.
- `outcome` is the body's result, as on `operation_completed`. A body the
  scheduler abandons at a park (a forced stop) writes no completion.
- A body whose start handoff argument is missing, or that is cancelled
  before it begins, writes neither event.

### Source modules

A telemetry event names its place by `file_id` and byte offsets. So a reader
can find that place again in a directory or workspace that loads a different
set of modules, `computation_started` also carries `attributes.modules`: one
entry per file id, in file id order, with the module path and its
fingerprint.

```json
{"schema":"chimi.runtime.v1","event":"computation_started","sequence":0,"attributes":{"time_ns":0,"start_unix_ns":0,"modules":[{"path":"main","fingerprint":"bea35bbdd91a2dd4"},{"path":"std/int","fingerprint":"7d45be76ecef7aba"}]}}
```

- Every module the build loaded MUST be listed, the standard library's
  included, and both backends MUST write the same list for one program. A
  file id the build had no module for is `{"path":null,"fingerprint":null}`.
- The fingerprint is FNV-1a 64 over the module's text, as 16 hex digits. A
  program build leaves test items out, so for it each test item counts as
  spaces of its length and the text after the module's last other item is
  left out: appending tests changes neither the executable nor the
  fingerprint. In a test build, a module with test items is fingerprinted over
  its whole text, since its places may be in them. It tells a
  viewer whether it was given the run's source; it is not a security check.
- A reader matches a recorded module by its path, or, when the source it
  was given has no module of that path, by being the only module there with
  that fingerprint: a file built alone is the module `main`, and the same
  file in a directory is named after the file.
- The field is additive: `chimi.runtime.v1` keeps its name, and a reader
  MUST accept a `computation_started` without it, as traces written before
  it have none.
- The native runtime writes the list from the program data the build
  embeds (`CHIMI_TRACE_MODULES`), so writing it costs one string per run.

The outcome sink contains exactly one terminal record for an ordinary
execution:

```json
{"schema":"chimi.runtime.v1","outcome":"success","value":42}
```

An outcome's `value` (or `error`) encodes a Chimi value as JSON. Int, Float,
Bool, and Text are JSON scalars, and a List or a Set (in ascending order) is an
array. A record is an object of its fields, and a struct or enum value is
`{"type":...,"variant":...,"fields":{...}}`. A tuple is `{"tuple":[...]}`, a
range `{"range":{"start":0,"end":3}}`, a Duration `{"duration":<nanoseconds>}`,
a Decimal `{"decimal":"<normalized text>"}`, a function `{"callable":"<name>"}`,
and `Void` is `{"type":"Void"}`. A map whose keys are Text is an object of its
entries, and an empty map is `{}`. Any other map is its entries in key order as
`[key, value]` pairs, so each key is written once however deep maps nest
through their keys:

```json
{"map":[[2,{}],[10,{"a":1,"b":2}]]}
```

A `Cause` value is opaque in an outcome, `{"type":"Cause","fields":{}}`. A
failure, defect, or cancellation record also has a `cause` field, the
`Cause.to_text` rendering of its cause, when that cause has a context frame,
a `caused_by` link, or a suppressed cause (time-and-outcomes.md, "Failure
context"); otherwise the field is absent.

The remaining event and outcome fields retain the executable-kernel contract.
Adding a field is backward compatible within version 1 only when consumers can
ignore it without changing record meaning. Removing, renaming, or changing the
meaning or type of a field requires a new schema identity.

## Implementation Model

The runner parses operational configuration before invoking the application.
It owns the telemetry and outcome files and passes the telemetry sink through
the native runtime context. Semantic operations emit directly to that sink;
terminal control is serialized separately after execution. Application code
never receives either runner-owned file.

The Rust oracle owns the canonical record serialization and the native runtime
must remain byte-shape compatible with it. The compiler does not add telemetry
to Chimi function types: tracing is caller policy, while application I/O stays
visible through dependencies and the `io` effect.

## Verification

Run:

```sh
cargo test -p chimi
cargo clippy -p chimi --all-targets --all-features -- -D warnings
python3 prototypes/kernel-validation/main.py --check
```

Native integration coverage must prove that stdout and stderr remain empty for
a program without application I/O, telemetry and outcome records reach only
their configured files, trace-off telemetry is empty, full and boundary event
shapes retain oracle semantics, and runner-looking application text is not
parsed outside the explicit boundary.
