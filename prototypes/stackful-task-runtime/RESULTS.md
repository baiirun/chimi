# Stackful Task runtime results

## Run 1

- Date: 2026-08-09
- Host: macOS 26.5.2, Apple ARM64
- Build: Rust release profile, `corosensei` 0.3.4
- Fiber stack: fixed 64 KiB usable allocation with a guard page
- Binary: 502 KiB, dynamically linked only to macOS system libraries

Observed semantic transitions:

```text
start:  Parked("Channel.receive"), roots=[42], cleaned=false
wake:   Succeeded(42), roots=[], cleaned=true
cancel: Cancelled, roots=[], cleaned=true
defect: Defected("prototype child defect"), roots=[], cleaned=true
```

Measured forcing results:

```text
cold create+complete p95: 2,584 ns (1,000 samples)
spawn+complete throughput: 3,053.5 ns/task (10,000 tasks)
resume-to-yield trace-off: 3.6 ns/round-trip (1,000,000 yields)
resume-to-yield traced: 6.5 ns/round-trip (1,000,000 yields)
parked high-water delta: 32,866,304 bytes (2,000 tasks, 8 KiB touched each)
reserved stack address space: 131,072,000 bytes
```

The resume-to-yield number is one complete scheduler-to-fiber-to-scheduler
round trip, not one directional context transfer. The traced variant constructs
and stores a compact in-memory event tuple; it does not include serialization
or file export.

## Generated-C bridge

The follow-up probe compiles generated-C entry and nested-call frames, then
parks them through a Rust runtime bridge. Its assertions establish:

```text
parked generated C frames expose roots=[42]; backtrace nested=true, entry=true
wake returned Succeeded(42); cleanup=true
cancel returned Cancelled through C normally; cleanup=true
defect returned Defected(-9223372036854775807) as runtime cause; cleanup=true
```

The bridge-enabled release executable is 789 KiB and dynamically links only
macOS system libraries. A repeat measurement after adding native backtrace
symbolization reported 4,084 ns cold create+complete p95, 4,392.7 ns/task
spawn+complete, 4.2 ns trace-off and 6.5 ns traced resume-to-yield round trips,
and a 32,800,768-byte high-water increase for the same 2,000 parked tasks.
These are local forcing measurements, not stable performance promises.

The root is read from an exact generated frame while the C stack is parked;
the local remains intact after wake. Cancellation is deliberately cooperative:
it resumes the child with a cancellation command and both generated C frames
return normally before fiber cleanup completes. No Rust unwind crosses C.

## Verdict

Accept guarded stackful fibers as the first Task runtime representation, using
a statically linked Rust runtime component behind a narrow C ABI and
`corosensei` 0.3.4 as the bootstrap context mechanism. Fibers remain absent
from Chimi syntax, Core, and persisted execution. Generated code publishes
precise live-root frames at approved park boundaries; cancellation and defects
cross the ABI as explicit outcomes. This preserves ordinary direct-style calls
and a single output executable without prematurely maintaining per-architecture
context assembly.

This accepts a mechanism, not the prototype crate as production runtime code.
The implementation tickets must still integrate root frames with GC, install
platform trap handling so fixed-stack overflow becomes a task defect, and
validate every supported target's unwind/backtrace metadata. Unix stacks are a
fixed writable mapping plus a guard page and do not grow; the initial 64 KiB is
a measured forcing size, not a language guarantee or final default.

The measured 16.4 KiB high-water increase per parked, partially touched task is
promising, but it is not a final memory model: untouched reserved address space,
allocator metadata, stack depth, and GC root metadata still need independent
measurements.
