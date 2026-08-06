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

M2 is complete. It replaces M1's deliberately narrow direct parser
with the accepted lossless lexer, parser event stream, immutable green CST,
recovery model, and deterministic semantic lowering without changing the
downstream representation interfaces.

The first M2 slice now establishes those boundaries in the Rust compiler for
the executable M1 grammar. The lexer retains trivia and invalid text, the
tree-independent parser emits events, the sink builds an immutable green CST,
and missing syntax becomes zero-width tokens while diagnostics remain separate.
Valid and malformed fixtures round-trip byte-for-byte, and CST lowering retains
the source ranges consumed by the existing semantic and execution pipeline.
M2 remains active for the full accepted grammar, broader recovery corpus, typed
syntax facade, stable identity work, fuzz invariants, and performance workloads.

The second M2 slice extends that same event/CST boundary across explicit module
imports, visibility, structs, enums, interfaces, generics, qualified and
callable types, `errors`, and `needs` rows. The executable semantic lowerer
still accepts only the original M1 program shape and now emits an explicit
lowering diagnostic for every parsed declaration it cannot represent, rather
than silently dropping valid CST structure. Expression and pattern parsing is
the next grammar slice.

The third M2 slice completes the accepted expression and pattern surface with
layered precedence parsing for unary, multiplicative, additive, comparison,
pipeline, and `using` expressions; postfix calls and member access; lists,
records, blocks, function values, cases, guards, and patterns. The production
Rust frontend now matches all 20 golden syntax classifications and all four
focused recovery fixtures from the disposable parser while preserving exact
source bytes. CST calls remain semantically neutral: the narrow M1 lowerer
recognizes its executable constructor shape from syntax tokens, and broader
valid programs stop explicitly at lowering. M2 remains active for the typed
syntax facade and cursor, stable source identity, fuzz invariants, and measured
syntax-performance workloads.

The fourth M2 slice adds the tooling-facing syntax seam. Lightweight red
cursors now derive absolute byte ranges, parent navigation, and deterministic
snapshot-scoped `SyntaxId` values from immutable green nodes; parse snapshots
resolve those IDs back to exact nodes or tokens. Typed wrappers for files,
structs, fields, functions, blocks, postfix expressions, and call suffixes
return optional children under recovery, and production semantic lowering now
selects declarations and their names and bodies through that facade. The first
wrappers are handwritten and isolated behind `syntax/typed.rs`; schema-driven
generation should replace repetition when the facade expands. IDs deliberately
do not claim stability across edits. This left fuzz invariants and measured
syntax-performance workloads as the final M2 gate.

The fifth M2 slice closes the frontend gate. A fixed-seed suite checks 2,048
malformed inputs for termination, exact round-trip, valid diagnostic and syntax
ranges, bounded diagnostic growth, parent navigation, subtree counts, and
resolvable unique syntax identities. The real `chimi check` executable also
classifies all 20 golden and four recovery fixtures at the syntax stage. A
feature-gated native benchmark connects the accepted typical, large,
malformed, editor-reparse, and retained-project workloads to the production
parser while reporting p50/p95 latency and allocated, peak-live, and retained
bytes separately. The [M2 baseline](../performance/frontend-m2-baseline.md)
passes the provisional 16 ms syntax target and malformed-cost gate on the
recorded Apple M1; it identifies allocation and retained CST size as future
optimization targets without justifying a representation change now.

M3 is complete. `Compiler::resolve_project` accepts validated workspace-relative
module paths, assigns deterministic file/module/symbol identities, lowers the
complete declaration and expression shapes needed by name resolution, and
orders modules dependency-first. Resolution keeps module, type, and value
namespaces separate; enforces explicit aliases and type/constructor member
imports; preserves private struct representation; binds lexical values,
requirements, patterns, and declared generics; and retains unresolved,
ambiguous, inaccessible, and type-deferred reference facts for tooling. The ten
prototype resolution scenarios now run through the Rust parser, lowerer, and
resolver in one process, with additional regression coverage for deterministic
qualified origins, diagnostic spans, nested function scopes, guards, record
values, and conflicting member imports. The narrow M1 executable adapter remains
intact until M4 supplies the full typed-program handoff.

M4 is active. Its first slice preserves exact product fields, enum payloads,
function inputs and results, nested callable `errors` rows, and named `needs`
rows in the semantic handoff. The type checker interns nominal and callable
types plus failure and needs rows against resolved `SymbolId` identities,
infers whether declared generics occupy value, failure-row, or needs-row kinds,
records poison facts for unresolved annotations, and exposes deterministic
callable contracts through `Compiler::type_project`. That header slice left
body inference, channel composition and discharge, structural provider
checking, `Success`/`Failure`, and pattern exhaustiveness open.

The second M4 slice makes literals and calls explicit semantic expressions and
infers concrete body types through names, field selection, calls, blocks, and
nested functions. Call sites validate arity and argument/result compatibility;
expected-failure and named-needs channels accumulate through expressions and
converge monotonically across forward function references. Captured bindings
remain available while nested function parameters shadow them. M4 remains open
for the rest of the expression algebra, generic call instantiation, `using`
discharge and providers, `Success`/`Failure`, and full pattern analysis.

The third M4 slice gives declared generic functions an explicit rank-one
polymorphic callable representation. Each direct invocation creates a private
substitution for value, failure-row, and needs-row parameters; repeated
positions constrain one another within that call, while separate calls cannot
share inferred bindings. Higher-order arguments bind complete callable
contracts, allowing `map_success`-style composition to preserve concrete error
and dependency rows. M4 remains open for contextual instantiation of generic
function values, remaining expressions, `using` and structural providers,
outcome recovery, and patterns.

The fourth M4 slice replaces the remaining flattened structural and operator
expressions with explicit semantic forms for labeled calls, lists, records,
groups, unary and binary operators, and pipelines. Nominal constructors own
their declared field shapes, validate labels independently of source order,
and instantiate generic struct or enum parameters per call. Structural records
have canonical sorted field types, lists unify one element type (including an
expected type for `[]`), and pipes reuse ordinary callable invocation and
channel propagation. M4 remains open for `using`, structural interface
conformance, outcomes, and pattern analysis.

Permanent backend and production runtime selection are tracked separately in
the draft map `ep-60dc6f` and begin only after the connected compiler provides
real program evidence. That decision must preserve the
[single-executable distribution contract](../research/single-binary-compiler-host-constraints.md):
one deployable application executable with the required Chimi runtime linked
in, inspectable external system dependencies, and fully static output only on
supported targets.
