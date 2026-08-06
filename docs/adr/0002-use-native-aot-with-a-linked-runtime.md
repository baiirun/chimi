---
status: accepted
---

# Use native AOT with a linked Chimi runtime

Chimi programs will compile ahead of time to native object code and link with
the Chimi runtime into one deployable executable. Native AOT is the permanent
execution architecture; Cranelift is the first code generator, behind the
existing opaque `BackendProgram` boundary. The code-generator library is an
implementation choice and may be replaced without changing Chimi source,
`CoreProgram`, the runtime contract, or the application artifact model.

The decision favors Chimi's performance ceiling and direct host integration.
The native and embedded-bytecode candidates both reproduced all five oracle
workloads exactly in off, boundary, and full tracing modes. Bytecode produced
12–21% smaller application-specific intermediate objects, but linked size,
cold startup, peak RSS, and hot execution were effectively tied because both
prototypes spend most work in the same boxed helper runtime. Those tied
measurements do not demonstrate tied optimization ceilings: native lowering
can remove dispatch, specialize representations, inline calls, and lower pure
control flow directly, while bytecode permanently pays an instruction-dispatch
boundary unless a second native compiler or JIT is added.

Evidence: [backend oracle](../performance/backend-m8-oracle-baseline.md),
[native AOT spike](../performance/native-aot-m8-spike.md), and
[embedded-bytecode spike](../performance/embedded-bytecode-m8-spike.md).
Primary implementation precedents and constraints are collected in
[permanent execution architecture constraints](../research/permanent-execution-architecture-constraints.md).

## Compiler and runtime boundary

The compiler consumes verified `CoreProgram` only through opaque
`BackendProgram`. The first implementation is a concrete native lowering
module, not a public backend plugin interface. Generated code owns ordinary
program control flow and statically known value operations. It calls a narrow,
versioned, compiler-private runtime ABI only for capabilities that require
shared runtime policy, including:

- allocation and future memory-management slow paths;
- scheduling, supervision, and runtime-owned child work;
- dependency dispatch and external-operation boundaries;
- expected-failure, defect, and cancellation integration where control crosses
  the runtime boundary;
- cancellation and durability safepoints;
- computation context, trace events, and exporters.

The prototype's helper call for every Core node is not the production ABI.
Arithmetic, branching, calls, and representation-specialized operations should
lower directly when the runtime does not need to mediate them. This keeps the
ABI narrow and leaves optimization decisions local to native lowering.

The runtime ABI is versioned with the compiler release but is not a stable FFI
surface for user code. Runtime metadata retains stable computation, source,
semantic, and Core identities needed for diagnostics, traces, cancellation,
and recovery without exposing backend instruction identities to the language.

## Artifacts and linking

`chimi build` produces native application objects, immutable runtime metadata,
and a Chimi runtime archive before the final platform link. The linker combines
those artifacts, compiled Chimi dependencies, and statically available native
dependencies into one deployable executable. A separately installed Chimi VM,
runtime, or package tree is not required.

Compilation-unit objects and metadata remain independently cacheable. Their
cache keys include the verified Core content, target triple and CPU features,
code-generator identity and version, optimization/debug settings, and runtime
ABI version. Trace mode is caller-owned execution configuration and does not
create different application objects. The first implementation may emit one
application object while compilation-unit partitioning is established; final
linking must not require whole-program code generation.

Build output reports the target, runtime ABI, code-generator configuration,
linked native inputs, and remaining dynamic-library dependencies. Chimi
guarantees one deployable executable on supported targets. Fully static output
is a separately reported target capability and is guaranteed only when the
target ABI, system runtime, and complete native dependency graph support it.

## Observability and cancellation

The accepted `off`, `boundaries`, and `full` tracing policy remains runtime
configuration supplied by the caller. Native code preserves stable semantic
event identities. Off mode bypasses event construction, buffering,
serialization, and export; it does not select a different compiled program.

Cancellation is cooperative at compiler-inserted safepoints and runtime
boundaries. Calls, allocation slow paths, backward control-flow edges, runtime
operations, and scheduler handoffs must provide enough safepoints to bound
cancellation latency. Expected failures remain typed values; defects and
cancellation remain separate runtime outcomes.

## Durability and recovery

BEAM-like recovery is a runtime concern, not a reason to make bytecode the
execution format. Durable state records stable computation identity, versioned
values, external-operation results, and logical checkpoint or journal position.
It must not serialize native addresses, stack frames, heap layout, register
state, or code-generator instruction positions. Recovery loads a compatible
compiled artifact and reconstructs execution from a semantic checkpoint or
replay boundary.

This rule also rejects bytecode program counters as Chimi's durable protocol.
Changing either native lowering or bytecode layout must not invalidate durable
state by itself. Hot code replacement is outside this decision; compatibility
and migration between different program versions require a later durability
contract.

## Rejected alternative

An embedded bytecode VM was viable and preserves the single-executable and
observability contracts. It offers cheap encoding, compact intermediate images,
uniform instrumentation, and a simpler portable execution engine. It is not the
selected production path because Chimi makes performance a core design goal,
does not currently require dynamic code loading, and would otherwise accept a
dispatch ceiling or eventually build a second JIT/AOT tier.

The existing interpreter remains a conformance oracle. The bytecode prototype
remains decision evidence, not a supported fallback runtime or durable artifact
format. A future sandboxing, dynamic-loading, or portability requirement may
reopen bytecode through a separate decision with its own measured workloads.

## Consequences and follow-up risks

- Native code generation and linking require an explicit supported-target
  matrix, linker/sysroot/SDK discovery, and dependency reporting.
- Cranelift's code quality, target coverage, and source-debug information must
  pass Chimi-specific gates. LLVM remains a replacement candidate if measured
  optimization, debugging, or target requirements exceed Cranelift's ceiling.
- The current boxed runtime does not answer value layout, memory management,
  scheduler, stack growth, or runtime ABI granularity. Representative programs
  must prove direct lowering and safepoint costs before freezing that ABI.
- Cache invalidation and reproducible linking require stable Core hashing,
  deterministic metadata, explicit runtime ABI versions, and recorded native
  toolchain inputs.
- Durability still needs a versioned value format, checkpoint/journal protocol,
  idempotent external-operation boundary, artifact retention policy, and
  program-version compatibility rules.
- Full tracing remains expensive in the prototypes. Runtime work must preserve
  the zero-event off path and measure boundary/full export costs under realistic
  concurrent workloads.
