# Chimi Frontend Performance Contract Prototype

> PROTOTYPE: this is a disposable workload and result-shape probe, not a
> benchmark implementation or a commitment to one compiler host language.

## Question

Do these deterministic workloads, measurements, invariants, and provisional
user-facing latency targets adequately constrain Chimi's batch compiler and
interactive tooling before the real lossless parser and semantic pipeline are
built?

The prototype generates complete Chimi project snapshots and representative
single edits in memory. It does not time the Python grammar recognizer because
those numbers would measure the throwaway host. The production Rust frontend
now implements the syntax and recovery workloads through the feature-gated
`frontend` benchmark; its accepted M2 baseline is recorded in
`docs/performance/frontend-m2-baseline.md`.

## Run

```sh
python3 prototypes/frontend-performance/main.py
```

The first screen explains the review. This prototype does **not** contain
performance results: it proposes the situations the future compiler must be
measured against. Press `n` to open the first scenario. Each scenario shows the
generated input or edit, the compiler phases under observation, the measurements
we would record, and the proposed pass condition. Use `d` for correctness
requirements and `r` only if you want to inspect the raw result shape.

Use `--all` for a noninteractive workload summary:

```sh
python3 prototypes/frontend-performance/main.py --all
```

Print the machine-readable result template for one workload with:

```sh
python3 prototypes/frontend-performance/main.py \
  --result-template editor.middle-brace-edit
```

## Contract layers

The contract separates requirements that must not depend on benchmark hardware
from measurements that must record their environment.

### Correctness invariants

- Every source byte remains represented in the lossless CST.
- Parsing consumes all input and never aborts on malformed source.
- Diagnostics and missing markers have valid source locations.
- Clean and incremental executions produce structurally equivalent results.
- Malformed regions do not cause unbounded diagnostic cascades.

### User-facing targets

- Typical full-file syntax and editor reparses target a 16 ms p95 envelope on
  the recorded reference machine.
- Local semantic edits target updated diagnostics within 100 ms p95.
- Malformed parsing must avoid catastrophic slowdowns relative to equivalent
  valid input.
- High-fanout semantic work must scale with the affected dependency closure and
  never exceed a clean full-project check.

These are provisional experience budgets, not portable claims about arbitrary
hardware. Every result records the implementation revision, release/debug
mode, operating system, architecture, CPU, core count, and memory.

### Implementation measurements

- p50 and p95 wall time, plus throughput where source size is meaningful;
- allocated, peak-live, retained-CST, and process-resident memory;
- reused nodes, invalidated bytes, queries, and modules for incremental work;
- diagnostic counts, recovered bytes, and invariant results.

## Workload families

- cold startup through the first useful project diagnostics;
- typical and unusually large valid files;
- malformed record/block boundaries and unfinished pipelines;
- prefix and middle-of-file editor edits;
- retained lossless CST storage across a project;
- local and high-fanout semantic invalidation.

`model.py` contains the pure workload definitions, deterministic source
generator, edit application, and result template. `main.py` is only the
throwaway terminal browser.

## Run the native frontend benchmark

```sh
cargo bench --bench frontend --features frontend-benchmarks -- \
  --warmups 5 --samples 40
```

The native runner emits JSON Lines. It validates lossless, range, parent, and
syntax-identity invariants outside the timed samples. Its counting allocator
reports requested allocation bytes, peak live bytes, and retained parse bytes
separately; peak RSS remains explicitly unavailable rather than estimated.

The same runner now measures the connected clean semantic pipeline plus no-op,
local-edit, and high-fanout reanalysis. Run one semantic case independently
without changing the accepted sampling contract with:

```sh
cargo bench --bench frontend --features frontend-benchmarks -- \
  --semantics-only --semantic-case local --warmups 5 --samples 40
```

Accepted M7 results are recorded in
`docs/performance/compiler-m7-baseline.md`.
