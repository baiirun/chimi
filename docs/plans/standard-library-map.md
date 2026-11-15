# Standard Library Map

## Status

Working inventory, not a specification. It groups the standard library Chimi
needs into modules, records how each module would be implemented, and names
the language features that block it. Gap IDs (`G1`…) are stable handles for
follow-up tickets.

## Where the standard library is today

Every standard symbol is a compiler intrinsic. `compiler/chimi/src/standard.rs`
enumerates 5 modules, 19 types, 26 functions, and 22 constructors. Their
signatures are built by hand in `typing.rs`, and each one is implemented
separately in `execution.rs` and `runtime/native/runtime.c`.

| Module | Contents |
| --- | --- |
| `std/io` | `Path`, `IO.path`, `Console`, `FileSystem.process_utf8_lines`, `FileError`, `ConsoleError`, `Utf8Error` |
| `std/json` | `Value`, `Number`, `decode`, `encode`, value builders, `number_to_int` |
| `std/map` | `Map<Text, V>`: `empty`, `get`, `put`, `update`, `map_values` |
| `std/task` | `Task`, `start`, `result`, `cancel` |
| `std/channel` | bounded MPSC `Sender`, `Receiver`, `Completion`, `send`, `receive`, `close`, `abort` |
| built-in namespaces | `List.first`, `List.length`, `Text.append`, `Text.equal`, `Int.to_text` |

The corpus also uses `std/process`, `std/supervisor`, `Duration.seconds`, and
`Retry.with`. These are designed or prototyped but not part of the standard
surface.

No standard module is written in Chimi. Adding one function means editing the
symbol table, resolution, typing, the interpreter, and the C runtime.

## How each module would be implemented

The standard library falls into four implementation tiers. The tier decides
which language gaps matter for a module.

- **A: Pure Chimi.** Ordinary source with no runtime support, for example
  `Retry` (already prototyped in `prototypes/retry-composition`), `Option`
  helpers, and `Duration` as an opaque struct over `Int`.
- **B: Runtime data with a Chimi surface.** A data structure the runtime
  represents efficiently, with the API written in Chimi on top of a few
  primitives: `List`, `Text`, `Map`, `Set`, `Bytes`.
- **C: Host capability.** A structural interface the caller supplies through
  `needs`, with a native provider and test fakes: `FileSystem`, `Console`,
  `Clock`, `Random`, `Environment`, `Network`.
- **D: Runtime operational primitive.** Scheduler-owned behavior: `Task`,
  `Channel`, `Process`, `Supervisor`, timers, `select`.

## Module inventory

| Area | Module | Tier | Status | Blocked by |
| --- | --- | --- | --- | --- |
| Core data | `std/option` (`map`, `unwrap_or`, `and_then`) | A | missing | G1 |
| | `std/result` (storable outcome, `capture`, `raise`) | A | missing; kernel defers it | G1, G13 |
| | `std/order` (`Ordering`, comparators) | A | missing | G2, G3 |
| | `Bool` helpers / logic | language | missing | G4 |
| | `std/int`, `std/float` (parse, format, checked arithmetic, `%`) | B | `Int.to_text` only | G5 |
| Collections | `std/list` (`get`, `append`, `fold`, `map`, `filter`, `find`, `sort`, `contains`) | B | `first`, `length`, literals | G1, G2, G3, G15 |
| | `std/map` (non-`Text` keys, `remove`, `fold`, `entries`, `size`) | B | `Text` keys, 5 functions | G2, G3, G15 |
| | `std/set` | B | missing | G2, G3 |
| Text and bytes | `std/text` (`length`, `slice`, `split`, `join`, `trim`, `starts_with`, `parse_int`, builder) | B | `append`, `equal` | G2, G9 |
| | `std/bytes` (binary data, UTF-8 encode/decode, base64, hex) | B | missing | G6 |
| Time | `std/duration` | A | ticket `ts-f31a20`; blocks the supervisor epic | G1 or intrinsic, G2 |
| | `std/clock` (monotonic and wall time, `sleep`) | C + D | missing | G14 |
| | `std/time` (instants, calendar, formatting) | A + C | missing | G14, G9 |
| Failure | `std/retry` (policies, attempts, backoff, selective retry) | A | prototype only | G1, G13, G14 |
| Concurrency | `std/task` (`all`, `race`, `timeout`) | D | `start`, `result`, `cancel` | G13, G14 |
| | `std/channel` (`select`, multi-producer helpers) | D | bounded MPSC | G14 |
| | `std/process`, `std/supervisor` | D | in progress (`ep-f29d47`) | G14 (Duration) |
| Resources | `std/resource` (scope-owned acquire/use/release) | D | only inside `process_utf8_lines` | G11, G12 |
| IO and host | `std/io` (binary read/write, directories, metadata, atomic write) | C | UTF-8 line processing and console | G6, G11, G12 |
| | `std/env` (environment variables, arguments, exit status) | C | arguments only, as `List<Text>` | — |
| | `std/random` (seeded test provider) | C | missing | G6 for bytes |
| Network | `std/net` (TCP, sockets), `std/http` (client and server) | C + D | missing | G6, G11, G12, G14 |
| Serialization | `std/json` (struct encoders and decoders built by hand, streaming) | B | `Value` tree | G9, G10 |
| Observability | `std/log`, `std/trace`, `std/metrics` (user-emitted structured events) | C or runtime | runner-owned tracing only | G10, G17 |
| Testing | `std/test` (assertions, fake providers, deterministic clock and random) | A + toolchain | missing | G2, G10, G16 |
| Security | `std/crypto` (hashing, HMAC, secure random) | C | missing | G6 |
| | `std/tls`, `std/x509` | C | missing | G6, G12, G18 |
| CLI | `std/cli` (typed flag and subcommand parsing, usage text) | A | missing | G8, G9 |
| | `std/process/exit` (exit status, which is separate from expected failure) | runner | implicit | — |
| | `std/signal` (SIGINT and SIGTERM mapped onto supervisor shutdown) | D | missing | G14 |
| | `std/terminal` (TTY detection, colors, line editing) | C | missing | G6 |
| OS | `std/exec` (subprocesses with pipes, exit status, timeouts) | C + D | missing | G6, G12, G14, G18 |
| | `std/path` (join, split, extension, normalize) | A | opaque `Path` only | G1 |
| | `std/fs` (directories, walking, metadata, temporary files) | C | missing | G11, G12 |
| | `std/user`, hostname, working directory | C | missing | — |
| Text formats | `std/url`, `std/mime`, `std/csv`, `std/base64`, `std/hex` | A or B | missing | G6 |
| | `std/regex` | B | missing | G1 |
| | `std/unicode` (classes, normalization, grapheme iteration) | B | missing | G15 |
| | `std/template` (typed text and HTML templates without reflection) | A | missing | G9, G10 |
| Compression | `std/gzip`, `std/zip`, `std/tar` | B | missing | G6, G18 |
| Data access | `std/sql` (driver interface, pooled connections, transactions) | C | missing | G11, G12, G13 |
| Math | `std/math` (bit operations, big integers, decimal) | B | missing | G5 |
| | `std/heap`, `std/deque` | A | missing | G3, G15 |
| Build | embedded files (`embed` in Go) | compiler | missing | G19 |

## Language gaps

Ordered by how many modules each gap unblocks. Items marked *verified* were
checked against the current compiler on 2026-11-10.

### G1. Standard modules written in Chimi

Standard modules cannot be written as `.chimi` source. A standard source tree
needs a primitive declaration form that is allowed only in `std/`, such as a
bodiless `fn` bound to a runtime operation ID, so Tier B and C modules can keep
a small native core and write the rest in Chimi. This keeps the No
User-Visible Unsafety rule: applications cannot declare primitives.

This gap comes first because every other module costs five hand edits per
function until it is closed.

### G2. Equality

There is no `==` or `!=` (*verified*: binary operators are `+ - * /` and
comparisons, all typed `Int`). `Text.equal` exists as a workaround, and `case`
literal patterns cover some uses.

Proposal: compiler-derived structural equality for every type that contains no
functions or capabilities. The compiler already derives Shareable Value
structurally, so the same traversal can decide where `==` is allowed. This
avoids adding a trait system.

### G3. Hashing and ordering

`Map` accepts only `Text` keys, and the map spec defers caller-defined hashing
and ordering. `sort`, `Set`, and `Map<Int, V>` need both.

Proposal: derive hashing together with G2 equality. Express ordering as an
explicit `fn(A, A) -> Ordering` comparator passed as a value, with derived
comparators for scalars. This follows the functions-as-values stance in place
of `Ord` constraints.

### G4. Boolean logic

There is no `and`, `or`, or `not`. Every compound condition becomes a nested
`case` on `True`/`False`. Short-circuit evaluation has to be defined because
operands can fail or park.

### G5. Numeric model

`Int` is the only number. Missing: `%`, defined overflow and division-by-zero
behavior (defect or expected failure), a float type (`Json.Number` is already
opaque because of this), and parsing.

### G6. Bytes

There is no binary data type. Networking, binary files, crypto, random bytes,
and encoding all need one, as does a defined conversion between `Text` and
`Bytes`.

### G7. Module constants

Module bindings are private and re-evaluated at every reference (`ts-f47931`).
Values such as `Duration.zero`, default retry policies, and `Http.Method.Get`
tables need exported, evaluated-once bindings.

### G8. Record update

Changing one field means repeating every field. The trace-summary program
rebuilds all 10 `Summary` fields on each update. Any stateful library code,
such as process state and parsers, has the same cost. This needs a decision
together with positional-only constructors (`ts-835fc0`).

### G9. Text formatting

Messages are built from `Text.append` chains. Logging, errors, and CLI output
need interpolation or a formatting function, and that design depends on G10.

### G10. Debug rendering

There is no way to turn an arbitrary value into text. Test assertion messages,
log attributes, and defect context need a derived structural rendering. It can
be derived the same way as G2.

### G11. Generic methods on interfaces

User interfaces cannot declare generic method types (*verified*: `fn<A>` in an
interface field is a syntax error). The kernel excludes higher-rank types, but
`FileSystem.process_utf8_lines` is already `fn<A, F, R>` as an intrinsic. As a
result, users cannot write a fake `FileSystem`, and no library can define its
own scope-owning interface such as `Database.transaction` or
`Http.with_connection`.

### G12. General scope-owned resources

Cleanup on success, failure, defect, and cancellation exists only inside one
host operation. Library-defined resources such as connection pools, temporary
files, sockets, and locks need a runtime primitive, for example
`Resource.scoped(acquire, use, release)`. Its release must run while
cancellation is masked and must not let the raw handle escape.

### G13. Storable outcomes

Done (CHI-200): a failure set projects into a user generic value position,
so `fn capture<T, E, R>(op: fn() -> T errors E needs R) -> Result<T, E>`
compiles, and `std/result` provides `Result`, `capture`, and `get`. Before
this, `Err(error)` failed with `T4203`, and `Result`, `Task.all`, channels that
carry failures, batch reporting, and selective retry predicates could not be
written.

### G14. Time, timers, and choice

There is no `Duration` (`ts-f31a20`), `Clock` capability, `sleep`, timeout, or
`select` across channels and timers. Supervisors, retry backoff, HTTP
timeouts, and rate limiting all depend on these. They must define how parking
and cancellation interact.

### G15. Collection primitives

Lists support literals, `first`, and `length`, with no indexing, append, or
list patterns. The repetition spec rejects iterators and `for`. Collection
iteration should therefore be higher-order functions (`fold`, `map`, `filter`)
whose callbacks carry their own failure and needs rows, as `Map.update` does.
The native core needs only `get`, `push`, `length`, and a fold over map
entries. The rest can be written in Chimi once G1 lands.

### G16. Tests

There is no test declaration or test runner. `using` already makes fake
providers easy to supply. The missing pieces are a discoverable test form,
assertions (which need G2 and G10), and deterministic `Clock` and `Random`
providers.

### G17. User-emitted observability

Tracing belongs to the runner. Application code cannot add attributes or emit
log or metric events. The open decision is whether logging is a `needs`
dependency (explicit, but it appears in many signatures) or a runtime
operation attached to the current computation context (ambient, but already
structured).

### G18. Byte streams

Go's `io.Reader` and `io.Writer` are what connect HTTP bodies, compression,
subprocess pipes, TLS, and files to each other. Chimi has no equivalent. The
repetition spec rejects public streams and iterators, and the only streaming
API is a producer-owned callback (`process_utf8_lines`). Chimi needs a
composable, bounded stream shape. It must have backpressure (or be built on
channels) and use scope-owned handles (G12), so that one implementation of
gzip or TLS works over files, sockets, and pipes.

### G19. Build-time embedding

A single-executable CLI or service often ships templates, migrations, or static
assets. Go does this with `embed`. Chimi has no compile-time way to include a
file as a `Text` or `Bytes` value.

## Coverage against Go's standard library

How Go's packages map onto Chimi:

| Go | Chimi |
| --- | --- |
| `fmt`, `strconv`, `strings`, `unicode`, `bytes` | `std/text`, `std/int`, `std/bytes`; need G5, G6, G9, G10 |
| `slices`, `maps`, `sort`, `cmp`, `container/*` | `std/list`, `std/map`, `std/set`, `std/order`, `std/heap`; need G2, G3, G15 |
| `errors` | language failure rows, plus `std/result` (G13) |
| `io`, `bufio`, `io/fs`, `os`, `path/filepath` | `std/io`, `std/fs`, `std/path`; need G11, G12, G18 |
| `os/exec`, `os/signal`, `flag`, `os` exit and env | `std/exec`, `std/signal`, `std/cli`, `std/env` |
| `net`, `net/http`, `net/url`, `net/netip`, `crypto/tls` | `std/net`, `std/http`, `std/url`, `std/tls`; need G6, G12, G14, G18 |
| `encoding/json`, `csv`, `base64`, `hex`, `binary`, `xml` | `std/json` and per-format modules; decoders written by hand, no reflection |
| `compress/*`, `archive/*` | `std/gzip`, `std/zip`, `std/tar`; need G18 |
| `crypto/*`, `hash/*` | `std/crypto`; needs G6 |
| `time` | `std/duration`, `std/clock`, `std/time`; need G14 |
| `context` | runtime-owned cancellation and computation context; no source value |
| `sync`, `sync/atomic` | processes and channels by design; no shared-memory locks planned |
| `log`, `log/slog`, `expvar`, `runtime/trace`, `runtime/pprof` | runner tracing, plus `std/log` and `std/metrics` (G17); profiling belongs to the toolchain |
| `database/sql` | `std/sql`; needs G11, G12 |
| `text/template`, `html/template` | `std/template`, typed and without reflection |
| `regexp` | `std/regex` |
| `math`, `math/big`, `math/bits`, `math/rand` | `std/math`, `std/random`; need G5 |
| `testing`, `httptest`, `fstest` | `std/test` plus fake providers through `using` (G16) |
| `embed` | G19 |
| `reflect`, `unsafe`, `plugin`, `syscall`, `go/*` | excluded: no reflection, no user-visible unsafety, no plugins |
| `image/*`, `debug/*` | out of scope for services and CLIs |

## Proposed sequence

1. **G1: standard modules in source.** Port `List`, `Text`, and `Map` helpers
   onto a small primitive core to prove the mechanism.
2. **Core data: G4, G2, G5 (`%` and arithmetic semantics), G15, G7.** This
   unblocks `std/list`, `std/text`, `std/option`, and `std/order`.
3. **Time: `Duration` and G14.** The supervisor epic is already waiting on
   `Duration`. It can land as a Tier A opaque struct once G1 exists, or as an
   intrinsic before G1 if the epic cannot wait.
4. **Failure and composition: G13.** Adds `std/result`, real `std/retry`, and
   `Task.all` and `Task.race`.
5. **Testing: G16 and G10.**
6. **CLI surface: `std/cli`, `std/path`, `std/fs`, `std/exec`,
   `std/signal`.** CLI tools are a named use case, and most of this is Tier A
   or C once G1, G11, and G12 exist.
7. **Service surface: G6, G18, G11, G12, G17, G19.** Adds bytes, streams,
   resources, `std/net`, `std/http`, `std/tls`, `std/sql`, and observability.

G8 (record update) and G9 (formatting) are ergonomic. They can be scheduled
whenever the trace-summary or supervisor programs make the cost too high.

## Open questions

- Should `std/` modules be versioned with the compiler and embedded in it, or
  resolved as an ordinary workspace package?
- Is derived structural equality and hashing acceptable without opt-in, or
  should types declare it?
- Logging as a `needs` dependency or as a context-attached runtime operation
  (G17)?
- Do G11 and G12 count as one feature (generic scope-owning interface
  methods) or two?
