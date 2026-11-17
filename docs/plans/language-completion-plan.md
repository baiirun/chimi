# Language Completion Plan

## Status

Working plan, not a specification. It catalogs everything Chimi still lacks
against its [vision](../../README.md), [philosophy](../design-philosophy.md),
and [use cases](../../CONTEXT.md), and orders the work into phases. Gap IDs
`G1`–`G32` are defined in the [standard library map](standard-library-map.md),
which holds the evidence from the use-case sketches; IDs `L*`, `R*`, and `T*`
are new here. Each phase becomes a `prog` epic when it starts.

## Decisions that settle earlier open questions

Recorded with the user on 2026-11-11:

- **Autonomy.** Build the whole language. Minor design details are decided from
  the philosophy and recorded decisions; only vision-level choices go back to
  the user.
- **Arguments are positional** everywhere, with no labels and no defaults
  (`ts-835fc0`, done). This resolves G31: APIs with many settings use small
  constructor functions or `|>` modifier chains instead.
- **Pattern-matched multi-clause functions are core** (`ts-be39c0`).
- **No local mutation** for now. State changes through `loop` rebinding,
  process state, and new values. May be revisited.
- **Equality, hashing, and ordering are derived structurally** for all data
  without opt-in; generics stay plain, with no traits or protocols unless a real
  program forces them. This resolves G2 and G3's open question; ordering is an
  explicit comparator value where a non-structural order is needed.

## Catalog

Status: **missing** unless noted. "Blocks" names what cannot be built without
it.

### Core language

| ID | Gap | Notes | Blocks |
| --- | --- | --- | --- |
| L1 | Multi-clause functions | Pattern matching in function heads with exhaustiveness across clauses (`ts-be39c0`) | idiomatic std and programs |
| G4 | Boolean logic | `and`, `or`, `not`, short-circuit with failing or parking operands | every compound condition |
| G2 | Equality | Derived structural `==`/`!=`; rejected for functions and capabilities | std, tests |
| G3 | Hashing and ordering | Derived hashing; `Ordering` and comparator values | `Map<K, V>`, `Set`, sort |
| G5 | Numeric model | `%`, overflow and division-by-zero as defects, `Float`, parsing | math, JSON numbers |
| G30 | Tuples | Anonymous products and tuple patterns | maps from pairs, recorders |
| G32 | `Void` as a value | Fakes for `-> Void` methods, stateless loops | test fakes |
| G15 | List patterns and collection primitives | `[a, b]`, `[head, ..rest]`; native `get`, `push`, entry fold (verified: list patterns are a syntax error) | `std/list`, routing |
| G26 | Failure-type patterns | One arm binds every variant of an error enum | failure handling |
| G8 | Record update | Copy with changed fields | ergonomic state updates |
| G9 | Text formatting | Interpolation or a formatting function | messages, logs, CLI output |
| G10 | Debug rendering | Derived text form of any value | assertions, defect context |
| G7 | Module constants | Exported, evaluated-once bindings (`ts-f47931`) | `Duration.zero`, defaults |
| G11 | Generic interface methods | `fn<A>` in interface fields | user fakes, scope-owning APIs |
| G13 | Storable outcomes and failure-row types | `Result`, naming combined failure types | retry, `Task.all`, decoders |
| G25 | Failure context | Cause chains, suppressed and grouped causes | diagnostics, observability |
| G24 | Needs groups | Named reusable bundles of needs | service signatures |
| G23 | Provider construction | Providers built from other providers, ordered startup and cleanup | services |
| G6 | Bytes | Binary data and `Text` conversion | IO, network, crypto |
| L2 | `Decimal` | Exact base-10 arithmetic for money and quantities; `Float` is wrong for them | services that handle money |
| L3 | Persistent `List` | Lists are flat arrays today, so building one element at a time is quadratic; needs a persistent vector with cheap append and indexed access | `std/list`, accumulation in loops |
| L4 | `Never` | A nameable type for code that cannot return (process bodies, `loop` with no `break`, `panic`-style helpers) | precise signatures |
| L5 | Byte-level integers | Reading and writing fixed-width integers inside `Bytes` (u8, u16 big/little endian) without adding sized integer types to the language | binary protocols, hashing |
| L6 | `Range` | Integer ranges for counted repetition and slicing | `std/list`, loops |
| G19 | Build-time embedding | Include a file as `Text` or `Bytes` | single-executable assets |

### Standard library mechanism

| ID | Gap | Notes |
| --- | --- | --- |
| G1 | Standard modules written in Chimi | A primitive declaration form only `std/` may use; today every function is five hand edits |
| G20 | Native bindings for std | C bindings restricted to `std/` and vendored modules (TLS, crypto, SQL drivers, compression) |

### Runtime

| ID | Gap | Notes |
| --- | --- | --- |
| R1 | Per-owner heaps | Prototype skeleton committed (`ts-a6b43b`); removes the shared-heap hazard that disables per-pass reclamation under the scheduler |
| R2 | Allocation speed | `malloc`/`free` dominate loop time (`ts-bad344`); bump allocation per heap |
| R3 | Direct integer loops | `ts-a15edc` |
| R4 | Parallel scheduler | The scheduler runs on one carrier thread; the safety model already assumes parallelism is possible |
| G14 | Time, timers, choice | `Duration` (`ts-f31a20`), `Instant`, `Clock`, `sleep`, timeouts, `select`, virtual time |
| G29 | Channel variants | Multi-consumer, broadcast, watch |
| G12 | Scope-owned resources | Acquire/use/release with masked-cancellation release |
| G18 | Byte streams | Bounded, backpressured stream shape |
| G21 | Async IO reactor | kqueue/epoll integration; prerequisite for networking |

### Processes and lifecycle

| ID | Gap | Notes |
| --- | --- | --- |
| P1 | Supervised-process kernel | `ep-f29d47`: native representation, conformance lab, implementation, gates |
| G22 | Application lifecycle and drain | Drain before cancel, ordered shutdown, signals, exit status |
| G27 | Process kernel extensions | Own address, monitors and links, mailbox metrics |
| G28 | Exclusive capability handoff | One-shot process start taking an exclusive argument (accepted sockets) |

### Standard library modules

Ordered by tier in the [standard library map](standard-library-map.md#module-inventory).
Present today: minimal `io`, `json`, `map` (`Text` keys), `task`, `channel`,
plus `List.first/length`, `Text.append/equal`, `Int.to_text`. Everything else
there is missing: option, result, order, list, set, text, int/float, bytes,
duration, clock, time, retry, schedule, resource, env, cli, terminal, fs, path,
exec, signal, random, net, http, websocket, tls, sql, queue, log, trace,
metrics, crypto, test, url/csv/regex/compression, math.

### Observability

| ID | Gap | Notes |
| --- | --- | --- |
| G17 | User-emitted observability | Attributes, logs, metrics attached to the computation context |
| O1 | Standard exporters | Only JSONL files exist; an OpenTelemetry exporter is the service baseline |

### Toolchain

| ID | Gap | Status |
| --- | --- | --- |
| T1 | Test declarations and runner | specified in draft (`testing.md`); G16 recorders, virtual time, and fake addresses are in the same spec |
| T2 | Language server breadth | diagnostics, hover with doc comments, definition, formatting, references, rename, completion after `Alias.`, inlay types for `let`, lint warnings |
| T3 | Linter | `chimi lint`, with findings in the editor |
| T4 | Documentation generator | `chimi doc` and `chimi doc --html`; doc comments with runnable examples |
| T5 | Local workspaces and packages | missing; local-first, reproducible, lockfile |
| T6 | Profiler and allocation views | runner tracing only |
| T7 | Debugger | `chimi trace show`, a post-mortem trace viewer; no REPL by decision (2026-11-17) |
| T8 | Cross-compilation | `chimi build --target` accepts the host only; other targets need toolchain installs |
| T9 | Self-hosting compiler | long-term goal of principle 15 |

### Deliberately deferred

Macros and compiler plugins, pluggable type systems or runtimes, public effect
handlers, STM, distribution and clustering, hot code upgrade, persistent
queues, and application-level FFI. Principle 13 keeps these out until concrete
programs need them.

## Plan of attack

Each phase ends with a forcing program that exercises it end to end. Phases
overlap only where noted.

| Phase | Scope | Forcing program |
| --- | --- | --- |
| 0. Runtime memory | R1 prototype, then per-owner heaps in `runtime.c`, re-enabling per-pass reclamation under the scheduler | `concurrency-pipeline` in flat memory |
| 1. Core language | G4, G2, G3, G5, G30, G32, G15, L1, L3, L4, L6, G26, G8, G9, G10, G7 | trace-summary rewritten idiomatically |
| 2. Std in Chimi | G1 with G20 designed alongside; port List, Text, Map, Json helpers; `std/option`, `std/list`, `std/text`, `std/order`, `std/set`, `std/decimal` (L2) | std test suite |
| 3. Time and failures | `Duration`, `Instant`, `Clock`, timers, `select`, virtual time (G14); G13, G25; `std/result`, `std/retry`, `Task.all`/`race`/`timeout` | retrying worker loop |
| 4. Processes | `ep-f29d47` through its gates, plus G22, G27, G28, G29 | dynamic supervised pool with drain and signals |
| 5. Testing | T1, G16, `std/test` | std and programs tested in Chimi |
| 6. CLI surface | G11, G12, G18, G6, L5, G19; `std/cli`, `fs`, `path`, `exec`, `env`, `terminal`, `signal`; exit status | CLI-tool sketch as a real program |
| 7. Service surface | G21, G20 bindings, G23, G24, G17, O1; `std/net`, `http`, `tls`, `sql`, `log`, `metrics` | HTTP API and background-worker sketches |
| 8. Performance | R2, R3, R4, T6 | published baselines per phase program |
| 9. Toolchain | T2–T5, T7, T8 | a multi-package workspace with docs and lint |
| Later | T9 self-hosting | compiler in Chimi |

Order rationale:

- **Memory first** because every later phase adds long-lived or concurrent work
  that the shared heap cannot reclaim, and the prototype is already scaffolded.
- **Core language before std** because writing std in Chimi needs equality,
  boolean logic, list patterns, tuples, and multi-clause functions; building
  std first would mean rewriting it.
- **Time before processes** because supervision needs `Duration` and timers,
  and drain (G22) needs deadlines.
- **Testing before the large surfaces** so CLI and service modules ship with
  Chimi-level tests.
- **Performance after the surfaces** except where a phase's forcing program
  shows a regression; each phase records a baseline.
