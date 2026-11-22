# Observability Modules

## Status

This is the normative contract for the Phase 7c observability modules
(CHI-289): `std/log` now; `std/metrics` and OpenTelemetry export are planned
in the same phase and are not specified yet. The words MUST, MUST NOT,
SHOULD, SHOULD NOT, and MAY are normative. The modules follow the
[standard library conventions](standard-library-conventions.md) and the
[runner-provided capability rules](system-modules.md#runner-provided-capabilities).

## Goals

- Logging is a need, as `console` and `clock` are (decisions.md, 2026-11-22):
  a function that logs says so in its signature, and a test sees what it logs
  by supplying a fake with `using`.
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
