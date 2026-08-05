# Chimi CST-to-Semantic Lowering Prototype

> PROTOTYPE: this is disposable representation research, not the production
> Chimi frontend.

## Question

Does lowering the Chimi corpus from the accepted lossless CST into a compact,
source-mapped semantic representation create a useful boundary for later name
resolution and type/contract inference without exposing formatting syntax,
losing malformed-source structure, or silently performing those later stages?

## Run

From the repository root:

```sh
go -C prototypes/semantic-lowering run .
```

The prototype opens `golden/valid/composition.chimi`. Use `s` for source, `c`
for its lossless CST, `m` for the lowered semantic representation, and `d` for
the current lowering decisions.

Check the complete golden and focused recovery corpus:

```sh
go -C prototypes/semantic-lowering run . --check
```

Benchmark lowering independently from parsing:

```sh
go -C prototypes/semantic-lowering run . --bench
```

## Scope

The lowerer covers every CST construct exercised by the current 23 golden and
focused recovery fixtures. The executable check requires deterministic output,
valid source spans, complete top-level coverage, a representation smaller than
the CST plus tokens, no unsupported CST kinds, and no lowering diagnostics for
syntax-valid programs.

It deliberately does not infer success, error, or requirement channels.
Explicit channels remain fields of `FunctionType`; omitted annotations remain
marked for later inference. Blocks structurally separate intermediate steps
from their final value. Pipelines normalize to ordinary calls while retaining
an origin marker for later diagnostics. `using` remains an explicit semantic
provision boundary with its input operation and named provider expressions.
Function and data generic binders lower to explicit located names so resolution
can distinguish declared parameters from misspelled nominal types.

The key review question is whether this representation contains enough source
identity and program structure for resolution and typing while being materially
smaller and easier to inspect than the lossless CST.

## Current evidence

- All 24 golden and focused recovery fixtures satisfy the lowering contract.
- Syntax-valid fixtures lower without diagnostics; syntax-invalid fixtures
  remain representable without crashes or invented meaning.
- An incomplete pipeline becomes an explicit invalid semantic node retaining
  its left-hand input rather than silently becoming that input.
- On the recorded `darwin/arm64` Go 1.25.5 host, lowering a generated 737,600
  byte CST takes 55.0 ms p95. The current semantic object layout retains about
  13.7 bytes per source byte beyond the already-retained source and CST and
  allocates about 127 bytes per source byte during construction.

The timings and memory ratios characterize this disposable Go layout, not a
portable production budget. They leave compact storage and allocation strategy
as implementation work without changing the lowering boundary.
