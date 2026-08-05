# Chimi

Chimi is a programming language design context for a service-first, general-purpose-over-time language where production-service failure, concurrency, observability, and performance are explicit enough to reason about while ordinary application code remains high-level and ergonomic. Durable networked services are the first proving ground, not the permanent boundary.

## Use Cases

**HTTP API Service**:
A durable networked service that handles requests, talks to persistent storage, runs background work, performs retries, emits traces, and shuts down gracefully.
_Avoid_: Toy web app, controller-only API, request handler demo

**Background Worker Service**:
A durable networked service that consumes queued work, performs external operations, handles retries and poison jobs, emits traces, and shuts down without losing work.
_Avoid_: Cron script, fire-and-forget job, untracked worker

**Real-Time Coordination Service**:
A durable networked service that manages long-lived connections, shared state, fanout, backpressure, latency, cancellation, and runtime observability.
_Avoid_: Static API, polling-only service, unobserved connection hub

**CLI Tool**:
A command-line program that performs local work with clear startup, IO, configuration, error reporting, and distribution expectations.
_Avoid_: One-off script, shell snippet, service-only program

## Non-Goals

**Frontend Browser App**:
A browser-first application centered on DOM UI, bundling, hydration, and frontend framework patterns.
_Avoid_: Primary target, core design center

**Embedded Kernel Firmware**:
Bare-metal, kernel, firmware, or hard real-time programming where the program owns the platform at the lowest level.
_Avoid_: Primary target, service forcing function, ordinary performance-sensitive code

## Concepts

**Durable Networked Service**:
A long-running program that communicates over a network and is expected to remain understandable, recoverable, observable, and performant under failure and concurrency pressure.
_Avoid_: Service app, backend, distributed app

**Expressive Clarity**:
A design principle that favors powerful, visible language constructs that compose predictably and remain readable under real program complexity.
_Avoid_: Minimal simplicity, hidden magic, cleverness

**Operational Surface**:
The official language and standard-library concepts that expose reliability, failure, effects, concurrency, cancellation, tracing, and performance-sensitive resource use to programmers.
_Avoid_: Runtime folklore, framework-only behavior, hidden operational behavior

**Official Surface**:
The combined language and standard-library experience that defines Chimi's core programming model.
_Avoid_: Syntax-only language model, ordinary package convention

**Small Language Surface**:
A design boundary where the core language starts small and earns new constructs through concrete use cases rather than broad platform ambition.
_Avoid_: Batteries-included language core, syntax-first expansion, broad audience design

**Service-Rich Standard Library**:
A standard-library stance where service-oriented operational primitives can be official library concepts even while the core language stays small.
_Avoid_: Syntax-heavy service model, third-party-only service primitives, minimal operational library

**Service Author**:
A programmer who builds and owns production behavior for durable networked services.
_Avoid_: Scripting user, library-only author, runtime specialist

**Primary User**:
The initial audience for Chimi is its creator as the first serious service author, with broader ergonomics deferred until the language proves its model.
_Avoid_: Beginner-first audience, mass-market design, committee audience

**Solo-User Phase**:
The initial design phase where Chimi optimizes for its creator's real programs instead of general adoption, beginner onboarding, or ecosystem growth.
_Avoid_: Adoption-first design, beginner-first curriculum, community governance

**Free To Break**:
An early-stage compatibility stance where syntax, semantics, standard-library names, and tooling conventions can change until real programs validate them.
_Avoid_: Premature stability, compatibility theater, design freeze

**Performance Ladder**:
A design principle where ordinary code uses managed ergonomic defaults, while performance-sensitive code can opt into stricter resource controls when needed.
_Avoid_: Systems-only language, always-manual resource control, performance by accident

**Safe Control**:
Explicit control over resources, performance, or execution behavior that preserves Chimi's safety guarantees.
_Avoid_: Unsafe escape hatch, unchecked manual control

**No User-Visible Unsafety**:
Chimi code should not expose unsafe keywords, unsafe packages, or unchecked memory and resource operations to ordinary programmers.
_Avoid_: Unsafe block, unsafe package, unchecked operation

**Integrated Toolchain**:
The canonical set of official tools for formatting, building, testing, documenting, packaging, analyzing, and editing Chimi programs.
_Avoid_: Fragmented tooling, convention-only workflow, third-party-first workflow

**Canonical Formatting**:
A single official formatting style applied by Chimi's formatter without project-level style negotiation.
_Avoid_: Configurable style, formatter bikeshedding, house style

**Teaching Diagnostic**:
A compiler or toolchain message that explains an error in terms of Chimi's programming model, not just the local syntax or type mismatch.
_Avoid_: Cryptic diagnostic, pointer-only error, compiler jargon

**Enforcement Split**:
A tooling principle where the compiler enforces central correctness guarantees while the linter guides conventions, style, and higher-level maintainability.
_Avoid_: Convention-only correctness, compiler-as-style-police, no lint layer

**Agent-Friendly Design**:
A language and toolchain design that makes Chimi programs easy for agents to read, modify, trace, and explain without making agents necessary for ordinary programming.
_Avoid_: Agent-dependent design, opaque tooling, undocumented magic

**Maintained Program Readability**:
A design priority where syntax and tooling optimize for real programs that are read, changed, tested, and operated over time rather than short snippets.
_Avoid_: REPL-first terseness, scripting convenience over clarity

**Inspectability**:
The ability to understand what ran, what failed, which effects happened, which resources were used, and why a program behaved the way it did.
_Avoid_: Black-box runtime, guess-and-check debugging, hidden execution

**Module-Qualified Use**:
A naming style where external behavior is called through its module, such as
`Module.function`. Module bindings are explicitly named at import sites rather
than inferred from paths. Explicit member imports may bind public types or
constructors but never erase a function's module origin.
_Avoid_: Wildcard import, invisible behavior origin, unqualified external call

**Explicit Local Reasoning**:
A design principle that favors visible dependencies, effects, failures, and concurrency behavior over terseness or ambient convention.
_Avoid_: Implicit magic, hidden dependency, ambient behavior

**Inference Boundary**:
The point where compiler inference remains acceptable because it improves ergonomics without hiding operational meaning.
_Avoid_: Operationally hidden inference, needless verbosity

**Failure Value**:
A represented error case that is part of a function or service contract rather than an out-of-band exception path.
_Avoid_: Exception path, null sentinel, hidden failure

**Defect**:
A broken invariant, impossible state, or programmer/runtime fault that is not modeled as an expected failure value.
_Avoid_: Expected error, recoverable business failure

**Requirement Contract**:
A callable unit's declared needs from its environment, such as time, logging, persistence, network, configuration, randomness, or queues.
_Avoid_: Ambient global, hidden dependency, framework injection

**Provision Boundary**:
An expression boundary where `using` supplies one or more dependency implementations and removes the corresponding entries from an operation's remaining needs.
_Avoid_: Root-only dependency choice, ambient service lookup, untyped provider map

**Composed Abstraction**:
A reusable behavior or data model built through functions, modules, interfaces, records, enums, or components rather than classical inheritance.
_Avoid_: Inheritance hierarchy, base class, subclass override

**Closed Variant**:
A data shape that can be one of a known set of cases, allowing the compiler to check exhaustive handling.
_Avoid_: Open subtype hierarchy, untracked case, sentinel value

**Optional Value**:
An explicit value that represents presence or absence without using null or nil.
_Avoid_: Null, nil, missing sentinel

**Absence**:
The expected lack of a value, distinct from an operation failing.
_Avoid_: Failure, error, null

**Default Immutability**:
A provisional design preference where values do not change unless mutation is explicitly requested and scoped.
_Avoid_: Implicit mutation, mutable-by-default state

**Visible Effect**:
An operation whose interaction with state, IO, time, randomness, resources, or the outside world is represented in the program model.
_Avoid_: Hidden side effect, ambient mutation, invisible IO

**Concurrent Cancellation**:
Built-in concurrency behavior for stopping work intentionally, including timeout-driven cancellation.
_Avoid_: Ad hoc timeout, orphaned task, framework-only cancellation

**Primary Concurrency Model**:
The blessed concurrency model Chimi programmers use by default, exposed through an official boundary that can evolve without making ordinary service code runtime-specific.
_Avoid_: Fully pluggable concurrency, runtime-specific application code, ad hoc scheduler

**Fault Isolation**:
A durability property where failures are contained to bounded units of work and can be observed, supervised, or recovered without corrupting unrelated execution.
_Avoid_: Whole-process fragility, hidden crash cascade, backend commitment

**Crash Containment**:
A recovery stance where defects stop a bounded unit of work with useful context instead of forcing defensive handling through every caller.
_Avoid_: Defensive plumbing, silent corruption, global crash

**Structured Event Flow**:
An observability model where logs, traces, metrics, failures, causality, and resource context are represented as structured data that can be composed and exported.
_Avoid_: String logging, print debugging, unstructured telemetry

**Composable Observability**:
Observability expressed through ordinary code flow, such as functions or pipelines, rather than attached only through decorators, macros, annotations, or external configuration.
_Avoid_: Decorator-only tracing, annotation-only logging, out-of-band telemetry

**Pipeline**:
A core expression pattern, written with `|>`, for passing a value through readable transformation, validation, failure handling, or observability steps.
_Avoid_: Deep nesting, decorator-only composition, hidden flow

**Block Expression**:
A scoped block that can run local statements and produce a value for use inside a larger expression.
_Avoid_: Statement-only block, temporary outer variable

**Final Expression Value**:
A block-expression value produced by the block's final expression rather than by a block-local `return`.
_Avoid_: Block-local return, hidden function return

**Intentional Variant**:
A named value or function shape that expresses behavioral choice more clearly than a boolean control flag.
_Avoid_: Boolean control flag, positional mode argument

**Pattern-Directed Clause**:
A function or callable case where a matched input shape has its own nearby body instead of one large parameter declaration followed by internal branching.
_Avoid_: Ad hoc overload, giant parameter header, hidden branch body

**Guarded Pattern Clause**:
A pattern-directed clause with an explicit condition that must hold before the clause applies.
_Avoid_: Hidden condition, implicit guard, unmarked branch condition

**Strong Static Typing**:
A type-system stance where program shapes and invalid operations are checked before execution while inference reduces unnecessary annotations.
_Avoid_: Dynamic core, annotation-only explicitness, weak typing

**Compiled Execution**:
An implementation stance where Chimi programs are compiled before running, without committing yet to a specific backend or artifact format.
_Avoid_: Interpreter-first language, backend commitment

**Single-Executable Distribution**:
A build contract where an application, its Chimi dependencies, and the required Chimi runtime are linked into one deployable executable without requiring a separately installed Chimi runtime. Operating-system libraries may remain external and must be reported; fully static linkage is a target-specific capability.
_Avoid_: Runtime bundle, implicit shared-library dependency, universally promised static linkage

**Self-Hosted Compiler**:
The long-term compiler state where a released Chimi compiler is implemented in Chimi and can rebuild itself reproducibly. The initial host compiler is a bootstrap implementation, not part of Chimi's permanent semantics.
_Avoid_: Permanent bootstrap dependency, host-language semantic leak, self-hosting before the language is ready

**Bootstrap Host**:
The temporary implementation language used to build Chimi before the compiler becomes self-hosted. It does not define Chimi's source semantics, backend, runtime, or memory model.
_Avoid_: Runtime target, permanent implementation language, language semantics

**Single Runtime First**:
A runtime strategy where Chimi validates one coherent execution target before optimizing for portability across many backends.
_Avoid_: Backend matrix, premature portability, lowest-common-denominator runtime

**Generational In-Place Deployment**:
A future deployment model where a new program version runs beside the current version, receives new work, and replaces the old version after existing work drains without migrating live execution state.
_Avoid_: Hot code upgrade, live code replacement, state migration

**Uniform Inference**:
A type-system stance where type annotations remain optional by default across both public and private code, avoiding different inference behavior at visibility boundaries.
_Avoid_: Public-only annotation requirement, visibility-dependent type checking

**Inspectable Contract**:
An inferred or annotated callable boundary whose types, failures, requirements, and operational behavior can be viewed through tools and documentation.
_Avoid_: Hidden inferred API, unreadable public surface

**Visible Requirement**:
An inferred or annotated environmental need that remains inspectable and understandable instead of becoming hidden dependency injection.
_Avoid_: Invisible dependency injection, ambient requirement, hidden effect

**Compiler-Derived Documentation**:
Documentation generated from the compiler's understanding of types, failures, requirements, variants, effects, and public module surfaces.
_Avoid_: Comment-only API docs, stale generated docs, hidden inferred contract

**Local-First Package Ecosystem**:
A package-management stance that prioritizes local workspaces, reproducible builds, and curated dependencies before an open public registry.
_Avoid_: Public-registry-first design, uncurated dependency sprawl, package governance before usage

**Private By Default**:
A visibility rule where declarations are module-private unless intentionally exposed.
_Avoid_: Everything public, accidental API, visibility by convention

**Parametric Generic**:
A reusable type or function parameterized over one or more types without weakening static checking.
_Avoid_: Untyped escape hatch, duplicated generic API

**Behavior Composition**:
A mechanism for expressing reusable behavior constraints and capabilities without relying on inheritance.
_Avoid_: Inheritance hierarchy, untyped convention, oversized trait system

**Deferred Advanced Type Machinery**:
Advanced type-system features such as higher-kinded types remain undecided until concrete effect, concurrency, or library examples prove they are necessary.
_Avoid_: Type-system maximalism, premature abstraction, research-first design

**Deferred Extensibility**:
A design stance where macros, compiler plugins, pluggable type systems, and custom toolchain hooks wait until the core language model is proven.
_Avoid_: Plugin-first design, macro escape hatch, extensibility before semantics

**Structured Concurrency**:
A concurrency model where spawned work belongs to a scope with defined cancellation, timeout, failure, and cleanup behavior.
_Avoid_: Orphaned task, invisible background work, ad hoc lifecycle

**Supervised Process**:
A long-lived unit of work with explicit failure, restart, shutdown, and observability behavior, distinct from a scoped task.
_Avoid_: Orphaned daemon, untracked loop, request-local task

**Backpressure**:
Visible capacity or demand behavior that prevents producers from overwhelming consumers, queues, streams, sockets, workers, or fanout paths.
_Avoid_: Hidden buffering, unbounded queue, silent overload

**Bounded Work**:
A design principle where concurrency, queues, resource use, and performance-sensitive operations have visible limits or scopes that programmers can reason about.
_Avoid_: Unbounded queue, unscoped work, invisible resource growth

**Explainable Performance**:
A performance philosophy where the language and toolchain help programmers understand allocations, blocking, queue growth, retries, contention, and slow paths.
_Avoid_: Benchmark-only performance, opaque optimization, unexplained latency

**Reproducible Behavior**:
A tooling and runtime preference where builds, dependency resolution, tests, formatting, generated docs, and diagnosable execution minimize accidental variance.
_Avoid_: Flaky tooling, nondeterministic builds, accidental test variance
