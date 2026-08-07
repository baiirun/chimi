# Native Direct Integer Baseline

`prog show ts-02d20b` is the first production slice to replace
helper-per-Core-node execution with direct native operations. It deliberately
targets only finite pure integer trees so the compiler can prove that no
dependency, failure, call, allocation policy, or scheduler boundary is hidden by
fusion.

## Lowering contract

A maximal tree containing integer constants, transparent identity nodes,
negation, addition, subtraction, and multiplication lowers to checked Cranelift
SSA. The generated root queries runtime trace policy once:

- off mode executes the SSA and boxes one final integer;
- boundary mode does the same, while reserving the node count so subsequent
  trace span identities remain unchanged;
- full mode uses the existing helper evaluator and emits every canonical
  operation event;
- any arithmetic overflow uses that same evaluator to preserve the established
  defect and origin.

The compiler-private runtime ABI moves from version 1 to version 2 for the trace
query and direct-integer success boundary. Module and entry cache identities
include the ABI and `maximal-pure-integer-ssa-v1` lowering revision, so old
objects cannot link against the new policy accidentally.

Only maximal roots receive fused code. An initial implementation emitted the
complete subtree in every eligible wrapper, growing the representative module
object from 13,368 bytes to 60,144 bytes. That design was rejected. Maximal-root
emission produces a 15,400-byte object and a 94,792-byte executable, compared
with 13,368 bytes and 94,544 bytes for the helper baseline.

## Correctness evidence

The native oracle now includes six workloads. The 132-node direct-integer
workload and the existing structural success, dependency, retry, defect, and
cancellation workloads match the interpreter byte-for-byte in off, boundary,
and full modes. CLI integration tests separately prove the fast result, boundary
event shape, full per-operation tracing, and overflow fallback. The planner unit
test accepts only the bounded operator set and proves that one nested expression
produces one maximal native root.

## Host measurements

Measured August 6, 2026 on Apple M1 (`arm64`, Darwin 25.5.0), using Apple clang
21.0.0. Both executables compile the same 132-node
`integer-arithmetic.chimi` source with C `-O2` runtime support. Values are native
runtime nanoseconds per execution after in-process warmup.

| Trace mode | Samples / repeats | Helper p50 | Helper p95 | Direct p50 | Direct p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Off | 40 / 10,000 | 12,745 ns | 12,819 ns | 175 ns | 181 ns |
| Boundaries | 40 / 5,000 | 16,768 ns | 17,090 ns | 4,088 ns | 4,104 ns |
| Full | 100 / 200 | 515,775 ns | 519,565 ns | 515,325 ns | 518,505 ns |

Direct lowering reduces off-mode p50 by 98.6% and boundary p50 by 75.6% on
this forcing workload. Full tracing remains within measurement noise because it
intentionally retains the helper evaluator. These numbers demonstrate the
lowering ceiling for one pure representation; they do not predict call,
allocation, matching, dependency, concurrency, or durability performance.
