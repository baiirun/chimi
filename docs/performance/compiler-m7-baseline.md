# M7 Compiler Incrementality Baseline

## Purpose

This is the first native measurement of Chimi's connected clean and
incremental semantic pipeline. It extends the M2 syntax baseline with project
lowering, resolution, typing, snapshot reuse, reverse-import invalidation, and
clean-result equivalence.

## Environment

- Date: 2026-08-06
- Revision snapshot: `fe38505` with the M7 changes uncommitted
- Mode: Cargo `bench` release profile
- Host: Apple M1, macOS arm64, 8 logical cores, 16 GiB memory
- Rust: 1.92.0
- Sampling: 5 warmups and 40 measured iterations
- Command: `cargo bench --bench frontend --features frontend-benchmarks -- --warmups 5 --samples 40`

The semantic cases can be run independently with `--semantics-only
--semantic-case <clean|no-op|local|fanout>`. The accepted results used those
selectors to keep each process invocation within the automation runner's
window without reducing the sample count.

The allocator reports requested allocation bytes, peak live bytes above the
operation baseline, and retained bytes. Peak process RSS and cold process
startup remain unavailable in this in-process harness and are reported as
unavailable rather than estimated.

## Results

### Syntax and retained CST

| Workload | Source | p50 | p95 | Allocated | Retained CST |
| --- | ---: | ---: | ---: | ---: | ---: |
| Typical valid | 57,297 B | 1.12 ms | 1.15 ms | 6.70 MB | 1.42 MB |
| Prefix editor reparse | 114,548 B | 2.24 ms | 2.26 ms | 13.40 MB | 2.85 MB |
| Middle-brace editor reparse | 114,546 B | 2.29 ms | 2.32 ms | 13.40 MB | 2.85 MB |
| Malformed record colon | 114,546 B | 2.25 ms | 2.34 ms | 13.40 MB | 2.85 MB |
| Incomplete pipeline | 114,528 B | 2.25 ms | 2.39 ms | 13.40 MB | 2.85 MB |

The 64-file CST storage workload retained 54.70 MB for 2,201,408 source
bytes, or 24.85 retained bytes per source byte. It allocated 249.43 MB and
peaked at 56.09 MB live.

### Connected semantic pipeline

The generated project contains 256 modules, 20 declaration groups per module,
and 1,184,512 source bytes.

| Workload | p50 | p95 | Clean p95 | Allocated | Peak live | Retained analysis |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Clean resolution | 105.09 ms | 106.21 ms | — | — | — | — |
| Clean type pipeline | 153.35 ms | 154.73 ms | — | 491.24 MB | 74.82 MB | 72.86 MB |
| No-op reanalysis | 0.18 ms | 0.20 ms | 167.92 ms | 0.14 MB | 0.10 MB | 0 B delta |
| Local implementation edit | 34.03 ms | 34.53 ms | 168.43 ms | 63.31 MB | 12.63 MB | 0 B delta |
| Shared-contract fanout edit | 78.47 ms | 79.88 ms | 176.97 ms | 122.09 MB | 13.93 MB | 0.30 MB delta |

The clean pipeline retains 10,245 canonical types, 10,240 callable contracts,
15,360 type-use facts, 51,200 expression facts, and 20,480 declaration types.

### Invalidation

| Workload | Parsed reuse | Semantic reuse | Rechecked modules | Reused type facts | Invalidated type facts | Invalidated bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| No-op | 256/256 | 256/256 | 0 | 87,040 | 0 | 0 B |
| Local edit | 255/256 | 255/256 | 1 | 86,700 | 340 | 4,627 B |
| Fanout edit | 255/256 | 255/256 | 256 | 0 | 88,830 | 4,669 B |

The fanout fixture has 255 modules importing the changed shared contract, so
its affected reverse-import closure is the complete project. It produces 255
expected type diagnostics. Incremental and clean diagnostics, callable
contracts, and measured arena facts are structurally equivalent in these cases.

## Gate

- Typical and edited syntax p95 remain below the provisional 16 ms target.
- The local semantic edit is 34.53 ms p95, below the provisional 100 ms target.
- The high-fanout edit is 79.88 ms p95, below its 176.97 ms clean check and
  bounded to the complete affected closure.
- No-op analysis reuses the exact typed snapshot and all 87,040 facts.
- Malformed edits, public-contract edits, and symbol-shape fallbacks have
  clean-result equivalence tests.

The M7 syntax, local semantic, fanout bound, equivalence, and invalidation gates
pass on this reference machine.

## Interpretation and limits

Two measured global scans dominated the first clean baseline. Resolution facts
and source symbols were found by repeatedly scanning project-wide vectors;
indexing them by semantic origin reduced the 256-module clean check from an
exploratory 1.19 s p95 to 154.73 ms p95, about 7.7x. Resolution now scales
linearly across the recorded 64, 128, and 256-module points.

`CompilerSession` centralizes source, parse, semantic, and typed snapshots.
Unchanged modules retain their exact CST and semantic arena. A changed module
is reparsed and relowered once; typing rechecks its reverse-import closure and
reuses facts outside that closure. If the module set or symbol identities
change, the session deliberately falls back to a full type check because dense
module and symbol IDs may shift.

Dense intern arenas may retain unused historical entries when a stable symbol
changes to a newly introduced type. Observable diagnostics and formatted
contracts remain clean-equivalent; dense arena identity and compacting are not
source semantics.

The parser still performs a full-file parse for a changed module. Its 2.32 ms
editor p95 remains far below the syntax budget, so subtree parsing is not
justified by this baseline. Invalidation bytes currently mean the complete
changed source size, not a minimal text-edit range. Peak RSS and true cold
startup need a future out-of-process harness; the allocator measurements here
remain the enforceable memory evidence.
