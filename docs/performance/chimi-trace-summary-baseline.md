# Chimi Trace Summary Baseline

This baseline validates the first maintained program built primarily in Chimi.
It is milestone evidence for `prog show ts-65a567`, not a stable performance
promise.

## Environment

- Apple M1
- Darwin 25.5.0 arm64
- native AOT through Cranelift 0.131.3
- Chimi runtime ABI 5
- trace summarizer source in `programs/chimi-trace-summary/app/main.chimi`

The native executable is 200,992 bytes. It dynamically links only
`/usr/lib/libSystem.B.dylib`; no Chimi VM, package tree, or runtime sidecar is
required.

## Startup

Thirty process samples summarized one terminal outcome. Each traced sample
created a fresh telemetry file. Times include process startup, input, summary
encoding, and application output.

| Summarizer trace mode | p50 | p95 |
| --- | ---: | ---: |
| off | 15.67 ms | 34.16 ms |
| boundaries | 16.72 ms | 49.73 ms |
| full | 16.11 ms | 39.82 ms |

The sample is intentionally small, so scheduler variance dominates the trace
mode differences. End-to-end tests separately prove that trace policy does not
change application output or terminal status.

## Streaming Throughput and Memory

The generated input contains 100,000 `operation_completed` events, followed by
one success outcome. It is 13,988,950 bytes. Every event has a strictly
increasing sequence and stable execution and computation identities.

| Metric | Result |
| --- | ---: |
| Wall time | 19.98 s |
| Throughput | 5,005 records/s |
| Process peak RSS | 2,752,512 B |
| Scoped peak live | 749,819 B |
| Scoped retained | 1,064 B |
| Scoped total allocation | 74,898,174,091 B |
| Scoped collections | 100,002 |

Live memory remains bounded by current state and per-record work. Cumulative
allocation is about 749 kB per event and dominates current throughput. This
identifies environment cloning and boxed intermediate values as the next
optimization frontier without weakening the scoped ownership contract.

## Environment-Sharing Follow-up

`prog show ts-445d99` profiles and removes the first allocation multiplier.
The generated trace-summary program has 207 symbols, so cloning its environment
requests 3,328 bytes before evaluating a child. Sampling the unoptimized path
showed memory clearing, copying, allocation, and reclamation as the dominant
CPU stacks because every Core node cloned both symbol tables.

Ordinary Core evaluation now borrows its incoming environment. Only semantic
boundaries that bind state create a copy: function invocation, sequence,
provision, pattern arms, and closure capture. This preserves lexical isolation
and scoped reclamation without changing the runtime ABI, value layout, Core,
tracing, or terminal outcomes.

A same-checkout before/after run used 100,000 operation events plus one outcome
in a 14,388,951-byte input. The output was byte-identical.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Wall time | 6.48 s | 2.88 s | -55.6% |
| Throughput | 15,432 records/s | 34,723 records/s | 2.25x |
| Process peak RSS | 2,785,280 B | 2,080,768 B | -25.3% |
| Scoped peak live | 749,835 B | 185,823 B | -75.2% |
| Scoped retained | 1,068 B | 1,068 B | unchanged |
| Scoped total allocation | 74,899,374,095 B | 18,323,164,431 B | -75.5% |
| Scoped collections | 100,002 | 100,002 | unchanged |

Five follow-up runs recorded 2.64 s p50 and 2.76 s p95, or about 37,879
records/s at p50. A native integration gate now requires the maintained
summarizer to stay below 250 kB allocated per record and 250 kB peak live while
retaining less than 2 kB. Remaining allocation is primarily boxed values,
controls, match work, and deliberate environment copies at mutation and capture
boundaries; those are separate measured optimization candidates.

## Artifact and Output

The successful report was:

```json
{"dependency_calls":{},"events":100000,"operations":{"call":100000},"records":100001,"schema":"chimi.trace-summary.v1","terminal_outcomes":{"success":1}}
```

The `chimi.build.v1` report recorded one application module object, one entry
object, program metadata, and the linked runtime archive. The executable SHA-256
for this measurement was
`c76e2bed710f7c384ae93b84166a06c2035c03861ba7a2b3ab6d3395c169a0de`.

## Verification Boundary

Native integration tests capture traces from compiled success, dependency,
expected-failure, defect, and cancelled programs. Exact goldens cover trace-off,
boundary, and full inputs. Negative cases cover malformed JSON, invalid UTF-8,
mixed identities, sequence violations, outcome placement, missing outcomes,
record limits, file failures, and argument counts.
