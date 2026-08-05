# M2 Frontend Performance Baseline

## Purpose

This is the first native measurement of the production Rust lossless frontend.
It closes the M2 syntax/recovery gate without setting portable performance
claims from one machine. The benchmark uses the deterministic workloads from
`prototypes/frontend-performance` and emits machine-readable JSON Lines.

## Environment

- Date: 2026-08-05
- Revision snapshot: `957aa19` with the benchmark changes uncommitted
- Mode: Cargo `bench` release profile
- Host: Apple M1, macOS arm64, 8 logical cores, 16 GiB memory
- Rust: 1.92.0
- Sampling: 5 warmups and 40 measured iterations
- Command: `cargo bench --bench frontend --features frontend-benchmarks -- --warmups 5 --samples 40`

The custom allocator counts requested bytes. It keeps live-byte accounting
enabled during latency samples, so timings include that atomic bookkeeping and
are conservative for a normal compiler build. Peak RSS is not measured and is
reported as unavailable.

## Results

| Workload | Source | p50 | p95 | Diagnostics | Allocated | Retained CST |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Typical valid | 57,297 B | 1.15 ms | 1.25 ms | 0 | 6.70 MB | 1.42 MB |
| Valid 500-declaration comparison | 114,547 B | 2.32 ms | 2.44 ms | 0 | 13.40 MB | 2.85 MB |
| Large valid | 1,145,047 B | 25.82 ms | 26.87 ms | 0 | 127.97 MB | 28.48 MB |
| Malformed record colon | 114,546 B | 2.37 ms | 2.54 ms | 1 | 13.40 MB | 2.85 MB |
| Incomplete pipeline | 114,528 B | 2.34 ms | 2.49 ms | 1 | 13.40 MB | 2.85 MB |
| Prefix editor reparse | 114,548 B | 2.33 ms | 2.47 ms | 0 | 13.40 MB | 2.85 MB |
| Middle-brace editor reparse | 114,546 B | 2.31 ms | 2.56 ms | 1 | 13.40 MB | 2.85 MB |

The retained-project workload parsed 64 files and 2,201,408 source bytes in
48.15 ms. It allocated 249.43 MB transiently, peaked at 56.09 MB live, and
retained 54.70 MB of parse results: 24.85 retained bytes per source byte.

## Gate

- Typical valid syntax p95 is 1.25 ms, below the provisional 16 ms target.
- Prefix and middle-of-file full reparses are 2.47 ms and 2.56 ms p95, below
  the provisional 16 ms target.
- Both malformed workloads are below the allowed 2x p95 of the comparable
  valid workload; each is effectively the same cost as valid input.
- The 20x source-size increase from typical to large produces a 21.42x p95
  increase, an initial near-linear result whose normalized cost is 1.07x.
- Every measured parse passes exact round-trip, source-range, parent-navigation,
  missing-token, subtree-count, and `SyntaxId` resolution invariants.

The M2 latency and recovery gates pass on this reference machine.

## Interpretation

Latency is comfortably inside the provisional editor budget, so parser
algorithm or cursor representation changes are not justified by this baseline.
Memory is the clear future comparison point: the current explicit-trivia,
individually allocated green tree retains about 24.85 bytes per source byte and
allocates about 113–117 bytes per source byte while building it. Packed child
storage, compact kind identifiers, and reduced parser/lexer intermediates
should be measured independently before choosing an optimization. Incremental
reuse, formatter cost, semantic invalidation, and peak RSS remain M7 work.
