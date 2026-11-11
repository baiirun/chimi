# Concurrency pipeline

This forcing program exercises the connected native Task and bounded Channel
runtime from Chimi source. It runs one finite worker, moves exclusive channel
capabilities into that worker, captures a configured dependency, fills bounded
buffers, drains messages in a `loop` until the channel reports `Drained`, closes
both producer sides, and joins the child.

From the repository root:

```sh
cargo run -p chimi -- build --root programs/concurrency-pipeline app/main -o target/concurrency-pipeline
target/concurrency-pipeline \
  --chimi-runner \
  --trace=full \
  --telemetry-file=target/concurrency-pipeline-trace.jsonl \
  --outcome-file=target/concurrency-pipeline-outcome.jsonl \
  --execution-id=concurrency-pipeline \
  --
```

The successful value is `25`: transformed values `11 + 12`, plus the worker's
processed count `2`.
