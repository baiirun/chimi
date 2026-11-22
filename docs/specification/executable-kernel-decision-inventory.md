# Chimi Executable-Kernel Decision Inventory

## Purpose

This inventory is the consolidation boundary between Chimi's completed kernel
exploration and its first normative language specification. It records what the
validated prototypes actually decided, what remains provisional, and what is
deliberately outside the kernel.

The inventory prevents older prototype questions from being mistaken for
current uncertainty and prevents disposable implementation choices from
silently becoming language requirements.

## Scope

This document covers the first executable kernel: source syntax, modules and
names, data and expressions, callable contracts, dependency provision,
evaluation, observable execution, compiler representations, diagnostics,
formatting, inspection, and frontend performance constraints.

It does not specify production scheduling, structured concurrency, resource
management, supervision, durability, recovery, a permanent backend, an
artifact format, package management, deployment, or interoperability.

## Accepted Language Contract

### Modules and names

- A source file defines its module; source files MUST NOT require module
  headers.
- Every imported module binding MUST be named explicitly with `as`. The
  language MUST NOT derive a source namespace from a path basename.
- External functions MUST remain module-qualified at call sites.
- Explicit member imports MAY bind public types and enum constructors.
  Functions MUST NOT be imported into unqualified value scope, and wildcard
  imports MUST NOT exist in the kernel.
- Module, type, and value namespaces are distinct. Every resolved reference
  MUST carry a stable module-qualified symbol identity.
- Declarations are private by default. Publishing a struct exposes its nominal
  type but MUST NOT expose its fields or implicit constructor.

Evidence: [name-resolution prototype](../../prototypes/name-resolution/README.md)
and [grammar prototype](../../prototypes/grammar/README.md).

### Data and values

- `struct` declares a nominal product type. `enum` declares a nominal closed
  sum type whose variants may carry data.
- Anonymous records are transparent structural product values and MUST contain
  at least one field.
- Struct fields and variant payloads MUST be checked against their declared
  shapes.
- Chimi MUST NOT have a nullable `Nil` or `null` value. Optional data uses an
  explicit `Option<T>`-like closed variant.
- Successful completion without data has an explicit `Void`-like type, but
  that type MUST NOT have a source-level value or public constructor.

Evidence: [golden corpus](../../prototypes/golden/README.md),
[grammar prototype](../../prototypes/grammar/README.md), and
[type-contract prototype](../../prototypes/type-contracts/README.md).

### Functions, expressions, and branching

- Functions are values. Named and anonymous functions use the same callable
  model and MAY return functions.
- Type annotations, including success return types, are optional by default for
  both public and private functions. Tooling MUST make inferred contracts
  inspectable.
- Every block is a nonempty expression. Its final expression is its value;
  blocks MUST NOT require a local `return` to produce that value.
- `case` is the kernel's branching expression. Ordinary matching over a closed
  variant MUST be exhaustive.
- An unguarded catch-all MUST be last. A guarded catch-all does not prove
  exhaustiveness. Unreachable arms and variant payload arity mismatches are
  diagnostics.
- `if` is available only as a case-arm guard. Guards MUST be proven free of
  dependencies or other observable execution.
- A pipeline is ordinary function application with the left operand as the
  first argument. `value |> transform` means `transform(value)`, and
  `value |> combine(other)` means `combine(value, other)`. The rule is
  syntactic and never falls back to applying a returned function. The language
  MUST NOT automatically partially apply missing arguments. (Amended
  2026-11-11: the user chose first-argument insertion, superseding the earlier
  unary-only rule.)

Evidence: [grammar prototype](../../prototypes/grammar/README.md),
[semantic-lowering prototype](../../prototypes/semantic-lowering/README.md), and
[type-contract prototype](../../prototypes/type-contracts/README.md).

### Callable contracts and failures

- Every expression has a success type, an expected-failure row, and a named
  dependency-needs row. These are distinct compiler channels even when their
  complete shape is inferred and omitted from source.
- Expected failures are values, not exceptions. An unhandled expected failure
  MUST short-circuit the remaining computation and join the caller's inferred
  failure row without propagation syntax.
- `Failure(error)` is the built-in constructor for a failed computation
  outcome. It MUST place the error value into the expected-failure channel and
  MUST NOT perform exception-style raising or untyped stack unwinding.
- An ordinary expression implicitly constructs a successful computation
  outcome. `Success(value)` is the equivalent optional explicit constructor;
  programmers MUST NOT be required to wrap ordinary successful expressions.
- `Success(pattern)` and `Failure(pattern)` inspect the corresponding channels
  when matching a fallible computation. They use the same outcome constructors
  rather than a separate context-only pattern mechanism.
- A storable `Result<T, E>` is ordinary data and remains distinct from an
  executing computation. Its constructors MUST be explicitly qualified, such
  as `Result.Success(value)` and `Result.Failure(error)`, when that distinction
  is relevant.
- Failure rows MUST retain variant identity internally. Tooling MAY display a
  whole error enum when every variant remains and individual variants after
  selective handling.
- Matching a fallible expression MAY handle selected failure variants. Any
  unmatched variants MUST continue bubbling automatically. This rule is
  distinct from exhaustive matching over an ordinary enum value.
- One declared generic MAY denote the same failure set in both ordinary value
  and `errors` positions. The value projection is the set's error-value union;
  the row projection is its expected-failure row.
- An unguarded selective failure pattern MUST subtract its fully covered
  variants from a closed or open row. A guarded pattern MUST NOT subtract.
  Tooling MUST preserve and display open residuals such as
  `E - PaymentError.Declined`.
- A captured failure value is ordinary data and MUST use exhaustive ordinary
  matching. Only a live outcome case permits omitted failure variants to
  bubble.
- Selective recovery MUST remain ordinary function and thunk composition. It
  MUST lower through existing outcome matching and MUST NOT introduce recovery
  syntax, a `Recover` Core node, or a runtime failure-row value.
- Defects and cancellation MUST remain outside expected-failure rows.
- Higher-order functions MUST preserve unknown success, failure, and needs
  relationships expressed by their callable parameters.
- Generic parameters MUST be declared explicitly after a function or data-type
  name. Their kind is inferred from use: value-type positions, `errors` rows,
  and `needs` rows constrain different kinds. An undeclared type-like name MUST
  be an unresolved-name diagnostic rather than an implicitly created generic.
- The first kernel supports declaration-scoped parametric polymorphism. It does
  not require higher-rank types or explicit kind annotations.

Evidence: [typed semantic precedents](../research/typed-semantic-pipeline-precedents.md)
and [type-contract prototype](../../prototypes/type-contracts/README.md).

### Dependencies and provision

- Dependencies MUST be represented in callable shapes separately from data
  inputs, success values, and expected failures.
- A dependency need has a lexical name and an interface type. Composition MUST
  union compatible needs and reject reuse of a name with an incompatible
  interface.
- Interfaces are structural records of functions. Conformance is implicit and
  MUST NOT add methods to a type or alter member lookup.
- A provision boundary MAY supply any subset of an operation's needs. Supplied
  needs are removed; unsupplied needs remain visible in the resulting callable
  contract.
- A provider MAY expose fewer failures or needs than its interface permits but
  MUST NOT expose more or change its required data inputs or success type.
- An execution root MUST have no unresolved dependency needs. Failed provider
  checking SHOULD suppress a redundant missing-root-dependency cascade for the
  same need.
- A runnable program exposes a conventional root function. The source language
  MUST NOT require an in-language `Runtime.run` call. The root closes its
  ordinary program dependencies through `using`; the toolchain or embedding
  host invokes it.
- Operational execution configuration—such as tracing export, execution
  identity, cancellation, and resource policy—belongs to the external runner
  boundary and MUST NOT be disguised as ordinary program dependency provision.

Evidence: [type-contract prototype](../../prototypes/type-contracts/README.md).

### Evaluation and observable execution

- Blocks and calls evaluate left to right. An expected failure short-circuits
  the remaining sequence unless handled.
- Retry is ordinary higher-order composition over an individual operation, not
  worker syntax or a distinguished operation field. A retry policy MAY select
  expected failure variants; defects and cancellation MUST NOT retry.
- Runtime completion has four distinct outcome classes: success, expected
  failure, defect, and cancellation.
- The external runner selects `off`, `boundaries`, or `full` tracing without
  changing evaluation or terminal outcomes. `off` MUST bypass event
  construction, buffering, serialization, and export work; `boundaries`
  retains computation lifecycle and dependency-call events.
- Every executed semantic operation MUST be observable as structured runtime
  events in full tracing mode. Events MUST retain deterministic sequence within
  one execution, trace/span causality, source identity, semantic node or
  operation identity, and typed attributes.
- `computation` is a stable low-cardinality logical kind. A unique trace or
  execution ID identifies one run. Dynamic identifiers belong in attributes.
- Structured computation context MUST propagate across runtime-owned child
  work, retries, and dependency calls. Process or task labels are optional
  debugging identity and MUST NOT define computation context.

Evidence: [runner prototype](../../prototypes/runner/README.md) and
[runtime context research](../research/runtime-computation-event-context.md).

## Accepted Toolchain and Compiler Contract

### Representation boundaries

- The parser MUST be lossless and error-tolerant: every source byte remains in
  an immutable semantic-free CST, including trivia and invalid text.
- Parsing MUST return a tree plus syntax diagnostics rather than fail as a
  whole. Missing syntax uses zero-width missing tokens; unexpected consumed
  syntax remains under explicit error nodes.
- CST construction SHOULD remain separate from the parsing algorithm through a
  tree-independent event stream.
- Semantic lowering MUST produce a compact source-mapped representation that
  excludes formatting trivia, preserves malformed holes without inventing
  meaning, and leaves name and type identity to later stages.
- Pipelines normalize to calls while retaining their source origin for
  diagnostics. Dependency provision remains an explicit semantic boundary.
- The resolved-to-typed and typed-to-executable handoffs are required compiler
  boundaries, but their concrete representations remain to be specified.

Evidence: [parser architecture research](../research/lossless-parser-architectures.md),
[lossless-parser prototype](../../prototypes/lossless-parser/README.md),
[semantic-lowering prototype](../../prototypes/semantic-lowering/README.md), and
[kernel validation](../../prototypes/kernel-validation/README.md).

### Formatting, diagnostics, and inspection

- Chimi has one canonical formatter. Project-level formatting styles are not a
  kernel feature.
- Formatting consumes the lossless CST. The formatter MUST leave source
  unchanged when syntax errors make a complete canonical rewrite unsafe.
- Diagnostics from all compiler stages share a stable envelope containing the
  originating stage, code, severity, primary source span, optional secondary
  labels, and optional source edits. A shared envelope MUST NOT collapse the
  distinction between syntax, name, type, and later-stage errors.
- Cursor inspection composes a containing CST path with source-mapped semantic
  facts. Tools MUST NOT recover semantic identity from formatted text.

Evidence: [tooling-seams prototype](../../prototypes/tooling-seams/README.md).

### Performance

- Correctness invariants remain independent of benchmark hardware: lossless
  source representation, total malformed-input consumption, valid diagnostic
  locations, bounded recovery, and structural equivalence across clean and
  future incremental execution.
- Performance results MUST record environment and implementation revision.
- The provisional experience targets remain 16 ms p95 for typical full-file
  syntax/editor reparses and 100 ms p95 for local semantic diagnostic updates
  on a recorded reference machine.
- The implementation MUST measure cold startup, valid and malformed parsing,
  editor edits, retained CST storage, local semantic invalidation, and
  high-fanout invalidation. It MUST NOT claim incremental performance before an
  incremental semantic engine exists.

Evidence: [frontend performance contract](../../prototypes/frontend-performance/README.md)
and [kernel validation](../../prototypes/kernel-validation/README.md).

## Provisional Surface Spellings

The behavior above is accepted, but these source spellings are not yet frozen:

- the `.chimi` extension;
- `fn`, `struct`, `enum`, `interface`, `pub`, `case`, and guard `if`;
- `errors`, `needs`, and postfix `using`;
- `Void` and `Option`;
- the `|>` pipeline operator.

Specification consolidation MAY retain these spellings as the first normative
dialect. Changing a spelling MUST NOT reopen its accepted semantic contract
without a separate decision.

## Explicitly Deferred Beyond the Kernel

- Effect tracking for async suspension, state, IO, resources, and concurrency;
  dependency needs are not a substitute for programming-language effects.
- `newtype`-like declarations. Structs, enums, and anonymous records are
  sufficient for every accepted kernel program; additional named data forms
  require a concrete future use case. Type aliases were accepted on
  2026-11-22 for naming settings records and failure sets (CHI-279; see
  decisions.md).
- The blessed structured-concurrency, cancellation, supervision, backpressure,
  and fault-isolation APIs.
- Production durability, poison-job policy, recovery, queues, scheduling, and
  distributed execution.
- Memory management, optimization, code generation, backend, artifact format,
  FFI, packaging, deployment, and hot code replacement.
- Full incremental compilation and semantic-query architecture.

## Verification

Run the combined validation:

```sh
python3 prototypes/kernel-validation/main.py --check
```

The validator MUST report every component probe passing,
`specification-ready=true`, and `end-to-end-compiler=true`. The focused Rust
probes establish the resolved-names-to-typed-contracts and
typed-contracts-to-executable-core handoffs.

Use the golden corpus as the behavioral acceptance input. A normative rule that
cannot be tied to a corpus fixture, accepted prototype verdict, design
principle, or explicit new decision does not belong in the first specification.

## Notes

Go and Python are disposable prototype hosts. Their object layouts, package
boundaries, and runtime behavior are not Chimi implementation commitments.
