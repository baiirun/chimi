# Host Capabilities and Execution Properties

## Status

This is the normative post-kernel contract for synchronous host operations. It
supersedes the user-facing effect-row portion of
[synchronous effects and host capabilities](synchronous-effects-and-host-capabilities.md)
while retaining its explicit dependency, resource, cancellation, and runner
boundaries.

## Callable contract

A Chimi callable exposes data inputs, a success value, expected failures, and
dependency needs:

```text
data inputs -> success type
errors expected-failure row
needs dependency row
```

There is no additional `effects` row. Implemented functions may infer success,
expected failures, and needs. Function values and interfaces may state those
same channels. I/O, suspension, resource access, tracing, and other execution
behavior do not add source syntax or generic row parameters.

`FileSystem` and `Console` are dependencies because the caller chooses their
implementations. Supplying them with `using` removes their entries from `needs`.
Expected failures remain ordinary values and are removed only by matching or
another explicit value transformation. Runner tracing, execution identity,
limits, and cancellation configuration remain outside callable types.

The compiler rejects an `effects` clause as unsupported Chimi syntax. Callable
contracts, generic kinds, inspection output, executable Core, and native
metadata contain no effect row.

## Compiler-private execution properties

Resolved Core may derive compact execution properties such as `may_park`,
`performs_host_operation`, or `emits_observable_boundary` when a concrete
compiler, backend, or tool decision consumes them. Such properties:

- are conservative summaries of resolved operations and known calls;
- treat unknown indirect calls conservatively;
- may guide code generation, target validation, caching, optimization safety,
  diagnostics at restricted runtime boundaries, generated documentation, LSP
  inspection, traces, and profiling;
- are not part of function identity, overload resolution, interface
  compatibility, generic substitution, or caller-visible error handling;
- are not a general purity proof and are not inferred merely to decorate every
  callable.

Ordinary calls may park without an `async`, `await`, or effect marker. Tooling
may explain that a call can park and show the transitive operation responsible.
A diagnostic is appropriate only at a separately specified boundary that
cannot safely park, such as a selected native callback or critical runtime
region.

## Host capability ownership

The official standard library owns structural `FileSystem` and `Console`
interfaces, their path and error values, and module-qualified operations.
Application code may provide compatible implementations.

The native runtime owns concrete OS operations, descriptors, stream access, and
cleanup on success, expected failure, defect, and cancellation. Provider lookup
is never ambient: execution receives a resolved provider plan for every root
need that source did not already satisfy with `using`.

The runner supplies arguments as ordinary `List<Text>` data, validates the root
provider plan before execution, and supplies trace, identity, cancellation, and
limit configuration separately. Missing or incompatible providers are setup
failures rather than expected values produced by the program body.

## Runner-provided capabilities

The system modules (`std/env` first) declare their capability interfaces in
standard source, each with a fixed need name the runner closes on `main` from
a private provider written in the module. The rules are in
[system modules](system-modules.md#runner-provided-capabilities); `Console`
and `FileSystem` below keep their built-in rules until `std/io` moves to
source.

## Synchronous files and console

The first standard-library boundary remains explicit and module-qualified:

<!-- chimi: check -->
```chimi
import std/io as IO
import std/io.{Console}
import std/io.{FileSystem}
```

`IO.path(Text) -> Path` validates the nominal path boundary.
`Console.write_line` and `Console.write_error` return `Void` and expose expected
`ConsoleError` values. Neither operation has an effect annotation.

The scope-owning file operation has this conceptual contract:

<!-- chimi: fragment -->
```chimi
interface FileSystem {
  process_utf8_lines: fn<A, F, R>(
    Path,
    A,
    fn(A, Text) -> A errors F needs R,
  ) -> A
    errors F | FileError | Utf8Error
    needs R
}

interface Console {
  write_line: fn(Text) -> Void errors ConsoleError
  write_error: fn(Text) -> Void errors ConsoleError
}
```

The callback's expected failures and needs compose normally. Filesystem and
UTF-8 failures are added by the host operation. Host interaction is visible in
the resolved operation and runtime trace rather than a callable row.

`process_utf8_lines` opens and closes the resource internally and passes text,
not a raw handle, to its callback. Cleanup completes before any success,
expected failure, defect, or cancellation leaves the boundary. A close failure
replaces pending success or expected failure with `FileError.Close`, retaining
the displaced outcome as structured suppressed context. Defect or cancellation
remains primary and retains the close failure as suppressed runtime context.

The native provider interprets `IO.path("-")` as standard input. Line buffering
must remain bounded by runner configuration, and invalid text must not be
silently truncated. The maintained ABI's reclamation scopes remain runtime
ownership machinery, not regions or lifetimes in the language.

## Waiting for descriptors (G21)

A native host operation that reads or writes a file descriptor waits for it
through the scheduler's IO reactor instead of blocking the scheduler thread.

- **Request.** `CHIMI_REQUEST_IO_WAIT` (kind 48, `include/chimi_fiber.h`)
  names a descriptor (`value`) and an interest (`token`: `CHIMI_IO_READABLE`,
  `CHIMI_IO_WRITABLE`, or both). It answers OK with the ready conditions,
  which may add `CHIMI_IO_HANGUP` or `CHIMI_IO_ERROR`, when the descriptor
  is ready now. Otherwise the task parks and the answer is PENDING; the host
  issues the request again after the wake. The wait is level-triggered and
  consumes nothing: the host then does its own read or write. A negative or
  closed descriptor, the runner's signal pipe, or any other interest
  answers ERROR. The request may also name the host resource the descriptor
  belongs to (`channel`: a nonzero key the host chooses, or 0 for none).
- **Order.** An OK answer resumes the task before any other task runs, so
  the read or write right after it sees the descriptor as reported. Another
  OS process sharing the descriptor can still drain it first; the host then
  waits again. Tasks ready together wake in the order they began waiting.
- **Idle wait.** When nothing is runnable and some task waits on a
  descriptor, the scheduler blocks in one wait on every such descriptor, the
  runner's signal pipe, and, under the real clock, the earliest timer
  deadline. While other tasks keep it busy, it checks the waits without
  blocking every 64 resumes, so a ready descriptor is never starved.
- **Virtual time.** A descriptor wait is a host operation in flight, so
  virtual time does not advance while one exists ([virtual
  time](time-and-outcomes.md#virtual-time)). Timeouts, `shutdown_within`
  deadlines and `--signal` injections later than the current instant
  therefore wait for the descriptor (one due at the current instant still
  fires). A real signal still ends the wait while `main` runs, because the
  signal pipe is in the same wait; once `main` has ended, signals are
  retired, so a process body still draining in a descriptor wait under the
  virtual clock ends only with a second signal. A parked task waiting on a
  descriptor is never a deadlock.
- **Cancellation.** The wait is a cancellation point. A cancellation, a
  timeout scope's deadline, or a forced shutdown withdraws it.
- **Closing.** `CHIMI_REQUEST_IO_WITHDRAW` (kind 44) wakes every wait that
  names a resource key and answers OK with how many it woke; it never parks
  and is not a cancellation point, so a cancelled task can still close what
  it owns. A host issues it before closing a resource's descriptors, so no
  waiter stays parked on a descriptor number the close frees for the next
  open; each waiter's reissue then learns from the host that the resource
  is closed. Closing a descriptor a task waits on without withdrawing first
  is a host bug: poll(2) reports the closed descriptor only while its number
  stays unused, and an epoll or kqueue backend would not report it at all.
- **Poll failure.** When the reactor's poll(2) fails other than by an
  interruption or a transient `EAGAIN` (too many descriptors, no memory),
  every waiting task wakes with its wait answered ERROR, instead of the
  scheduler retrying a wait that cannot succeed. A host treats ERROR as a
  failed read or write, never as ready: reading then could block the
  scheduler thread.
- **Backend.** poll(2) on macOS and Linux. Each idle wait costs time linear
  in the number of waits, which is fine until a server holds thousands of
  connections; the reactor's wait is the one place an epoll or kqueue
  backend replaces it.

`process_utf8_lines` reads through this wait under the scheduler: while a
read waits for input, the program's other tasks run, and the first SIGINT or
SIGTERM ends the read as it cancels `main`. Standard input is read with
read(2) into a process-wide buffer, so bytes read past an early exit wait
for the next read of `"-"`. Regular files go through the same path, where
the wait answers at once. Standard input is never made non-blocking,
because its file description is shared with the parent; a descriptor a
parent left non-blocking is waited on and read again.

A reader whose input is always ready (`yes | program`) keeps the CPU as a
busy loop does: each ready answer resumes it first, so other tasks run and
timers fire only when its line callback parks or the input stalls. That is
no worse than the blocking read it replaces.

`write_line` and `write_error` write through the same wait under the real
clock (CHI-266), so a task writing to a pipe, a terminal or a socket that
is not being read parks while the other tasks run:

- Standard output and standard error are never made non-blocking, for the
  same reason as standard input. After the wait answers writable, the
  runtime writes at most `PIPE_BUF` bytes with one write(2), which poll's
  answer guarantees fits in a pipe. A socket is written with
  `send(MSG_DONTWAIT)`. A terminal may still hold such a write briefly.
- Each line goes whole onto its stream's queue before its writer waits, and
  whichever writer wakes writes the front of the queue, so lines from
  different tasks never interleave. Standard error shares standard output's
  queue when both name the same open file (`2>&1`, one terminal). A call returns once its whole line is
  written. The queue holds at most 1 MiB; a writer waits before queueing a
  line that would pass that, except into an empty queue.
- A line once queued is written whole even if its writer is cancelled while
  it waits (a `Task.timeout`, a stop signal): a later write, or the end of
  the program, writes it. A body ending under a terminal cancel (CHI-265)
  queues its line and returns without waiting. A second stop signal ends the program at once and
  drops what is still queued.
- A failed write, or an ERROR answer from the wait, drops the queue and
  fails every line not yet fully written with `WriteFailed`. SIGPIPE keeps its default
  action, so a closed pipe still ends the program as it did.
- A regular file, a program without the scheduler, and every program under
  `--clock=virtual` write directly and block as before. Under the virtual
  clock the order tasks run in must not depend on how fast the reader drains
  the pipe, and a parked writer would hold virtual time anyway.

Phase 7b's sockets own their descriptors, make them
non-blocking, and wait with the same request. Phase 7b decides whether an
idle `accept` should hold virtual time.

## Cancellation and restricted contexts

Cancellation is a runtime outcome, not an expected failure or callable row. A
synchronous host call checks cancellation before entering the OS boundary and
after regaining control, then completes cleanup before propagation. Each host
operation documents any interval that cannot be interrupted safely.

This contract imposes no general purity restriction on case guards or ordinary
functions. Any language or runtime context that forbids host operations,
suspension, allocation, or another behavior must justify and diagnose that
restriction in the specification that introduces the context.

## Inspection and verification

Inspection exposes success, expected failures, needs, and any tool-relevant
compiler-private execution properties as distinct information. Generated docs
may say that a call can park or crosses a host boundary, but must not render
those facts as a source-level row.

Conformance evidence must cover:

1. host dependencies remain explicit in `needs` until supplied;
2. expected host failures compose as values;
3. `using` changes needs without hiding the resolved host operation;
4. ordinary and higher-order calls require no effect generic;
5. root provider validation precedes execution;
6. cleanup precedence and cancellation remain unchanged;
7. tooling can explain a compiler-private `may_park` fact without changing the
   public callable type;
8. trace-off execution does not construct execution-property events.
