# Standard Library Conventions

## Status

Normative contract for `prog show ts-de8a01`, under the standard library epic
`ep-a6f138`. It owns every convention that more than one standard module
shares: naming, argument order, failure types, `Option` and `Result` helpers,
collection functions, comparators, text positions, native declarations, and
documentation. Phase 2 module authors MUST follow it and MUST NOT decide these
questions in a module spec. A module that needs a different rule changes this
document first.

This document exists because parallel specs without one owner per
cross-cutting concept produced 19 contradictions in phase 1
([orchestration AAR](../process/orchestration-aar.md), finding N1).

Nothing here is implemented beyond what other specifications already
implement. The reference listings are signatures only. Source spelling is
provisional, as in the other specifications.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative.

This contract builds on, and does not restate:

- the [decision ledger](../decisions.md), which binds it;
- [standard modules and constants](standard-modules-and-constants.md):
  the embedded tree, `native fn`, `native type`, the operation registry, the
  prelude, interpolation, and debug rendering;
- [core data and operators](core-data-and-operators.md): equatable and
  ordered properties, `Ordering`, comparators, numbers, `Text`, and `Range`;
- [executable kernel](executable-kernel.md): module-qualified use, pipelines,
  positional calls, failure rows, and `case`;
- [patterns and function clauses](patterns-and-function-clauses.md):
  failure-type patterns and list patterns;
- [time and outcomes](time-and-outcomes.md): `Result`, failure-set types,
  `Cause`, and combinator argument order;
- [repetition](repetition.md): `loop` and `break`.

## Goals

- A programmer who knows JavaScript, Python, Go, Gleam, or Elixir guesses the
  name of a standard function on the first try, and reads every name aloud
  without a glossary.
- Every standard function works in a pipeline: `x |> M.f(a)` is `M.f(x, a)`.
- A failure from the standard library keeps its variant all the way to the
  caller that handles it, and a failure-type pattern covers it in one arm.
- A callback passed to a standard function can fail and can need providers,
  and the caller's contract shows both, with no special syntax.
- A module author writes a new module from the skeleton in this document and
  makes no naming or shape decision that another author could make
  differently.

## Summary of decisions

| Topic | Decision |
| --- | --- |
| Function names | `Module.verb` in `snake_case`; plain verbs; no prefix that repeats the module (`List.length`, not `List.list_length`) |
| Conversions | `T.to_text`, `T.parse`, `T.to_<target>`, `T.from_<source>`, owned by the module that depends on the other type |
| Checked arithmetic | `_checked` suffix returns `Option<T>`; no `_unchecked` forms |
| Predicates | `is_<state>` for a state, a third-person verb for a relation (`contains`, `starts_with`, `has_key`) |
| Constructors | `empty`, `from`, `of`, unit names, or a domain verb; never `new`; `pub const` for fixed non-generic values |
| Argument order | subject first, then configuration, then callbacks last |
| Failure types | one enum per operation family per module, named `<Family>Error`; variants name the condition |
| Failure payloads | named fields; identifiers yes, content no; equatable and shareable |
| `Option` helpers | `map`, `and_then`, `or`, `unwrap_or`, `unwrap_or_else`, `unwrap_or_fail`, `is_some`, `is_none` |
| `Result` helpers | storage only: `capture`, `get`, `map`, `map_error`, `unwrap_or`, `to_option`, `error`, `is_ok`, `is_err`; no `and_then` |
| Aggregation | `List.reduce(list, initial, step)`; no `fold` |
| Effects over items | `List.each` |
| Laziness | none; every collection function is eager; streams are a later, separate module |
| Sorting | `sort` uses structural order, `sort_by` takes a comparator; both stable |
| Text positions | UTF-8 byte offsets, with `byte` in the function name; counting for display uses graphemes |
| Counts | `length` for sequences (`List`, `Range`), `size` for `Map` and `Set`; `Text` has no bare `length` |
| Native code | Chimi by default; `native fn` only for runtime representation, host access, or a measured hot path |
| Docs | `///` comments with a fixed shape: summary, then `Fails with`, `Defects when`, `Calls`, `Cost` |

## Naming

### Module functions

- A public function is called as `Module.verb`, as Principle 4 requires. Its
  name is `snake_case`.
- The name MUST NOT repeat the module or the subject type. Write
  `List.length` and `Text.trim`, not `List.list_length` or `Text.trim_text`.
- The name SHOULD be a plain verb or verb phrase that a working programmer
  reads at once: `map`, `filter`, `split`, `join`, `sort`, `send`. A noun is
  acceptable for a pure query that returns a part of the subject: `first`,
  `keys`, `start`.
- A name MUST NOT come from functional-programming or category-theory jargon
  when a plain verb exists. `fold`, `traverse`, `sequence`, `bind`, `pure`,
  `fmap`, `cata`, and `zip_with` are not standard names. `reduce` and
  `flat_map` are acceptable, because JavaScript and Python programmers use
  them daily.
- Two modules that offer the same operation MUST use the same name and the
  same argument shape: `List.map` and `Option.map`, `List.reduce` and
  `Map.reduce`, `List.contains` and `Text.contains`. A reader who learns one
  module learns the others.
- A function MUST NOT take a `Bool` that selects between two behaviors. Write
  two functions (`trim_start` and `trim_end`) or take a closed enum. The
  README's "pattern matched function overloads instead of control booleans"
  rule applies to the standard library first.
- Std MUST NOT define a second, function-returning form of a function only
  to make it pipeable, as [time and outcomes](time-and-outcomes.md#argument-order)
  already requires.

### Conversions

A conversion between two types is a function in exactly one module: the
module that would otherwise import the other. The more fundamental type never
learns about the richer one.

| Conversion | Name | Owner | Why |
| --- | --- | --- | --- |
| `T` to user-facing text | `T.to_text(value) -> Text` | `T`'s module | `std/text` imports nothing |
| Text to `T` | `T.parse(text) -> T errors T.ParseError` | `T`'s module | the same |
| `T` to a more fundamental `U` | `T.to_<u>(value) -> U` | `T`'s module | `Float.to_int`, `Duration.to_seconds` |
| A more fundamental `U` to `T` | `T.from_<u>(value) -> T` | `T`'s module | `Float.from_int`, `Bytes.from_text` |
| Many values to one | `T.from(values) -> T` | `T`'s module | `Map.from(pairs)`, `Set.from(list)` |

Rules:

- `to_text` is the one name for a canonical, user-facing text form. Std MUST
  NOT also define `to_string`, `format`, or `show`. A type without one obvious
  text form has no `to_text`, and callers use `Debug.render` for diagnostics.
- `parse` is the one name for reading a value from text. When the text form
  comes from `to_text`, `T.parse(T.to_text(value))` MUST return `value`.
- Std MUST NOT offer both directions from both modules. `Text.from_int` does
  not exist, because `Int.to_text` does.
- A conversion that can lose information says so in its name or fails:
  `Float.to_int` fails with `NotInteger`, and `Float.round` is the visible,
  separate rounding step. A lossy text decode is `Bytes.to_text_lossy`, not a
  silent replacement inside `Bytes.to_text`.

The sketches' `Text.from_utf8_lossy` becomes `Bytes.to_text_lossy` under this
rule, because `std/bytes` depends on `Text` and not the reverse.

### Checked forms

- An operator that defects on overflow or division by zero has named forms
  with the `_checked` suffix that return `Option<T>`: `Int.add_checked`,
  `Int.divide_checked`. `None` means the exact result does not exist.
- `_checked` forms MUST NOT fail through the failure row. Overflow is not an
  expected failure of a business operation, and `Option` keeps the call site
  local.
- Std MUST NOT define `_unchecked`, `_unsafe`, or `_wrapping` names for
  ordinary arithmetic. Wrapping arithmetic for hashing belongs to the later
  bit-operations work, as [core data](core-data-and-operators.md#int)
  records, and will use plain names such as `Int.wrapping_add` only there.
- A function that already fails through its row, such as `Int.parse`, has no
  `_checked`, `_or_default`, or `try_` twin. The caller converts with `case`
  or `Result.capture`.

### Predicates

- A predicate returns `Bool`, never fails, needs nothing, and is pure, so a
  `case` guard can call it.
- A predicate about the state of one value is `is_<state>`: `is_empty`,
  `is_some`, `is_none`, `is_ok`, `is_err`.
- A predicate about a relation between the subject and another value is a
  third-person verb: `contains`, `starts_with`, `ends_with`, `has_key`.
- A predicate over a callback is `any` or `all`.
- Names MUST NOT end in `?` or start with `has_` for a state. `has_key` is a
  relation, because it names the other value.

`contains` always means "is this element in this container":
`List.contains(list, item)`, `Set.contains(set, item)`,
`Range.contains(range, n)`, and `Text.contains(text, needle)`. `Map` uses
`has_key`, because "contains" is ambiguous between keys and values.

### Constructors

A type whose representation is private is built by module functions. They
are named by what distinguishes the value:

| Kind | Name | Examples |
| --- | --- | --- |
| The empty collection | `empty()` | `Map.empty()`, `Set.empty()` |
| From another value or a collection of parts | `from(value)` | `Map.from(pairs)`, `Set.from(items)` |
| From raw component parts | `of(parts...)` | `Decimal.of(1999, 2)` |
| From a quantity in a unit | the unit, plural | `Duration.seconds(30)`, `Duration.milliseconds(5)` |
| Allocating a runtime resource | a domain verb or adjective | `Channel.bounded(8)`, `Task.start(thunk)` |
| One obvious wrapping constructor | the type name, lowercase | `IO.path("a.txt")` |

Rules:

- Std MUST NOT use `new`. Chimi values have no identity, so `new` suggests an
  allocation and an object that do not exist.
- A fixed, non-generic value is a `pub const`, not a function with no
  arguments: `Duration.zero`, not `Duration.zero()`. A generic value cannot be
  a constant ([constants](standard-modules-and-constants.md#syntax-and-visibility),
  `T4324`), so `Map.empty()` stays a function.
- A struct with public fields is built with its positional constructor.
  Std MUST NOT add a `make_x` wrapper that only forwards its arguments.
- A constructor that validates its input fails with the module's error type
  or returns `Option`, following the absence and failure rules below.

### Counts and sizes

- `length` counts elements of an ordered sequence: `List.length`,
  `Range.length`.
- `size` counts entries of a keyed or unordered collection: `Map.size`,
  `Set.size`. This is JavaScript's split between `Array.length` and
  `Map.size`, and it matches the existing `Map.size` example in
  [standard modules](standard-modules-and-constants.md#native-declarations).
- `Text` has no bare `length`, because "length" of text means bytes to a Go
  or Rust programmer and characters to a Python or Swift programmer. See
  [Text](#text).

### Absence, failure, and defect

Every standard function picks one of three outcomes for the input it cannot
serve. The rule is the same in every module:

| Situation | Outcome | Examples |
| --- | --- | --- |
| "Not there" is a normal answer to the question asked | `Option<T>` | `Map.get`, `List.first`, `List.find`, `Text.strip_prefix`, `Int.add_checked` |
| The input comes from outside the program, or a resource refused | expected failure in the row | `Int.parse`, `Duration.parse`, file and network operations |
| The caller broke a precondition that it could check cheaply | defect | `List.chunks(list, 0)`, `Text.split(text, "")`, a `Text.byte_slice` bound inside a UTF-8 sequence |

A function MUST NOT return `Option` to report why something failed, and MUST
NOT fail through the row for a condition that is a plain lookup miss.

Each defect MUST name the function and the broken precondition in its
message, as in ``List.chunks: size must be positive, got 0``.

## Argument order

### Subject first

`|>` inserts its left operand as the first argument
([pipelines](executable-kernel.md#pipelines)). Every standard function
therefore takes its **subject** first. The subject is the value the function
is about: the collection, the text, the option, the result, the duration, or
the operation thunk of a combinator.

<!-- chimi: check -->
```chimi
import std/list as List
import std/text as Text

struct Order {
  id: Text,
  open: Bool,
}

fn open_ids(orders: List<Order>) -> Text {
  orders
  |> List.filter(fn(order) { order.open })
  |> List.map(fn(order) { order.id })
  |> List.sort
  |> Text.join(", ")
}
```

Rules:

- The subject is first even when it is a function. In `Task.timeout(operation,
  limit)` and `Retry.run(operation, policy)` the operation is the subject.
- After the subject come configuration values, in the order an English
  sentence names them: `Text.replace(text, old, new)`,
  `Text.pad_start(text, width, fill)`, `List.reduce(list, initial, step)`.
- A callback that the function calls once per element, or once per call, is
  the **last** argument. A multi-line closure then closes the call:

  <!-- chimi: fragment -->
  ```chimi
  List.reduce(prices, 0, fn(total, price) {
    total + price
  })
  ```

- A function with more than three arguments after the subject, or with two
  adjacent same-typed arguments whose order is not fixed by English, SHOULD
  take a configuration struct instead. Calls are positional with no labels
  ([decisions](../decisions.md), `T4210`), so a struct with named fields is
  the only way to name an argument at the call site. `Retry.policy(schedule,
  retryable)` and `Schedule.exponential(attempts, first, limit)` are at the
  limit.
- When two arguments have the same type, the doc comment MUST say which is
  which in its first sentence.

### Callback shapes

A callback's parameters come in the order the function holds them:

| Function | Callback |
| --- | --- |
| `List.map`, `List.filter`, `List.each`, `List.find`, `List.any`, `List.all` | `fn(item)` |
| `List.reduce`, `Range.reduce` | `fn(state, item) -> state`, as JavaScript's `reduce` and the `loop` state |
| `Map.reduce` | `fn(state, key, value) -> state` |
| `Map.each`, `Map.filter` | `fn(key, value)` |
| `Map.map_values` | `fn(value)` |
| `Map.update` | `fn(Option<value>) -> value` |
| `List.sort_by`, `List.min_by`, `List.max_by` | a comparator `fn(a, b) -> Ordering` |

Callbacks receive a tuple only where a function produces tuples, such as
`List.enumerate`. A callback MAY destructure it in its parameter:
`fn((index, item)) { ... }`.

## Failure types

### Shape

- A module declares one enum per **operation family**: the set of failures
  one group of its functions can produce. `std/int` has `ParseError`.
  `std/float` has `ParseError` and `ConversionError`. `std/io` has
  `FileError` and `ConsoleError`.
- An enum with several variants is named `<Family>Error`. A failure type
  with one condition is named for that condition, as `Task.Timeout` is.
- The name is module-relative. Write `ParseError`, not `IntParseError`:
  the qualifier `Int.ParseError` already says which one.
- Std MUST NOT declare a shared error enum such as `StdError` or `IoError`
  that several modules raise. Failure rows compose with `|`, so a shared
  enum would add variants to every row that the function cannot produce, and
  failure-type patterns would stop telling callers what can happen.
- Error types MUST be declared in source, not built in
  ([standard modules](standard-modules-and-constants.md#native-declarations)).
  A native operation that constructs one lists it as a runtime-known
  declaration.

### Variants

- A variant is `PascalCase` and names the **condition** the caller branches
  on: `Empty`, `InvalidDigit`, `OutOfRange`, `NotInteger`, `Closed`,
  `TimedOut`. For a resource operation whose failures differ by step, the
  variant MAY name the step: `FileError.Open`, `FileError.Read`.
- A variant MUST NOT end in `Error` and MUST NOT repeat its type name:
  `ParseError.Invalid`, not `ParseError.InvalidParse` or
  `ParseError.ParseError`.
- A nullary variant is written bare, never with `()`
  ([decisions](../decisions.md)): `ParseError.Empty`.
- Two public enums in one module SHOULD NOT share a variant name. When the
  shared name reads best, as `OutOfRange` does in `std/float`, the variant is
  reachable only through its type, as `Float.ParseError.OutOfRange`. Std
  docs and examples always write variants type-qualified, so they read the
  same whether or not a name is shared.

### Payloads

- A payload uses named fields, as every enum variant does:
  `Open(path: Path, message: Text)`.
- A payload carries facts the caller does not already have and needs to
  report or recover: a limit (`TimedOut(limit: Duration)`), an operating
  system message, a position in the input, or the identifier of an external
  resource (a path, a URL, a key, an address). The identifier is included
  because the failure often surfaces far from the call that named it.
- A payload MUST NOT copy content input, such as the text being parsed.
  The caller already holds it, it can be large, and it can be a secret that
  would then reach traces and logs. The caller adds "which value" with
  `Cause.context`, which is the designed place for that information
  ([failure context](time-and-outcomes.md#adding-context)).
- A failure that hands a transferred input back carries it. This is not a
  copy: the operation took ownership of the value, and the payload returns
  it, as `Channel.SendError.Closed(value)` and
  `Process.SendError.Full(message)` do. Such a field has the properties of
  the value it returns, so the equatable and shareable rule below does not
  apply to it.
- A payload MUST NOT hold a cause. Causes live outside values in `Cause`.
- Every payload field MUST be equatable, shareable, and constant-representable.
  Failures are stored in `Result`, sent through channels, and compared in
  tests. A payload SHOULD also be ordered, so that `Result<T, E>` with a
  single error enum is ordered.
- A message field is `message: Text` and holds text from outside Chimi, such
  as an operating-system error. Std code MUST NOT put its own English
  sentence in a payload in place of a variant.

### Rows, coverage, and failure sets

- A standard function's row lists only the variants it can produce. A
  native declaration states that row exactly (`T4320`). A Chimi function's
  row is inferred. Std MUST NOT widen a row to a whole enum for symmetry.
- A standard function that calls another one lets the callee's failures
  through unchanged. It wraps a failure into its own variant only when the
  caller needs a fact to branch on that the callee's failure lacks. It then
  keeps the callee's failure as a field, so that nothing is lost.
- Callers handle a whole family with one failure-type pattern, and combine
  families with `|`:

<!-- chimi: pending ts-0ba63e ts-80cbfb -->
```chimi
import std/int as Int
import std/int.{ParseError}
import std/text as Text

enum PortError {
  Missing
  NotANumber
  OutOfRange(port: Int)
}

fn port(value: Option<Text>) -> Int {
  case value {
    None -> Failure(PortError.Missing)
    Some(text) -> case Int.parse(Text.trim(text)) {
      Success(port) if port >= 1 and port <= 65535 -> port
      Success(port) -> Failure(PortError.OutOfRange(port))
      Failure(_: ParseError) -> Failure(PortError.NotANumber)
    }
  }
}
```

### Conformance of existing error types

| Type | Status |
| --- | --- |
| `Int.ParseError` (`Empty`, `InvalidDigit`, `OutOfRange`) | conforms |
| `Float.ParseError`, `Float.ConversionError` | conforms; `OutOfRange` is type-qualified |
| `IO.FileError` (`Open(path, message)`, ...) | conforms; `path` is an identifier |
| `IO.ConsoleError.WriteFailed(stream, message)` | conforms |
| `Channel.SendError` (`Closed`, `Aborted`) | conforms |
| `Task.Timeout.TimedOut(limit)` | conforms; a one-condition type |
| `Duration.ParseError` (`Empty`, `Invalid`, `OutOfRange`) | conforms |
| `Process.SendError` (`Full(message)`, `Unavailable(message)`) | conforms; `message` hands the transferred input back |
| `Supervisor.StartError` (`CapacityReached`, `NameTaken(name)`, `Stopped`) | conforms; `name` is an identifier (the `process name` label) |
| `Json.NumberError` (`NotInteger`, `OutOfRange`) | does not conform: rename to `Json.ConversionError` to match `Float.ConversionError` when `std/json` moves to source (migration step 6) |

## `Option` and `Result` helpers

### Why helpers exist

Chimi has no `if` expression and no propagation operator. Without helpers,
every default value and every lookup chain is a `case` with two arms. The
sketches already reached for `Option.unwrap_or` after every `Map.get`.
Helpers stay a small, fixed set. Anything with more than one decision is a
`case`.

### `Option`

| Need | Chimi | Rust | Gleam | Java `Optional` | Swift |
| --- | --- | --- | --- | --- | --- |
| Transform a present value | `map` | `map` | `option.map` | `map` | `map` |
| Chain a lookup that may miss | `and_then` | `and_then` | `option.then` | `flatMap` | `flatMap` |
| First present of two | `or` | `or` | `option.or` | `or` | `??` |
| Value or a default | `unwrap_or` | `unwrap_or` | `option.unwrap` | `orElse` | `??` |
| Value or a computed default | `unwrap_or_else` | `unwrap_or_else` | `option.lazy_unwrap` | `orElseGet` | `??` |
| Value or an expected failure | `unwrap_or_fail` | `ok_or` then `?` | `option.to_result` | `orElseThrow` | `guard let` |
| Test | `is_some`, `is_none` | `is_some`, `is_none` | `option.is_some`, `option.is_none` | `isPresent`, `isEmpty` | `!= nil` |

`and_then` is chosen over Gleam's `then` because `Order.then` already means
"compare by this, then by that", and one word MUST NOT carry two meanings in
the standard library. `unwrap_or_fail` turns absence into a failure in the
row, which is how most service code meets a missing record:

<!-- chimi: pending ts-0ba63e ts-80cbfb -->
```chimi
import std/map as Map
import std/map.{Map}
import std/option as Option

enum LookupError {
  UnknownUser(id: Int)
}

fn display_name(names: Map<Int, Text>, id: Int) -> Text {
  Map.get(names, id) |> Option.unwrap_or_fail(LookupError.UnknownUser(id))
}
```

`Option.expect(option, message)`, which defects on `None`, waits for a
source-level defect function ([open question 4](#open-questions)).

### `Result`

`Result<T, E>` is a **storage form**. Live failures compose through failure
rows, which need no helpers. A `Result` exists to be stored in a value, sent
through a channel, or returned by `Task.results`. Its helpers inspect and
reshape stored outcomes:

| Need | Function |
| --- | --- |
| Store an operation's outcome | `Result.capture(operation)` |
| Turn it back into a live outcome | `Result.get(result)` |
| Reshape the stored value or error | `Result.map`, `Result.map_error` |
| Read with a default | `Result.unwrap_or` |
| Read one side as an `Option` | `Result.to_option`, `Result.error` |
| Test | `Result.is_ok`, `Result.is_err` |

Std MUST NOT define `Result.and_then`, `Result.or_else`, or other chaining
helpers. To chain stored results, call `Result.get` inside a function and let
the failure row carry the error, then `Result.capture` the whole function if
it must be stored again.

### Callbacks in helpers

A helper's callback MAY fail and MAY have needs. Its failures join the
helper's own row. They never enter a returned `Option` or `Result`:
`Result.map(result, f)` where `f` can fail with `F` has the type
`Result<U, E> errors F`. A stored error and a live failure stay separate.

## Collections

### Which functions exist

The repetition spec rejects iterators, so traversal is a fixed set of eager
higher-order functions plus `loop`. Every `List` function below has the same
name and argument shape wherever another module offers the operation.

| Need | Chimi | JavaScript | Python | Go `slices` | Gleam `list` | Elixir | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Transform each | `map` | `map` | `map` | — | `map` | `Enum.map` | `map` |
| Keep matching | `filter` | `filter` | `filter` | `DeleteFunc` (inverse) | `filter` | `Enum.filter` | `filter` |
| Transform, keep present | `filter_map` | — | — | — | `filter_map` | — | `filter_map` |
| Transform, then flatten | `flat_map` | `flatMap` | — | — | `flat_map` | `Enum.flat_map` | `flat_map` |
| Combine into one value | `reduce` | `reduce` | `functools.reduce` | a `for` loop | `fold` | `Enum.reduce` | `fold` |
| Run for an effect | `each` | `forEach` | a `for` loop | a `for` loop | `each` | `Enum.each` | `for_each` |
| First match | `find` | `find` | `next(...)` | `IndexFunc` | `find` | `Enum.find` | `find` |
| First match, transformed | `find_map` | — | — | — | `find_map` | `Enum.find_value` | `find_map` |
| Position of first match | `find_index` | `findIndex` | — | `IndexFunc` | — | `Enum.find_index` | `position` |
| Any or all match | `any`, `all` | `some`, `every` | `any`, `all` | `ContainsFunc` | `any`, `all` | `Enum.any?`, `Enum.all?` | `any`, `all` |
| Count matches | `count` | — | — | — | `count` | `Enum.count` | `filter().count()` |
| Membership | `contains` | `includes` | `in` | `Contains` | `contains` | `Enum.member?` | `contains` |
| Join lists | `concat` | `flat` | `itertools.chain` | `Concat` | `flatten` | `Enum.concat` | `concat` |
| Pair with index | `enumerate` | `entries` | `enumerate` | — | `index_map` | `Enum.with_index` | `enumerate` |
| Pair two lists | `zip` | — | `zip` | — | `zip` | `Enum.zip` | `zip` |
| Split by predicate | `partition` | — | — | — | `partition` | `Enum.split_with` | `partition` |
| Fixed-size groups | `chunks` | — | `itertools.batched` | `Chunk` | `sized_chunk` | `Enum.chunk_every` | `chunks` |
| Drop repeats | `unique` | — | — | `Compact` (adjacent) | `unique` | `Enum.uniq` | `dedup` (adjacent) |

**The `fold` question.** The combining function is `reduce`, with the initial
value required: `List.reduce(list, initial, step)`. `fold` is the
functional-programming name, used by Gleam, Rust, and Kotlin. `reduce` is the
name JavaScript, Python, and Elixir programmers already use for the same
operation, and it is a plain English verb. The initial value is required
because Chimi has no exceptions, and a reduce over an empty list without one
would need an `Option` result that most callers would unwrap. Std MUST NOT
define `fold`, `fold_left`, `fold_right`, `inject`, or `aggregate`.

`concat` joins a list of lists, as `Text.concat` joins a list of texts.
`append` joins exactly two values of the same type, as `Text.append` does.
`push` adds one element at the end. The sketches' `List.flatten` is
`List.concat` under this rule.

Type-specific aggregates live with the element type, because Chimi has no
overloading: `Int.sum(numbers)` and `Float.sum(numbers)`, not `List.sum`.
Ordered aggregates live in `std/list` because they need only the ordered
property: `List.min`, `List.max`.

### Indexes and slices

- An index is a zero-based `Int`. A negative index is never counted from the
  end.
- A read at an index that may be out of range returns `Option`: `List.get`,
  `List.first`, `List.last`.
- A slice takes a `Range` and clamps it to the list's bounds, as JavaScript's
  and Python's slices do: `List.slice(list, 2..5)`. It never fails or defects.
  `List.take(list, n)` and `List.drop(list, n)` clamp the same way.
- A write at an index MUST have a valid index. `List.set(list, index, value)`
  with an index outside `0..List.length(list)` is a defect, because the
  caller can check the length and the alternative would make every write
  return an `Option`.

### How callbacks behave

Every higher-order collection function is generic over its callback's
failure row and needs row, and passes both through unchanged:

```text
List.map: fn<T, U, E, R>(List<T>, fn(T) -> U errors E needs R) -> List<U> errors E needs R
```

Rules for every collection function that takes a callback:

- It calls the callback in the calling task, synchronously, once per
  element, in element order. It never starts a task. Concurrent mapping is
  `Task.map_bounded`, which has a visible bound.
- When a call of the callback fails, the function stops, calls the callback
  no more times, and fails with that failure. Partial results are dropped.
  Defects and cancellation pass through as for any call.
- `find`, `find_map`, `find_index`, `any`, and `all` stop at the first
  element that decides the answer and do not call the callback for later
  elements.
- `sort_by`, `min_by`, and `max_by` call the comparator an unspecified
  number of times in an unspecified order. A comparator with effects is
  allowed, but its effects are not a contract.
- A pure callback makes the call pure, so it can appear in a guard:
  `items if List.any(items, is_urgent) -> ...`.
- The doc comment of each such function MUST state its call order and its
  stopping rule in a `Calls` line (see [Documentation](#documentation)).

### `Range`

A `Range` is a value, not a loop. `std/range` offers the traversals a
counted repetition needs, with the same names and callback shapes as
`std/list`, so counting never builds a list first:

- `Range.each(range, f)`, `Range.map(range, f) -> List<T>`,
  `Range.reduce(range, initial, step)`, `Range.any`, `Range.all`;
- `Range.to_list(range) -> List<Int>`;
- the existing `Range.start`, `Range.end`, `Range.length`, and
  `Range.contains`.

`Range.to_list` lives in `std/range` because `std/range` already imports
`std/list` for `Range.map`. `List.slice` takes a `Range` but needs no Range
function, so `std/list` does not import `std/range`.

### When to write `loop` instead

Use a collection function when one name says the whole traversal. Write a
`loop` when any of these holds:

- the traversal stops early with a value that `find`, `any`, or `all` cannot
  express;
- the state has a "done" flag or a phase that changes how later elements
  are handled. A `reduce` whose state carries a stop flag is always a
  `loop`;
- the work repeats an unknown number of times, as a channel consumer, retry,
  or search does;
- each step needs several pieces of state that a struct would hold anyway.

<!-- chimi: check -->
```chimi
import std/list as List

// One name says the whole traversal.
fn total(prices: List<Int>) -> Int {
  List.reduce(prices, 0, fn(sum, price) { sum + price })
}

// Stops early with a running value: a loop.
fn first_over_budget(prices: List<Int>, budget: Int) -> Option<Int> {
  loop (remaining, sum) = (prices, 0) {
    case remaining {
      [] -> break None
      [price, ..] if sum + price > budget -> break Some(price)
      [price, ..rest] -> (rest, sum + price)
    }
  }
}
```

### Laziness

There is none. Every collection function is eager and returns a complete new
value. Chimi has no iterator protocol and no lazy sequence type in the
collection modules. A later stream module (G18) will be a separate type with
its own module and backpressure. It SHOULD reuse these function names and
callback shapes, so `Stream.map` reads like `List.map`, but it MUST NOT make
`List` functions lazy.

### `Map` and `Set`

- `Map` and `Set` follow the `List` names where the operation is the same:
  `map_values`, `filter`, `reduce`, `each`, `is_empty`, `size`, `from`.
- Writing an entry is `Map.put(map, key, value)`. Changing one through a
  callback is `Map.update(map, key, change)`. Adding to a set is
  `Set.add(set, item)`. Removing is `remove` in both. `Map.merge(base,
  overrides)` keeps the second map's value for a shared key.
- Reading the parts is `Map.keys`, `Map.values`, and
  `Map.entries -> List<(K, V)>`.
- Every function that enumerates a `Map` or `Set` (`keys`, `values`,
  `entries`, `reduce`, `each`, `filter`, `map_values`, and
  `Set.to_list`) visits entries in ascending structural order of the key, and
  requires the key type to be ordered. This keeps output reproducible and
  makes no internal order observable. It is the recommended answer to
  [open question 2](#open-questions), which records the cost.

## Comparators and ordering

[Core data](core-data-and-operators.md#comparators) specifies `std/order`.
The conventions for every module that sorts or compares:

- A bare name uses the structural order and requires an ordered type:
  `List.sort`, `List.min`, `List.max`, `Order.compare`, `Order.min`,
  `Order.max`.
- The `_by` suffix takes a **comparator** `fn(A, A) -> Ordering`:
  `List.sort_by`, `List.min_by`, `List.max_by`. Sorting by a key is
  `List.sort_by(users, Order.by(fn(user) { user.name }))`. Std MUST NOT add a
  separate `sort_by_key`, `sort_on`, or key-extracting `sort_by`.
- Descending order is `Order.reverse(comparator)`. Std MUST NOT add
  `sort_descending` or a direction flag.
- Comparator combinators are subject-first, so a compound order reads as a
  pipeline:

<!-- chimi: check -->
```chimi
import std/list as List
import std/order as Order

struct User {
  name: Text,
  age: Int,
}

fn oldest_first(users: List<User>) -> List<User> {
  let by_age = Order.by(fn(user: User) { user.age }) |> Order.reverse
  let order = by_age |> Order.then(Order.by(fn(user: User) { user.name }))
  List.sort_by(users, order)
}
```

- Every sort in std MUST be stable: elements that compare `Equal` keep their
  input order. A comparator that is not a total preorder gives an
  unspecified permutation, never a defect or a lost element.
- A module that defines an ordering other than the structural one exports a
  comparator, not a sort: `Text.compare_ignoring_case`, not
  `Text.sort_ignoring_case`.
- Comparison operators never take a comparator. Chimi has no operator
  overloading.

## Text

### Positions are bytes

`Text` is always valid UTF-8. Every standard function that takes or returns a
position, an offset, or a length in `Text` counts **UTF-8 bytes**, and has
`byte` in its name:

- `Text.byte_length(text) -> Int`, O(1);
- `Text.byte_slice(text, range) -> Text`, which clamps the range to the text
  and defects when a clamped bound falls inside a UTF-8 sequence.

Bytes are the unit Go and Rust use, they are O(1), and they agree with the
wire formats services read. Putting the unit in the name keeps a Python or
Swift programmer from reading a byte offset as a character count.

When text is counted for a person, as for padding or truncating a column,
the unit is the **grapheme**: what a reader sees as one character, as Swift,
Elixir, and Gleam count. `Text.graphemes(text) -> List<Text>` and
`Text.grapheme_count(text) -> Int` expose it, and `Text.pad_start` and
`Text.pad_end` count width in graphemes. Unicode code points are not a unit
of any `std/text` function. A later `std/unicode` MAY expose them for
protocol work.

`Text` has no bare `length`, `slice`, `index_of`, or `char_at`. The
unknown-member diagnostic for `Text.length` SHOULD suggest
`Text.byte_length` and `Text.grapheme_count`.

### Prefer functions that need no positions

Most service text work never needs an index. `std/text` offers those
operations first, and module authors SHOULD add a position-free function
before they add a position-returning one:

| Need | Chimi | JavaScript | Python | Go `strings` | Gleam `string` | Elixir `String` | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Join with a separator | `join(parts, sep)` | `parts.join(sep)` | `sep.join(parts)` | `Join` | `join` | `Enum.join` | `join` |
| Split on a separator | `split` | `split` | `split` | `Split` | `split` | `split` | `split` |
| Split once | `split_once` | — | `partition` | `Cut` | `split_once` | `split(parts: 2)` | `split_once` |
| Lines | `lines` | — | `splitlines` | — | — | — | `lines` |
| Trim whitespace | `trim`, `trim_start`, `trim_end` | `trim`, `trimStart`, `trimEnd` | `strip` | `TrimSpace` | `trim` | `trim` | `trim` |
| Prefix test | `starts_with`, `ends_with` | `startsWith` | `startswith` | `HasPrefix` | `starts_with` | `starts_with?` | `starts_with` |
| Remove a prefix | `strip_prefix`, `strip_suffix` | — | `removeprefix` | `CutPrefix` | — | — | `strip_prefix` |
| Replace all | `replace` | `replaceAll` | `replace` | `ReplaceAll` | `replace` | `replace` | `replace` |
| Case | `lowercase`, `uppercase` | `toLowerCase` | `lower` | `ToLower` | `lowercase` | `downcase` | `to_lowercase` |

Rules:

- `split`, `replace`, and `contains` take a literal separator or pattern,
  not a regular expression. An empty separator or pattern is a defect.
  Pattern matching belongs to `std/regex`.
- `strip_prefix` and `strip_suffix` return `Option<Text>`: `None` when the
  prefix is absent. `split_once` returns `Option<(Text, Text)>`.
- `lines` splits on `\n` and `\r\n` and drops one final empty line, as Rust
  does.
- `trim` removes characters with the Unicode `White_Space` property.
  `lowercase` and `uppercase` use the Unicode default case mapping, with no
  locale. Locale-aware operations are out of scope for `std/text`.

### Interpolation, not format strings

- Messages use interpolation, `"${expression}"`
  ([text interpolation](standard-modules-and-constants.md#text-interpolation-g9)).
- Std MUST NOT add a format mini-language, `printf`-style function, or
  `Text.format`. Width and padding are `Text.pad_start` and `Text.pad_end`.
  Number formatting is a function in the numeric module, such as a later
  `Float.to_text_fixed(value, decimals)`.
- A type that has a canonical user-facing text form exports `to_text`, so
  callers write `"${Duration.to_text(elapsed)}"`. Interpolation itself
  accepts only `Text`, `Int`, and `Float`.

## Native declarations and the registry

### What is native

A standard function is written in Chimi unless one of these holds:

1. It reads or builds the runtime representation of a `native type`, such as
   a `Map` node or the bytes of a `Text`.
2. It is a `runtime` or `host` operation: it needs the scheduler or the
   operating system.
3. A measured benchmark shows that the Chimi version is asymptotically
   slower, or more than about five times slower on a realistic input, and
   the function is on a hot path of a real program.

Rules:

- The native core of a module SHOULD be the smallest set from which the rest
  is written in Chimi. `std/list` needs `length`, `get`, `push`, `slice`, and
  `set` natively. `map`, `filter`, `reduce`, and the rest are Chimi `loop`s
  over that core.
- A native operation SHOULD be private, with a public Chimi wrapper that
  checks preconditions and states the defect message. `host` operations MUST
  be private (`N3106`).
- Whether a function is native MUST NOT be visible in its public contract.
  Moving a function between Chimi and native is not a breaking change, and
  module docs do not mention it.
- A private native operation follows the same naming rules as a public
  function. The `fold_entries` example in
  [standard modules](standard-modules-and-constants.md#native-declarations)
  becomes `reduce_entries`.

### Properties of a native type

Every `native type` and every runtime-known std type MUST state these
properties in the operation registry. Private representation grants none of
them silently:

| Property | Values |
| --- | --- |
| Equatable | always, never, or "when these type arguments are" |
| Ordered | the same three forms |
| Concurrent property | shareable, transferable, exclusive, or parent-local, possibly per type argument |
| Constant-representable | yes or no |
| Render override | none, or the registry-named function |

The rules for choosing them:

- A value-like type (`Map`, `Set`, `Decimal`, `Json.Number`, `Bytes`) is
  equatable and shareable per its type arguments, and constant-representable.
  It is ordered only when its values have one natural order without a
  comparator.
- A capability (`Task`, `Channel.Sender`, `Channel.Receiver`,
  `Channel.Completion`) is neither equatable nor ordered, is never
  constant-representable, and has no render override. It renders as
  `<Receiver>`, its simple name.
- Generated documentation and `chimi inspect` MUST show every property.

### Render overrides

- A std type MAY have a render override only when it has a canonical text
  form that `T.parse` reads back. The override MUST produce the same text as
  `T.to_text`, as `Duration` renders `30s`.
- Equal values MUST render the same, so the override renders the normalized
  form.
- A capability or a value that holds a secret MUST NOT have an override that
  reveals its contents. `Secret<T>` renders as `<redacted>`.

## Documentation

### Doc comments

Until the documentation generator (T4) defines its own form, std modules use
`///` comments directly above a declaration, and one `///` block at the top
of the file, before the imports, for the module. The lexer reads them as
ordinary comments today, so they cost nothing now and carry over to T4.

Each public declaration's doc comment has this shape:

1. One sentence in the present tense that says what the function returns or
   does, starting with a verb: "Returns the first element of `list`, or
   `None` when it is empty."
2. Optional paragraphs for behavior a caller must know.
3. Labelled lines, in this order, each present only when it applies:
   - `Fails with:` the variants and when each occurs;
   - `Defects when:` each broken precondition;
   - `Calls:` for a function argument, how many times, in what order, and
     when it stops;
   - `Cost:` time, and allocation when it is not proportional to the result.

Doc comments MUST NOT restate the types, failure row, or needs row. The
generator prints the inferred contract, as
[compiler-derived documentation](../../CONTEXT.md) requires. A doc comment on
a multi-clause function goes above the first clause
([patterns](patterns-and-function-clauses.md#documentation)).

### Examples

- Every public function has at least one example program under
  `compiler/chimi/std-tests/<module>/`, as
  [standard modules](standard-modules-and-constants.md#testing-the-standard-library-before-a-test-runner-exists)
  requires. A doc comment MAY quote that program; it is the checked copy.
- A Chimi block in a `docs/` page that uses a standard module not yet in the
  embedded tree is annotated `<!-- chimi: pending ts-0ba63e -->`, plus any
  other ticket whose feature it needs. The harness fails the block once it
  compiles, so the annotation flips to `check` when the module lands.
- A signature listing, an excerpt, or a shape sketch is annotated
  `<!-- chimi: fragment -->`.
- Examples import every module they use with its conventional alias
  (`import std/list as List`), and member-import only types and
  constructors.

## Module skeleton

A standard module lays out its declarations in this order. Each section is
present only when the module needs it.

<!-- chimi: fragment -->
```chimi
/// Short description of what the module is for, in one or two sentences.
/// Name the main type and the most common entry function.

import std/option as Option
import std/order.{Ordering}

// 1. Public types: the main type first, then supporting types.

/// A ...
pub native type Thing<T>

// 2. Failure types, one per operation family.

/// Failures of `Thing.parse`.
pub enum ParseError {
  Empty
  Invalid
}

// 3. Constants.

/// The ...
pub const limit = 64

// 4. Constructors: empty, from, of, unit names.

/// Returns a `Thing` with no items.
///
/// Cost: O(1).
pub native fn empty<T>() -> Thing<T>

// 5. Queries and predicates.

/// Returns `True` when `thing` has no items.
///
/// Cost: O(1).
pub fn is_empty<T>(thing: Thing<T>) -> Bool { ... }

// 6. Transformations, with callbacks last.

/// Returns a `Thing` with `change` applied to every item.
///
/// Calls: `change` once per item, in order; stops at its first failure.
/// Cost: O(n).
pub fn map<T, U, E, R>(thing: Thing<T>, change: fn(T) -> U errors E needs R) -> Thing<U>
  needs R
{ ... }

// 7. Conversions: to_text, parse, to_<x>, from_<x>.

/// Reads a `Thing` from its text form.
///
/// Fails with: `ParseError.Empty` for empty text; `ParseError.Invalid`
/// otherwise.
pub fn parse(text: Text) -> Thing<Text> { ... }

// 8. Private native core and helpers.

native fn length_of<T>(thing: Thing<T>) -> Int
```

## Reference listings

These listings apply the conventions to five modules. They fix names,
argument order, and result shapes. The module specs fill in exact contracts.
Failure rows and needs rows of callback-taking functions follow
[How callbacks behave](#how-callbacks-behave): each has failure-row parameter
`E` and needs-row parameter `R` from its callback, written once below.

### `std/option`

<!-- chimi: fragment -->
```chimi
pub enum Option<T> {
  None
  Some(value: T)
}

pub fn map<T, U, E, R>(option: Option<T>, change: fn(T) -> U errors E needs R) -> Option<U>
pub fn and_then<T, U, E, R>(option: Option<T>, next: fn(T) -> Option<U> errors E needs R) -> Option<U>
pub fn or<T>(option: Option<T>, other: Option<T>) -> Option<T>
pub fn unwrap_or<T>(option: Option<T>, fallback: T) -> T
pub fn unwrap_or_else<T, E, R>(option: Option<T>, fallback: fn() -> T errors E needs R) -> T
pub fn unwrap_or_fail<T, E>(option: Option<T>, error: E) -> T            // errors E
pub fn is_some<T>(option: Option<T>) -> Bool
pub fn is_none<T>(option: Option<T>) -> Bool
pub fn expect<T>(option: Option<T>, message: Text) -> T                 // waits for open question 4
```

### `std/result`

<!-- chimi: fragment -->
```chimi
pub enum Result<T, E> {
  Ok(value: T)
  Err(error: E)
}

pub fn capture<T, E, R>(operation: fn() -> T errors E needs R) -> Result<T, E>
pub fn get<T, E>(result: Result<T, E>) -> T                             // errors E
pub fn map<T, U, E, F, R>(result: Result<T, E>, change: fn(T) -> U errors F needs R) -> Result<U, E>
pub fn map_error<T, E, G, F, R>(result: Result<T, E>, change: fn(E) -> G errors F needs R) -> Result<T, G>
pub fn unwrap_or<T, E>(result: Result<T, E>, fallback: T) -> T
pub fn to_option<T, E>(result: Result<T, E>) -> Option<T>
pub fn error<T, E>(result: Result<T, E>) -> Option<E>
pub fn is_ok<T, E>(result: Result<T, E>) -> Bool
pub fn is_err<T, E>(result: Result<T, E>) -> Bool
```

### `std/list`

<!-- chimi: fragment -->
```chimi
// Queries
pub native fn length<T>(list: List<T>) -> Int
pub fn is_empty<T>(list: List<T>) -> Bool
pub native fn get<T>(list: List<T>, index: Int) -> Option<T>
pub fn first<T>(list: List<T>) -> Option<T>
pub fn last<T>(list: List<T>) -> Option<T>
pub fn contains<T>(list: List<T>, item: T) -> Bool                     // T equatable
pub fn min<T>(list: List<T>) -> Option<T>                              // T ordered
pub fn max<T>(list: List<T>) -> Option<T>                              // T ordered

// Building
pub fn repeat<T>(item: T, count: Int) -> List<T>
pub native fn push<T>(list: List<T>, item: T) -> List<T>
pub fn append<T>(list: List<T>, other: List<T>) -> List<T>
pub fn concat<T>(lists: List<List<T>>) -> List<T>
pub native fn set<T>(list: List<T>, index: Int, item: T) -> List<T>    // defects outside 0..length
pub native fn slice<T>(list: List<T>, range: Range) -> List<T>         // clamps
pub fn take<T>(list: List<T>, count: Int) -> List<T>
pub fn drop<T>(list: List<T>, count: Int) -> List<T>
pub fn reverse<T>(list: List<T>) -> List<T>
pub fn enumerate<T>(list: List<T>) -> List<(Int, T)>
pub fn zip<A, B>(list: List<A>, other: List<B>) -> List<(A, B)>        // stops at the shorter
pub fn chunks<T>(list: List<T>, size: Int) -> List<List<T>>            // defects when size < 1
pub fn unique<T>(list: List<T>) -> List<T>                             // keeps first; T equatable

// Callbacks: each adds `errors E needs R` from its callback
pub fn map<T, U>(list: List<T>, change: fn(T) -> U) -> List<U>
pub fn filter<T>(list: List<T>, keep: fn(T) -> Bool) -> List<T>
pub fn filter_map<T, U>(list: List<T>, change: fn(T) -> Option<U>) -> List<U>
pub fn flat_map<T, U>(list: List<T>, change: fn(T) -> List<U>) -> List<U>
pub fn reduce<T, S>(list: List<T>, initial: S, step: fn(S, T) -> S) -> S
pub fn each<T>(list: List<T>, action: fn(T) -> Void) -> Void
pub fn find<T>(list: List<T>, matches: fn(T) -> Bool) -> Option<T>
pub fn find_map<T, U>(list: List<T>, change: fn(T) -> Option<U>) -> Option<U>
pub fn find_index<T>(list: List<T>, matches: fn(T) -> Bool) -> Option<Int>
pub fn any<T>(list: List<T>, matches: fn(T) -> Bool) -> Bool
pub fn all<T>(list: List<T>, matches: fn(T) -> Bool) -> Bool
pub fn count<T>(list: List<T>, matches: fn(T) -> Bool) -> Int
pub fn partition<T>(list: List<T>, keep: fn(T) -> Bool) -> (List<T>, List<T>)

// Ordering
pub fn sort<T>(list: List<T>) -> List<T>                               // T ordered; stable
pub fn sort_by<T>(list: List<T>, compare: fn(T, T) -> Ordering) -> List<T>   // stable
pub fn min_by<T>(list: List<T>, compare: fn(T, T) -> Ordering) -> Option<T>
pub fn max_by<T>(list: List<T>, compare: fn(T, T) -> Ordering) -> Option<T>
```

### `std/text`

<!-- chimi: fragment -->
```chimi
// Building
pub native fn concat(parts: List<Text>) -> Text
pub fn append(text: Text, other: Text) -> Text
pub fn join(parts: List<Text>, separator: Text) -> Text
pub fn repeat(text: Text, count: Int) -> Text
pub fn pad_start(text: Text, width: Int, fill: Text) -> Text          // width in graphemes
pub fn pad_end(text: Text, width: Int, fill: Text) -> Text

// Queries
pub fn is_empty(text: Text) -> Bool
pub native fn byte_length(text: Text) -> Int
pub fn grapheme_count(text: Text) -> Int
pub native fn contains(text: Text, needle: Text) -> Bool
pub fn starts_with(text: Text, prefix: Text) -> Bool
pub fn ends_with(text: Text, suffix: Text) -> Bool

// Taking apart
pub native fn split(text: Text, separator: Text) -> List<Text>         // defects on ""
pub fn split_once(text: Text, separator: Text) -> Option<(Text, Text)>
pub fn lines(text: Text) -> List<Text>
pub fn graphemes(text: Text) -> List<Text>
pub fn strip_prefix(text: Text, prefix: Text) -> Option<Text>
pub fn strip_suffix(text: Text, suffix: Text) -> Option<Text>
pub native fn byte_slice(text: Text, range: Range) -> Text             // clamps; defects mid-sequence

// Changing
pub fn trim(text: Text) -> Text
pub fn trim_start(text: Text) -> Text
pub fn trim_end(text: Text) -> Text
pub native fn replace(text: Text, old: Text, new: Text) -> Text        // all occurrences
pub native fn lowercase(text: Text) -> Text
pub native fn uppercase(text: Text) -> Text

// Ordering
pub fn compare_ignoring_case(text: Text, other: Text) -> Ordering
```

### `std/map`

<!-- chimi: fragment -->
```chimi
pub native type Map<K, V>       // equatable when K and V are; not ordered; shareable when K and V are

// Constructors
pub native fn empty<K, V>() -> Map<K, V>
pub fn from<K, V>(pairs: List<(K, V)>) -> Map<K, V>                    // a later pair wins

// Queries
pub native fn get<K, V>(map: Map<K, V>, key: K) -> Option<V>
pub fn has_key<K, V>(map: Map<K, V>, key: K) -> Bool
pub native fn size<K, V>(map: Map<K, V>) -> Int
pub fn is_empty<K, V>(map: Map<K, V>) -> Bool

// Changing
pub native fn put<K, V>(map: Map<K, V>, key: K, value: V) -> Map<K, V>
pub native fn remove<K, V>(map: Map<K, V>, key: K) -> Map<K, V>
pub fn merge<K, V>(map: Map<K, V>, overrides: Map<K, V>) -> Map<K, V>

// Enumerating: ascending key order; K ordered (open question 2)
pub fn keys<K, V>(map: Map<K, V>) -> List<K>
pub fn values<K, V>(map: Map<K, V>) -> List<V>
pub fn entries<K, V>(map: Map<K, V>) -> List<(K, V)>

// Callbacks: each adds `errors E needs R` from its callback
pub fn update<K, V>(map: Map<K, V>, key: K, change: fn(Option<V>) -> V) -> Map<K, V>
pub fn map_values<K, V, W>(map: Map<K, V>, change: fn(V) -> W) -> Map<K, W>
pub fn filter<K, V>(map: Map<K, V>, keep: fn(K, V) -> Bool) -> Map<K, V>
pub fn reduce<K, V, S>(map: Map<K, V>, initial: S, step: fn(S, K, V) -> S) -> S
pub fn each<K, V>(map: Map<K, V>, action: fn(K, V) -> Void) -> Void
```

## Changes to other specifications

Each change follows from a rule above. The owner of each document applies it
in its next edit. This contract wins where they disagree.

| Document | Change |
| --- | --- |
| [patterns and function clauses](patterns-and-function-clauses.md#anonymous-functions) | `List.fold(prices, 0, ...)` becomes `List.reduce(prices, 0, ...)` |
| [standard modules](standard-modules-and-constants.md#native-declarations) | private `fold_entries` becomes `reduce_entries` |
| [time and outcomes](time-and-outcomes.md#text-form) | `Duration.ParseError` variants `Invalid` and `OutOfRange` drop their `text` fields |
| [core data](core-data-and-operators.md#hashing) | "the internal order MUST NOT be observable through enumeration" holds because enumeration uses the structural key order (open question 2) |
| `std/json` at migration step 6 | `Json.NumberError` becomes `Json.ConversionError` |
| [standard library map](../plans/standard-library-map.md) | the inventory's `fold` entries read `reduce`; owned by another session, so it changes there |

## Open questions

These are vision-level. Each has a recommendation, and the contract above
already follows it, so module work does not wait on them.

1. **Should higher-order collection functions exist at all, given `loop`?**
   `loop` can express every traversal, and one form is simpler to teach.
   *Recommendation:* keep the fixed set above. A `loop` for "keep the open
   orders" is six lines of `case` and pattern-matched rest, where
   `List.filter` is one, and every sketch reached for `map`, `filter`, and
   `each` first. The set is closed: a new collection function needs a real
   program that wrote the same `loop` more than once.
2. **In what order does enumerating a `Map` or `Set` visit entries?**
   [Immutable maps](trace-data-and-processing.md#immutable-maps) promises no
   order, and [core data](core-data-and-operators.md#hashing) says the
   internal order MUST NOT be observable through enumeration. Any order that
   enumeration produces is observable, so the two rules together leave
   `Map.keys` undefined.
   *Recommendation:* enumerate in ascending structural order of the key, and
   require the key type to be ordered for enumerating functions. Output is
   then reproducible across runs and releases, which `Json.encode`, test
   expectations, and CLI output need. An implementation whose internal order
   agrees with the structural order for ordered keys pays nothing. The cost:
   a map keyed by a record, a map, or a set can be built and queried but not
   enumerated; its owner keys it by an ordered type instead. Go's randomized
   order was rejected because it breaks reproducible behavior.
3. **Should `Text` have a bare `length`?** Every mainstream language has one,
   and its absence will surprise people once.
   *Recommendation:* no. Byte length and grapheme count are both common in
   services, and the wrong one is a silent bug in padding, truncation, and
   protocol limits. A teaching diagnostic on `Text.length` costs one
   surprise. A bare `length` that means bytes costs a bug report each time a
   non-ASCII name misaligns a table.
4. **How does Chimi source raise a defect on purpose?** `Option.expect`, and
   any std wrapper that checks a precondition in Chimi, needs a function that
   ends the unit of work with a message. No specification defines one.
   *Recommendation:* add `Debug.crash(message: Text) -> Never` to
   `std/debug`, a `pure` native operation. "Crash" is the word the design
   philosophy and Erlang already use for a bounded failure. `panic` suggests
   a whole-process abort, and `fail` collides with expected failures. Until
   it exists, std wrappers keep preconditions in their native operations.
