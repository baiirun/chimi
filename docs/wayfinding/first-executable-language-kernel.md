# Find Chimi's First Executable Language Kernel

The canonical Wayfinder map is the `prog` epic `ep-9abb2d`. Use `prog show ep-9abb2d` for its current destination, notes, fog, and scope boundaries; use `prog graph -p chimi` for the live dependency graph.

## Destination

Reach a decision-complete design, supported by throwaway prototypes, for Chimi's first executable language kernel: source syntax, a fast lossless parsing pipeline, semantic analysis, lowering, executable semantics, diagnostics, and a disposable runner.

A compact background-worker program is only a composite forcing example. Production runtime execution and durability will be refined later.

## Tickets

| Decision ticket | Type | `prog` pointer |
| --- | --- | --- |
| Build the golden Chimi source corpus | Prototype | `prog show ts-ffa0aa` |
| Study lossless parser architectures | Research | `prog show ts-08dd56` |
| Study typed semantic pipeline precedents | Research | `prog show ts-8ec777` |
| Choose the compiler prototype foundation | Grilling | `prog show ts-70701e` |
| Define Chimi's lexical and surface grammar | Prototype | `prog show ts-929d55` |
| Prototype the lossless parser and CST | Prototype | `prog show ts-6f54dc` |
| Define CST-to-semantic lowering | Prototype | `prog show ts-556783` |
| Define names and modules | Prototype | `prog show ts-39cad7` |
| [Define types and operation contracts](../../prototypes/type-contracts/README.md) | Prototype | `prog show ts-b47a2e` |
| [Define executable semantics and runner boundary](../../prototypes/runner/README.md) | Prototype | `prog show ts-d7eaf3` |
| [Design formatting, diagnostics, and inspection seams](../../prototypes/tooling-seams/README.md) | Prototype | `prog show ts-92334c` |
| Define the frontend performance contract | Prototype | `prog show ts-892f0b` |
| [Validate the executable language kernel](../../prototypes/kernel-validation/README.md) | Prototype | `prog show ts-7109ff` |

## Frontier

This map is complete. The kernel validated as coherent and specification-ready,
but not yet as an end-to-end compiler. Continue with
[Consolidate Chimi's Executable Kernel](executable-kernel-consolidation.md)
(`prog show ep-aae72a`).

## Decisions So Far

- [Study lossless parser architectures](../research/lossless-parser-architectures.md) (`prog show ts-08dd56`) — Use a never-failing event parser, immutable lossless green CST, generated typed syntax facade, and separate compact semantic representation; benchmark before sophisticated incrementality.
- [Study typed semantic pipeline precedents](../research/typed-semantic-pipeline-precedents.md) (`prog show ts-8ec777`) — Model executable work with separately inferred success, expected-failure, and requirement rows; typed composition transforms those rows while defects remain outside expected failures.
- [Build the golden Chimi source corpus](../../prototypes/golden/README.md) (`prog show ts-ffa0aa`) — Drive the kernel with 8 valid and 12 invalid fixtures spanning the accepted provisional syntax, inferred computation shapes, runtime configuration, and required diagnostics.
- Choose the compiler prototype foundation (`prog show ts-70701e`) — Keep the compiler architecture and performance contracts implementation-independent; select any disposable prototype host and libraries tactically without implying permanent compiler, backend, or runtime choices.
- [Define Chimi's lexical and surface grammar](../../prototypes/grammar/README.md) (`prog show ts-929d55`) — Use contextual line breaks, nonempty anonymous records and expression blocks distinguished by `name:` lookahead, exhaustive `case` expressions without standalone `if`, an explicit non-nullable `Void` success type with no source value, and unary function pipelines without automatic partial application or argument insertion.
- [Define the frontend performance contract](../../prototypes/frontend-performance/README.md) (`prog show ts-892f0b`) — Keep all 10 deterministic workloads spanning startup, valid and malformed syntax, editor edits, CST storage, and semantic invalidation; require correctness invariants alongside environment-recorded measurements, with provisional 16 ms syntax and 100 ms local-semantic p95 experience targets.
- [Prototype the lossless parser and CST](../../prototypes/lossless-parser/README.md) (`prog show ts-6f54dc`) — Keep a lossless lexer, tree-independent event parser, immutable semantic-free green CST, explicit missing/error recovery, separate syntax diagnostics, and red cursor views. The boundaries are accepted provisionally; compact storage and lower transient allocation remain implementation concerns.
- [Define CST-to-semantic lowering](../../prototypes/semantic-lowering/README.md) (`prog show ts-556783`) — Lower every current corpus construct into a deterministic compact representation with source spans; discard formatting syntax, keep explicit channels on function types, separate block steps from results, normalize pipelines to calls with origin metadata, defer pattern/name identity to resolution, and preserve malformed semantic holes without inventing meaning.
- [Define names and modules](../../prototypes/name-resolution/README.md) (`prog show ts-39cad7`) — Use file paths as stable module identities; require every module source binding to be explicitly aliased; keep module, type, and value namespaces separate; require module-qualified function calls; allow explicit public type/constructor member imports without wildcards; keep struct representations private; and defer runtime-value member selection until receiver typing.
- [Define types and operation contracts](../../prototypes/type-contracts/README.md) (`prog show ts-b47a2e`) — Infer success, variant-level expected failures, and named dependency needs as separate callable channels; bubble unhandled failures and union needs through ordinary calls; allow selective failure recovery; preserve unknown rows through higher-order transformations; use `using` for typed provision at arbitrary boundaries; require closed runtime roots; allow structural providers to narrow channels; type pipelines as unary application; and enforce nominal struct fields plus exhaustive closed-variant patterns.
- [Design formatting, diagnostics, and inspection seams](../../prototypes/tooling-seams/README.md) (`prog show ts-92334c`) — Format from the lossless CST and leave syntax-invalid source unchanged; give every diagnostic a shared stage-aware source-mapped envelope while preserving stage ownership; and answer cursor inspection by composing the containing CST path with source-mapped semantic facts.
- [Validate the executable language kernel](../../prototypes/kernel-validation/README.md) (`prog show ts-7109ff`) — All component probes pass and the accepted concepts form a coherent specification-ready kernel. The Rust probes now connect resolution through typing and typed contracts through verified executable core, so the validator also gates the end-to-end compiler path.
