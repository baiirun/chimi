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
  -- \
  application arguments
```

Supported trace modes are `off`, `boundaries`, and `full`. Cancellation,
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
`chimi run` on the reference interpreter writes its outcome record to stdout
and exits with the same status for the same outcome (0, 1, 70, or 130), so a
script sees one contract on both backends. A program that does not compile
exits 1 with diagnostics on stderr and no outcome record.
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
