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

Summarize a combined telemetry and outcome file.

```sh
target/chimi-trace-summary target/execution.jsonl
```

## Usage

Pass `-` to consume standard input.

```sh
cat target/execution.jsonl | target/chimi-trace-summary -
```

Export the summarizer's own telemetry separately.

```sh
target/chimi-trace-summary \
  --chimi-runner \
  --trace=boundaries \
  --telemetry-file=target/summary.trace.jsonl \
  --outcome-file=target/summary.outcome.jsonl \
  -- \
  target/execution.jsonl
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

## Contributing

Run the compiler and end-to-end gates.

```sh
cargo test -p chimi
cargo clippy -p chimi --all-targets --all-features -- -D warnings
python3 prototypes/kernel-validation/main.py --check
```

## License

No license has been declared.
