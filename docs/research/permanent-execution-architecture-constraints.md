# Permanent Execution Architecture Constraints

## Question

What do primary implementations establish about choosing native AOT through
Cranelift versus an embedded bytecode interpreter for Chimi's permanent
execution architecture, given the requirements for one deployable executable,
future recovery and durability, caller-configured tracing, cancellation,
cross-target builds, cacheable artifacts, and useful debugging?

## Repository evidence

The two M8 candidates already answer the local feasibility question. Both
consume the same opaque `BackendProgram`, link the Chimi runtime into one host
executable, reproduce all five oracle outcomes and all three trace modes, and
leave independently cacheable application and runtime artifacts before the
final link ([oracle contract](../performance/backend-m8-oracle-baseline.md),
[native measurements](../performance/native-aot-m8-spike.md),
[bytecode measurements](../performance/embedded-bytecode-m8-spike.md)).

Their linked size, cold startup, RSS, and current hot execution are effectively
tied because both prototypes spend most of their time in the same boxed runtime.
The bytecode image is smaller and cheaper to encode, but this experiment does
not measure either candidate's performance ceiling. The permanent decision must
therefore turn on architectural obligations rather than the tied microbenchmark.

## Finding

Use **Cranelift native AOT as Chimi's first production execution backend**, with
all concurrency, supervision, durability, cancellation, outcomes, and canonical
events owned by a backend-neutral runtime contract. Do not make an embedded
bytecode VM part of the initial production architecture merely to obtain those
runtime capabilities: primary implementations show that they work over native
code as well.

Keep the opaque backend boundary capable of admitting a portable bytecode
backend later. A bytecode backend earns that maintenance cost when Chimi needs a
target that Cranelift cannot serve, a portable distribution image independent
of a target ISA, or an interpreter-specific tooling mode. It should not become
the semantic source of truth; `CoreProgram`, stable origins, and the runtime
contract already fill that role.

## The closest direct precedent: Cranelift plus a portable interpreter

Wasmtime now has essentially the same architectural pair. It normally compiles
to native code with Cranelift on supported targets and uses Pulley, a portable
bytecode interpreter, on targets Cranelift does not support. Pulley is presented
as another execution strategy without changing Wasmtime's public API or most of
its internals ([Pulley documentation](https://docs.wasmtime.dev/examples-pulley.html),
[accepted Pulley RFC](https://github.com/bytecodealliance/rfcs/blob/main/accepted/pulley.md)).

That precedent establishes both sides of the tradeoff:

- native compilation provides the near-native performance target;
- an interpreter extends execution to platforms on which its implementation
  language can run, without adding a machine-code backend for every ISA;
- bytecode does not automatically minimize startup or maximize throughput.
  Pulley preprocesses into a register-oriented format because directly
  interpreting a compact stack format would reduce preparation work while
  increasing opcode-dispatch cost;
- a fast interpreter grows compiler-like machinery: register allocation,
  peephole optimization, superinstructions, encoding design, validation,
  disassembly, and profiling support.

This makes bytecode a credible **portability strategy**, not a free runtime
architecture. Shipping both strategies would mean owning and differentially
testing two lowerings or sharing a common optimizing pipeline as Pulley does.

## Durability and recovery are above instruction dispatch

Erlang/OTP defines fault recovery through processes and supervision trees:
supervisors monitor workers and restart them according to explicit strategies
([OTP design principles](https://www.erlang.org/doc/system/design_principles.html)).
On supported JIT builds, modern ERTS converts BEAM instructions to native code
at load time. The BeamAsm documentation says the runtime remains largely
unchanged apart from places that directly inspect loaded instructions, such as
tracing and code loading
([BeamAsm](https://www.erlang.org/docs/27/apps/erts/beamasm.html)). This
is direct evidence that BEAM-like scheduling and recovery do not depend on
interpreting bytecode.

Durable execution across process or machine failure adds a separate invariant.
Temporal persists an append-only event history to recover application state,
and replays deterministic workflow code by comparing newly emitted commands
with recorded events. Nondeterministic external operations are kept outside the
replay path or have their results recorded
([events and recovery](https://docs.temporal.io/workflow-execution/event),
[deterministic replay](https://docs.temporal.io/workflow-definition#deterministic-constraints)).
Temporal exposes this model through SDKs for several ordinary compiled and VM
languages rather than requiring one continuation representation.

The architectural inference for Chimi is:

- persist semantic events, effect results, stable operation identities, program
  version, and checkpoint state at runtime-defined suspension boundaries;
- make replay determinism and code-version compatibility explicit contracts;
- do not persist raw native stacks, host pointers, or bytecode PCs as the
  durable protocol;
- treat an in-memory continuation representation as a runtime implementation
  detail that may differ between native and interpreted backends.

This design works with either candidate and remains portable across backend
changes.

## Cancellation and scheduling require explicit safepoints

An interpreter has a convenient dispatch point at which to decrement a budget
or inspect cancellation. That convenience does not make cancellation exclusive
to bytecode. Wasmtime instruments Cranelift-compiled native code for two forms
of interruption: deterministic fuel and lower-overhead epoch deadlines. On
interruption, execution can trap or asynchronously yield to the host
([Wasmtime interruption](https://docs.wasmtime.dev/examples-interrupting-wasm.html)).

Chimi therefore needs a backend-neutral safepoint policy. Both backends should
observe cancellation and scheduler budgets at equivalent semantic locations,
initially calls, loop backedges, dependency/effect boundaries, and allocation
slow paths. The native lowering inserts polls; an interpreter checks at dispatch
or selected opcodes. Tests must verify bounded cancellation latency, and
benchmarks must measure disabled and enabled poll cost. Terminating an OS thread
is not a substitute because it bypasses Chimi outcomes, cleanup, and terminal
events.

## Observability belongs to the runtime contract

The M8 exact-trace comparison already proves that either dispatch strategy can
emit Chimi's canonical events. External precedents explain the implementation
tradeoff:

- Lua can enable call, return, line, and instruction-count hooks at the VM
  execution layer, and disable them entirely with a zero mask
  ([Lua 5.4 debug hooks](https://www.lua.org/manual/5.4/manual.html#4.7)).
- BeamAsm preserves tracing over native code with patchable function prologues
  and shared breakpoint fragments rather than retaining interpreter dispatch
  ([BeamAsm tracing](https://www.erlang.org/docs/27/apps/erts/beamasm.html#tracing-and-nif-loading)).
- Wasmtime can emit DWARF for native debugging and still provides runtime-level
  interruption and profiling controls over compiled guest code
  ([Wasmtime configuration](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.debug_info)).

Native observability takes deliberate metadata and instrumentation, but it does
not require full tracing on the hot path. Chimi's accepted `off`, `boundaries`,
and `full` modes should remain caller-owned runtime policy. Stable semantic and
source origins must survive lowering; machine addresses and bytecode offsets
may index metadata but must not become canonical event identity.

## Portability and cross-compilation

Cranelift selects a target ISA dynamically and describes itself as a
cross-compiler. Its currently exposed native backends cover a finite set of
architectures, and individual ISAs can be omitted from a compiler build
([Cranelift target selection](https://docs.rs/cranelift-codegen/latest/cranelift_codegen/isa/index.html)).
`cranelift-object` emits ordinary target object files and resolves their
relocations before handing the object to the linker
([`ObjectModule`](https://docs.rs/cranelift-object/latest/cranelift_object/struct.ObjectModule.html)).

A Chimi bytecode image can be ISA-neutral, but the promised deployable artifact
cannot: the embedded interpreter, runtime, startup objects, system interfaces,
and executable format still target a specific OS and architecture. Pulley makes
even this portable layer distinguish pointer width and endianness. Bytecode
therefore removes application code generation for each native ISA; it does not
remove the target runtime and link toolchain.

The product contract should be:

- one executable per supported target, containing the application and required
  Chimi runtime;
- no separately installed Chimi VM or package tree;
- an explicit target support matrix covering code generation, runtime, linker,
  SDK/sysroot, object format, and system libraries;
- a future portable-bytecode fallback only where its target coverage justifies
  the second backend.

## Artifact caching and trust

Both candidates naturally preserve a link boundary between application code or
image and a reusable runtime archive. Native cache entries must include at least
the normalized backend input, target triple, CPU features, Cranelift and Chimi
compiler versions, code-generation options, and runtime ABI version. Wasmtime's
cache and precompilation APIs likewise separate compilation from execution and
require target-aware configuration
([fast compilation](https://docs.wasmtime.dev/examples-fast-compilation.html),
[precompilation](https://docs.wasmtime.dev/examples-pre-compiling-wasm.html)).

A bytecode cache can reuse the application image across targets sharing the
format's width, endianness, and runtime contract, while caching the target
runtime separately. It still requires a bytecode-format version, semantic ABI
version, validation rules, and integrity checking. Precompiled artifacts are a
trust boundary: Wasmtime explicitly treats deserialization of compiled modules
as unsafe because incompatible or malicious bytes can violate runtime
assumptions ([module serialization](https://docs.wasmtime.dev/examples-serialize.html)).
Chimi should validate any externally loaded bytecode; compiler-produced bytes
embedded during the same trusted build can use a narrower verified path.

## Debugging and source maps

Bytecode offers a stable logical program counter, and mature VMs use the pair of
method and instruction location for breakpoints, stepping, and source-line
mapping ([JVM Tool Interface](https://docs.oracle.com/en/java/javase/26/docs/specs/jvmti.html#Breakpoint)).
This makes an interpreter debugger straightforward to bootstrap, but native
debuggers otherwise see the interpreter loop unless the VM builds its own
logical stack and tooling protocol.

Native AOT gives profilers real application functions, but Chimi must emit and
retain the mapping from machine ranges back to stable Chimi origins. Cranelift
has source-location, trap, call-site, stack-map, and unwind metadata primitives;
its object emitter is not by itself a complete Chimi debugging experience
([Cranelift IR metadata](https://docs.rs/cranelift-codegen/latest/cranelift_codegen/ir/index.html),
[machine-code metadata](https://docs.rs/cranelift-codegen/latest/src/cranelift_codegen/machinst/buffer.rs.html)).
BEAM's native JIT similarly adds frame-pointer and profiler metadata to make
native call graphs useful.

The common requirement is a versioned debug map keyed by stable function and
origin IDs. Native builds map instruction ranges to those IDs; bytecode builds
map byte offsets to them. Stack traces, traces, and diagnostics should resolve
through that shared semantic map.

## One executable versus fully static output

Neither dispatch strategy changes the final link dependency graph. A single
distributable executable may still depend on OS system libraries. Full static
linkage depends on the target C runtime and every native dependency; unsupported
targets can ignore a static-CRT request, so the result must be inspected
([Rust linkage reference](https://doc.rust-lang.org/stable/reference/linkage.html#static-and-dynamic-c-runtimes)).
Go's own static-build test skips Darwin and Solaris because those targets do not
support that static-linking mode
([Go static-build test](https://go.dev/src/cmd/go/testdata/script/build_static.txt)).

Chimi should promise one deployable executable on supported targets, report its
native dependencies, and promise fully static output only on target profiles
that test and enforce it. This requirement does not favor native AOT or embedded
bytecode.

## Decision consequences

Selecting native AOT first means:

- `CoreProgram -> BackendProgram -> native object` remains a narrow lowering
  seam, while the runtime ABI owns values, tasks, channels, scheduling,
  cancellation, tracing, replay, and supervision;
- generated code contains explicit safepoints and calls runtime operations
  through a versioned ABI;
- application objects, debug maps, and the runtime archive remain independent
  cache entries before the final link;
- target support is initially limited by Cranelift plus the complete target
  link/runtime toolchain;
- the compiler must implement source-level debug metadata and cannot delegate
  that obligation to Cranelift alone.

Rejecting bytecode as the initial production default means accepting less ISA
coverage until a second backend is justified. In return, Chimi avoids owning a
production VM, bytecode specification, verifier, disassembler, debugger, and a
second performance pipeline before those costs solve a measured need.

## Required follow-up proofs

The primary-source evidence narrows the architecture but does not replace Chimi
measurements. Before calling the native runtime production-ready, verify:

1. cooperative scheduling and cancellation latency with inserted safepoints;
2. deterministic replay from persisted semantic events across process restart;
3. source-mapped defects, native stack traces, and profiler symbols;
4. per-module object cache hits across isolated edits and invalidation when any
   target, backend, compiler, or runtime-ABI input changes;
5. one executable and inspected dependency manifests on each supported target;
6. a representative allocation/value-layout optimization showing that native
   lowering can exceed the boxed-helper prototype's current ceiling;
7. runtime ABI versioning that rejects incompatible cached objects and durable
   execution versions safely.
