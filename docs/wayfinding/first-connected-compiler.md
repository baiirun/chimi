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

The fifth M4 slice retains postfix `using` as an explicit semantic provision
scope and checks structural providers at any expression boundary. Successful
providers remove matching needs from an executed expression or a latent
function contract; partial provision preserves unsupplied needs. Provider
records may contain extra members, and method contracts may narrow failures and
needs but cannot widen channels or change inputs and success. Unknown,
duplicate, missing, and incompatible providers produce bounded field-level type
diagnostics without discharging the failed need. M4 remains open for outcomes,
pattern analysis, and closed-root validation.

The sixth M4 slice gives the contextual `Success` and `Failure` constructors
their computation-channel semantics without introducing a storable result
wrapper. Ordinary calls continue to bubble failure rows. `Failure(error)` adds
the statically known enum variant (or the error enum's complete variant set)
with a contextually inferred success type, while `Success(value)` is equivalent
to the ordinary successful value. Outcome cases bind success and error values,
remove selectively handled failure variants, preserve unmatched variants, and
union any failures produced by recovery arms. Missing outcome channels,
impossible variants, invalid error values, and malformed constructors receive
bounded type diagnostics. M4 remains open for complete ordinary-pattern
analysis, guard rules, explained-need root suppression, and closed-root
validation.

The seventh M4 slice preserves literal identity and source ranges in semantic
patterns, then applies recursive usefulness analysis to ordinary and outcome
cases. Closed enum and Boolean spaces must be exhaustive; nested constructor
patterns and multiple case subjects are analyzed without enumerating their
Cartesian product, while integer and text literals remain open domains.
Duplicate arms, arms after an unguarded catch-all, mismatched patterns, and
non-exhaustive cases receive bounded diagnostics. Guards must be Boolean and
may not expose failures or dependency needs; because a guard can reject its
row, guarded patterns never establish coverage or discharge an outcome failure
variant. M4 remains open for explained-need root suppression and closed-root
validation.

The eighth M4 slice closes callable-channel typing. Each normalized need now
retains whether a failed provider check has already explained why it remains
open. That provenance composes through calls and blocks, while union keeps a
need unexplained if any contributing path is still missing a provider without a
more specific diagnostic. After body inference converges, every conventional
exported `main` receives an execution-setup diagnostic for each unresolved,
unexplained dependency; successfully provided roots are closed, and provider
shape errors do not trigger a redundant root error. M4 is complete. M5 owns the
full `VerifiedRoot` reachability proof, executable-core lowering, and execution
of the complete typed language.

M5 is active. Its first slice makes the project-typed executability gate the
canonical `seal_entry` interface and labels the original one-struct M1 path as
legacy. Sealing now identifies an entry by module-local semantic identity,
requires exactly one exported zero-input `main`, rejects generic or unresolved
entry contracts, preserves all blocking compiler diagnostics, and returns an
opaque `VerifiedRoot`. M5 remains open for the complete reachable-node and
origin proof, backend-neutral core lowering, and runtime execution.

The second M5 slice completes the first reachable-root proof. Semantic lowering
now preserves the exact source range of every expression shape instead of
borrowing a child range or synthesizing `0..0`. Sealing walks resolved function
and module-binding references from `main`, excludes unreferenced declarations,
requires every reachable expression to have a complete non-poisoned type fact
and valid source range, and assigns deterministic snapshot-local `OriginId`s in
module/semantic order. Qualified module receivers remain namespace syntax, not
executable value nodes. The opaque root retains the reachable declarations and
origin table for the next M5 slice: backend-neutral core lowering.

The third M5 slice introduces the canonical backend-neutral `CoreProgram` and
renames the original fixed M1 representation `LegacyCoreProgram`. Lowering now
consumes only `VerifiedRoot`, creates dense typed node arenas, preserves the
verified origin on every node, records declared and closure parameter symbols,
and makes block bindings and evaluation order explicit. Nominal calls normalize
to construction nodes; ordinary calls, constants, references, selection,
collections, records, operators, provision scopes, closures, and matches retain
distinct core operations without falling back to semantic nodes. Contextual
success and failure construction and patterns remain explicit in the core. A
structural assertion pass verifies node, child, entry, function, binding, type,
and origin links immediately after lowering.

The fourth M5 slice connects that project core to the disposable execution
adapter through `Compiler::run_project`. Lowering now records runtime symbol and
nominal-constructor metadata in the core itself and normalizes dependency method
selection to stable requirement, interface, and method identities. The adapter
evaluates values, module bindings, closures, calls, pipelines, sequences,
matches, provision scopes, and dependency calls without consulting semantic or
typed trees. Expected failures bubble as values until a contextual case handles
them. Reports preserve distinct success, expected-failure, defect, and
cancellation outcomes.

`ExecutionConfig` selects full or boundary tracing and owns execution identity
and an initial cancellation request. Full tracing emits paired start/end events
for every executed core node; both modes retain computation start/end and
dependency-call events. Every event carries deterministic sequence, causal span
identity, computation and execution identity, core and semantic origin, source
span, operation kind, and typed attributes. M5 remains open for executing the
composite retry forcing example and replacing initial cancellation with a live
cooperative signal.

The fifth M5 slice makes the canonical pipeline directly testable from the
command line. `chimi check <path>` and `chimi run <path>` treat the supplied
file as a validated `main` module, then use project resolution, typing, entry
sealing, core lowering, and—when running—the project evaluator. Diagnostics are
rendered against that same immutable source snapshot. The CLI MUST NOT infer or
silently search for imported modules yet: a supplied file must be
self-contained until the project loader defines discovery and package-root
rules. The checkout dependency fixture is the end-to-end CLI gate and must
retain two dependency-call events plus its nested `Order` success value. The
fixed M1 library adapter remains only as transitional regression coverage and
is no longer the user-facing command path.

The sixth M5 slice adds explicit filesystem project loading without choosing a
package manifest format. `chimi check --root <project-dir> <entry-module>` and
`chimi run --root <project-dir> <entry-module>` map a module such as
`billing/money` to `<project-dir>/billing/money.chimi`. The loader starts at the
entry module and recursively follows its declared imports; unrelated source
files under the root are not part of the compilation. It rejects missing
modules, import cycles, paths that escape through symlinks, and different module
names that resolve to one physical file. Compiler diagnostics retain the
owning module's source path after loading. Single-file commands remain valid
and do not infer a project root. The bootstrap loader currently parses each
module once to discover imports and the canonical compiler parses it again;
the incrementality milestone must cache and reuse that frontend snapshot rather
than preserve this prototype cost.

The seventh M5 slice proves retry-once exhaustion through ordinary function
composition. A library `with` function accepts a policy and returns a
polymorphic thunk transformer; piping an operation into that transformer
produces a new thunk with the operation's success, failure, and `needs`
channels. The executable fixture invokes the original thunk twice and returns
its final expected failure. No retry syntax, compiler operation, or runtime
intrinsic exists. This forced the general type system to preserve unresolved
generic parameters on returned function values and exposed a fixed-point bug
where module bindings compared raw type identities instead of equivalent
resolved structure. Generic inspection of an open failure row remains a
separate language-design question for selective policies.

The first M6 slice connects canonical formatting to the real lossless CST.
`Compiler::format` returns complete canonical text for syntax-valid snapshots
and the original bytes plus syntax diagnostics for malformed snapshots; it
never applies a partial guessed rewrite. Formatting normalizes whitespace and
brace layout while using CST ancestry to distinguish type arguments from
comparison operators, and it preserves comment text and structurally
significant line breaks. `chimi format <path>` exposes the same report by
writing source to stdout without modifying the file. The accepted syntax-valid
golden corpus gates reparsing, idempotence, and comment preservation.

The second M6 slice makes the compiler's shared diagnostic envelope explicit.
Every diagnostic carries its owning stage, stable code, severity, primary
source span, message, secondary labels, and source fixes; stages still own the
meaning and production of their records. `chimi check --json` resolves internal
snapshot file identities into module paths and source paths and emits the
versioned `chimi.diagnostics.v1` document. Syntax, name, and type failures keep
distinct stage values, multi-module diagnostics retain their owning source,
and successful checks emit an empty diagnostic list. Human diagnostics and
their stdout/stderr behavior remain unchanged. Label and fix arrays are part of
the stable envelope even though current producers do not populate them yet.

The third M6 slice retains the exact lossless CST snapshot consumed by semantic
lowering and connects a source cursor to the compiler's later facts without
reparsing. `Compiler::inspect_project` returns the containing syntax path and
token, reference status and declaration origin, inferred type, structured
success/failure/needs channels, and the reachable executable origin when the
project seals. Malformed snapshots remain inspectable alongside their
diagnostics. `chimi inspect` exposes the versioned `chimi.inspection.v1`
document; offsets and ranges are zero-based UTF-8 bytes, split code-point
offsets are rejected, and displayed lines and byte columns are one-based.

M7 profiles the connected clean pipeline before adding reuse. Semantic-origin
indexes remove project-wide symbol and reference scans. `CompilerSession`
centralizes source, CST, semantic-module, and typed snapshots: no-op analysis
reuses the complete typed result, changed modules reparse and relower once, and
typing invalidates the reverse-import closure while retaining facts elsewhere.
Filesystem project loading passes its import-discovery parse into the compiler
instead of parsing each module twice. Symbol-shape or module-set changes fall
back to a clean check because dense identities may shift. The accepted
256-module baseline records 34.53 ms p95 for an isolated edit and 79.88 ms p95
for a complete fanout closure, with clean-result equivalence and explicit
allocation, retained-memory, reuse, and invalidation counters. Peak RSS and
cold process startup remain future out-of-process measurements.

Permanent backend and production runtime selection are tracked separately in
the active map `ep-60dc6f`, using the connected compiler's real program
evidence. That decision must preserve the
[single-executable distribution contract](../research/single-binary-compiler-host-constraints.md):
one deployable application executable with the required Chimi runtime linked
in, inspectable external system dependencies, and fully static output only on
supported targets.

M8 begins with an opaque `BackendProgram` boundary over verified `CoreProgram`.
Its shared comparison runner fixes five workloads covering successful
structural execution, dependency calls, retry exhaustion, defects, and
cancellation. It records the complete core operation and value-layout profile,
prepare and interpreter execution latency, allocation, retained output, and
full-versus-boundary trace counts using the interpreter as a conformance oracle.
Cold-process startup and artifact size remain explicit `null` values until a
candidate produces an executable. The accepted oracle measurements and the
contract that both native AOT and embedded-bytecode spikes must satisfy are in
`docs/performance/backend-m8-oracle-baseline.md`.
