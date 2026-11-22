# Observability Modules

## Status

This is the normative contract for the Phase 7c observability modules
(CHI-289): `std/log` and `std/metrics`, and `std/otel`'s OpenTelemetry
encoding and metrics push (CHI-296). Offline export is
`chimi trace export --otlp` and `chimi metrics export --otlp` (tooling.md);
`Otel.export` pushes metrics, and logs written through `Otel.log`, to a
collector while the program runs. The words MUST, MUST NOT,
SHOULD, SHOULD NOT, and MAY are normative. The modules follow the
[standard library conventions](standard-library-conventions.md) and the
[runner-provided capability rules](system-modules.md#runner-provided-capabilities).

## Goals

- Logging and metrics are needs, as `console` and `clock` are (decisions.md,
  2026-11-22): a function that logs or records says so in its signature, and
  a test sees what it logs or records by supplying a fake with `using`.
- A log line is structured: a machine reads its level, message and fields
  without parsing prose.
- Log output is the application's, not the runtime's: it never goes to the
  telemetry or outcome sinks
  ([runtime output channels](runtime-output-channels.md)).

## std/log

```text
pub enum Level { Debug, Info, Warn, Error }
pub struct Field
pub struct Entry
pub interface Log { write: fn(Entry) -> Void }

pub fn text(key: Text, value: Text) -> Field
pub fn int(key: Text, value: Int) -> Field
pub fn float(key: Text, value: Float) -> Field
pub fn bool(key: Text, value: Bool) -> Field
pub fn json(key: Text, value: Json.Value) -> Field

pub fn level(entry: Entry) -> Level
pub fn message(entry: Entry) -> Text
pub fn fields(entry: Entry) -> List<(Text, Json.Value)>
pub fn fields_object(entry: Entry) -> Json.Value
pub fn at_least(level: Level, minimum: Level) -> Bool

pub fn debug(message: Text, fields: List<Field>) -> Void needs { log: Log }
pub fn info(message: Text, fields: List<Field>) -> Void needs { log: Log }
pub fn warn(message: Text, fields: List<Field>) -> Void needs { log: Log }
pub fn error(message: Text, fields: List<Field>) -> Void needs { log: Log }
```

<!-- chimi: check -->
```chimi
import std/log as Log

fn charge(order: Int, cents: Int) -> Void needs { log: Log.Log } {
  Log.info("charged", [Log.int("order", order), Log.int("cents", cents)])
}
```

- Each level function MUST build one `Entry` with its level, the message, and
  the fields in the order written, and pass it to the `log` provider's
  `write` once. It adds nothing else: the time and any filtering are the
  provider's.
- `Log.float` MUST write the number as `Float.to_text` spells it, which is
  always a JSON number, since a `Float` is never NaN or infinite.
- `at_least` MUST order the levels `Debug < Info < Warn < Error`, as the
  runner's provider and `--log-level` filter.
- `fields` MUST return every field in the order written, a repeated key
  included. `fields_object` MUST return them as one JSON object in which a
  key written twice keeps its last value.
- `Log` is `log`'s fixed need name, and the runner closes it on `main` with
  the private provider `std/log.host`. A test that reaches `log` without
  supplying it with `using` is `T4400`.
- Writing an entry MUST NOT fail or defect for any reason the program could
  act on: the provider's `write` returns `Void` with no failures, and a line
  that cannot be written is lost. As with standard error, a pipe whose reader
  has gone raises `SIGPIPE`, which ends the program unless its parent
  ignores the signal.

### The runner's provider

- The provider MUST write one line per entry it keeps:

  ```text
  {"time":"2026-11-22T06:00:00.250Z","level":"warn","message":"slow charge","fields":{"order":42,"seconds":1.5}}
  ```

  - The members are `time`, `level`, `message` and `fields`, in that order.
  - `time` is the wall-clock time the line was written, in RFC 3339 UTC with
    exactly three fraction digits.
  - `level` is `debug`, `info`, `warn` or `error`.
  - `fields` is `fields_object` of the entry, encoded by `Json.encode`. It is
    `{}` when there are none, so every line has the same members.
  - The line is valid JSON on one line. A newline inside a message or field
    is escaped.
- It MUST write to standard error by default, through the same queue as
  `Console.write_error`, so a log line and a program's own error line keep
  the order they were written in and never interleave.
- The runner option `--log-file=<path>` MUST send the lines to that file
  instead, appended to it and created if missing.
  - The file is written the way standard error is: a pipe or terminal
    through its own queue, so a slow reader never blocks other tasks and a
    pending write can be cancelled, and a regular file directly. A log file
    that is the same open file as standard output or error shares that
    stream's queue, so lines never tear.
  - The file MUST differ from the runner's report files (telemetry, outcome,
    memory report, profile), compared as files rather than spellings of the
    path; a clash is invalid configuration, exit 64.
  - A file that cannot be opened is a sink failure, exit 74, before `main`
    runs.
- The runner option `--log-level=debug|info|warn|error` (default `info`)
  MUST leave out every entry less severe than the level. Any other value is
  invalid configuration, exit 64.
  - The provider reads the level once, when `main` starts.
  - A filtered entry is never encoded.
- The interpreter's provider MUST write the same lines into the run's
  application stderr, or to `ExecutionConfig::with_log_file`, filtered by
  `ExecutionConfig::with_log_level`.

### Not in this slice

- A log entry carries no trace or span id, and tracing writes no log event to
  the telemetry file. Correlating the two needs an amendment to the
  telemetry schema in [runtime output channels](runtime-output-channels.md)
  first.
- There is no OpenTelemetry log export.
- There is no per-module level, no sampling, and no rotation of the log file.

## std/metrics

```text
pub enum Kind { Count, Gauge, Observe }
pub struct Measurement
pub interface Metrics { record: fn(Measurement) -> Void }

pub fn kind(measurement: Measurement) -> Kind
pub fn name(measurement: Measurement) -> Text
pub fn value(measurement: Measurement) -> Float
pub fn labels(measurement: Measurement) -> List<(Text, Text)>

pub fn count(name: Text, by: Int, labels: List<(Text, Text)>) -> Void needs { metrics: Metrics }
pub fn gauge(name: Text, value: Float, labels: List<(Text, Text)>) -> Void needs { metrics: Metrics }
pub fn observe(name: Text, value: Float, labels: List<(Text, Text)>) -> Void needs { metrics: Metrics }

pub fn seconds(duration: Duration) -> Float

pub fn runner_document() -> Text needs { metrics: Metrics }
```

<!-- chimi: check -->
```chimi
import std/float as Float
import std/metrics as Metrics.{Metrics}

fn charge(cents: Int) -> Void needs { metrics: Metrics } {
  Metrics.count("charges", 1, [("currency", "usd")])
  Metrics.observe("charge_cents", Float.from_int(cents), [])
}
```

- Each recording function MUST build one `Measurement` with its kind, the
  name, the value and the labels in the order written, and pass it to the
  `metrics` provider's `record` once. `count`'s value is `by` as a `Float`.
- A counter only grows: `count` with a negative `by` MUST defect, with the
  message `Metrics: count must not be negative, got <by>`. A value that goes
  down is a gauge.
- `seconds` MUST return the duration's nanoseconds divided by 10^9. A
  duration is observed in seconds.
- `Metrics` is `metrics`'s fixed need name, and the runner closes it on
  `main` with the private provider `std/metrics.host`. A test that reaches
  `metrics` without supplying it with `using` is `T4400`.
- Recording MUST NOT fail for any reason the program could act on: the
  provider's `record` returns `Void` with no failures.

### The runner's aggregate

- The provider MUST record nothing unless the runner option
  `--metrics-file=<path>` is given or the program has called
  `runner_document`; it asks the runtime on each measurement, and until one
  of them holds a label set is never encoded.
- It MUST add each measurement into one aggregate per name and label set.
  A label set is the labels as one JSON object, so the order they were
  written in does not matter and a label written twice keeps its last value.
  - A count adds to a running total, kept as a whole number that saturates
    at the largest `Int`. `count` passes `by` as a `Float`, so a `by` above
    2^53 is rounded to a nearby whole number first.
  - A gauge keeps the last value set.
  - An observation adds to a count, a sum, and a histogram whose buckets
    have the upper bounds `0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25,
    0.5, 0.75, 1, 2.5, 5, 7.5, 10` (OpenTelemetry's defaults) and one more
    bucket for larger values. A value equal to a bound falls in that bound's
    bucket. The sum saturates at the largest `Float`, so it stays finite.
- A name keeps the kind it was first recorded with. A measurement of
  another kind under that name MUST be dropped and counted in
  `dropped.kind_conflicts`.
- A name MUST keep at most 1000 label sets. A measurement that would make
  one more MUST be dropped and counted in `dropped.label_sets`, so a label
  holding an unbounded value such as a user id cannot use up memory.
- Tasks on every carrier MUST record into the one aggregate, and no
  measurement is lost.
- When `main` ends, whether it returns, fails, defects or is stopped, the
  runner MUST write one JSON document to the file, replacing what it held.
  A run that ends before `main` starts, or on a fatal runtime error, leaves
  the file empty. With `--benchmark-repeats`, every repeat records into the
  one aggregate.

  The document looks like this:

  ```text
  {"schema":"chimi.metrics.v1","metrics":[
    {"name":"charge_seconds","labels":{},"kind":"observe","count":2,"sum":0.75,"bounds":[0.0,0.005,...,10.0],"bucket_counts":[0,...,1,1,...,0]},
    {"name":"orders","labels":{"route":"/pay"},"kind":"count","value":3},
    {"name":"queue_depth","labels":{},"kind":"gauge","value":12.0}
  ],"dropped":{"label_sets":0,"kind_conflicts":0},
  "start_unix_ns":1794303000000000000,"time_unix_ns":1794303002000000000}
  ```

  - The document is on one line, ending in a newline; it is wrapped above
    for reading.
  - The series are in order of name, then label set, comparing the
    JSON-encoded text byte by byte.
  - A count's `value` is an integer. A gauge's `value`, a `sum` and the
    `bounds` are written as `Float.to_text` spells them.
  - `bucket_counts` has one more entry than `bounds`.
  - `start_unix_ns` is the Unix time in nanoseconds when the run started,
    the same value as the trace's `start_unix_ns`
    (runtime-output-channels.md, "Event times"). `time_unix_ns` is when
    `main` ended: the start plus the execution clock's reading then. Under
    the virtual clock both come from `--clock-start` and the time slept. A
    program that needs no scheduler reads no clock, so its end equals its
    start. The interpreter writes 0 for both.
- The file MUST differ from the runner's report files and from the log
  file, compared as files rather than spellings of the path; a clash is
  invalid configuration, exit 64. A file that cannot be opened is a sink
  failure, exit 74, before `main` runs. A failed write at exit is exit 74.
- The interpreter's provider MUST aggregate the same way and write the same
  document to `ExecutionConfig::with_metrics_file`.

### Reading the aggregate while main runs

- `runner_document` MUST return the runner's aggregate as it stands, as the
  document above, with `time_unix_ns` the start plus the execution clock's
  reading at the call. Every measurement recorded before it returns, on any
  carrier, is in it.
- From its first call on, the provider MUST record without
  `--metrics-file`, on every carrier. What was recorded before that call
  without the file is not in the aggregate, so an exporter reads it first,
  at the top of `main`.
- With `--metrics-file`, the file at exit is what a read at the end of
  `main` would return, apart from `time_unix_ns`, which is when it is
  written.
- With `--benchmark-repeats`, the repeats share the aggregate, as they do
  for the file, and the first read: a repeat after it records from its
  start, and its reads include the earlier repeats.
- It reads the runner's aggregate whatever provider closes `metrics`: under
  a fake it returns what the runner's provider recorded, which is nothing
  in a test. The need says that the function reads metrics.
- The interpreter's provider MUST return the same document, with both times
  0.

### Not in this slice

- The file is written once, at exit. `chimi metrics export --otlp` turns it
  into OpenTelemetry metrics (tooling.md). A program reads the aggregate
  while it runs with `runner_document`; pushing it to a collector is
  `std/otel` (CHI-296).
- There are no custom histogram bounds and no up-down counter.

## std/otel

```text
pub type Settings = { endpoint: Text, service_name: Text, interval: Duration, timeout: Duration, flush_within: Duration, headers: List<(Text, Text)>, client: Http.ClientSettings, log_level: Log.Level, log_buffer: Int }
pub const defaults: Settings
pub struct Exporter

pub enum DocumentError { Invalid(message: Text) }

pub fn export<T, E, R>(settings: Settings, body: fn(Exporter) -> T errors E needs R) -> T needs R + { network: Network, clock: Clock, metrics: Metrics }
pub fn log(exporter: Exporter) -> { write: fn(Log.Entry) -> Void } needs { log: Log, clock: Clock, metrics: Metrics }
pub fn metrics_request(document: Text, service_name: Text) -> Text
pub fn logs_request(entries: List<(Log.Entry, Int)>, service_name: Text) -> Text
```

`programs/otel-worker` is a worked example, with a guide to setting the
exporter up in its README.

### Pushing metrics

```text
pub fn main() -> Void needs { network: Network, clock: Clock, metrics: Metrics } {
  let settings = { ..Otel.defaults, endpoint: "http://collector:4318", service_name: "checkout" }
  Otel.export(settings, fn(_) { serve() })
}
```

- `export` MUST read the runner's aggregate (`Metrics.runner_document`)
  before anything else, so recording is on from then on, then run `body`
  with an `Exporter` and return what it returns, or fail with what it fails
  with.
- While `body` runs, it MUST push the aggregate every `settings.interval`,
  and once more when `body` ends, however it ends: it returns, fails,
  defects, or is stopped, as on `SIGTERM`. The last push is a release
  (scope-owned-resources.md), so a stop waits for it.
- A push is `POST <endpoint>/v1/metrics` with `content-type:
  application/json`, then `settings.headers`, and the body
  `metrics_request(Metrics.runner_document(), settings.service_name)`,
  sent with `Http.send` and `settings.client` (network-modules.md). Each
  push holds the totals since the run's start (cumulative temporality), so
  a lost push loses no count.
- A push succeeds on a 2xx status. Any other status, a connection or TLS
  failure, or a push past its limit MUST be dropped, never raised into
  `body`, and counted as the count `chimi.otel.failed_exports`, which the
  next push carries. A periodic push is given `settings.timeout`; the last,
  `settings.flush_within`, so a collector that is down or slow delays the
  program's end by at most that.
- The defaults are `http://localhost:4318`, `unknown_service`, a 10 s
  interval, 5 s for a push and 5 s for the last one, no headers,
  `Http.client_defaults`, logs at `Info` and above, and 1024 waiting logs.
  A `log_buffer` below 1 is a defect, as the other settings are.

### Pushing logs

```text
Otel.export(settings, fn(exporter) {
  serve() using { log: Otel.log(exporter) }
})
```

- `Otel.log(exporter)` MUST be a `log` provider that writes each entry to
  the `log` provider it is built under, unchanged, and then, when
  `Log.at_least(level, settings.log_level)`, sends it with the wall-clock
  Unix time of the write to the exporter's logs process.
- The send never waits. At most `log_buffer` entries wait to be pushed,
  in the logs process's mailbox and again in what it holds between pushes;
  an entry over either bound MUST be dropped and counted as
  `chimi.otel.dropped_logs`. A request to flush does not go through the
  mailbox, so a full one cannot lose it.
- The logs process MUST push the entries waiting at each interval's end to
  `<endpoint>/v1/logs`, as `logs_request` writes them, under the same rules
  as a metrics push (headers, `timeout`, failures counted as
  `chimi.otel.failed_exports`). An interval with no entries pushes nothing.
- When the body ends, however it ends, the entries waiting MUST be pushed,
  those still in the mailbox included, while the last metrics push runs:
  the two share one `flush_within`. Then the logs process stops. An entry
  is pushed once.
- `logs_request` writes one `logRecord` per entry, in order, with
  `timeUnixNano` and `observedTimeUnixNano` the entry's time, the severity
  number and text (`Debug` 5 `DEBUG`, `Info` 9 `INFO`, `Warn` 13 `WARN`,
  `Error` 17 `ERROR`), the message as a string body, and each field as an
  attribute: text a `stringValue`, a number `Json.number_to_int` accepts an
  `intValue` in decimal digits (`1e3` is `"1000"`), another number a
  `doubleValue` as spelled, a boolean a `boolValue`, an
  array an `arrayValue`, an object a `kvlistValue`, and `null` an empty
  value.
- It needs the native runtime, as `Http.send` does: the interpreter refuses
  the program (`E5204`).

### Encoding

- `metrics_request` MUST turn a `chimi.metrics.v1` document ("The runner's
  aggregate") into the body of an OTLP/JSON `ExportMetricsServiceRequest`:
  the same metrics, data points and resource as `chimi metrics export --otlp`
  with `--service-name` (tooling.md), compared as JSON values. Its scope is
  `{"name":"chimi"}` without a version, because Chimi code cannot read the
  toolchain's version.
- Object members are written in ascending byte order, as `Json.encode`
  writes every object; OTLP/JSON readers do not depend on member order.
  Attributes are in ascending key order, as the CLI writes them.
- A document it cannot read MUST fail with `DocumentError.Invalid` and the
  reason the CLI gives: not JSON, not an object, the wrong schema, no
  `metrics` list, a start or end time missing, before 1970 or not a whole
  number below 2^64, or a series that is not an object or not a count,
  gauge or observation (named by its index).
- It is pure: no need, no host operation, the same text on both backends.

### Not in this slice

- Retries within an interval, compression, protobuf, and log records
  tied to traces. Traces stay offline: a trace has one span per
  operation, too fine to push live, and an explicit span API is a later
  ticket.

## Conformance

| Behavior | Test |
| --- | --- |
| One JSON line per kept entry on stderr; member order; escaping; last value of a repeated key; order against `write_error` lines | `tests/std_log.rs` `the_runner_writes_one_json_line_per_entry_to_stderr` |
| `--log-level` filters, and rejects a bad level with 64 | `tests/std_log.rs` `the_log_level_option_filters_entries` |
| `--log-file` appends there, must differ from the report files even when spelled differently (64), and fails to open with 74 | `tests/std_log.rs` `the_log_file_option_appends_lines_there` |
| A log file whose reader falls behind does not block the program or a timeout | `tests/std_log.rs` `a_slow_log_file_reader_does_not_block_the_program` |
| Level functions build the entry and reach a fake through `using`, on both backends | `tests/std_log.rs` `a_test_records_entries_through_a_fake` |
| A test without a `log` provider is `T4400` | `tests/std_log.rs` `a_test_without_a_log_provider_is_rejected` |
| The interpreter's provider writes the same lines and takes the same settings | `tests/std_log.rs` `the_interpreter_writes_the_same_lines` |
| One aggregate per name and label set; label order and repeats; count, gauge and histogram; series order; escaping; kind conflicts; the file is replaced | `tests/std_metrics.rs` `the_runner_writes_one_aggregate_per_name_and_label_set` |
| Nothing recorded without `--metrics-file`; clashes with report and log files (64), even spelled differently; open failure (74) | `tests/std_metrics.rs` `the_metrics_file_option_is_checked_before_main_runs` |
| At most 1000 label sets per name, the rest counted as dropped | `tests/std_metrics.rs` `one_name_keeps_at_most_a_thousand_label_sets` |
| Tasks on four carriers lose no measurement | `tests/std_metrics.rs` `tasks_on_several_carriers_record_into_one_aggregate` |
| A negative count is a defect | `tests/std_metrics.rs` `a_negative_count_is_a_defect` |
| Recording functions build the measurement and reach a fake through `using`, on both backends; `seconds` | `tests/std_metrics.rs` `a_test_records_measurements_through_a_fake` |
| A test without a `metrics` provider is `T4400` | `tests/std_metrics.rs` `a_test_without_a_metrics_provider_is_rejected` |
| The interpreter's provider writes the same document | `tests/std_metrics.rs` `the_interpreter_writes_the_same_document` |
| `runner_document` reads the aggregate mid-run with the read's time, turns recording on without the file, and matches the exit file, on both backends | `tests/std_metrics.rs` `the_aggregate_can_be_read_while_main_runs` |
| After a read, tasks on four carriers record without the file | `tests/std_metrics.rs` `a_read_turns_recording_on_for_every_carrier` |
| `metrics_request` matches `chimi metrics export --otlp` as JSON, on both backends | `tests/std_otel.rs` `a_metrics_request_is_what_the_cli_exports` |
| An unreadable document fails with the CLI's reason | `tests/std_otel.rs` `a_document_that_is_not_metrics_fails_with_the_reason` |
| `export` pushes every interval and when the body ends, to `/v1/metrics` with the content type and headers | `tests/std_otel.rs` `metrics_are_pushed_every_interval_and_when_the_body_ends` |
| A refused, 5xx or silent collector is counted once per failed push, not raised | `tests/std_otel.rs` `a_collector_that_is_down_slow_or_failing_is_counted_not_raised` |
| The last push is bounded by `flush_within` | `tests/std_otel.rs` `the_last_push_is_bounded_by_flush_within` |
| A body that defects still pushes | `tests/std_otel.rs` `a_body_that_defects_still_pushes` |
| Settings that are not positive are a defect | `tests/std_otel.rs` `settings_that_are_not_positive_are_a_defect` |
| A stopped program pushes before it exits | `tests/std_otel.rs` `a_stopped_program_pushes_before_it_exits` |
| `Otel.log` writes to the outer log, filters by level, and pushes each entry once with typed attributes | `tests/std_otel.rs` `logs_are_pushed_and_still_written` |
| A full log buffer drops and counts | `tests/std_otel.rs` `a_full_log_buffer_drops_and_counts` |
| A stopped program pushes its waiting logs | `tests/std_otel.rs` `a_stopped_program_pushes_its_waiting_logs` |
| Entries waiting behind a slow push are flushed; the flush is not lost to a full mailbox | `tests/std_otel.rs` `entries_waiting_behind_a_slow_push_are_flushed` |
| At most `log_buffer` entries wait while the process is idle | `tests/std_otel.rs` `at_most_log_buffer_entries_wait` |
| The last logs and metrics pushes share `flush_within` | `tests/std_otel.rs` `the_last_pushes_share_flush_within` |
| The `programs/otel-worker` example pushes its jobs' metrics and logs | `tests/std_otel.rs` `the_otel_worker_example_pushes_its_jobs` |
| The `programs/otel-worker` example pushes its jobs' metrics and logs | `tests/std_otel.rs` `the_otel_worker_example_pushes_its_jobs` |
