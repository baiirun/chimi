# C or Rust for the Native Runtime

Byron asked (2026-11-23) whether C is the right language for Chimi's native
runtime, or whether the runtime should use Rust as far as is reasonable. This
record lays out what the repository shows, the options, and a recommendation.
It changes no code. The decision itself goes in `decisions.md` once it is
made.

## Decision (2026-11-24)

The runtime moves to Rust. Code stays C only with a good reason written here,
such as a measured hot-path cost that Rust cannot match. Byron first took the
split recommended below, then revised it the same day: "Rust everything unless
we have a _good_ reason for it to be C." The heap, collector and intrinsic
dispatch are no longer kept in C by default; they move too, unless a gate
below fails and the measurement is recorded as the reason.

### What stays C, and why

| Code | Reason |
| --- | --- |
| Vendored SQLite and mbedTLS | Upstream is C. Rewriting them is not on the table, and their maintainers ship C. |
| The G20 binding shims (`net`, `tls`, `exec`, `sqlite`, `postgres`, `terminal`) | For now. Each shim is thin glue against a C library's headers, compiled in the same `cc` pass and per-package archive as that library. A shim with no C library under it (`net`, `exec`, `terminal`) has no such reason and moves to Rust in step 4; all three have (CHI-309), into `runtime/chimi-runtime/src/binding/`. |

Nothing else has a recorded reason. Anything added to this table needs one.

### Migration plan

Each step is one or more PRs. The size and speed gates hold for every step.

0. **Hardening (CHI-304).** ASan and UBSan over the program outputs,
   ThreadSanitizer required on Linux, fuzz targets for JSON, text, decimal and
   each shim. These are the oracle every later step is checked against, so
   they land first. Touches CI and tests only.
1. **One Rust runtime archive.** A runtime crate that `chimi-fiber` joins (or
   that joins it), so every program links one Rust archive with one copy of
   the standard library. The part every program links is `no_std` plus
   `alloc` with `panic = "abort"`, so a program with no concurrency stays
   within the size gate. The build embeds the prebuilt archive as it does the
   fiber one; the per-project `cc` fallback for `runtime.c` goes away when
   the last C leaves. Ship it first with nothing moved, to measure the
   baseline cost.
2. **Leaves.** JSON, then decimal and duration, then text and graphemes, then
   random, metrics and telemetry export, files and console. Each allocates
   through the C allocator by callback until step 5. One subsystem per PR,
   with differential fuzzing against the C version, which is deleted after.
   These can start before the multi-core work ends where they do not overlap
   the code it is changing.
3. **Scheduler protocols** (after the multi-core work settles): channels,
   select and the process-kernel glue move next to the Rust scheduler, so
   each protocol has one owner in one language.
4. **Handles and the binding call path:** the handle table, `BindingCall`,
   and the shims with no C library under them. The shims (CHI-309 part 1)
   and the handle slots (part 2) have moved. `BindingCall`'s argument
   readers and value and result builders stay C for now: each reads
   `ChimiValue` or allocates on the C heap, so they move with it in step 5.
   The native handle counts moved too (part 2, step 2).
5. **Heap, collector, values, intrinsics and dispatch, bottom-up**
   (planned 2026-11-26, CHI-305; inventory in
   [runtime-c-inventory.md](runtime-c-inventory.md)). The first plan moved
   them as one unit so that allocation would keep inlining into the
   intrinsics. It does not inline today: built with the runtime's flags
   (`-std=c11 -O2`), `allocate`, `new_value` and `new_control` are
   out-of-line functions with over 200 call sites, so a Rust `allocate`
   swaps a direct C call for a direct cross-language call in the same
   static link. Cross-language LTO is rejected: it needs a clang and lld
   matching rustc's LLVM on every machine that links a program, which
   Apple's clang cannot promise, for a cost the measurement says is absent.
   The generated code inlines no value layout either, so `ChimiValue` can
   change. The phases, one PR each unless noted, each within the gates
   (`scripts/runtime-gate.py`):
   0. Prerequisites. The heap and value structs get `#[repr(C)]` Rust
      mirrors whose sizes, offsets and scalar kinds are asserted on both
      sides (done 2026-11-26: `chimi-runtime/src/heap/layout.rs`, and a
      generated block of `_Static_assert`s in `runtime.c`; the structs stay
      in `runtime.c` rather than an internal header, since a header would
      add a build input and cache key for nothing until Rust owns them); the collector gets an oracle (memory-report counters and
      program outputs identical to the C, a verify pass after each sweep
      under ASan, poisoned reset chunks); `handle_count_lock` moves into
      Rust as a pthread mutex ThreadSanitizer still sees. Civil dates and
      timestamps move as a missed step 2 leaf.
   1. The allocator: small blocks, areas and footprint, then young chunks,
      `allocate` and allocation scopes (two PRs). Watch the thread-local
      carrier read, which C does with an initial-exec access.
   2. The collector: mark, forward, promote and sweep.
   3. Fragment copy, the transfer walk, and the task and process heap
      lifecycle; handle drops become Rust-to-Rust calls.
   4. Values: scalar and text builders with environments, the list vector,
      and the AVL map together with structural compare (three PRs).
   5. Intrinsics by family, pure ones first, then task, select, clock, fs,
      console and stdin, each family taking its C lock with it (six to eight
      PRs).
   6. The binding argument readers and value and result builders.
   7. Dispatch: telemetry emission, then the node helpers with `run_node`,
      `invoke` and the loop passes in one PR (splitting them would put a
      cross-language call on every node), then match.
   8. Startup, CLI parsing, signals and reports.
   Soundness in Rust: collection runs only at pass boundaries and scope
   ends, with explicit roots, so values carry a pass lifetime that a
   collection ends, allocation goes through a context that owns the target
   heap, and a parked fiber reloads its carrier from that context. If a
   phase misses the 2% gate and the gap cannot be closed, the measurement
   goes in the table above as the reason that code stays C.
6. **Delete `runtime.c`.** `runtime.h` becomes the generated or hand-kept C
   view of the Rust ABI that the code generator targets.

Gates for every step: the stripped size of a program without concurrency
(345 KB today) grows by at most 50 KB over the step-1 baseline; `loop-bind`,
the map loop and the HTTP benchmark regress by no more than 2%; cold
`chimi build` time is unchanged; differential fuzzing and the sanitizer jobs
are green.

## Original recommendation (superseded)

C was never chosen against Rust. No document compares them: `runtime.c` came
in with the native AOT spike (ADR 0002, commits `f0c97be` and `ec51403`), and
ADR 0002 is about native code against bytecode, not about the runtime's
language. Since then the runtime has become a mix anyway: the scheduler and
fibers are Rust (`runtime/chimi-fiber`), and the heap, values, intrinsics and
host operations are C.

The recommendation is a split by kind of code, not a rewrite:

1. Harden the C now (sanitizer CI and fuzzing). This is cheap, does not touch
   the files the multi-core work is changing, and any later migration needs it
   as its correctness oracle.
2. After the multi-core work settles, write new shared-state and concurrency
   code in Rust only, and move the C halves of the scheduler protocols
   (channels, select, process-kernel glue) next to the Rust scheduler, so each
   protocol has one owner in one language.
3. Move the leaf code that parses untrusted bytes (JSON, decimal, duration,
   text and graphemes) to Rust one subsystem at a time, behind the existing
   runtime ABI, each passing size, speed and differential-fuzz gates.
4. Keep the heap, the collector, the intrinsic dispatch and the bump paths in
   C, unless the bug record later says otherwise.
5. Keep vendored C libraries (SQLite, mbedTLS) and their thin G20 shims in C.

## What is there today

| Part | Language | Size |
| --- | --- | ---: |
| `compiler/chimi/runtime/native/runtime.c`, `runtime.h`, `chimi_binding.h` | C | about 14,800 lines |
| G20 binding shims (`tls`, `postgres`, `sqlite`; `exec`, `net` and `terminal` moved to Rust in CHI-309) | C | about 3,000 lines before the move |
| Vendored SQLite and mbedTLS | C | about 480,000 lines |
| Scheduler, fibers, channels (`runtime/chimi-fiber`) | Rust | about 9,900 lines of source |
| Compiler, LSP | Rust | most of the 164,000 Rust lines |

Rough split of `runtime.c` by function prefix: the intrinsic dispatch
(`invoke_*`) about 2,300 lines; text, JSON, decimal, duration, parsing and
debug rendering about 2,000; files, console, random and metrics about 1,000;
the binding and handle table about 550; heap work (allocation, young chunks,
marking, copy, transfer) about 800; process, channel and select glue about
500; the rest is construction, causes, the entry point and helpers.

The generated code reaches the runtime through about 22 coarse `chimi_rt_*`
entry points in `runtime.h` (`chimi_rt_call`, `chimi_rt_construct`,
`chimi_rt_loop`, `chimi_rt_select` and so on). Allocation, marking and copying
happen inside the runtime, so in the C build they inline into the intrinsic
bodies within one translation unit.

## Safety

Where Rust pays and where it does not:

- **Parsing untrusted bytes: high payoff.** JSON, decimal, duration, text and
  grapheme handling, and file and console argument handling work on buffers
  that come from the network or from users. Safe Rust removes out-of-bounds
  and use-after-free bugs from that code outright, and this is the code a
  service exposes to attackers. Chimi promises no user-visible unsafety
  (`design-philosophy.md` section 7), and a memory bug in a JSON parser is
  exactly the kind of unsafety a user sees.
- **Shared state and protocols: high payoff, but only when both sides move.**
  The bugs the multi-core work found were ownership and locking bugs across
  the C and Rust halves of one protocol. A force wrote `RESPONSE_FORCED` into
  a request that had already been answered, so the C side freed a message the
  receiver owned (`parallel-scheduler.md`, 5e). ThreadSanitizer found a race
  in constant compaction (5d-1). Rust's ownership and `Mutex<T>` help with
  these only when both halves of the protocol are Rust. Moving one half
  leaves a raw-pointer seam that neither compiler checks.
- **The heap and collector: low payoff.** Marking, copying and the bump
  chunks are raw-pointer code. In Rust it would be `unsafe` throughout, under
  stricter aliasing rules than C's, and Miri cannot run it end to end through
  Cranelift-generated frames.
- **Not fixed by either language: fibers that move threads.** A fiber can
  resume on another carrier thread, and neither compiler sees the stack
  switch. A thread-local address or a `!Send` guard held across a park
  compiles in Rust as it does in C. The 4b rule (decisions.md, 2026-11-22)
  stays a convention in both.

`chimi-fiber` shows what runtime Rust looks like here: `unsafe` appears 465
times in its sources (tests included), with about 100 `catch_unwind` or abort
sites to keep panics out of generated frames. Rust lowers the bug rate of
such code; it does not make it safe.

What is missing today, in either language: CI runs no AddressSanitizer or
UndefinedBehaviorSanitizer job, no fuzzing, and the one ThreadSanitizer
fixture (`compiler/chimi/tests/runtime_state.rs`) silently falls back to a
plain build when TSan is unavailable.

## Performance

The language does not change the cost of a call from generated code: Cranelift
calls the runtime through the C ABI either way, and cannot inline into either.

The risk is splitting the allocator from its callers. Today an intrinsic and
the allocation it does sit in one C file and inline together. If the
intrinsics were Rust and the allocator C (or the reverse), every allocation
would become an out-of-line call across languages, unless the build used
cross-language LTO, which ties rustc to a matching clang and drops the
"compile with the user's `cc`" fallback. So the allocator and its hot callers
must stay in one language. That is why the heap and dispatch come last, if at
all.

The current hot cost does not depend on the language. The HTTP request cost
is now allocation spread thinly across the standard library and one control
allocated per node (`docs/performance/http-request-cost.md`, "What is left").
Removing that is a change to how the runtime represents results, and it should
be done in place in C rather than waiting on a migration.

## Linking and binary size

Measured on this branch (Linux x86-64, release compiler, stripped
executables):

| Program | Links the Rust fiber archive | Stripped size |
| --- | --- | ---: |
| `pub fn main() -> Int { 21 + 21 }` | no | 345 KB |
| `programs/concurrency-pipeline` | yes | 2.52 MB |

The 2.2 MB difference is mostly Rust's standard library and the `backtrace`
crate's symbolication stack: `core` 297 KB, `alloc` 268 KB, `chimi_fiber`
266 KB, `std` 241 KB, `gimli` 181 KB, then `rustc_demangle`, `memchr`,
`addr2line`, `backtrace` and `object` at 27 to 44 KB each (by `nm` symbol
size, unstripped). The workspace's release profile sets only
`strip = "symbols"`: no `panic = "abort"`, no LTO, no size tuning.

Two things follow:

- Every service program already pays this, because serving HTTP needs the
  scheduler. Moving more of the runtime into the same Rust archive costs such
  a program little more.
- A program with no concurrency, such as a small CLI, carries no Rust today.
  An always-linked Rust runtime would take it from about 0.35 MB to about
  2.5 MB unless the always-linked part is built without `std` and `backtrace`
  (`no_std` plus `alloc`, `panic = "abort"`). CLIs are Chimi's second target
  (`CONTEXT.md`), so this is a gate, not a footnote.

There must be one Rust archive, not two: two Rust staticlibs each carry their
own copy of `std` and collide or duplicate at link time. New runtime Rust goes
into `chimi-fiber` or a crate linked through it.

## Cross-compilation (T8)

Building for another target already needs a runtime pack per target, because
the fiber archive is prebuilt Rust (`tooling.md`, "Cross-compilation"). Rust
makes that pack slightly easier: `cargo build --target <triple>` instead of a
C cross toolchain at pack-build time. It does not remove the C toolchain:
G20 packages (SQLite, mbedTLS) are compiled with `cc`, and the final link
needs a linker and C library for the target in every option.

Moving runtime code to Rust removes the per-project fallback that compiles
`runtime.c` with the user's `cc` when the embedded archive's key does not
match (`native-runtime-prebuild.md`). The fiber archive has no such fallback
today, so the pack has to be the only path anyway.

## C bindings (G20)

Leave G20 as it is. The libraries are C, the shims are thin, and
`chimi_binding.h` is the right seam. Fuzz the shims rather than rewrite them.

## Build time

`runtime.c` at `-O2` takes about 2 s and is prebuilt at `cargo build` time, so
users never pay it on the default path. A Rust runtime crate moves the cost
from users to compiler builds, where it is small next to the compiler itself.

## Options

| Option | What it means | For | Against |
| --- | --- | --- | --- |
| A. Stay C, harden | Sanitizer CI, fuzzing, mandatory TSan | Cheapest. The recorded bugs were protocol bugs that sanitizers and stress tests caught. One C file is easy to audit. | The runtime language is close to permanent (it cannot be written in Chimi, which has no `unsafe`), and every month adds C. Parsers of untrusted input stay in C. |
| **B. Split by kind (recommended)** | A's hardening now; new shared state in Rust; untrusted-input parsers to Rust behind the ABI; heap, collector and dispatch stay C | Rust where it removes a bug class; no hot-path risk; incremental and reversible per subsystem | Three seams (C core, Rust leaves, Rust scheduler) unless the protocol moves really happen. Needs `no_std` care to keep CLIs small. |
| C. Move everything to Rust | Including the heap, collector and dispatch, over time | One language; the concurrent heap is where C is weakest | Big cost for the lowest-payoff code; allocator inlining and size risks; collides with active runtime work. |

## Order and gates for B

1. **Now, without touching the runtime files the multi-core work is
   changing:** an ASan plus UBSan job over the program-output suite; TSan
   mandatory on Linux; libFuzzer targets for JSON, text, decimal and each
   binding shim.
2. **After the multi-core work lands:** new shared-state code in Rust only;
   move channel, select and process-kernel glue beside the scheduler.
3. **Leaf migration:** JSON, then decimal and duration, then text and
   graphemes, written as `no_std` plus `alloc` code that allocates through the
   runtime's allocator. One subsystem per PR, with the C version kept for one
   release as a differential-fuzz oracle.
4. **Last or never:** heap, collector, `invoke_*` dispatch, bump paths.

Gates for each migrated subsystem:

- **Size:** the stripped size of a program without concurrency (345 KB today)
  grows by at most 50 KB.
- **Speed:** `loop-bind`, the map-building loop and the HTTP request benchmark
  (the `docs/performance/` records) regress by no more than 2%.
- **Build:** cold `chimi build` time unchanged.
- **Correctness:** differential fuzzing agrees with the C version; sanitizer
  jobs green.
- **Review:** runtime bugs counted by kind each quarter. If heap bugs dominate
  after hardening, the heap question reopens.

If the size or speed gates fail at step 3, option A plus Rust for concurrency
only is a sound place to stop.

## Method

- Repository at `94d7ea5` (main, 2026-11-23). Line counts with `wc -l`;
  `runtime.c` split by function-name prefix.
- Sizes: `cargo build --release -p chimi --bins`, then `chimi build` of the
  one-line program and of `programs/concurrency-pipeline`, `strip`, and
  `nm -S --size-sort -C` grouped by crate for the breakdown. Linux 6.18 cloud
  container, x86-64, gcc 13 as `cc`, Rust 1.97.0.
- An advisor review (Fable 5.2) checked the analysis against the repository
  and proposed the order and gates; the binary sizes above were measured after
  that review, and the performance effects of migration are not measured.
