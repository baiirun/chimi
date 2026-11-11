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
