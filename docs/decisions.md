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
