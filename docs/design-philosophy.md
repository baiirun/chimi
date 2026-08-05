# Chimi Design Philosophy

Chimi is a service-first, general-purpose-over-time programming language. Its first proving ground is durable networked services, but the language should not permanently narrow itself to backend services.

The early design phase optimizes for one primary user building real programs. Chimi is free to break while the model is being proven.

## Use Cases

Chimi should first prove itself against:

- HTTP API services with persistence, background work, retries, tracing, and graceful shutdown
- Background worker services with queues, retries, poison-job handling, observability, and safe shutdown
- Real-time coordination services with long-lived connections, shared state, fanout, backpressure, latency, and cancellation
- CLI tools as a secondary use case to keep startup, IO, config, errors, and distribution honest

## Non-Goals

Chimi should not optimize early design for:

- Frontend browser apps centered on DOM UI, bundling, hydration, and frontend framework patterns
- Embedded, kernel, firmware, bare-metal, or hard real-time programming
- Beginner-first adoption, mass-market ergonomics, or public ecosystem growth

## Principles

### 1. Service-First, General-Purpose Over Time

Chimi starts with durable networked services because they force hard decisions about failure, concurrency, observability, performance, deployment, and tooling. Those decisions should be made in ways that leave room for broader general-purpose programming later.

### 2. Expressive Clarity

Chimi should be more expressive than Go, not simpler in the minimalistic sense. Pattern matching, closed variants, failure values, pipelines, block expressions, requirement contracts, and visible effects are acceptable complexity when they make real programs clearer.

The goal is not fewer concepts. The goal is concepts that compose predictably and remain readable.

### 3. Small Language, Service-Rich Standard Library

The core language should start small and earn new constructs through concrete programs. Service-oriented operational primitives can live in the official standard library when syntax would be too heavy.

The standard library is still part of the official Chimi experience, not just another package.

### 4. Explicit Local Reasoning

A reader should be able to understand dependencies, effects, failures, module origins, and concurrency behavior without chasing hidden imports, ambient globals, framework conventions, or runtime magic.

Chimi should require module-qualified behavior use, such as `Module.function`.
Every module qualifier should be explicitly declared at its import site rather
than inferred from the imported path.
Explicit member imports may bind public types or constructors where their names
carry the relevant identity, but must not erase a function's module origin at
the call site.

Dependency implementations may be supplied at any expression boundary. A
runtime root closes and validates the remaining dependency needs rather than
being the only place where dependencies can be chosen.

### 5. Inference Without Hidden Operations

Type annotations should be optional by default across both public and private code. Chimi should avoid different inference behavior at visibility boundaries.

That does not mean contracts should be hidden. The toolchain should make inferred types, failures, requirements, effects, and public module surfaces easy to inspect through editor support, documentation, and compiler output.

### 6. Failures Are Values; Defects Are Contained

Expected failure should be represented as values that are part of callable contracts. Absence should be modeled separately from failure, and null/nil should not be part of the core model.

Broken invariants, impossible states, and programmer/runtime faults are defects. Defects should crash a bounded unit of work with useful context, not become defensive plumbing through every caller.

### 7. Safe Control, No User-Visible Unsafety

Chimi should give programmers explicit control over performance, resources, and execution behavior without exposing unsafe keywords, unsafe packages, or unchecked operations to ordinary code.

The performance model should be a ladder: ordinary code uses managed ergonomic defaults, while performance-sensitive code can opt into stricter safe controls.

### 8. One Primary Concurrency Model

Chimi should have one blessed concurrency model first. That model should support structured concurrency, cancellation, timeouts, supervision for long-lived processes, backpressure, and bounded work.

The concurrency surface should be designed behind an official boundary so the runtime can evolve, but application code should not become runtime-specific.

### 9. Inspectability

When a program behaves unexpectedly, Chimi should help answer what ran, what failed, which effects happened, which resources were used, and why.

Observability should be structured, composable, and exportable. Logs, traces, metrics, failures, causality, and resource context should not collapse into string logging.

### 10. Explainable Performance

Performance should be observable and explainable, not only benchmark-fast. The language and toolchain should help identify allocations, blocking, queue growth, retries, contention, and slow paths.

Concurrency, queues, and resource-sensitive operations should have visible limits or scopes where practical.

### 11. Integrated Toolchain

Chimi should have canonical official tools for formatting, building, testing, documenting, packaging, analyzing, and editing programs.

The formatter should be non-negotiable and canonical. The compiler should enforce central correctness guarantees. The linter should guide style, conventions, and higher-level maintainability.

### 12. Agent-Friendly, Not Agent-Dependent

Chimi should be easy for agents to read, modify, trace, and explain. Canonical formatting, module-qualified use, inspectable contracts, teaching diagnostics, structured observability, and stable tooling APIs all support that.

Ordinary programming should not require agents to be understandable.

### 13. Defer Extensibility Until the Core Works

Macros, compiler plugins, pluggable type systems, custom toolchain hooks, and advanced type machinery should wait until concrete programs prove they are necessary.

Libraries and official tooling should carry the early design before the language grows extension mechanisms.

### 14. Local-First Ecosystem

Package management should first solve local workspaces, reproducible builds, dependency boundaries, and curated dependencies. An open public registry and ecosystem governance can wait.

### 15. Single-Executable Distribution and Eventual Self-Hosting

The blessed application build should produce one deployable executable containing
the application, its Chimi dependencies, and the required Chimi runtime. It
should not require a separately installed Chimi VM, runtime, or package tree.
Operating-system libraries may remain external when required and must be
reported; fully static linkage is a target-specific capability rather than a
universal promise.

This deployment contract does not make compilation monolithic. The compiler
should compile modules independently, retain cacheable intermediate artifacts,
and perform a final link into the executable. Library artifact formats may be
added when concrete interoperability needs justify them.

The released compiler should eventually be implemented in Chimi, distributed
as one executable, and able to rebuild itself reproducibly. The first compiler
host is a bootstrap choice and must not define Chimi semantics or leak its
object model into compiler representation contracts.

## Open Philosophy Questions

- How much of the performance ladder should be ownership-like, region-like, compiler-mode-based, or library-based?
- Where exactly should the line sit between language syntax and standard-library operational concepts?
- What is the concrete shape of behavior composition: interfaces, traits, protocols, modules, or another mechanism?
- How much advanced type machinery is needed once effects and concurrency become concrete?
- What is the first runtime target?
