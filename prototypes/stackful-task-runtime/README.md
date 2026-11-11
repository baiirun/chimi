# Stackful Task runtime prototype

PROTOTYPE: disposable runtime-representation evidence, not Chimi runtime code.

## Question

Can a guarded stackful context preserve ordinary nested Chimi calls across a
park/wake boundary, unwind task-local cleanup on cancellation and defects, and
meet the first Task runtime's memory and switching budget without exposing
fibers or `async`/`await` in source?

This mechanism probe uses `corosensei` 0.3.4 because it supports Darwin and ELF
on AArch64 and x86-64, provides guard-page stacks, propagates unwinding, links
backtraces across stacks, and exposes sanitizer integration. It is evidence for
a stackful representation and a Rust runtime component behind a narrow C ABI;
it does not make the crate part of Chimi's public language model.

The second half compiles a small generated-C child with frame pointers and
unwind tables. That child parks from a nested ordinary call, publishes its
precise live-value frame, and resumes or cooperatively cancels through those C
frames. The probe also checks that both generated frame names occur in the
parked backtrace.

## Run

    cargo run --release --manifest-path prototypes/stackful-task-runtime/Cargo.toml

Press `n` to start or wake the normal child, `c` to cancel a parked child, `d`
to run a defecting child, `f` to run the generated-C bridge, `b` to run the
bounded measurements, and `r` to reset. Every action redraws the complete task
state, registered roots, cleanup status, and event history.

For a non-interactive transcript and measurements:

    cargo run --release --manifest-path prototypes/stackful-task-runtime/Cargo.toml -- --all

## Boundaries this probe makes explicit

- A fiber stack is runtime representation; the Chimi task remains the semantic
  unit and owns cancellation, outcome, children, capabilities, and traces.
- On Unix the probe uses a fixed 64 KiB writable mapping plus one inaccessible
  guard page. Pages become resident on demand, but the address range cannot
  grow. The crate exposes an unsafe trap-recovery mechanism; integrating that
  with Chimi's carrier-thread signal handling remains production work. The
  prototype does not choose a final stack size.
- Chimi's precise collector cannot discover live `ChimiValue` pointers merely
  because they sit on an opaque fiber stack. Generated code must publish a
  linked frame of exact live-value slots before any park. The C bridge proves
  that ABI shape and lets the scheduler enumerate the live local `42`; it does
  not yet connect those frames to the maintained collector.
- Generated Chimi C frames may suspend only through the runtime's park ABI and
  must carry unwind metadata. Arbitrary host FFI remains non-suspending: a host
  adapter starts external work, returns from foreign code, and lets the runtime
  park at an approved boundary.
- Cancellation is an explicit resume command. Generated frames run normal
  cleanup and return a cancelled outcome; the runtime must never force a Rust
  panic unwind through C. Defects likewise cross the ABI as explicit runtime
  causes rather than language errors or foreign exceptions.
- Static linking preserves one output executable. The accepted bootstrap
  boundary is a Rust runtime component beside the current C runtime, with
  `corosensei` as the first implementation candidate. Chimi can later replace
  the crate behind the same ABI when self-hosting or platform control warrants
  it; vendoring architecture-specific context code now would duplicate its
  unwind, sanitizer, and platform work before Chimi has a scheduler.

The benchmark is a local forcing measurement, not a cross-machine target. It
reports its fixed scale and raw nanoseconds/bytes so future runs remain
comparable.

## Mechanism references

- [`corosensei` documentation](https://docs.rs/corosensei/0.3.4/corosensei/)
  documents supported targets, guard-page stacks, unwinding, linked
  backtraces, trap recovery, and sanitizer support.
- [Boost.Context](https://www.boost.org/library/latest/context/) establishes
  the same nested-call stackful behavior but requires C++11 and the Boost
  dependency surface.
- The Linux [`getcontext` manual](https://man7.org/linux/man-pages/man3/getcontext.3.html)
  records that POSIX.1-2008 removed the API for portability reasons, so it is
  not a suitable portable foundation.

See `RESULTS.md` for the measured verdict.
