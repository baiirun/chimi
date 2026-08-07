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

## Contributing

Run the compiler and end-to-end gates.

```sh
cargo test -p chimi
cargo clippy -p chimi --all-targets --all-features -- -D warnings
python3 prototypes/kernel-validation/main.py --check
```

## License

No license has been declared.
