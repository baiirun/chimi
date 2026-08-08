# Native Streaming Memory Baseline

This baseline validates the scoped reclamation boundary used by
`FileSystem.process_utf8_lines`. It is evidence for `prog show ts-da83af`, not a
general runtime throughput target.

## Strategy

The linked runtime opens an allocation scope immediately before it begins
invoking line callbacks. After each successful callback, it precisely traces
the returned state through Chimi values, persistent map nodes, closures,
captured environments, and dependency providers. It then reclaims every
unreachable allocation created inside the scope. A failure, defect, or
cancellation is retained instead of the success state so its complete value
remains valid after the file is closed.

This boundary preserves structural sharing: a returned immutable map may point
to nodes retained from an earlier state, while obsolete paths and evaluation
temporaries are collected. Nested line processors transfer their retained
allocations to the parent scope. Top-level retained values remain valid until
process exit. Chimi source does not expose pointers, handles, regions, or manual
memory operations.

The first implementation uses individually allocated, precisely classified
runtime objects. That keeps the ownership rule small and testable, but it does
not optimize allocation rate. Arenas, compact value layouts, or generational
collection can replace this mechanism behind the same host-operation boundary.

## Measurement

Measured on Apple M1, Darwin 25.5.0 arm64, from compiler base `1048e9b`. The
program is `compiler/chimi/tests/fixtures/project-host-io/app/main.chimi`; each
line decodes the same JSON object and replaces one key in a persistent map.
Tracing was off, stdout was discarded, and the default 1 MiB record limit was
active.

| Input | Scoped total allocation | Scoped peak live | Scoped retained | Collections | Peak RSS | Wall time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 100,000 records / 1.1 MiB | 3,004,100,032 B | 30,464 B | 455 B | 100,001 | 1,769,472 B | 1.55 s |

The integration gate compares 64 and 16,384 records. It requires identical
scoped peak-live and retained bytes while total allocation and collection count
grow with the number of records. This demonstrates that memory depends on live
state and the largest record, not total input length.

The remaining ceiling is allocation churn: this workload requests about 30 KiB
per record even though almost all of it dies at the callback boundary. That is
a future value-layout and allocator optimization target, not evidence of
retention.

## Reproduction

Build the fixture, generate repeated `{"value":1}` JSONL records, and run the
executable with:

```text
--chimi-runner \
  --trace=off \
  --memory-report-file=/tmp/chimi-memory.json \
  -- \
  /tmp/input.jsonl
```

The report uses schema `chimi.runtime-memory.v1` and contains scoped total
allocated bytes, allocation bytes by runtime object kind, peak-live bytes,
retained bytes, and collection count. The report is diagnostic evidence; it is
not application output or telemetry.
