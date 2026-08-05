# Chimi Compiler Representation Handoffs

## Purpose

This contract connects the two compiler seams that the executable-kernel
prototypes modeled independently:

```text
resolved semantic program -> typed program -> executable core
```

The design must preserve source identity and useful partial analysis for editor
tooling while making invalid programs impossible to lower as runnable roots. It
must also leave room for a fast incremental implementation without selecting a
permanent backend or runtime representation.

## Scope

This document defines the interfaces, ownership, invariants, diagnostics, and
verification of:

- resolved semantic representation to typed representation;
- typed representation to a verified root;
- verified root to executable semantic core.

It does not specify a query engine, memory layout, optimizer IR, bytecode,
machine code, garbage collector, scheduler, serialization format, or production
runtime.

## Design Principles

- Each compiler phase is a deep module with one transformation interface.
- The immutable source-mapped semantic representation is shared across analysis
  phases; resolution and typing attach facts by stable IDs instead of cloning
  and rewriting the whole tree.
- Partial analysis and executable validity are different types. Tooling can
  inspect an invalid typed snapshot, but executable lowering accepts only an
  opaque verified root.
- Stable semantic identity is never derived from pointer addresses, display
  names, formatted text, or backend locations.
- These transformations are pure in-process dependencies. The compiler SHOULD
  call them directly and MUST NOT introduce adapter interfaces merely to mock
  them.

## Shared Identity Model

Every representation uses compact IDs whose meaning is scoped to an immutable
compiler snapshot:

| ID | Identifies | Stability requirement |
| --- | --- | --- |
| `FileId` | One source file | Stable while the file belongs to the project snapshot |
| `ModuleId` | Module derived from a workspace-relative file path | Stable across unchanged project snapshots |
| `SyntaxId` | A CST node or token | Maps exactly to lossless source structure |
| `SemanticId` | A lowered declaration, expression, pattern, or type reference | Stable when that semantic entity is reused |
| `SymbolId` | A declared module, type, constructor, function, binding, parameter, generic, or requirement | Derived from declaration identity, not spelling alone |
| `TypeId` | An interned canonical type | Equal types share identity within the snapshot |
| `FailureRowId` | An interned normalized failure row | Members are variant `SymbolId`s or row parameters |
| `NeedsRowId` | An interned normalized dependency row | Members retain requirement identity, name, and interface `TypeId` |
| `OriginId` | Source and semantic origin of an executable node | Resolves to `FileId`, span, `SemanticId`, and lowering origin |
| `CoreNodeId` | One executable semantic operation | Stable within the executable program |

IDs are opaque outside the module that owns their arena or table. Tools use
query methods rather than depending on integer layout.

Every `SemanticId` resolves to a valid source span or an explicit synthetic
origin tied to the nearest source construct. Missing and malformed semantic
nodes retain identity but can never enter a verified root.

## Ownership Model

Representations form immutable snapshots with explicit ownership:

```text
SourceSnapshot
  owns source text and lossless CSTs

SemanticProgram
  retains SourceSnapshot
  owns semantic arenas and SemanticId -> SyntaxId/source mappings

ResolvedProgram
  retains SemanticProgram
  owns symbols, scopes, exports, and reference-resolution tables

TypedProgram
  retains ResolvedProgram
  owns type/row arenas, substitutions, contracts, pattern facts, and diagnostics

VerifiedRoot
  borrows or retains one TypedProgram
  proves one entry point is executable

CoreProgram
  owns executable nodes, constants, closed contracts, and origin table
```

A later phase MUST NOT mutate an earlier snapshot. Implementations MAY use
persistent arenas, reference counting, copy-on-write pages, or another internal
strategy as long as the semantic ownership above remains observable.

## Handoff 1: Resolution to Typing

### Interface

Conceptually, the type-checking module exposes one interface:

```text
check(resolved: ResolvedProgram) -> TypedProgram
```

`check` always returns a `TypedProgram`, including for invalid source. User
diagnostics are data on the snapshot, not a failed host-language call.

### ResolvedProgram input

`ResolvedProgram` contains:

- the immutable source-mapped `SemanticProgram`;
- the complete symbol table and module export tables;
- a resolution result for every name, type reference, pattern constructor, and
  requirement reference;
- lexical scope and visibility facts needed by diagnostics and tooling;
- name-resolution diagnostics and explicit unresolved/ambiguous sentinels.

A resolution entry is either a concrete `SymbolId` or an explicit invalid
state such as unresolved, ambiguous, or inaccessible. Missing entries are an
internal compiler defect.

### TypedProgram output

`TypedProgram` adds keyed facts without rewriting semantic nodes:

- `SemanticId -> TypeId` for every value-producing expression and pattern;
- `SemanticId -> CallableContract` for every callable expression;
- generic parameter kinds, constraints, instantiations, and substitutions;
- normalized expected-failure and needs rows;
- resolved member selection after receiver typing;
- struct construction and field-shape checks;
- case coverage, reachability, guard purity, and bound-pattern types;
- provider conformance and provision results;
- diagnostic-suppression facts such as an already-explained provider mismatch;
- type-stage diagnostics and explicit poisoned facts for invalid nodes.

The callable contract is:

```text
CallableContract {
  inputs: [TypeId]
  success: TypeId
  failures: FailureRowId
  needs: NeedsRowId
}
```

Failure rows identify enum variants by `SymbolId`; they MUST NOT normalize by
textual name. A complete enum row MAY retain a compact whole-enum form as long
as selective handling produces the same variant-level semantics.

Needs rows contain entries equivalent to:

```text
Need {
  requirement: SymbolId
  lexical_name: NameId
  interface: TypeId
}
```

Composition unions compatible entries and diagnoses one lexical name paired
with incompatible interfaces. Provision removes only successfully matched
requirements.

### Generic inference

Every generic binder has one inferred kind:

- value type;
- failure row;
- needs row.

Constraint generation is driven by its declared uses. A generic used in
incompatible kinds is a diagnostic. Function application instantiates declared
parameters with fresh inference variables and unifies all three callable
channels.

The first implementation needs declaration-scoped, rank-one polymorphism only.
It MUST preserve row parameters through higher-order functions but need not
support higher-rank arguments or explicit kind syntax.

### Failure and recovery inference

- Ordinary calls union argument channels with the called contract's channels.
- `Failure(error)` has the error's variant in its failure row and an
  unconstrained success type that unifies with its context.
- An ordinary expression and `Success(value)` have the value's success type and
  add no failure.
- A failure-channel case removes handled variants, unions recovery-arm channels,
  and leaves unhandled variants in the result.
- An ordinary enum case proves exhaustive coverage independently from failure-
  channel bubbling.
- Defects and cancellation never appear in `FailureRowId`.

### Requirements and `using`

The checker treats requirement bindings as lexical values whose types come from
the declared interface. `using` checks each provider structurally against its
named requirement and produces the input contract minus successfully provided
needs.

A provider can narrow method failures and needs but cannot widen them, change
inputs, or change success. A failed provider match records an explained need so
root validation does not emit a redundant missing-dependency diagnostic.

### Diagnostics

Typing continues through unresolved and poisoned facts to maximize useful local
tooling. It MUST suppress diagnostics whose only cause is an earlier unresolved
or malformed node. Independent errors in unaffected expressions remain visible.

All diagnostics use the shared stage/code/severity/span/labels/fixes envelope.
The type checker does not emit syntax or name diagnostics itself; it retains
those from `ResolvedProgram` in the combined snapshot.

## Executability Gate

### Interface

Executable validity is a separate deep module interface:

```text
seal_entry(
  typed: TypedProgram,
  entry: SymbolId,
) -> VerifiedRoot | DiagnosticSet
```

`VerifiedRoot` is opaque and cannot be directly constructed by callers.

### VerifiedRoot invariants

Successful sealing proves:

- `entry` identifies the conventional exported `main` function;
- every reachable reference resolves to a concrete accessible symbol;
- every reachable semantic node has a non-poisoned type and callable contract;
- every reachable generic is instantiated or validly generalized;
- every ordinary closed match is exhaustive and has no invalid patterns;
- every provider conforms;
- the root needs row is empty;
- no blocking syntax, lowering, name, or type diagnostic is reachable;
- every reachable semantic operation has a valid `OriginId`.

The runtime MAY defensively validate serialized or foreign-produced core input,
but that check does not weaken the compiler's sealing obligation.

Sealing does not execute code and does not choose tracing, cancellation, or
resource configuration.

## Handoff 2: Typed Root to Executable Core

### Interface

The executable-lowering module exposes one interface:

```text
lower(root: VerifiedRoot) -> CoreProgram
```

Lowering a valid `VerifiedRoot` does not produce user diagnostics. A violated
invariant is a compiler defect, not a source error.

### CoreProgram output

`CoreProgram` is a backend-neutral executable semantic representation. It owns:

- one closed root operation and its stable computation kind;
- normalized executable nodes and value layouts;
- callable and closure references;
- dependency requirement/method identities needed for observable calls;
- constant values and nominal constructor identities;
- the `CoreNodeId -> OriginId` mapping;
- enough type and outcome metadata to validate runtime transitions.

The core MUST distinguish at least these semantic operations:

- constant and nominal/record/list construction;
- closure creation and ordinary call;
- left-to-right sequence and local binding;
- ordinary value match and guarded arm selection;
- successful outcome construction;
- expected-failure construction and selected failure handling;
- dependency method invocation;
- provision scope introduced by `using`;
- explicit defect/trap operations needed for compiler or runtime invariants.

Cancellation is a runtime transition observed at semantic boundaries, not a
source error constructor.

The executable core MAY represent these operations as trees, control-flow
blocks, instructions, or another internal form. Its interface exposes semantic
meaning rather than layout.

### Normalization rules

- Blocks lower to explicit left-to-right sequencing whose final operation
  supplies success.
- Pipelines are already ordinary calls but retain their `OriginId` as pipeline
  syntax for diagnostics and traces.
- `Success(value)` and ordinary successful expressions normalize to the same
  success outcome.
- `Failure(error)` becomes an expected-failure transition.
- Failure-channel cases become handlers over selected variant `SymbolId`s;
  unmatched variants propagate.
- Requirement calls use stable requirement and method identities rather than
  lexical strings.
- `using` remains visible as a provision scope so runtime events can record
  provider origin.

### Standard-library operations

Retry is not a language-core node merely because the forcing example uses it.
`Retry.with(...)` is an ordinary higher-order standard-library function. A
future implementation MAY lower selected official-library operations to
runtime intrinsics, but such recognition must pass through a separately
specified intrinsic interface keyed by stable symbols, never source spelling.

The first executable lowering can represent retry composition with ordinary
closures and calls or with a provisional intrinsic adapter. That choice does
not alter the language contract.

### Source identity and observability

Every observable core operation has an `OriginId`. The origin table provides:

- primary source file and span;
- originating `SemanticId`;
- syntax origin such as direct call, pipeline, provision, or generated wrapper;
- optional parent origin for compiler-generated nodes.

Runtime events use `CoreNodeId` and `OriginId` to report source and semantic
identity. The backend MUST NOT use generated instruction offsets as the only
source of event identity.

A later optimizer that merges or duplicates operations must preserve a primary
origin and MAY retain additional contributing origins. Optimization must not
erase the causal identity required by full tracing.

### Runtime interface

`CoreProgram` is closed over ordinary dependency needs. The external runner
supplies operational configuration and begins evaluation:

```text
execute(core: CoreProgram, config: ExecutionConfig) -> ExecutionReport
```

`ExecutionConfig` owns tracing export, unique execution identity, cancellation,
and resource policy. It is not a Chimi source-level provider record.

`ExecutionReport` distinguishes success, expected failure, defect, and
cancellation and exposes the structured event stream or exporter result.

This interface does not require an interpreter. A native binary, VM, or other
backend may internalize `execute` while preserving its semantic contract.

## Module Depth and Seam Placement

The intended compiler modules are:

| Module | Interface | Complexity hidden |
| --- | --- | --- |
| Resolution | `resolve(SemanticProgram) -> ResolvedProgram` | scopes, imports, visibility, namespaces, stable symbols |
| Type checking | `check(ResolvedProgram) -> TypedProgram` | kinds, unification, rows, structural providers, patterns, suppression |
| Executability | `seal_entry(TypedProgram, SymbolId)` | reachability and all runnable-root invariants |
| Executable lowering | `lower(VerifiedRoot) -> CoreProgram` | sequencing, handlers, provision, closures, origin preservation |

These interfaces are the test surfaces. Internal solvers, arena builders,
interners, and traversal passes remain implementation details.

The executability module earns a separate seam because it turns a partial
tooling snapshot into an opaque proof object. Resolution, typing, and lowering
must not be split into pass-through modules that merely rename the same data.

## Performance Constraints

- Resolution and typing SHOULD attach compact tables keyed by stable IDs rather
  than clone the semantic tree per phase.
- Canonical types and rows SHOULD be interned. Hot paths SHOULD compare compact
  IDs instead of strings or recursive object graphs.
- Symbol and origin identity MUST be deterministic for an equivalent project
  snapshot.
- Phase interfaces MUST allow later reuse of unchanged semantic arenas and fact
  tables without making incrementality part of source semantics.
- The compiler MUST measure allocation, retained bytes, invalidated facts, and
  latency at these handoffs before claiming incremental performance.

## Rejected Shapes

- **One mutable AST for every phase:** makes partial states implicit, couples
  tools to pass ordering, and lets later phases corrupt earlier facts.
- **A fresh rewritten tree after every phase:** preserves phase distinction but
  duplicates structure, weakens identity, and raises the memory ceiling.
- **One mega-representation with optional resolution, type, and core fields:**
  makes illegal phase combinations representable and spreads state checks
  through every caller.
- **Lowering any TypedProgram and returning more user errors:** allows invalid
  executable states and makes the runtime compensate for compiler uncertainty.
- **Backend-specific executable core:** would turn the first backend into a
  language constraint before the runtime and performance work selects it.
- **Mockable adapter interfaces between pure compiler phases:** add indirection
  without real variation; deterministic in-process transformations are tested
  directly through their module interfaces.

## Verification

The first connected compiler implementation must demonstrate:

1. Every golden file reaches parsing, lowering, and resolution through the same
   representations used by tooling.
2. Every positive golden file produces complete type and contract facts with no
   poisoned reachable nodes.
3. Every negative golden file stops sealing with its expected stage, diagnostic
   code, and source span; unrelated errors remain visible and cascades remain
   bounded.
4. Every runnable positive root seals only after `using` closes its needs.
5. Lowering a verified root is deterministic and produces no user diagnostics.
6. Executing the lowered forcing scenarios preserves success/failure/defect/
   cancellation outcomes and required event ordering.
7. Every emitted event resolves through `OriginId` to the expected source and
   semantic operation.
8. Clean and future incremental analysis produce equivalent resolved, typed,
   sealed, and core results.

Tests cross the four module interfaces above and assert observable facts. They
must not inspect solver worklists, arena allocation order, or traversal passes.

Until items 1–7 are connected, the kernel remains specification-ready but not
an end-to-end compiler.

## Notes

The current Go prototypes use copied structs, strings, and hand-built scenarios
for clarity. Those choices are evidence for semantics, not endorsed production
layouts. The first implementation should preserve the interfaces and invariants
in this document while measuring candidate storage strategies against the
frontend performance contract.
