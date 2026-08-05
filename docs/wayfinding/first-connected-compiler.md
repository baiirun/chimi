# Build Chimi's First Connected Compiler

The canonical implementation map is the `prog` epic `ep-703bbc`. Use
`prog show ep-703bbc` for its destination and `prog graph -p chimi` for the
dependency graph.

## Destination

Implement the executable-kernel specification as one source-to-execution
compiler with connected tooling and measured incremental performance.

The implementation follows the dependency-ordered milestones in the
[first compiler roadmap](../plans/first-compiler-implementation-roadmap.md).

## Milestones

| Milestone | `prog` pointer |
| --- | --- |
| M0 — Select the compiler implementation foundation | `prog show ts-0846e5` |
| M1 — Connect the thin source-to-execution compiler slice | `prog show ts-3ce62b` |
| M2 — Complete the lossless frontend and source identity | `prog show ts-e87da7` |
| M3 — Implement modules, names, and declared generics | `prog show ts-737964` |
| M4 — Implement types, failures, needs, and patterns | `prog show ts-b79f17` |
| M5 — Seal, lower, and execute the full kernel | `prog show ts-b70040` |
| M6 — Connect formatter, diagnostics, and inspection | `prog show ts-1bf9f2` |
| M7 — Add incrementality and enforce performance | `prog show ts-fa8494` |

## Frontier

M0 selected Rust as the bootstrap host through
[ADR 0001](../adr/0001-use-rust-for-the-bootstrap-compiler.md). The disposable
Go-versus-Rust host spike is retained as primary evidence on the
`codex/compiler-host-spike` branch rather than production compiler code. The
real release-built `chimi` executable now owns the repository's Rust workspace
and reports the explicit frontend boundary required by the M0 gate.

M1 is complete. The real Rust compiler reads the minimum accepted `.chimi`
program through `SourceSnapshot`, `SemanticProgram`, `ResolvedProgram`,
`TypedProgram`, opaque `VerifiedRoot`, and `CoreProgram`, then emits structured
start/completion events and a successful nominal value. Missing or renamed
`main`, unresolved constructors, and invalid field values stop sealing with
source-mapped execution-setup, name, and type diagnostics respectively.

M2 is the active frontier. It replaces M1's deliberately narrow direct parser
with the accepted lossless lexer, parser event stream, immutable green CST,
recovery model, and deterministic semantic lowering without changing the
downstream representation interfaces.

Permanent backend and production runtime selection are tracked separately in
the draft map `ep-60dc6f` and begin only after the connected compiler provides
real program evidence. That decision must preserve the
[single-executable distribution contract](../research/single-binary-compiler-host-constraints.md):
one deployable application executable with the required Chimi runtime linked
in, inspectable external system dependencies, and fully static output only on
supported targets.
