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
      add a build input and cache key for nothing until Rust owns them); the collector gets an oracle (done 2026-11-26: memory reports
      and program outputs compared with the base branch by
      `scripts/compare-programs.sh`, the heap check
      `chimi-runtime/src/heap/verify.rs` after every pass under ASan, and
      the existing poisoned reset chunks); `handle_count_lock` moves into
      Rust as a pthread mutex ThreadSanitizer still sees. Civil dates and
      timestamps move as a missed step 2 leaf.
   1. The allocator: small blocks, areas and footprint, then young chunks,
      `allocate` and allocation scopes (two PRs). Watch the thread-local
      carrier read, which C does with an initial-exec access. First PR
      (2026-11-26): `allocate`, `release_allocation`, the footprint and the
      small-block cache move together, in `chimi-runtime/src/heap/allocator.rs`,
      with a `Carrier` mirror. The cache alone failed the gate (the map loop
      +4.6%, from a cross-language call on every allocation and free), so a
      piece moves only with the callers that call it per allocation. The
      carrier read is the watched one: C passing it from an inlined wrapper
      grew a small program by 13 KB, and from an out-of-line wrapper cost a
      loop 4%, so Rust reads the initial-exec thread-local itself, from the
      thread pointer and the offset C computes. The young bump saves no
      registers (every call on it is cold or a tail call), and the block path
      is out of line with its area passed in.
      Second PR (2026-11-26): the young chunks (take, give, the cache and
      the poisoned reset), the pending-count flush, the chunk reset after a
      pass, and allocation scope begin and end move to
      `chimi-runtime/src/heap/young.rs`; `runtime.c` keeps the collector
      that calls them once per pass. The reset's walk rounds sizes with a
      mask, because `div_ceil` there cost the loop 2.2%.
   2. The collector: mark, forward, promote and sweep. Done (2026-11-26) in
      `chimi-runtime/src/heap/collector.rs`, with the process-wide mark
      epoch counter. Marking and forwarding share one walk over each
      allocation's pointers, so they cannot drift apart. `runtime.c` keeps
      the heap check's bookkeeping, which the collector calls only in a
      checking build, and the C test harnesses call the pass and the epoch
      through the same ABI.
   3. Fragment copy, the transfer walk, and the task and process heap
      lifecycle; handle drops become Rust-to-Rust calls.
      First PR (2026-11-26): fragment copy, adoption, duplication and free,
      and heap creation and end, in `chimi-runtime/src/heap/fragment.rs`,
      with the heap owner counter. A copy's forwarding table keeps its
      first 16 entries inline, so the common small message allocates no
      table. `runtime.c` keeps the task and process heap lifecycle, which
      calls fragments through the same ABI.
      Second PR (2026-11-26): the capability walk that Task.start and the
      sends run over a value graph, in `chimi-runtime/src/heap/transfer.rs`,
      with a read-only list reader (`heap/list.rs`) that the fragment copy
      uses too. What a Receiver, Completion or managed handle does when the
      walk meets it stays in `runtime.c` behind one callback, since it is a
      scheduler request; it moves with the task and channel intrinsics.
      That closes phase 3. What `runtime.c` still has of the task and
      process heap lifecycle is the two fiber entries, which create and end
      the heap around `invoke` and the tracing, so they move with dispatch
      (phase 7); and the scheduler's release hooks, which are one call into
      the Rust fragment free each and move with the task and process
      contexts they also free (phase 5).
   4. Values: scalar and text builders with environments, the list vector,
      and the AVL map together with structural compare (three PRs).
      First PR (2026-11-26): values, controls, value tables, environments
      and the scalar and text builders, in `chimi-runtime/src/heap/values.rs`,
      allocating in the order and with the kinds the C did. `runtime.c`'s
      builders became one-line calls into it. A call frame's environment
      now takes one call into Rust where the C made three into the
      allocator, which is most of loop-bind's 8.7%.
      Second PR (2026-11-26): the list vector's builders (push, set, build
      from values, append to a window, window copies and slicing with its
      compaction and shared-constant rule) in `chimi-runtime/src/heap/list.rs`,
      with the C recursions as loops that allocate in the same order. The
      reads (`vector_leaf`, `list_get`) stay inline in `runtime.c` for the C
      intrinsics that walk lists, beside the Rust reader, until those
      intrinsics move (phase 5).
      Third PR (2026-11-26): the Map tree that Set shares
      (`heap/map.rs`), structural order (`heap/compare.rs`) and the exact
      order and canonical text of JSON numbers (`heap/json_number.rs`), with
      the nodes allocated in the C's order. `runtime.c` keeps one-line
      wrappers under the old names for its intrinsics and fixtures. This
      closes phase 4.
   5. Intrinsics by family, pure ones first, then task, select, clock, fs,
      console and stdin, each family taking its C lock with it (six to eight
      PRs).
      First PR (2026-11-26): the program's symbol and constructor tables are
      mirrored in `heap/layout.rs` (pinned by the layout block and digests,
      which now read `runtime.h` too), and `intrinsics/standard.rs` builds
      standard values and controls from them. std/map and std/set moved first
      (`intrinsics/collections.rs`); a callback goes back through
      `runtime.c`'s `invoke` until dispatch moves (phase 7).
      Second PR: std/json (`intrinsics/json.rs`, which builds values from
      the decoder's tape directly) and std/decimal (`intrinsics/decimals.rs`).
      Third PR: std/bytes and std/http's byte scan (`intrinsics/bytes.rs`),
      with numbered defect messages in `intrinsics/message.rs`. Fourth PR:
      std/duration (`intrinsics/durations.rs`), over the parser and text form
      `crate::duration` already had. Fifth PR: `Debug.crash`,
      `Option.unwrap_or_fail`, and the native cores of std/text and std/list
      (`intrinsics/core.rs`), which call `crate::text` and
      `crate::graphemes` directly instead of through C. Sixth PR: the
      debug renderer behind `Debug.render`, assertion messages and causes
      (`intrinsics/debug.rs`); runtime.c's `debug_render` forwards to it.
      Seventh PR: std/test's assertions (`intrinsics/assertions.rs`).
      Eighth PR: std/test's recorders, scripts and test mailboxes, with
      `Process.send` to a test address (`intrinsics/test_log.rs`); their
      table moves to Rust behind its own lock, and runtime.c keeps only
      the capability check a send makes, since the transfer state is
      still C's.
      Ninth PR: failure causes and std/cause (`intrinsics/causes.rs`):
      building, linking, bounding and rendering causes, and the hidden
      field a stored Err keeps its cause in. `Cause.context` stays in C
      until dispatch moves, because it calls a function.
      Then Float text drops core's float formatting for its own exact
      shortest-digit generation (`float.rs`), giving the step's size
      budget back 16 KiB.
      Tenth PR: the host cores of std/env and std/fs and the open-file
      table's lock (`intrinsics/host.rs`); runtime.c keeps the scheduler
      wait a blocked read or write makes.
      Eleventh PR: the scalar intrinsics runtime.c still switched on
      (Int, Float, Range, Path, `Text.append`, `Text.equal`, `List.length`,
      `Debug.render`) in `intrinsics/scalars.rs`, dropping the unreachable
      `List.first` and std/order ones; signals, log and metrics wait for
      phase 8.
   6. The binding argument readers and value and result builders.
      First PR: the value half of `chimi_binding.h` (argument readers,
      value builders, results) in `intrinsics/binding_values.rs`; handles,
      waits and cancellation checks stay in runtime.c with the handle lock
      and the scheduler requests until dispatch moves.
   7. Dispatch: telemetry emission, then the node helpers with `run_node`,
      `invoke` and the loop passes in one PR (splitting them would put a
      cross-language call on every node), then match.
      First PR: the telemetry event writers (`telemetry.rs`), with the
      trace lock, over mirrors of `ChimiRuntime`, the node and origin tables
      and the scheduler's trace event.
      Second PR: the evaluator (`eval.rs`): every core node but the loops,
      `run_node`, each node's span and profile frame, match, and `invoke`
      for declared functions, closures, constructors and dependency calls.
      Intrinsics, host functions and C bindings go back to `runtime.c`
      through one call, `chimi_runtime_invoke_native_v1`, until their
      dispatch moves. The loop nodes stay C for now, with their passes,
      which make scheduler requests; they open their frame through Rust.
      Third PR: the loop nodes (`chimi_rt_loop`, `chimi_rt_direct_loop` and
      their passes) and the region totals (`end_measured_scope`) in
      `eval.rs`; a pass asks the scheduler about its scope through one C
      call (now `scheduler.rs`, see the fourth PR), only when the program
      starts the scheduler. Intrinsic routing moves too
      (`intrinsics/dispatch.rs`): the evaluator calls the Rust families
      directly, and only the intrinsics `runtime.c` still serves (tasks,
      channels, processes, waits, clocks, signals, the log and metrics
      hosts, `Cause.context`) and host functions and C bindings take the
      C call.
      Fourth PR: the intrinsics that call back inside a scope,
      `Cause.context`, `Test.capture_defect` and `Resource.scoped`
      (`intrinsics/scopes.rs`), over a mirror of `ChimiSchedulerRequest`.
      Rust intrinsics reach the scheduler through `scheduler.rs`, whose one
      C call, `chimi_runtime_scheduler_request_v1`, restores the fiber's
      heap and carrier as every request must; the loop's scope check uses it
      too.
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
