# Testing

## Status

This is the contract for test declarations, the `chimi test` runner,
`std/test`, deterministic time and randomness in tests, fakes, recorders, and
runnable documentation examples. It covers T1 and G16 of the
[language completion plan](../plans/language-completion-plan.md) and the
testing row of the [standard library map](../plans/standard-library-map.md).
Test declarations are implemented: they parse, format, and type-check
(`N3120`, `L2120`, `L2121`, `T4400`, `T4402`), and builds drop them. The
runner, `std/test`, and the rest are not implemented yet. The build is tracked
by CHI-208; the spec was written under CHI-202.

Four choices here touched questions only Byron could settle: the test
declaration form, stateful fakes and mutation, a built-in assertion form, and
doc-comment examples. He took the recommended option of each on 2026-11-15;
[Settled questions](#settled-questions) keeps the options he chose between.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative.

This contract builds on:

- [executable kernel](executable-kernel.md): needs, `using`, interfaces, the
  program root, and the four terminal outcomes;
- [time and outcomes](time-and-outcomes.md): the `Clock` capability, virtual
  time, cancellation scopes, and `std/result`;
- [standard modules and constants](standard-modules-and-constants.md):
  `Debug.render_indented`, the operation registry, and the std-tests programs
  this runner replaces;
- [task and channel capabilities](task-and-channel-capabilities.md): task
  containment and shareable values.

## Goals

- A test is ordinary Chimi code. Fakes are ordinary values supplied with
  `using`; there is no mocking framework, reflection, or patching.
- Tests are deterministic by default. Time is virtual and randomness is
  seeded, so a failing test fails the same way on every run and machine.
- A failure report says where it failed and shows both values in the debug
  format, without the author writing a message.
- Tests check observable behaviour: values, failures, defects, output, and
  time as the program sees it.
- No new extension mechanism. Nothing here needs macros, attributes, or
  compiler plugins.

Out of scope: benchmarks (Phase 8), property-based generators, snapshot
files, coverage reports, and test fixtures that own resources (they wait for
G12 scope-owned resources).

## Example

<!-- chimi: pending CHI-208 -->
```chimi
import std/debug as Debug
import std/result as Result
import std/result.{Err}
import std/test as Test

enum ChargeError {
  Declined(reason: Text)
}

interface Payments {
  charge: fn(Int) -> Text errors ChargeError
}

fn checkout(cents: Int) -> Text needs { payments: Payments } {
  case cents > 0 {
    True -> payments.charge(cents)
    False -> Debug.crash("checkout: cents must be positive, got ${cents}")
  }
}

test "charges the total once" {
  let charges = Test.recorder()
  let payments = {
    charge: fn(cents: Int) -> Text {
      Test.record(charges, cents)
      "receipt-1"
    },
  }
  let receipt = checkout(2500) using { payments: payments }
  Test.assert_equal(receipt, "receipt-1")
  Test.assert_equal(Test.recorded(charges), [2500])
}

test "passes a declined charge through" {
  let payments = {
    charge: fn(_cents: Int) -> Text { Failure(ChargeError.Declined("expired")) },
  }
  let outcome = Result.capture(fn() { checkout(100) using { payments: payments } })
  Test.assert_equal(outcome, Err(ChargeError.Declined("expired")))
}

test "rejects a total that is not positive" {
  let payments = {
    charge: fn(_cents: Int) -> Text { Test.fail("charge must not be called") },
  }
  Test.capture_defect(fn() { checkout(0) using { payments: payments } })
  |> Test.assert_equal("checkout: cents must be positive, got 0")
}
```

`chimi test` finds the three tests, runs each in its own execution, and
prints:

```text
test app/checkout "charges the total once" ... ok
test app/checkout "passes a declined charge through" ... ok
test app/checkout "rejects a total that is not positive" ... ok

3 passed, 0 failed
```

## Test declarations

A test is a module item:

```text
test "<name>" { <block> }
```

- `test` is a contextual word, like `process` and `name`
  ([decision](../decisions.md), 2026-11-12). It is read as a keyword only at
  module item position when a text literal follows it, so `test` stays usable
  as an identifier everywhere else.
- The name is a plain text literal, without interpolation (`L2121`). It MUST
  be unique within its module (`N3120`). It is the test's name in reports and filters.
- A test item takes no visibility modifier (`L2120`) and is not a value. Code
  cannot reference, call, or import a test.
- A test MAY appear in any module, including a module that also declares
  `main`. A test sees every item of its own module, public or private, so
  private helpers are tested where they live. A module that holds only tests
  and imports the module under test is equally valid; it sees only public
  items, as any importer does.
- The formatter formats a test item like a function declaration with no
  parameters: one blank line between items, the block indented two spaces.

The body is a block, typed like the body of a function with no parameters and
an inferred contract:

- Its success type MUST be `Void` (`T4402`). A block whose last expression is
  a value the author forgot to check, such as `checkout(100)`, is the mistake
  this rule catches. Assertions return `Void`, so a body that ends in one
  satisfies it.
- Its failure row is unrestricted. An expected failure that escapes the body
  fails the test, so a test may call fallible code directly and the failure
  joins the body's row without any unwrapping.
- Its needs row MAY hold only needs that the [test provider
  plan](#the-test-provider-plan) closes. Any other need MUST be supplied with
  `using` inside the body (`T4400`).

`chimi check` type-checks test items with the rest of the module, so tests
cannot rot. `chimi build` and `chimi run` drop test items before lowering:
they add no code, data, or symbols to an executable, and an import used only
by test items is not an unused import.

## Test outcomes

Each test runs as its own execution, with its own root task, scheduler, heap,
time source, and random source. Nothing a test does can reach another test:
Chimi has no mutable globals, and module constants are immutable.

A test **passes** when its body returns `Void`. It **fails** with one of these
kinds:

| Kind | When |
| --- | --- |
| `assertion` | an assertion in `std/test` did not hold, or `Test.fail` was called |
| `failure` | an expected failure escaped the body; the report renders the failure value |
| `defect` | any other defect ended the root, with its message and context |
| `cancelled` | the root was cancelled |
| `deadlock` | under virtual time, every task parked with no timer pending ([virtual time](time-and-outcomes.md#virtual-time)); the report lists the parked tasks |
| `time_limit` | the test ran longer than the runner's wall-clock limit |

A test body's own children follow the ordinary task rules: when the body
returns, its remaining children are cancelled and drained before the test is
reported, and a child defect becomes the test's defect. A supervisor the test
started and did not stop is stopped at the end of the body, as at the end of
`main`.

## The test provider plan

A test root has a fixed provider plan, as `main` has one
([program entry](executable-kernel.md#program-entry-and-execution)). It closes
exactly these needs:

| Need | Provider in a test |
| --- | --- |
| `clock: Clock` | the execution clock under the **virtual** source by default |
| `random: Random` | a seeded deterministic generator ([Randomness](#randomness)) |
| `console: Console` | a capture: each line is kept with the test's report and shown when the test fails, or always with `--show-output` |

Every other need, including the file system and the network, MUST be supplied
with `using` (`T4400`). A test that wants a real file system supplies one on
purpose; `std/test` will gain temporary-directory providers with G12. The test
plan is fixed: a module cannot add to it.

## Assertions

`std/test` provides assertions as ordinary functions:

| Function | Type |
| --- | --- |
| `Test.assert_equal` | `fn<T>(T, T) -> Void` (actual, expected; `T` equatable) |
| `Test.assert_not_equal` | `fn<T>(T, T) -> Void` (`T` equatable) |
| `Test.assert` | `fn(Bool, Text) -> Void` (condition, message) |
| `Test.fail` | `fn(Text) -> Never` |

- Equality is derived structural equality, the same `==` the program uses. A
  type that is not equatable, such as a function, is rejected at the call
  (`T4300`, "`Test.assert_equal` needs `T` to be equatable").
- The actual value comes first, so a pipeline reads in the order the work
  happens: `parse("12") |> Test.assert_equal(12)`.
- `assert_not_equal` exists because its report renders both values, which
  `Test.assert(a != b, ...)` cannot. There are no forms for other
  comparisons: `Test.assert(count > 3, "count")` covers them, and the message
  says what was being checked.

### An assertion failure is a defect

A failed assertion ends the test with a defect that the runner reports with
the kind `assertion`. It is not an expected failure. This decides two things:

- An assertion works anywhere, including inside a fake. A fake's method may
  expose no failure that its interface does not declare
  ([interfaces](executable-kernel.md#dependency-needs-and-interfaces)), so an
  assertion that failed with `errors Test.Failure` would be rejected inside
  almost every fake. A defect is allowed everywhere.
- Code under test cannot swallow an assertion failure by matching
  `Failure(_)`. Defects bypass recovery arms.

Rust's `assert_eq!`, Go's `t.Fatal`, and Gleam's `should` all end the test the
same way.

### Failure reports

An assertion failure report MUST contain:

- the source location of the assertion call: file, line, and column. The
  registry marks each `std/test` assertion as receiving its caller's location,
  as Rust's `#[track_caller]` does; source cannot read that location;
- both values rendered with `Debug.render_indented`, labelled `actual` and
  `expected`;
- when either rendering spans more than one line, a line diff of the two
  renderings;
- when the renderings are identical but the values differ, the sentence
  "the values differ but render the same". Equal values always render the
  same, but distinct values may not.

```text
test app/checkout "charges the total once" ... FAILED
  assertion failed at app/checkout.chimi:27:3
  actual:
    [
      2500,
      2500,
    ]
  expected:
    [
      2500,
    ]
  diff:
      [
        2500,
    -   2500,
      ]
```

The report format is for people. Tools read the [JSON
output](#machine-readable-output).

## Testing failures and defects

### Expected failures

A test checks an expected failure by storing it with `Result.capture` and
comparing the stored value:

<!-- chimi: fragment -->
```chimi
Result.capture(fn() { Int.parse("") })
|> Test.assert_equal(Err(Int.ParseError.Empty))
```

A literal argument works here because `Int.parse` is not checked at compile
time. `Duration.parse("")` with a literal is `T4349`, so a test of its
failures passes the text through a variable or a helper parameter.

`std/test` adds no `assert_fails` form. `Result.capture` already turns any
outcome into a value, and one rule for comparing values covers successes and
failures alike.

### Defects

`Test.capture_defect` runs an operation and returns the message of the defect
it raised:

```text
Test.capture_defect:
  fn<T, F, R>(fn() -> T errors F needs R) -> Text needs R
```

- The operation runs in the calling task inside a cancellation scope, as
  `Task.timeout` runs its operation
  ([timeouts](time-and-outcomes.md#semantics)). When the operation defects,
  the scope's children are cancelled and drained, the exclusive capabilities
  the scope owns are cleaned up as at task exit, and the defect's message is
  the result.
- When the operation succeeds or fails with an expected failure,
  `capture_defect` fails the test with the kind `assertion`, naming what
  happened instead and rendering the value. The scope has no timer, so it is
  never cancelled on its own.
- An assertion failure inside the operation is not captured. It ends the test
  as usual, so a fake's assertion cannot be mistaken for the defect under
  test.
- Cancellation from outside the scope passes through unchanged.

A defect is a contained crash of a bounded unit of work, and a test is such a
unit. `capture_defect` makes the operation its own smaller unit. It is the
only way source observes a defect, so it is a **test-only operation**:

- `chimi build` and `chimi run` reject a program in which a test-only
  operation is reachable from `main` (`T4401`). Reachability follows the same
  call graph as the build: a helper module that wraps `capture_defect` may be
  imported by test modules, but not by the program.
- `std/test` marks its test-only operations in the registry. They are
  `capture_defect`, the [recorder and script
  operations](#recorders-and-scripted-fakes), and [fake
  addresses](#fake-process-addresses). Assertions are not test-only, so a
  program may assert an invariant with them.

The defect message is the text a defect reports, so a test of a precondition
checks the message that the conventions prescribe ("`Function: precondition,
got value`"), not an implementation detail.

## Time in tests

A test runs under the virtual time source unless the run asks for real time
(`chimi test --clock=real`). The clock rules are those of [virtual
time](time-and-outcomes.md#virtual-time):

- `Clock.now()` starts at the origin, and `Clock.timestamp()` starts at
  `1970-01-01T00:00:00Z` or the run's `--clock-start` value.
- Time advances only when every task is parked and a timer is pending, so a
  test of a 30-second timeout takes no real time, and the same test reads the
  same instants on every run.
- Supervisor restart windows and `shutdown_within` deadlines use the same
  source, so supervision is tested without passing a clock anywhere.

<!-- chimi: pending CHI-208 CHI-201 -->
```chimi
import std/clock as Clock
import std/duration as Duration
import std/test as Test

fn wait_twice() -> Void needs { clock: Clock } {
  Clock.sleep(Duration.seconds(30))
  Clock.sleep(Duration.seconds(30))
}

test "virtual time passes only by waiting" {
  let start = Clock.now()
  wait_twice()
  Test.assert_equal(Clock.since(start), Duration.minutes(1))
}
```

A test that needs wall time pinned uses `Clock.starting_at` through `using`,
as [time sources](time-and-outcomes.md#execution-time-source-and-providers)
describes. A fake whose method waits captures the clock from a helper that
declares the need, so the interface method itself needs nothing:

<!-- chimi: fragment -->
```chimi
fn slow_payments(delay: Duration) -> Payments needs { clock: Clock } {
  {
    charge: fn(_cents: Int) -> Text {
      clock.sleep_until(Clock.add(clock.now(), delay))
      "late"
    },
  }
}
```

## Randomness

`std/random` does not exist yet. This section fixes the part of it that
testing depends on; the rest of the module is specified with it.

`Random` is a structural interface in `std/random`, needed under the name
`random`, as `Clock` is needed under `clock`:

<!-- chimi: fragment -->
```chimi
interface Random {
  next_int: fn() -> Int
}
```

`next_int` returns 64 uniformly distributed bits as an `Int`. Std functions
over it declare `needs { random: Random }`. At least these exist:

| Function | Type |
| --- | --- |
| `Random.int` | `fn(Range) -> Int needs { random: Random }`; defects on an empty range |
| `Random.float` | `fn() -> Float needs { random: Random }`; in `0.0` up to but excluding `1.0` |
| `Random.pick` | `fn<T>(List<T>) -> Option<T> needs { random: Random }` |
| `Random.shuffle` | `fn<T>(List<T>) -> List<T> needs { random: Random }` |

Each of them is written in Chimi over `next_int`, so a seeded provider fixes
their results on both backends.

Providers:

- A program's runner closes a root `random` need with a generator seeded from
  the operating system. It is not suitable for keys or tokens; secure
  randomness belongs to `std/crypto`.
- A test's provider is the generator **xoshiro256\*\*** seeded through
  **SplitMix64**. Its seed is derived from the run seed and the test's
  identity (module path and name), so a test draws the same numbers whether it
  runs alone, filtered, or among others, and adding a test does not change
  another test's numbers.
- The run seed is 0 unless `--seed=<n>` sets it. A failure report of a test
  that read its random source names the seed, so the failure can be replayed.
- Programs MAY supply another `Random` through `using`, as with `Clock`.

## Fakes

A fake is any value that satisfies an interface, supplied with `using`. That
is already the language: conformance is structural, and a provider method may
expose fewer failures and needs than its interface declares. A stateless fake
is a record of functions:

<!-- chimi: fragment -->
```chimi
let offline = {
  charge: fn(_cents: Int) -> Text { Failure(ChargeError.Declined("offline")) },
}
```

`std/test` provides no mock objects, no expectation DSL, and no way to replace
a function that is not a need. Code that should be faked takes the dependency
as a need; that is the existing rule for every effect.

A fake for an interface with a generic member (G11), such as `FileSystem`,
is a generic function declared with explicit parameters, because a lambda
cannot promise to work for every type.

### Recorders and scripted fakes

A fake often has to remember what it was called with, or answer a sequence
of calls with a sequence of replies. Chimi has no mutation
([decision](../decisions.md), 2026-11-11), so `std/test` provides two
runtime-owned, append-only types for tests:

| Function | Type |
| --- | --- |
| `Test.recorder` | `fn<T>() -> Recorder<T>` |
| `Test.record` | `fn<T>(Recorder<T>, T) -> Void` |
| `Test.recorded` | `fn<T>(Recorder<T>) -> List<T>` |
| `Test.script` | `fn<T>(List<T>) -> Script<T>` |
| `Test.next` | `fn<T>(Script<T>) -> T` |

- A `Recorder<T>` is a shareable handle to a log owned by the test execution;
  the registry declares the type shareable. `record` requires `T` to be
  shareable, because `recorded` hands the same values out again and an
  exclusive value such as a `Receiver` would get two owners. `recorded`
  returns every value recorded so far, in the order the records linearized. Records
  from several tasks or processes interleave in that order.
- A `Script<T>` is a shareable handle to a queue made from a list. `next`
  removes and returns its first value, so each value has one reader and `T`
  need only be transferable. Calling `next` on an empty script fails
  the test with the kind `assertion`: "script exhausted after N values".
- Both are test-only operations (`T4401`). They give tests one kind of
  observable state without adding a mutable cell to programs. If mutation
  returns to the language, these stay as conveniences or become ordinary
  Chimi.
- Neither renders its contents: `Debug.render` of a recorder is `<Recorder>`.

Byron chose this as [question 2](#q2-stateful-fakes-and-mutation).

### Fake process addresses

Code that sends to a process takes its `Process.Address<M>`. A test can start
the real process under a supervisor, which virtual time makes cheap, or hand
the code a fake address:

| Function | Type |
| --- | --- |
| `Test.mailbox` | `fn<M>(Int) -> (Process.Address<M>, Mailbox<M>)` (capacity) |
| `Test.received` | `fn<M>(Mailbox<M>) -> List<M>` |

- `Test.Mailbox<M>` is its own type, not `Process.Inbox<M>`: `Process.receive`
  does not accept it, and it renders as `<Mailbox>`.
- The address is a third kind of address, beside named and direct, that no
  process owns. `Process.send` to it
  follows the mailbox rules: it never parks, and it returns
  `SendError.Full(message)` once `capacity` messages are held.
- `received` returns every message delivered so far, in delivery order, and
  removes them, which frees capacity.
- Such an address equals only itself and renders as `<Address test>`, a row
  this spec adds to the [debug rendering
  table](standard-modules-and-constants.md#format).
- Both are test-only operations and native-only, as processes are.

## The runner

### Command

```text
chimi test [<path> | --root <project> <module>] [options]
```

`<path>` is a source file or a directory. A directory, or a project root,
means every module under it. Options:

| Option | Meaning |
| --- | --- |
| `--filter=<text>` | run only tests whose `<module> <name>` contains the text; repeatable, any match runs |
| `--backend=native\|interpreter` | default `native` |
| `--clock=virtual\|real` | default `virtual` |
| `--clock-start=<RFC 3339>` | the initial `Timestamp`, default `1970-01-01T00:00:00Z` |
| `--seed=<n>` | the run seed, default 0 |
| `--time-limit=<duration>` | wall-clock limit per test, in the `Duration` text form; default `60s` |
| `--jobs=<n>` | tests run at once; default the number of cores |
| `--show-output` | show captured console output for passing tests too |
| `--json` | write the [machine-readable output](#machine-readable-output) to stdout instead of the report |
| `--trace=off\|boundaries\|full` | trace each test's execution, written per test |

The interpreter has no clock yet, so under `--backend=interpreter` a test that
reaches the clock reports the kind `defect` with the interpreter's existing
message, until the interpreter gains virtual time.

### Discovery and order

- Discovery is static: the runner loads the modules, type-checks them, and
  collects test items. A module that fails to type-check fails the run before
  any test runs, and the diagnostics are printed as `chimi check` prints them.
- A test's identity is its module path and name, written `app/checkout
  "charges the total once"`.
- Reports are printed in module path order, then declaration order, however
  many tests run at once. The same inputs MUST produce the same report apart
  from wall-clock durations.

### Isolation

Each test MUST run as its own execution. The native runner SHOULD build one
test executable per run and start it once per test (`<exe> --test=<index>`),
so a crash in the runtime itself loses one test, not the run. Running several
tests in one OS process is allowed when that guarantee holds.

### Exit status

`chimi test` exits 0 when every selected test passed, 1 when any test failed,
and 2 when the run could not start: a diagnostic, a bad option, or no test
selected by a filter that was given.

### Machine-readable output

With `--json`, the runner writes one JSON object per line, schema
`chimi.test.v1`:

- one `test_finished` record per test, in report order, with `module`,
  `name`, `line`, `outcome` (`passed` or `failed`), `kind` for a failure,
  `message`, `location` for an assertion, `actual` and `expected` renderings
  for `assert_equal`, `output` (captured console lines), `seed`,
  `virtual_elapsed_ns`, and `wall_ns`;
- one final `run_finished` record with `passed`, `failed`, and `seed`.

Fields are added, never renamed, within `v1`.

## Documentation examples

Doc comments are `///` lines directly before an item, as `std/` already
writes them. The documentation generator (T4) owns the rest of their form;
this section fixes only how an example inside one runs. A fenced ```` ```chimi ```` block in a doc comment is an
**example**:

- `chimi test` runs each example as a test named `<item> example <n>`, with `n` counting from 1 in source order.
- An example compiles as its own test module that imports the documented
  module, so it sees public items only, as a reader's code would. Lines at
  the top of the block that are `import` items are that module's imports; the
  rest is the test body: statements that end in `Void`, usually an assertion.
- The formatter formats example code, and `chimi check` type-checks it, so an
  example cannot drift from the item it documents. The documentation
  generator (T4) shows it as written.
- A fenced block with another info string, such as `text`, is not an example.
  A Chimi block that is not meant to run says so with ```` ```chimi fragment ````.

<!-- chimi: fragment -->
```chimi
/// Returns the value inside `option`, or `fallback` when it is `None`.
///
/// ```chimi
/// import std/option as Option
/// import std/test as Test
///
/// Test.assert_equal(Option.unwrap_or(Some(5), 0), 5)
/// Test.assert_equal(Option.unwrap_or(None, 7), 7)
/// ```
pub fn unwrap_or<T>(option: Option<T>, fallback: T) -> T {
  ...
}
```

Markdown documents outside source keep the existing harness
(`tests/doc_examples.rs`), whose annotations check and run them. Byron
chose this as [question 4](#q4-runnable-examples-in-doc-comments).

## Migrating the std-tests programs

The std-tests programs ([testing the standard
library](standard-modules-and-constants.md#testing-the-standard-library-before-a-test-runner-exists))
convert to test items once the runner exists:

- A program that prints `Debug.render` lines and compares them with an
  `.expected` file becomes test items that assert the same values. Its
  directory stays `compiler/chimi/std-tests/<module>/`.
- A program whose point is the runner's own behaviour, such as how a failure
  escaping `main` is reported, stays an expected-output program.
- The Rust integration test runs `chimi test --json` on both backends and
  fails when a test fails or when the two backends disagree on a test's
  outcome. It keeps the rule that every registered operation is reached by
  some test.

## Diagnostics

Codes are provisional until the runner lands. This specification claims
`T4400`–`T4419`, `N3120`–`N3129`, and `L2120`–`L2129`, outside the blocks
[standard modules](standard-modules-and-constants.md#diagnostics) owns.

| Code | When |
| --- | --- |
| `N3120` | two tests in one module have the same name |
| `L2120` | a test item has a visibility modifier |
| `L2121` | a test name is an interpolated text literal |
| `T4400` | a test body needs something the test provider plan does not close and no `using` supplies |
| `T4401` | a test-only `std/test` operation is reachable from a program's `main` |
| `T4402` | a test body's success type is not `Void` |

Each message names the fix: `T4400` names the need and suggests `using`;
`T4401` names the call path from `main` to the operation; `T4402` shows the
type and suggests an assertion.

## Conformance matrix

Each row passes on the native backend. Rows marked "both" also pass on the
interpreter.

| Row | Behaviour | Backends |
| --- | --- | --- |
| Declaration | `test "name" { ... }` parses, formats, and type-checks; `test` stays an identifier elsewhere | both |
| Duplicate name | two tests with one name in a module are `N3120` | both |
| Void body | a body ending in a non-`Void` value is `T4402` | both |
| Unclosed need | a need outside the test plan without `using` is `T4400` | both |
| Build excludes tests | `chimi build` output is byte-identical with and without test items | native |
| Pass and fail | a passing and a failing assertion report `ok` and `FAILED` with the location, renderings, and diff | both |
| Escaped failure | an expected failure leaving the body fails with the kind `failure` and its rendering | both |
| Assertion in a fake | an assertion inside a provider method fails the test | both |
| Capture defect | `capture_defect` returns the message; a success or expected failure fails the test; an inner assertion is not captured | both |
| Test-only from main | `capture_defect`, `record`, or `mailbox` reachable from `main` is `T4401` | both |
| Virtual clock | a 30-second sleep takes no real time and `Clock.since` reads exactly 30s | native |
| Deadlock | a test that parks forever fails with the kind `deadlock`, listing the parked task | native |
| Seeded random | the same test draws the same numbers alone and among others; `--seed` changes them | both |
| Recorder order | records from two tasks come back in linearization order | native |
| Script exhausted | `next` past the end fails with the kind `assertion` | both |
| Fake address | a full test mailbox returns `SendError.Full`; `received` frees capacity | native |
| Isolation | a test that defects does not affect the next test's result | both |
| Report order | `--jobs=1` and `--jobs=8` print the same report | both |
| Exit status | 0, 1, and 2 as specified | both |
| JSON | `--json` records validate against `chimi.test.v1` | both |
| Doc example | a doc-comment example runs as `<item> example 1` and a broken one fails `chimi check` | both |

## Implementation notes

What this needs from other work:

- **Lane K (CHI-201):** the virtual time source for every test, and
  cancellation scopes for `capture_defect`.
- **Lane T (CHI-200):** `std/result` and failure-set types for
  `Result.capture` comparisons.
- **G11 generic interface methods (CHI-104):** fakes for interfaces with
  generic methods.
- **Lane B:** linear-time `Debug.render_indented` for any depth, because
  every assertion renders both values.

Parts:

- `syntax.rs`: the contextual `test` item. `formatter.rs`: its layout.
- `resolution.rs`: test items as unnamed module members, `N3120`, `L2120`.
- `typing.rs`: the test body contract, `T4400`, `T4402`; `T4401` as a
  reachability pass over the program call graph after sealing.
- `operations.rs`: registry flags for test-only operations and for operations
  that receive their caller's location.
- `std/test.chimi`: assertions written in Chimi over a native
  `Test.fail_assertion(location, actual, expected)` core; recorders, scripts,
  and mailboxes as native types.
- `main.rs`: the `test` command. The native test executable's entry takes
  `--test=<index>` and writes one `chimi.test.v1` record.

## Decisions made in this spec

Besides the settled questions, decided from the vision docs and recorded in [decisions](../decisions.md):

- A failed assertion is a defect reported as an assertion failure, not an
  expected failure, so assertions work inside fakes.
- Assertions are ordinary functions with the actual value first; equality is
  derived `==`; no comparison twins.
- Expected failures are tested through `Result.capture`; there is no
  `assert_fails`.
- `Test.capture_defect` is the only way source observes a defect, and it and
  every stateful test helper are test-only (`T4401`).
- Each test is its own execution; tests run in parallel with a deterministic
  report order.
- The test provider plan closes `clock` (virtual), `random` (seeded), and
  `console` (captured), and nothing else. A program's runner plan also closes
  `random`, from operating-system entropy, as it closes `clock`.
- Test randomness is xoshiro256\*\* seeded through SplitMix64 from the run seed
  and the test identity; the run seed defaults to 0.
- A test body's success type is `Void`.

## Settled questions

Byron chose the recommended option (the first) of each on 2026-11-15. The
options stay here as the record of what was weighed.

### Q1. How a test is declared

1. **A `test "name" { ... }` item (recommended).** Zig and Elixir write tests
   this way, and Jest and Vitest read the same with `test("name", ...)`. The
   name is a sentence. Tests can sit beside private code in any module. It
   costs one contextual word.
2. **A naming convention, no new syntax.** Functions named `test_...` in files
   named `*_test.chimi`, as Go, Gleam, and pytest do. Nothing to parse, but
   names become identifiers (`test_rejects_an_empty_cart`), the runner finds
   tests by spelling, and tests cannot see private items of the module under
   test unless they live in it.
3. **An attribute on a function: `@test fn rejects_empty_cart() { ... }`**, as
   Rust, Java, and Kotlin do. Familiar, but it introduces attributes, a
   general annotation mechanism that the pluggability question has not
   settled.

### Q2. Stateful fakes and mutation

1. **Test-only recorders and scripts in `std/test` (recommended).** A
   runtime-owned, append-only log and reply queue, usable only from tests
   (`T4401`). Fakes that remember calls are one line each today; programs
   still have no mutable state, and the mutation question stays open.
2. **No hidden state: record through a channel.** The test makes a bounded
   channel, the fake captures the sender, and the test drains the receiver.
   Works with today's kernel and adds nothing, but every recording fake must
   size the channel and handle `Full` inside a method whose interface cannot
   expose it, and scripted replies need a process.
3. **Decide mutation now.** Bring back a scoped mutable cell for all code;
   recorders become ordinary Chimi. Settles a larger question early, on the
   evidence of tests alone.

### Q3. A built-in assertion form

1. **Library functions only (recommended).** `Test.assert_equal(actual,
   expected)` and `Test.assert(condition, message)`. Reports show both values
   rendered, so `assert_equal` needs no message; a bare `assert` needs one.
   Revisit if the Phase 5 forcing program shows message noise.
2. **A compiler-known `assert <expression>` form.** On failure it prints the
   source of the condition and the value of each sub-expression, as pytest,
   power-assert, Swift's `#expect`, and Elixir's `assert` do. No message
   argument and better reports, but it is a macro-like form only the compiler
   can have.

### Q4. Runnable examples in doc comments

1. **Fenced ```` ```chimi ```` blocks in `///` comments run as tests
   (recommended).** As Rust, Elixir, and Python doctests do, and as this
   repository already checks the examples in its Markdown docs. The example
   sits with the item a reader is looking at; the toolchain formats and
   type-checks it.
2. **Examples as separate items**, such as `example Option.unwrap_or { ... }`,
   which the documentation generator attaches to the item, as Go's `Example`
   functions and Kotlin's samples work. The code is ordinary source for the
   formatter, rename, and the language server; doc comments stay prose; it
   costs a second contextual item form.
3. **No runnable examples in docs.** Docs link to tests. Least machinery, but
   examples in prose can drift.
