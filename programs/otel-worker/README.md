# OTel worker

This example shows how a Chimi program sends its metrics and logs to an
OpenTelemetry collector while it runs, with `std/otel` (CHI-296). The rules
it follows are in `docs/specification/observability.md`, "std/otel".

`app/main.chimi` runs six jobs. Each one sleeps for 20 to 50 ms, records how
long it took and whether it failed, and logs a line. Every third job fails.

## Setting the exporter up

The exporter is set up in code, in `main`, rather than from environment
variables. Start from `Otel.defaults` and change the fields you need:

```text
let settings = {
  ..Otel.defaults,
  endpoint: "http://collector:4318",
  service_name: "otel-worker",
}
Otel.export(settings, fn(exporter) {
  run_jobs(count) using { log: Otel.log(exporter) }
})
```

- `Otel.export` runs the function it is given and pushes the metrics
  aggregate to `<endpoint>/v1/metrics` every `interval` (10 s by default),
  then once more when the function ends, however it ends. A program stopped
  by Ctrl-C or SIGTERM still makes that last push.
- `Otel.log(exporter)` is a `log` provider. Code under it logs as usual with
  `Log.info` and the other level functions. Each entry still goes to standard
  error, and entries at `log_level` (`Info` by default) and above are also
  pushed to `<endpoint>/v1/logs`, in batches, once per interval.
- A collector that is down, slow or failing never stops the program. A
  failed push is counted as `chimi.otel.failed_exports`, and a log entry
  that cannot wait is dropped and counted as `chimi.otel.dropped_logs`.
  Both counts go out with the next metrics push.
- The last pushes are bounded by `flush_within` (5 s by default), so a
  collector that is down delays the program's end by at most that.

Here the address comes from `--collector`, and `interval` is 100 ms so that a
short run pushes more than once. The `finished` line is written after
`Otel.export` returns, so it goes to standard error only.

## Running it

It needs the native build, because the interpreter has no network. Start a
collector that accepts OTLP over HTTP on port 4318, for example the
OpenTelemetry Collector with its `otlp` receiver. Then, from the repository
root:

```sh
cargo run -p chimi -- build --root programs/otel-worker app/main -o target/otel-worker
target/otel-worker -- --collector http://localhost:4318 --jobs 6
```

Without `--collector`, the program uses `Otel.defaults.endpoint`,
`http://localhost:4318`. `target/otel-worker -- --help` lists the options.

`tests/std_otel.rs` `the_otel_worker_example_pushes_its_jobs` runs it against
a stand-in collector and checks what each push holds.
