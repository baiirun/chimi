# Decision Ledger

One line per decision that shapes the language or how it is built, newest
first. Each entry names who decided and where the detail lives. Specs and
tickets hold the full reasoning; this page is for scanning and overruling.

`User` means Byron decided directly. `Coordinator` means the implementation
coordinator decided under the standing instruction to settle minor details
from the philosophy and earlier decisions; any of these can be overruled.

## Language

| Date | Decision | By | Detail |
| --- | --- | --- | --- |
| 2026-11-11 | `Duration.ParseError` is `Empty \| Invalid \| OutOfRange`, all nullary (no copy of the parsed text); `Duration.parse` accepts a fraction in any unit when it is exact, the literal `0`, and nothing else (`-0`, `+`, spaces are `Invalid`). | Coordinator (conventions) and implementer | `time-and-outcomes.md` Text form; `ts-f31a20` |
| 2026-11-11 | Standard library conventions: `Module.verb` plain-verb names, no functional jargon (`List.reduce` replaces `fold`); subject first, callbacks last; conversions owned by the importing type (`Int.to_text`, `T.parse`); `_checked` returns `Option`; `length` for List/Range, `size` for Map/Set. | Coordinator (conventions owner) | `standard-library-conventions.md`; `ts-de8a01` |
| 2026-11-11 | Option helpers `map`, `and_then`, `or`, `unwrap_or`, `unwrap_or_else`, `unwrap_or_fail`, `is_some`, `is_none`; `Result` is storage only (`capture`, `get`, `map`, `map_error`, `unwrap_or`, `to_option`), chaining goes through failure rows. | Coordinator (conventions owner) | `standard-library-conventions.md` |
| 2026-11-11 | Text positions are UTF-8 bytes named as such (`byte_length`, `byte_slice`); display widths count graphemes; no bare `Text.length`. Error payloads carry identifiers and limits, never copies of content input. | Coordinator (conventions owner) | `standard-library-conventions.md` |
| 2026-11-11 | Map/Set enumerate in ascending structural key order; `Debug.crash(message) -> Never` raises a defect on purpose; a fixed set of higher-order collection helpers exists. | Coordinator (conventions owner) | `standard-library-conventions.md` |
| 2026-11-11 | The native runtime archive is built once per compiler, not once per project. | Coordinator | `ts-1db9f0` |
| 2026-11-11 | Module items are `const` / `pub const`, not module `let` (`L2101`); the std registry may mark inert shareable std types (`Duration`, `Process.Spec`, `Supervisor.Member`, `Supervisor.Policy`, `Supervisor.Restarts`) constant-representable. | Coordinator (cross-spec review) | `executable-kernel.md`; `standard-modules-and-constants.md` |
| 2026-11-11 | The std registry may give a runtime-known std type a debug-render override; `Duration` renders as its canonical text (`30s`). | Coordinator (cross-spec review) | `standard-modules-and-constants.md` Debug rendering |
| 2026-11-11 | Nullary constructors are written bare (`None`, `Status.Pending`, `Supervisor.Restart.OnDefect`), never `None()`, in expressions and patterns. | Coordinator (cross-spec review) | `executable-kernel.md` Enums; `ts-dc28b2` |
| 2026-11-11 | Debug rendering normalizes `Float`, `Decimal`, and `Json.Number` so equal values render the same; `Map` entries use an internal canonical order that is not stable. | Coordinator (cross-spec review) | `standard-modules-and-constants.md` Debug rendering |
| 2026-11-11 | A root may keep needs that the runner's provider plan closes (such as `clock`, `Console`); every other need must be provided with `using`. | Coordinator (cross-spec review) | `executable-kernel.md` Program entry |
| 2026-11-11 | The kernel defers to core data for the `Void` value and to `std/result` for storable `Result<T, E>`. | Coordinator (cross-spec review) | `executable-kernel.md` |
| 2026-11-11 | `Process.Address` equality: named equals named with the same declaration, direct equals direct to the same incarnation, named never equals direct. | Coordinator (cross-spec review) | `core-data-and-operators.md` |
| 2026-11-11 | `Float`, `Range`, `std/order`, `std/decimal`, and `std/set` functions are embedded std modules that need imports, not built-in qualifiers. | Coordinator (cross-spec review) | `core-data-and-operators.md` Implementation notes |
| 2026-11-11 | `Duration + Duration` reports `T4303` with a `Duration`-specific help message; `T4344` is retired. | Coordinator (cross-spec review) | `time-and-outcomes.md`; `core-data-and-operators.md` |
| 2026-11-11 | A guard is pure when its failure row and needs row are empty; there is no separate parking rule for guards. | Coordinator (cross-spec review) | `executable-kernel.md` Case expressions; `time-and-outcomes.md` |
| 2026-11-11 | Failure sets of several members are equatable, not ordered; `Result<T, E>` is ordered only when `E` is a single ordered type. | Coordinator (cross-spec review) | `core-data-and-operators.md`; `time-and-outcomes.md` |
| 2026-11-11 | Failure-set syntax examples use function types, not implemented headers with `errors`. | Coordinator (cross-spec review) | `time-and-outcomes.md` Failure-set types |
| 2026-11-11 | `Never` is the empty failure set and is accepted as a failure-set member. | Coordinator (cross-spec review) | `core-data-and-operators.md` `Never`; `time-and-outcomes.md` |
| 2026-11-11 | `Float`, `Decimal`, tuples, `Range`, `Void`, and `Set` are constant-representable. | Coordinator (cross-spec review) | `standard-modules-and-constants.md` Allowed initializers |
| 2026-11-11 | Failure-type patterns may name a struct or a single variant, not only an enum. | Coordinator (cross-spec review) | `patterns-and-function-clauses.md` |
| 2026-11-11 | `std/option` declares `None` before `Some`, so derived ordering puts `None` first. | Coordinator (cross-spec review) | `standard-modules-and-constants.md` Prelude |
| 2026-11-11 | Conformance row E10 uses an annotated `let` instead of call-site type arguments. | Coordinator (cross-spec review) | `core-data-and-operators.md` |
| 2026-11-11 | Failure rows use `\|` in host capabilities too; `T4263` covers literal negative `shutdown_within` and non-positive restart windows; start failures are spelled `Supervisor.StartError.*`. | Coordinator (cross-spec review) | `host-capabilities-and-execution-properties.md`; `process-and-supervisor-static-contracts.md`; `supervisor-policy-and-lifecycle.md` |
| 2026-11-11 | Pending spec examples carry the imports they use (`Duration`, `Process`, `Supervisor`, `Int`, `Text`); the time example uses interpolation. | Coordinator (cross-spec review) | process specs; `patterns-and-function-clauses.md`; `time-and-outcomes.md` |
| 2026-11-11 | `\|>` inserts its left operand as the first argument: `x \|> f(a)` is `f(x, a)`. No pipeable twin APIs. | User | `executable-kernel.md` Pipelines; `ts-5414e7` |
| 2026-11-11 | Literal `Duration.parse("...")` is checked at compile time (`T4349`); a valid literal suggests the constructor. | User | `time-and-outcomes.md` |
| 2026-11-11 | Multi-clause functions are core: adjacent `fn` clauses with the same name, first clause is the header, exhaustive across clauses, no overloading by type or arity. | User (core); Coordinator-approved spec | `patterns-and-function-clauses.md`; `ts-be39c0` |
| 2026-11-11 | No local mutation for now; may be revisited. | User | `language-completion-plan.md` |
| 2026-11-11 | Equality, hashing, and ordering are derived structurally for all data; no traits or protocols unless a real program forces them. | User | `core-data-and-operators.md` |
| 2026-11-11 | Construction and calls are positional everywhere; no argument labels and no defaults. Labels are `T4210`. | User | `executable-kernel.md`; `ts-835fc0` |
| 2026-11-11 | Record update is `{ ..base, field: value }`; braces name fields, parentheses are positional. | Coordinator (spec) | `patterns-and-function-clauses.md` |
| 2026-11-11 | Boolean logic is `and` / `or` / `not`; comparisons do not chain. | Coordinator (spec) | `core-data-and-operators.md` |
| 2026-11-11 | `Int` is 64-bit; overflow and division by zero are defects. `Float` is finite-only (NaN/infinity are defects). `Decimal` has operators but no literal. | Coordinator (spec) | `core-data-and-operators.md` |
| 2026-11-11 | Tuples `(a, b)` read through patterns only; `Void` is a value; `Never` is nameable; ranges `a..b` and `a..=b`. | Coordinator (spec) | `core-data-and-operators.md` |
| 2026-11-11 | Lists are persistent vectors with O(1) slice windows; a window shorter than a quarter of its vector is copied. | Coordinator | `ts-8cb35c`; Phase 1 lane |
| 2026-11-11 | Standard modules are Chimi source embedded in the compiler; `native fn`/`native type` only in `std/`; `List`, `Text`, `Int` need imports. | Coordinator (spec) | `standard-modules-and-constants.md` |
| 2026-11-11 | `const` / `pub const` replace module `let`; evaluated at build time. | Coordinator (spec) | `standard-modules-and-constants.md`; `ts-f47931` |
| 2026-11-11 | Interpolation `"${expr}"` for `Text` and `Int`; `Debug.render` for other values; `Secret<T>` renders as `<redacted>`. | Coordinator (spec) | `standard-modules-and-constants.md` |
| 2026-11-11 | `Duration` is nanoseconds with function arithmetic; `Clock` is a `needs` capability with virtual time in tests; `Select.first` is a library function; `Result<T, E>` with `Ok`/`Err`; failure sets are `A \| B`; causes live outside values. | Coordinator (spec) | `time-and-outcomes.md`; `ts-f31a20` |

## Processes and runtime

| Date | Decision | By | Detail |
| --- | --- | --- | --- |
| 2026-11-11 | Per-owner heaps with loop regions, copy-on-transfer fragments, owner-tagged chunks; supervisors are kernel data, not fibers. | User (approach); Coordinator (details) | `architecture/supervised-process-runtime.md`; `ts-a6b43b` |
| 2026-11-11 | Small in-transit messages share slabs instead of a 64 KiB chunk each. | User | `ts-df67fb` |
| 2026-11-11 | `receive` after cancellation returns already-accepted messages, then `Cancelled` without parking. | Coordinator | `process-execution-and-mailboxes.md` |
| 2026-11-11 | Processes run native-only at first; the interpreter reports them unsupported until it has a scheduler. | Coordinator | `supervised-process-conformance.md`; `ts-b9cbe1` |
| 2026-11-11 | Process cancellation interrupts every park except `Process.receive` while accepted messages remain; the non-zero `shutdown_within` forced path is tested in C (SD04). | Coordinator | `supervised-process-conformance.md` open question 2; `ts-6ccbc9` |
| 2026-11-11 | A non-literal mailbox capacity below 1 defects at `Process.spec`, the call that receives it. | Coordinator | `supervised-process-conformance.md` open question 3; `ts-6ccbc9` |
| 2026-11-11 | Cleanup accepts no new messages, and the body may drain the ones already accepted. | Coordinator | `supervised-process-conformance.md` open question 4; `ts-6ccbc9` |
| 2026-11-11 | Transcripts keep the prototype event set through U8; `supervisor_started` joins them at U9. | Coordinator | `supervised-process-conformance.md` open question 5; `ts-6ccbc9` |
| 2026-11-11 | The lab's clock is `Clock.sleep(Duration)` under a root-closed `clock: Clock` need and runner `--clock=real\|virtual` (default real), native-only; nothing more of `std/clock` yet. | Coordinator | `time-and-outcomes.md`; `ts-6ccbc9`; `ts-80cbfb` |
| 2026-11-12 | `Process.SendError.Full(message)`/`Unavailable(message)` conform to the payload rule: handing a transferred input back is not a copy; `NameTaken(name: Text)`. | Coordinator | `standard-library-conventions.md` Payloads; `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | `process` and `name` are contextual words, read only at module item position before `<identifier>:`. | Coordinator | `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | Process and supervisor range defects use `Function: precondition, got value`. | Coordinator | `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | `T4264` follows module constants, never function calls; what it cannot see is `NameTaken` at start. | Coordinator | `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | A `process name` is a value: reach it as `Module.name`; member imports (`import m.{name}`) do not bind it. | Coordinator | `process-identity-and-routing.md`; `ts-6ccbc9` |
| 2026-11-12 | A value built from an exclusive binding carries it: literals, constructors, blocks, `case` arms, `break`, a field read out of a built value, and a call whose result type can hold the argument's type (callees are not followed; a function, interface, or open type parameter can hold anything). A `let`, loop state, destructuring `let`, or `case` on a built value moves what it carries; a send, `Task.start`, `using` provider, or removal transfers it. A field read or `case` on a binding moves nothing. Supersedes the send-syntax rule (U2). | Implementer | `task-and-channel-capabilities.md` (Moving exclusive values); `process-and-supervisor-static-contracts.md`; `ts-184972` |
| 2026-11-12 | Exclusive moves are checked within a function body only; there are no cross-function transfer summaries (the attempt on `archive/capability-places-summaries` did not converge). A callee that transfers its parameter, a capability duplicated through a function value or generic, and one field read twice into two tasks are caught at run time by the owner token, which defects naming the capability, the violating task, and the owner. | User | `task-and-channel-capabilities.md` (Where each rule is checked) |
| 2026-11-12 | `Channel.Receive<T>` has the concurrent properties of `T`, as `Process.SendError<M>` has those of `M`. | Implementer | `task-and-channel-capabilities.md` (Moving exclusive values); `ts-bb95dc` |
| 2026-11-12 | An inbox escapes "beyond the body" only through the body's `Failure(E)`: a `Process.spec` body whose failure type is not transferable is `T4240`. | Implementer (U2) | `process-and-supervisor-static-contracts.md` (SF22); `ts-6ccbc9` |
| 2026-11-12 | `T4260` for a provider follows needs out of `Process.spec` through named function calls to the `using` that binds them, reported at the provider. | Implementer (U2) | `process-and-supervisor-static-contracts.md` (SF01); `ts-6ccbc9` |
| 2026-11-12 | `T4263` "literal" is syntactic (an integer literal, `Duration.zero`, or a constructor of an integer literal); a constant or arithmetic is checked where it is received. | Implementer | `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | The process data constructors are pure, so constants may hold specs, members, and policies; every other process operation is runtime-only. | Implementer | `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | Lab MF06 (pay-for-play linking) unlocks at U5, when process programs build natively; its no-concurrency and tasks-only half is a `tests/cli.rs` test now. | Coordinator | `supervised-process-conformance.md` Unlock order; `ts-6ccbc9` |
| 2026-11-12 | Lab CB04 unlocks at U7 (it needs `remove_process`); its task analogue (a finished task's channel value survives its heap) is a `tests/cli.rs` test now. | Coordinator | `supervised-process-conformance.md` Unlock order; `ts-6ccbc9` |
| 2026-11-12 | Lab memory assertions live in a hand-written `expected.memory` (`equal <field>` across runs, `at_most <field> <limit>`); MF05 bounds its peak at 7 KiB so batched reclamation fails; `--backend interpreter` reports executed fixtures `unsupported`. | Implementer | `supervised-process-conformance.md` Expected files, Harness entry points; `ts-6ccbc9` |
| 2026-11-12 | Process programs link a runtime variant built with `CHIMI_PROCESS_KERNEL` (prebuilt next to the plain one); `chimi_rt_process_entry` marks it for MF06. The scheduler's own process code stays inside `libchimi_fiber.a`. | Implementer (U5) | `supervised-process-conformance.md` MF06; `ts-6ccbc9` |
| 2026-11-12 | Native specs, members, policies, and restart limits are the interpreter's records; `Supervisor.start`/`start_process` marshal them into the scheduler's C structs, each spec's body copied into a detached fragment that every incarnation copies into its own heap. | Implementer (U5) | `supervised-process-runtime.md`; `ts-6ccbc9` |
| 2026-11-12 | A scheduler name id is the `process name` symbol index plus 1, and that Core symbol carries its trace label. | Implementer (U5) | `runtime.h` `CHIMI_SYMBOL_PROCESS_NAME`; `ts-6ccbc9` |
| 2026-11-12 | Memory report: an allocation stands in for a chunk (`heap_chunks_live` = live allocations); `copied_values`/`copied_bytes` count `Process.send` copies only; `trace_events_constructed` counts events the runner built. | Implementer (U5) | `supervised-process-conformance.md` Heap: memory flatness; `ts-6ccbc9` |
| 2026-11-12 | Scheduler fibers (root, tasks, incarnations) get a guarded 1 MiB stack instead of 64 KiB: about 1,000 nested Chimi calls fit, and 1,000 parked processes peaked at 23 MB resident (Apple M1, debug compiler). A process body's defect is named in its `process_terminated` telemetry (`defect`), `Process stack overflow` for an overflow, which the scheduler flags in the event's `outcome`. | Coordinator (stack size, named overflow); Implementer (details) | `supervised-process-conformance.md` Runner telemetry; `chimi_fiber.h`; `ts-6ccbc9` |
| 2026-11-12 | Each lab fixture run has a 60 s limit, so a runtime that lost a fast path fails instead of hanging. | Implementer (U5) | `compiler/chimi/src/process_lab/fixture.rs`; `ts-6ccbc9` |
| 2026-11-12 | Lab runs pass `--clock=virtual` on every run; `expected.memory` gains `exactly <field> <value>`; IP01 always runs on the interpreter; MF01 rounds N down to whole batches of 64. | Implementer (U5) | `supervised-process-conformance.md` Expected files; `ts-6ccbc9` |
| 2026-11-12 | A mailbox capacity, policy capacity, or restart limit above 4,294,967,295 defects where it reaches the scheduler (`Supervisor.start`, `start_process`); an escalated root answers `Supervisor.wait` with the defect `Supervisor.RestartLimitExceeded`. | Implementer (U5) | `process-and-supervisor-static-contracts.md`; `ts-6ccbc9` |
| 2026-11-12 | A root escalation ends the program with the defect `Supervisor.RestartLimitExceeded` whether main is parked (in `Supervisor.wait` or elsewhere) or runnable: the scheduler ends every parked or runnable root task with code -6, and the runner maps that code to the defect. | Implementer (U6) | `supervised-process-conformance.md` ES01, RA02; `ts-6ccbc9` |
| 2026-11-12 | RA08 counts its restart storm in a hand-written `expected.events` (`<event> [<class fields>] <count>`, classed by cause, reason, send outcome, and attempt), over two `--trace=full` runs that must match apart from `execution_id`. | Implementer (U6) | `supervised-process-conformance.md` Expected files; `ts-6ccbc9` |
| 2026-11-12 | OB04 compares two `--trace=full` runs of every transcript fixture unlocked at `CURRENT_STAGE`, reusing the executable each fixture's own run built in the same lab process (and building only fixtures that process has not run). OB06 proves "no real waiting" with 2m35s of virtual time against the 60 s run limit; its 2 s wall-time bound is a performance gate (`ts-1765ba`), not a default-suite assertion. | Implementer (U6) | `supervised-process-conformance.md` Observability; `ts-6ccbc9` |
| 2026-11-12 | RA06 injects its failed replacement start by defining `mmap` in `kernel_faults.c` (the scheduler's fiber stacks are one `PROT_NONE` anonymous mmap each), so the scheduler ABI gains no test-only entry point; runtime-kind C harnesses link the bundled `libchimi_fiber.a`. | Implementer (U7) | `supervised-process-conformance.md` RA06; `ts-6ccbc9` |
| 2026-11-10 | One `Supervisor.Member` type without the message type; `Supervisor.Restart` is an argument (`Never`/`OnDefect`). | User | `process-and-supervisor-static-contracts.md`; `ts-5961ee` |
| 2026-11-10 | Bounded FIFO mailboxes; `Process.send` never parks and returns the message on `Full`/`Unavailable`; no selective receive. | User | `process-execution-and-mailboxes.md` |
| 2026-11-10 | Names are `process name` declarations; one `Process.Address<M>` with named or direct routing. | User | `process-identity-and-routing.md` |
| 2026-11-10 | `loop state = initial { body }` with `break value` is the canonical repetition; no `continue`. | User | `repetition.md` |

## Process of building

| Date | Decision | By | Detail |
| --- | --- | --- | --- |
| 2026-11-11 | Build the whole language without stopping for minor questions; escalate only vision-level choices. | User | `language-completion-plan.md` |
| 2026-11-11 | Tests assert behaviour and contracts, never implementation details or tautologies. | User | — |
| 2026-11-11 | Fix only review findings that are reachable, break a contract, or carry into kept code. | User | — |
| 2026-11-11 | Compiler implementers may use worktrees merged by the lane coordinator; push `main` after every green checkpoint (`scripts/check-head.sh`). | User | — |
