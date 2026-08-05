# Single-binary output and compiler-host constraints

## Question

Does requiring Chimi programs to compile into one executable, like Go programs,
constrain the language used to implement the Chimi compiler?

## Finding

Not directly. The compiler implementation language determines how the **compiler
itself** is built and distributed. The emitted program's artifact shape is
determined by Chimi's backend, runtime representation, linker, target ABI, and
native dependency graph.

Go, Rust, Zig, or another systems language can all host a compiler that emits
the same object code or intermediate representation. LLVM makes this separation
explicit: its target-independent code generator translates LLVM IR to machine
code for a selected target, while `llc` selects output architecture using a
target triple and can emit native object files for a later linker
([LLVM code generator](https://llvm.org/docs/CodeGenerator.html),
[`llc`](https://llvm.org/docs/CommandGuide/llc.html)).

The requirement therefore does not eliminate a compiler-host candidate. It does
constrain the architecture that candidate must implement or integrate.

## Define "single binary"

There are two materially different promises:

1. **One deployable executable:** Chimi code and the Chimi runtime are contained
   in one file, but the executable may use OS-provided shared libraries and
   frameworks.
2. **Fully static executable:** the file has no runtime shared-library
   dependencies beyond the kernel/OS ABI.

The first is the useful Go-like product requirement and is feasible across the
major desktop and server platforms. The second is target-specific. Go's own
build tests explicitly skip static linking on Darwin and Solaris, demonstrating
that universal full-static output is not a portable assumption
([Go static-build test](https://go.dev/src/cmd/go/testdata/script/build_static.txt)).

Chimi should promise the first. It can additionally offer fully static output
for supported targets, such as Linux with a compatible static libc or no libc
dependency.

## Runtime and garbage collection

If Chimi uses garbage collection, scheduling, failure propagation, tracing, or
other runtime services, those services can be linked into every executable.
They do not require a separately installed VM merely because they are a
runtime. Go's linker builds an executable from a main package and its
dependencies, and the Go runtime participates in the resulting program rather
than being deployed as a separate VM ([Go linker](https://go.dev/cmd/link/)).

The design consequence is binary size and runtime ABI ownership, not compiler
host selection. Chimi needs a linkable runtime library—or generated equivalents
of its runtime services—and a stable boundary between executable core lowering
and that runtime.

## Native and libc dependencies are the main caveat

Pure Go is the simplest precedent: setting `CGO_ENABLED=0` avoids cgo, while
cross-compiling cgo code requires an appropriate C cross-compiler. cgo can also
record dynamic native-library imports in the final executable
([cgo command](https://go.dev/cmd/cgo/),
[cgo implementation documentation](https://go.dev/src/cmd/cgo/doc.go)).

Rust similarly incorporates Rust crate dependencies into an executable by
default, but C-runtime linkage is target-specific. Some musl targets default to
a static C runtime; `crt-static` only affects targets that support switching,
and the Rust reference recommends inspecting the produced binary because an
unsupported target may ignore the request
([Rust linkage reference](https://doc.rust-lang.org/stable/reference/linkage.html#static-and-dynamic-c-runtimes)).
Cargo build scripts may still request dynamic native libraries, so a single
self-contained artifact requires every native dependency to have a compatible
static form
([Cargo native linking](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-lib)).

Zig shows another viable backend/toolchain model. Its standard library does not
require libc, it demonstrates small static executables, and its toolchain ships
libc support for cross-target builds. Its examples also show the distinction:
GNU-libc targets can produce dynamically linked executables while musl targets
can produce static ones
([Zig overview](https://ziglang.org/learn/overview/)).

For Chimi, FFI and system integration must therefore be designed with artifact
guarantees in mind. A native extension can silently turn a self-contained
program into one that requires a shared library unless the compiler diagnoses
or records that dependency.

## Cross-compilation

Cross-compilation is likewise a backend and link-toolchain property. Pure Go
selects targets with `GOOS` and `GOARCH`; cgo adds the need for a target C
compiler ([Go source installation and targets](https://go.dev/doc/install/source),
[cgo command](https://go.dev/cmd/cgo/)). Rust can install a target standard
library and build with `--target`, but `rustup` generally does not supply the
target linker, SDK, or sysroot
([rustup cross-compilation](https://rust-lang.github.io/rustup/cross-compilation.html)).
Clang is natively a cross-compiler, but its documentation still calls out the
need for the correct target triple, sysroot, libraries, and headers
([Clang cross-compilation](https://clang.llvm.org/docs/CrossCompilation.html)).

Zig bundles more of that cross-toolchain surface and can compile supported
targets independently of the host, including libc for documented targets
([Zig platform support](https://ziglang.org/learn/platform-support/)). This is a
toolchain-distribution advantage, not evidence that Chimi's compiler must be
implemented in Zig.

## Implications for compiler-host selection

The host shortlist should not be filtered by whether a trivial program in the
host language itself produces one executable. Instead, score candidates on:

- ability to emit or integrate the chosen backend representation;
- control over linking and target triples;
- feasibility of embedding or shipping Chimi's runtime as static code;
- access to target SDKs, sysroots, linkers, and object formats;
- whether compiler dependencies complicate distributing the **compiler**;
- iteration speed, profiling, fuzzing, memory control, and implementation
  complexity.

Using LLVM may make the compiler distribution larger or require packaging LLVM,
depending on whether it is linked or invoked externally. That does not make
Chimi-produced programs depend on LLVM: the final executable contains the
generated machine code plus whatever Chimi runtime and native libraries the
link step selects.

## Recommendation

Keep all credible compiler-host candidates. Add these output requirements to
the backend/runtime evaluation instead:

- `chimi build` emits one executable containing application code and the Chimi
  runtime;
- no separately installed Chimi VM or runtime is required;
- OS system-library dependencies are allowed and reported;
- fully static output is a target capability, not a universal guarantee;
- native/FFI dependencies that weaken the artifact guarantee are inspectable;
- supported cross-target builds define their required linker, SDK, sysroot, and
  libc explicitly.

This preserves the Go-like deployment goal without coupling Chimi's compiler
implementation language to its program execution model.
