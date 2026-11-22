# Chimi First Compiler Implementation Roadmap

## Outcome

Build the first real Chimi compiler as one connected implementation that can:

```text
read .chimi source
  -> preserve a lossless CST
  -> resolve and type-check the program
  -> prove a closed main root
  -> lower it to executable semantic core
  -> run it with structured events
```

The first completion milestone is not a production compiler or runtime. It is
the first time actual Chimi source—not a source-shaped scenario or hand-built
node graph—executes through every accepted compiler interface.

## Scope

This roadmap implements the [executable-kernel specification](../specification/executable-kernel.md)
and [compiler handoff contract](../architecture/compiler-representation-handoffs.md).

It includes the batch compiler, diagnostic model, source-preserving frontend,
semantic analysis, typed contracts, executable sealing, a disposable execution
adapter, canonical formatting, inspection queries, and performance measurement.

It excludes production scheduling, supervision, durability, distributed
execution, a permanent backend, optimization, package publishing, deployment,
FFI, and self-hosting. Even though implementation is deferred, permanent
backend work must preserve the accepted single-executable distribution
contract, and compiler architecture must preserve a path to eventual
self-hosting.

## Delivery Strategy

Implement one thin end-to-end path before completing each phase horizontally.
The first slice supports only enough syntax and semantics to execute a closed,
successful `main`. Every later milestone deepens the same modules and interfaces
until the full golden corpus passes.

This avoids recreating the prototype state where frontend, typing, and runtime
semantics are independently convincing but not connected.

The real compiler MUST live outside `prototypes/`. Prototype code is executable
design evidence and an oracle for behavior; it is not a library dependency or a
production package layout.

## Milestone Dependency Graph

```text
M0 implementation foundation
  -> M1 thin connected compiler
      -> M2 lossless frontend and source identity
          -> M3 modules, names, and declared generics
              -> M4 types, failures, needs, and patterns
                  -> M5 executable core and observable runner
                      -> M6 formatter, diagnostics, and inspection
                          -> M7 incrementality and performance
                              -> M8 backend and runtime decision gate
```

Each milestone leaves the compiler runnable. A milestone does not pass by
producing an isolated library that the preceding command path does not use.

## M0 — Select the Implementation Foundation

**Decision:** Rust is the bootstrap compiler host. See
[ADR 0001](../adr/0001-use-rust-for-the-bootstrap-compiler.md). The equivalent
Go-versus-Rust spike is retained as primary evidence on the
`codex/compiler-host-spike` branch and summarized on `prog` ticket `ts-0846e5`.

### Purpose

Choose the real compiler host and storage foundations using Chimi's measured
workloads rather than prototype convenience or language reputation.

### Work

- Define the compiler executable, workspace layout, release build profile, and
  benchmark harness.
- Evaluate at most two serious host-language candidates with the same compact
  spike: tokenize, arena-allocate, traverse, and retain a representative source
  workload.
- Measure cold startup, build iteration time, throughput, total allocation, and
  retained bytes.
- Confirm support for compact tagged data, arenas/interning, deterministic
  builds, profiling, fuzzing, eventual native/backend integration, and
  distribution of the bootstrap compiler without an installed host-language
  runtime.
- Confirm that compiler representations and phase interfaces do not expose the
  host language's object model, error model, or runtime semantics, preserving a
  feasible path to a self-hosted implementation.
- Select one host. Record why the rejected candidate loses for this compiler.
- Select or implement only foundational libraries whose interfaces do not leak
  into Chimi semantics.

### Gate

- One implementation host is selected with recorded evidence.
- `chimi --version` and `chimi check <path>` start through one release-built
  executable without a separately installed host-language runtime, even if
  `check` initially reports “frontend unavailable.”
- The benchmark harness records environment and revision in the accepted result
  shape.
- No backend, VM, query framework, or parser generator is selected merely
  because it is conventional in the host ecosystem.

## M1 — Connect the Thin Compiler Slice

### Purpose

Exercise every permanent compiler seam before deep implementations make those
interfaces expensive to change.

### Minimum language slice

<!-- chimi: check -->
```chimi
struct Answer {
  value: Int
}

pub fn main() {
  Answer { value: 42 }
}
```

### Work

- Create `SourceSnapshot`, `SemanticProgram`, `ResolvedProgram`, `TypedProgram`,
  opaque `VerifiedRoot`, and `CoreProgram` with stable IDs and source origins.
- Implement the smallest direct transformations for the program above.
- Add a disposable in-process execution adapter that returns the successful
  nominal value.
- Route one CLI command through the complete path.
- Emit computation start/completion events with computation, execution, node,
  and source identity.

### Gate

- `chimi run` executes the source file above and prints or exports its structured
  success outcome.
- Deleting or renaming `main`, corrupting its field type, or leaving an
  unresolved name prevents `VerifiedRoot` construction with a source-mapped
  stage-correct diagnostic.
- Executable lowering accepts only `VerifiedRoot` and cannot report a user
  diagnostic.
- No stage passes source strings or host pointers as semantic identity.

## M2 — Complete the Lossless Frontend

### Purpose

Replace the thin parser with the accepted lossless, error-tolerant frontend
without changing downstream interfaces.

### Work

- Implement the lossless lexer, tree-independent parser event stream, immutable
  green CST, missing tokens, error nodes, and lightweight cursor views.
- Cover the complete source grammar, including explicit generic binders,
  records versus blocks, functions, types, case arms, pipelines, and `using`.
- Implement deterministic semantic lowering with stable `SemanticId`s and
  source origins.
- Integrate the syntax/recovery performance workloads from the first commit of
  this milestone.
- Add malformed-input fuzzing with termination, round-trip, and valid-span
  assertions.

### Gate

- All golden and recovery fixtures satisfy exact round-trip and syntax-stage
  expectations through the real `chimi check` path.
- Malformed files still produce CSTs and semantic holes without crashes or
  invented valid meaning.
- Typical valid and editor full reparses meet the provisional 16 ms p95 target
  on the recorded reference machine or produce an explicit measured exception
  before downstream work continues.
- CST storage and transient allocation are reported separately.

## M3 — Implement Modules, Names, and Declared Generics

### Purpose

Produce a complete `ResolvedProgram` from real project files.

### Work

- Implement file-path module identity, explicit module aliases, explicit
  type/constructor member imports, and separate namespaces.
- Implement private-by-default declarations and private struct representation.
- Resolve lexical bindings, pattern constructors, requirement bindings, and
  type references to stable `SymbolId`s.
- Bind explicit generic parameters and reject undeclared type-like names.
- Retain unresolved, ambiguous, and inaccessible sentinels for partial tooling.
- Implement project-level dependency ordering without requiring a package
  manager.

### Gate

- The complete name-resolution corpus runs through parser, lowerer, and resolver
  in one compiler process.
- Every resolvable reference has a deterministic `SymbolId`; every invalid
  reference has the expected name-stage diagnostic and source span.
- A one-character type typo cannot silently create a generic parameter.
- Qualified origin remains visible in diagnostics, inspection, and generated
  contract display.

## M4 — Implement Types and Callable Channels

### Purpose

Connect resolved semantic programs to the accepted success, expected-failure,
and dependency-needs type system.

### Work

- Implement interned canonical `TypeId`, `FailureRowId`, and `NeedsRowId`
  arenas.
- Implement declaration-scoped rank-one generic inference and kind constraints
  for value types, failure rows, and needs rows.
- Infer callable success, failure, and needs channels through blocks, calls,
  functions, and higher-order composition.
- Implement `Success`, `Failure`, automatic bubbling, selective recovery, and
  variant-level failure identity.
- Implement nominal construction, structural records/interfaces, provider
  narrowing, `using`, and explained-need suppression.
- Implement ordinary case exhaustiveness, reachability, payload arity, guard
  purity, and result-type unification.
- Keep poisoned facts queryable while suppressing derivative cascades.

### Gate

- Every accepted type-contract scenario is expressed as real `.chimi` input or
  a focused compiler fixture and passes through `chimi check`.
- Every value-producing reachable `SemanticId` has a type and callable contract
  fact, or an explicit poison tied to an earlier diagnostic.
- Invalid golden files emit their expected type-stage code and span without
  redundant cascades.
- Contract display exactly exposes inferred success, failure variants, and
  named needs for public and private functions.
- Local semantic baseline measurements are captured even before incrementality.

## M5 — Seal, Lower, and Execute the Full Kernel

### Purpose

Reach the first complete execution of actual Chimi source across expected
failure, dependency, and observability semantics.

### Work

- Implement `seal_entry(TypedProgram, SymbolId)` and every `VerifiedRoot`
  invariant.
- Lower verified roots to backend-neutral executable core with origin mappings.
- Implement the disposable execution adapter for values, closures, calls,
  sequence, matches, outcomes, dependency calls, and provision scopes.
- Execute providers embedded by root `using` boundaries.
- Implement structured events and the four terminal outcome classes.
- Implement retry through ordinary standard-library composition. Retry-once
  exhaustion is verified with a polymorphic thunk transformer and requires no
  intrinsic. Keep selective inspection of open failure rows as a separate
  type-system question.

### Gate

- A real `.chimi` forcing program executes from source with all needs closed by
  `using`.
- Success, handled and unhandled expected failures, selective retry, exhaustion,
  defect, and cancellation fixtures produce the accepted outcomes.
- Defects and cancellation never enter failure rows and never retry.
- Every full-tracing event resolves to a computation kind, execution ID,
  causal span, core node, semantic origin, and source span.
- The kernel validator reports both formerly independent handoffs as
  `connected` and gates `end-to-end-compiler=true` with focused Rust probes.

## M6 — Connect the Developer Toolchain

### Purpose

Make the same compiler snapshots power formatting, diagnostics, inspection, and
documentation-facing contract display.

### Work

- Implement the canonical formatter against the real lossless CST.
- Refuse unsafe whole-file formatting for malformed source while preserving
  diagnostics and inspection.
- Normalize every stage's diagnostics into the shared envelope.
- Implement cursor queries for syntax path, resolved symbol, inferred type,
  callable contract, failure variants, needs, and executable origin where
  available.
- Expose stable machine-readable commands for agents and editors before adding
  an LSP transport.

### Gate

- Formatting is deterministic and idempotent across the golden corpus and
  preserves comments.
- Malformed formatting leaves source bytes unchanged.
- Syntax, name, and type errors remain visibly distinct in human and structured
  output.
- One cursor query can explain where a name came from and show the complete
  inferred callable contract.
- CLI interfaces provide the same information needed by a later LSP; the LSP
  is an adapter, not the owner of compiler behavior.

## M7 — Add Incrementality and Enforce Performance

### Purpose

Meet interactive performance goals after the connected clean implementation
establishes correctness and baseline costs.

### Work

- Profile the full clean path before selecting an incremental strategy.
- Add stable snapshot reuse at the smallest measured high-leverage seams:
  source/CST, semantic arenas, resolution facts, and typed facts.
- Centralize invalidation and dependency-closure policy rather than scattering
  caches through phase implementations.
- Batch module and fact processing where it amortizes setup and simplifies hot
  loops.
- Add cold startup, prefix edit, middle edit, local semantic edit, high-fanout
  edit, malformed input, and retained-project benchmarks.

### Gate

- Clean and incremental results are structurally equivalent.
- Typical editor syntax work remains within 16 ms p95 and local semantic
  diagnostics within 100 ms p95 on the recorded reference machine.
- High-fanout work is bounded by the affected closure and never exceeds a clean
  full-project check.
- Reports include latency, allocation, retained memory, reused nodes/facts, and
  invalidated modules/bytes.
- Any missed target has a measured cause and a specific next optimization;
  architecture is not replaced on intuition alone.

## M8 — Decide the Permanent Backend and Runtime Direction

**Decision:** Native AOT with a linked Chimi runtime is the permanent execution
architecture. Cranelift is the first code generator, while recovery, tracing,
cancellation, scheduling, and supervision remain runtime semantics above the
backend. See
[ADR 0002](../adr/0002-use-native-aot-with-a-linked-runtime.md).

### Purpose

Use a working typed core and measured programs to choose the next execution
architecture without making that decision part of the first compiler's
critical path.

### Inputs

- executable-core operation mix and value layouts;
- allocation and call profiles from real Chimi programs;
- cold startup and artifact-size requirements;
- the requirement that an application, its Chimi dependencies, and required
  Chimi runtime support link into one deployable executable;
- target-specific static-linking limits and inspectable OS/native-library
  dependencies;
- observability and cancellation costs;
- future concurrency, supervision, and durability requirements;
- integration constraints of candidate native, VM, or existing-runtime targets.

### Gate

- The decision compares at least two viable execution strategies against the
  same programs and operational requirements.
- The selected backend consumes `CoreProgram` through a narrow lowering seam.
- `chimi build` produces one deployable executable without a separately
  installed Chimi runtime or package tree.
- Intermediate module artifacts remain independent and cacheable before the
  final link; the deployment contract does not force whole-program compilation.
- OS and native-library dependencies are reported, and fully static output is
  promised only for explicitly supported targets.
- Backend-specific types, instructions, and lifecycle rules do not leak back
  into Chimi source semantics or typed contracts without a separate language
  decision.
- The disposable execution adapter remains a conformance oracle until the new
  backend matches its observable semantics.

## Cross-Cutting Engineering Rules

- Assertions enforce every representation and handoff invariant in debug and
  test builds.
- A regression starts as a failing corpus fixture or interface-level test.
- Tests exercise compiler module interfaces; they do not bind to solver
  worklists, arena order, parser event order, or backend offsets unless those
  are the behavior under test.
- Diagnostics are never string-matched internally; stages use stable codes and
  structured data.
- Hot representations use compact IDs and interned values rather than repeated
  strings or recursive copies.
- No new abstraction is added solely for a hypothetical second implementation.
- CLI, LSP, documentation, and agent tools consume the same compiler snapshots
  through narrow query interfaces.
- Every milestone records correctness and performance evidence before the next
  dependency begins.

## Long-Term Bootstrap Constraint

Self-hosting begins only after Chimi can express the compiler and the permanent
backend is proven. The bootstrap sequence is:

```text
host compiler -> first compiler implemented in Chimi
              -> Chimi compiler rebuilds itself
              -> reproducibly equivalent released compiler
```

The bootstrap host may change during early implementation. Chimi's source
semantics, compiler representations, artifact contract, and runtime behavior
must remain independent of that host.

## Recommended Project Sequence in `prog`

After this roadmap is accepted, create one implementation epic with milestones
M0–M7 as dependency-ordered child tickets. Keep M8 as a separate decision epic
because it begins a new runtime/backend design phase rather than completing the
first connected compiler.

Each milestone ticket should link its normative specification section, handoff
invariants, corpus fixtures, benchmark workloads, and explicit completion gate.

## Completion Definition

The first compiler phase is complete after M7 when:

- actual `.chimi` source reaches execution through every specified compiler
  representation;
- all accepted positive and negative corpus behavior is verified;
- formatting, diagnostics, inspection, and structured execution use the same
  snapshots and identities;
- full clean and incremental performance evidence is recorded;
- the compiler remains backend-neutral and production runtime work is still an
  explicit later decision.
