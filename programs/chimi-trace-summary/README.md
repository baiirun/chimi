# Chimi Trace Summary

Summarize one Chimi runtime JSONL execution with bounded memory.

## Installation

Build the native executable from the repository root.

```sh
cargo run -q -p chimi -- build \
  --root programs/chimi-trace-summary \
  app/main \
  -o target/chimi-trace-summary
```

## Quick Start

Build an instrumented Chimi program.

```sh
cargo run -q -p chimi -- build \
  prototypes/golden/valid/dependencies_and_execution.chimi \
  -o target/checkout
```

Export its telemetry and terminal outcome.

```sh
target/checkout \
  --chimi-runner \
  --trace=boundaries \
  --telemetry-file=target/checkout.trace.jsonl \
  --outcome-file=target/checkout.outcome.jsonl \
  --
```

Summarize both streams through standard input.

```sh
cat target/checkout.trace.jsonl target/checkout.outcome.jsonl \
  | target/chimi-trace-summary -
```

## Usage

Combine both streams before using a file argument.

```sh
cat target/checkout.trace.jsonl target/checkout.outcome.jsonl \
  > target/checkout.execution.jsonl
target/chimi-trace-summary target/checkout.execution.jsonl
```

Export the summarizer's own telemetry separately.

```sh
target/chimi-trace-summary \
  --chimi-runner \
  --trace=boundaries \
  --telemetry-file=target/summary.trace.jsonl \
  --outcome-file=target/summary.outcome.jsonl \
  -- \
  target/checkout.execution.jsonl
```

## Configuration

| Option | Description |
| --- | --- |
| `--trace=off` | Disable runtime telemetry. |
| `--trace=boundaries` | Export computation and dependency events. |
| `--trace=full` | Export every semantic operation. |
| `--telemetry-file=PATH` | Write runtime events to `PATH`. |
| `--outcome-file=PATH` | Write the terminal outcome to `PATH`. |
| `--memory-report-file=PATH` | Write scoped allocation counters. |
| `--max-line-bytes=N` | Bound each input record. |

Place runner options before `--`. Place application arguments after `--`.

## How It Works

`app/main.chimi` folds the input into one immutable `Summary`, one line at a
time, through `FileSystem.process_utf8_lines`. Each step either returns the
next `Summary` or a `TraceError` that names the line. The program is the Phase 1
forcing program, so it exercises these language features:

| Feature | Where |
| --- | --- |
| Module constants (`const`) | schema names, the usage text, and `empty_summary` |
| Multi-clause functions with literal patterns and guards | `finish` and `is_terminal_outcome` |
| Record update (`{ ..state, field: value }`) | every change to `Summary` |
| Pipelines with first-argument insertion | `add_fields`, `counts_json`, `render`, `summarize` |
| `==` and `<=` in case guards | `same_identity`, `check_schema`, `next_sequence` |
| Text interpolation | the `requirement.method` dependency key |
| Tuples and `Map.from` | the output object in `render` |
| List patterns | the single `[path]` argument in `main` |
| Failure-type and alias patterns | the `TraceError`, `LineTooLong`, `FileError`, and `Utf8Error` arms in `main` |

## Contributing

Run the compiler and end-to-end gates.

```sh
cargo test -p chimi
cargo clippy -p chimi --all-targets --all-features -- -D warnings
python3 prototypes/kernel-validation/main.py --check
```

## License

No license has been declared.
