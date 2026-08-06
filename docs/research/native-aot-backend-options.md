# Native AOT backend options for the bootstrap compiler

## Question

What is the smallest viable path from Chimi's typed, backend-neutral
`CoreProgram` to a native object file and one linked executable, and should the
disposable M8 spike use Cranelift or LLVM?

This is a spike recommendation, not the permanent backend decision. Both
candidates still need to run the shared M8 workloads and match the interpreter
oracle before Chimi can compare their measured behavior.

## Short answer

Use **Cranelift for the disposable native-AOT spike**. Its Rust API directly
models independently compiled functions and data, and `cranelift-object` emits
a native `.o` without requiring an installed LLVM distribution. That gives the
spike the shortest path to testing Chimi's actual unknowns: value layout,
runtime calls, relocations, linking, traces, and a single executable.

LLVM remains the stronger control candidate for a permanent optimizing backend:
it supports more targets and has a substantially more complete source-level
debug-information pipeline. Those advantages do not justify paying its bootstrap
and distribution cost before the M8 spike has established Chimi's native ABI.

## Common architecture

Neither backend should consume parser or typing internals. Add one internal
lowering visitor over the existing opaque `BackendProgram`, preserving
`CoreProgram` IDs and source origins, and translate it into backend IR. The
generated application object should:

- export a platform `main` shim;
- use a small, explicit Chimi value ABI;
- call imported `chimi_rt_*` functions for allocation, tracing, dependency
  dispatch, and other services that are not plain arithmetic or control flow;
- leave source semantics and error/needs channels in `CoreProgram`, not in the
  backend adapter.

Build the runtime as a static archive and invoke the platform linker with the
application object plus that archive. Cranelift and LLVM are object generators,
not complete deployment toolchains; producing a target executable still needs
the target linker, startup objects, system libraries, SDK/sysroot, and any
target runtime libraries. LLVM's own cross-compilation guide makes the same
boundary explicit through its separate target triple, sysroot, and linker
configuration ([LLVM cross-compilation](https://llvm.org/docs/HowToCrossCompileLLVM.html)).

For the first spike, support only the host target and record the exact linker
command and dynamic-library dependencies. Cross-compilation is a later test of
the same boundary, not a requirement to solve before emitting the first object.

## Comparison

| Concern | Cranelift | LLVM |
| --- | --- | --- |
| Consuming `CoreProgram` | Lower Chimi functions directly to CLIF with `cranelift-frontend`; the module layer declares functions, data, linkage, and relocations. Cranelift compiles functions independently, so this naturally preserves Chimi's backend boundary ([CLIF reference](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/ir.md), [`cranelift-module`](https://docs.rs/cranelift-module/latest/cranelift_module/trait.Module.html)). | Lower to LLVM IR modules through the LLVM C API. This is also a clean custom-language seam, but requires ownership of LLVM contexts, modules, types, data layout, verification, optimization, and target machines. |
| Native object output | `ObjectModule::finish` resolves module relocations and produces an object; `cranelift-object` is explicitly the module implementation for native `.o` files ([`ObjectModule`](https://docs.rs/cranelift-object/latest/cranelift_object/struct.ObjectModule.html)). | `LLVMTargetMachineEmitToFile` or `LLVMTargetMachineEmitToMemoryBuffer` compiles an LLVM module as an object file ([LLVM C target API](https://llvm.org/doxygen/group__LLVMCTarget.html)). |
| Linking one executable | Declare runtime functions as imports and let the native linker resolve them from a Chimi runtime archive. `cranelift-module` exposes imported, local, hidden, and exported linkage directly ([Cranelift linkage](https://docs.rs/cranelift-module/latest/cranelift_module/enum.Linkage.html)). | Declare runtime functions in LLVM IR and link the emitted object with the same archive. LLD can be embedded, but its documented API still consumes ordinary objects and linker arguments; using the host linker is the smaller spike ([LLD](https://lld.llvm.org/)). |
| Compilation latency | Cranelift is designed for quick machine-code generation and exposes a direct per-function pipeline. This is a design signal, not a Chimi result; the spike must measure lowering, code generation, and linking separately. Cranelift's own project describes it as generating machine code quickly both at runtime and AOT ([Wasmtime/Cranelift](https://github.com/bytecodealliance/wasmtime)). | LLVM exposes optimization levels and a fast instruction-selector switch, but its full optimization pipeline creates more policy and tuning surface ([LLVM C target API](https://llvm.org/doxygen/group__LLVMCTarget.html)). Do not assume it is slower for Chimi until the comparison harness measures equivalent optimization settings. |
| Target support | Current native ISAs are x86-64, AArch64, RISC-V 64, and s390x; Cranelift selects a target dynamically and can omit unused ISAs from the compiler build ([Cranelift ISA API](https://docs.rs/cranelift-codegen/latest/cranelift_codegen/isa/), [Bytecode Alliance target inventory](https://github.com/bytecodealliance/rfcs/blob/main/accepted/pulley.md)). | LLVM supports a much broader object/architecture matrix, including ARM, AArch64, LoongArch, MIPS, PowerPC, RISC-V, SPARC, SystemZ, and X86 for ELF alone ([LLVM code generator](https://llvm.org/docs/CodeGenerator.html)). |
| Runtime and GC freedom | Neither backend mandates a GC, scheduler, failure model, or durability runtime. Both lower calls, memory operations, and control flow; Chimi owns its value representation and runtime ABI. Cranelift's module imports make runtime calls ordinary external symbols. | The same freedom applies. LLVM IR also has garbage-collection infrastructure available, but Chimi is not required to use it; the runtime contract remains a frontend decision. |
| Object caching | Cache the emitted object by a stable hash of `CoreProgram`, target triple/features, backend version, runtime ABI version, and codegen options. Cranelift's independent-function model also leaves a plausible later path to function-granular caching; its codegen crate exposes an optional incremental-cache feature, but the spike should cache final objects first ([Cranelift crate features](https://docs.rs/crate/cranelift-codegen/latest/features)). | Cache emitted module objects using the same key ingredients. Finer-grained reuse requires choosing module partitioning or adding LLVM bitcode/ThinLTO cache policy, so it is unnecessary for the first executable. |
| License and compiler distribution | `cranelift-object` is Apache-2.0 with LLVM exception and is consumed as Rust crates; it writes objects through the Rust `object` library ([upstream crate manifest](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/object/Cargo.toml)). The compiler still needs a linker, but not an installed Cranelift toolchain. | LLVM uses the same permissive license ([LLVM developer policy](https://llvm.org/docs/DeveloperPolicy.html)). Rust's `llvm-sys` requires a compatible LLVM installation at build time; dynamic linkage also requires the shared LLVM library at runtime, while static linkage increases the compiler binary size ([`llvm-sys`](https://docs.rs/crate/llvm-sys/latest)). This affects distribution of the Chimi compiler, not the generated application. |
| Debug and source mapping | CLIF carries source locations and Cranelift can emit unwind data, but `cranelift-object` documents its optional `gimli` support specifically for `.eh_frame`, not a turnkey source-language DWARF pipeline ([object crate manifest](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/object/Cargo.toml), [Cranelift unwind representation](https://docs.rs/cranelift-codegen/latest/src/cranelift_codegen/isa/unwind.rs.html)). The spike should preserve symbol names and emit a Chimi sidecar address/source map; native debugger-quality variable metadata would require additional work. | LLVM has a documented frontend debug-metadata model and emits DWARF or CodeView for native debuggers. It can map source files, functions, variables, and instruction locations through `DIBuilder`/debug metadata ([LLVM source-level debugging](https://llvm.org/docs/SourceLevelDebugging.html)). |
| Cross-compilation | Code generation is cross-target when the desired ISA feature is built, but the final link still needs the target's object format support, linker, sysroot/SDK, startup objects, and libraries. The smaller target inventory is a real ceiling. | Target machines and triples provide broad cross-target code generation, but final linking has the same sysroot/SDK/runtime constraints. LLVM can bundle only selected targets, while LLD includes all of its targets to simplify cross-toolchains ([LLVM build configuration](https://llvm.org/docs/GettingStarted.html), [LLD](https://lld.llvm.org/)). |

## Smallest Cranelift spike

1. Add only `cranelift-codegen`, `cranelift-frontend`, `cranelift-module`,
   `cranelift-object`, and `target-lexicon`; do not embed Wasmtime or its runtime.
2. Define a host-only `NativeTarget` and a versioned C-compatible ABI between
   generated code and a tiny runtime archive.
3. Lower the smallest oracle workload first: constants, arithmetic, direct
   function calls, and the entrypoint. Add heap values, closures, matches,
   dependency calls, failures, and trace events only as demanded by the shared
   workloads.
4. Emit one application object with `ObjectModule`, then link it with the
   runtime archive through the host C compiler driver. The driver supplies the
   platform's startup objects and system-library conventions.
5. Execute the result out of process and compare its terminal value and ordered
   trace events with `BackendProgram::execute_oracle`.
6. Record frontend-to-object time, link time, cold execution time, peak RSS,
   object/executable sizes, imported libraries, and the cache key. A repeated
   build should demonstrate final-object reuse even if fine-grained caching is
   deferred.

## Spike decision rule

The Cranelift spike succeeds if it links a single host executable, matches the
oracle workloads and traces, keeps backend details behind `BackendProgram`, and
produces the required measurements. It does **not** select Cranelift
permanently. After the native and bytecode spikes expose Chimi's real runtime
shape, revisit LLVM only if production-code quality, broader targets, mature
debugging, or optimizer infrastructure outweigh compiler size and integration
cost. That later decision should use Chimi measurements, not either project's
general reputation.

## Implementation note

The completed host spike pins Cranelift 0.131.3 so it remains compatible with
the repository's Rust 1.92 toolchain. Its conformance and measurement results
are recorded in `docs/performance/native-aot-m8-spike.md`.
