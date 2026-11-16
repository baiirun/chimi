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
  answers ERROR.
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
  deadlines and `--signal` injections therefore wait for the descriptor; a
  real signal still ends the wait, because the signal pipe is in the same
  wait. A parked task waiting on a descriptor is never a deadlock.
- **Cancellation.** The wait is a cancellation point. A cancellation, a
  timeout scope's deadline, or a forced shutdown withdraws it.
- **Closing.** Closing a descriptor while a task waits on it is a host bug.
  The current poll(2) backend wakes the waiter, whose next request answers
  ERROR, but an epoll or kqueue backend would not notice.
- **Backend.** poll(2) on macOS and Linux. Each idle wait costs time linear
  in the number of waits, which is fine until a server holds thousands of
  connections; the reactor's wait is the one place an epoll or kqueue
  backend replaces it.

`process_utf8_lines` reads through this wait under the scheduler: a program
reading standard input lets its other tasks run, and the first SIGINT or
SIGTERM ends the read as it cancels `main`. Standard input is read with
read(2) into a process-wide buffer, so bytes read past an early exit wait
for the next read of `"-"`. Regular files go through the same path, where
the wait answers at once. Standard input is never made non-blocking,
because its file description is shared with the parent; a descriptor a
parent left non-blocking is waited on and read again.

Not yet covered: writes to standard output and standard error still block
the scheduler thread, so a program writing to a stalled pipe is ended only
by a second signal. Phase 7b's sockets own their descriptors, make them
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
