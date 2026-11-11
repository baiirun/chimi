# Patterns and Function Clauses

## Status

This is the normative contract for multi-clause functions (`L1`), list
patterns (`G15`), tuple, literal, and struct patterns, failure-type patterns
(`G26`), and record update (`G8`). The design ticket is `prog show ts-be39c0`.
Nothing here is implemented yet. Today a module allows one function per name
(`E_DUPLICATE_SYMBOL`), and patterns are limited to bindings, `_`, literals,
and enum variants.

The words MUST, MUST NOT, SHOULD, and MAY are normative. Examples use
positional construction. Examples that call standard-library modules beyond
today's `std/io`, `std/json`, `std/map`, `std/task`, and `std/channel` use the
names from the [standard library sketches](../plans/standard-library-sketches/).

This document depends on:

- tuples with `(a, b)` syntax and derived structural equality, specified in
  `core-data-and-operators.md`;
- the persistent `List` representation (`L3`) for the list-pattern cost model;
- documentation comments (`T4`) for the doc-placement rule;
- derived `==` (`G2`) for the guard examples that compare values.

## Goals

- A function that does different work for different input shapes says so in
  its declaration, one nearby body per shape, instead of a boolean mode
  argument or one header followed by a large `case`.
- Function clauses are a spelling of `case`, not a second matching system.
  Exhaustiveness, unreachable-clause detection, guard purity, and first-match
  order are the rules `case` already has.
- Clauses dispatch on values. Chimi has no overloading by type or by arity: one
  name in a module is one function with one contract.
- Patterns stay positional, like construction. The one place source names
  fields is where fields are already named: access and update.
- Matching a list costs no more than indexing it. No pattern copies a list.

## Multi-clause functions

### Declaration form

A multi-clause function is two or more adjacent `fn` declarations with the
same name. Each declaration is a clause. Its parameter list holds patterns
instead of only names:

<!-- chimi: pending ts-e43af0 -->
```chimi
enum Shape {
  Circle(radius: Int)
  Rectangle(width: Int, height: Int)
}

pub fn area(Circle(radius)) -> Int {
  3 * radius * radius
}
fn area(Rectangle(width, height)) {
  width * height
}
```

The compiler reads the group as one function whose body is a `case` over all
parameters, with one arm per clause in source order:

<!-- chimi: fragment -->
```chimi
pub fn area(shape) -> Int {
  case shape {
    Circle(radius) -> {
      3 * radius * radius
    }
    Rectangle(width, height) -> {
      width * height
    }
  }
}
```

The desugared parameter names are compiler-generated and are not visible to
source. With two or more parameters, the clause patterns become the arm's
comma-separated patterns, exactly as in a multi-subject `case`:

<!-- chimi: fragment -->
```chimi
pub fn route(Get, ["health"]) -> Response needs { db: Database } {
  case db.ping() {
    Success(_) -> Http.text(Status.Ok, "ready")
    Failure(_) -> Http.text(Status.ServiceUnavailable, "database unavailable")
  }
}
fn route(Get, ["orders", raw_id]) {
  Http.json(Status.Ok, order_json(find_order(parse_id(raw_id))))
}
fn route(_, _) {
  Http.json(Status.NotFound, error_json("no such route"))
}
```

A clause MAY have a guard. The guard follows the parameter list and any
contract, and it comes immediately before the body:

<!-- chimi: pending ts-e43af0 -->
```chimi
import std/int as Int
import std/text as Text

enum CountError {
  Negative(count: Int)
}

fn describe(0) -> Text {
  "no items"
}
fn describe(1) {
  "one item"
}
fn describe(count) if count < 0 {
  Failure(CountError.Negative(count))
}
fn describe(count) {
  Text.append(Int.to_text(count), " items")
}
```

Tooling displays this as `fn(Int) -> Text errors CountError`.

A function with one clause is an ordinary function. Its parameters MAY be
patterns, but those patterns MUST be irrefutable (see
[Irrefutable patterns](#irrefutable-patterns)).

### Grouping rules

- Clauses of one function MUST be adjacent. Comments MAY appear between them.
  Any other declaration between two same-named functions ends the group. A
  later function with that name is `E_SEPARATED_FUNCTION_CLAUSES`, with a
  secondary label on the first group.
- A same-named declaration that is not a function, such as a struct named like
  the function, keeps the existing `E_DUPLICATE_SYMBOL`.
- Every clause MUST have the same number of parameters (`T4280`). Chimi has no
  arity overloading: `describe/1` and `describe/2` cannot coexist.
- Clauses are tried from top to bottom. The first clause whose patterns match
  and whose guard, if present, produces `True` runs. No later clause runs.

### The first clause is the header

The first clause carries everything that describes the function as a whole:

- `pub`;
- the generic parameter list;
- the success annotation (`-> T`);
- the `needs` declaration;
- the documentation comment.

A later clause contains only `fn`, the name, the parameter patterns, an
optional guard, and the body. A later clause that repeats `pub`, generic
parameters, a success annotation, or `needs` is `L2020`. The fix removes the
item, or moves it to the first clause when the first clause lacks it.

Generic parameters, needs names, and the success annotation are in scope in
every clause. A typed binding in a later clause MAY name a generic parameter
from the first clause:

<!-- chimi: pending ts-e43af0 ts-553107 ts-dc28b2 -->
```chimi
fn last<T>([]) -> Option<T> {
  None
}
fn last([.., item: T]) {
  Some(item)
}
```

Rationale: one place states the public surface, so a reader and the
documentation generator never compare several headers for agreement. Inferred
contracts make repeated annotation unnecessary, and principle 5 keeps
annotations optional.

### Documentation

A documentation comment immediately above the first clause documents the whole
function. Ordinary comments above later clauses describe those clauses. The
documentation generator MUST render the function once, with its complete
inferred contract, and MUST NOT render clauses as separate entries.

### Formatting

The canonical formatter separates clauses of one function with a single line
break and no blank line. It separates different declarations with one blank
line. The group therefore reads as one declaration. The formatter MUST NOT
reorder clauses, because order selects the clause that runs.

### Coverage and unreachable clauses

Coverage uses the same usefulness matrix as `case`. The clause patterns form
the matrix rows, and the parameter types form its columns.

- The clauses MUST cover every combination of argument values. A gap is
  `T4231`. The message names the function and SHOULD show one missing argument
  shape as a witness, such as `describe(_)` or `route(Post, [_, _, ..])`.
- A clause that no argument can reach is `T4232`, with a secondary label on the
  earlier clause that covers it.
- A clause whose patterns are all catch-alls and that has no guard MUST be
  last (`T4233`).
- A guard MUST be pure (`T4234`). A guarded clause does not count toward
  coverage.

A function's patterns MUST NOT be partial. Chimi has no runtime "no clause
matched" defect. An input the program cannot handle is either a modeled
failure, written as a clause that returns `Failure(...)`, or a closed type that
excludes it.

### Inference across clauses

Clauses share one callable contract:

- Parameter `i`'s type is the unification of every clause's `i`-th pattern
  type, every typed binding in that position, and every use of the binding in
  each body. Two clauses that require different types for one position are
  `T4281`. Its message says that clauses select by value and that Chimi does
  not overload functions by type.
- A pattern against a generic parameter MAY only be a binding, a typed binding
  that names the same parameter, or `_`. A literal, constructor, tuple, or list
  pattern there is `T4230`. No clause can inspect the type of a generic value.
- The success type unifies every clause body with the success annotation, as
  case arms do.
- The failure row is the union of every clause body's failures. Guards
  contribute none, because they are pure.
- The needs row is the union of every clause body's needs. Needs names from
  the first clause's `needs` declaration are lexical values in every clause.
- Recursive calls to the function from any clause refer to the whole group.

`chimi inspect` on the name or a pattern of any clause MUST report the group's
complete contract and the clause's position in the group.

### Parameters are values

Callers evaluate arguments before clause selection. An argument whose
evaluation fails has already left the caller through its failure channel. A
parameter therefore never holds a live outcome. `Success(...)` and
`Failure(...)` patterns in a clause head are `T4282`. The teaching message
points at the call site and suggests matching the operation with `case`, or
passing a thunk to a recovery function.

### Function values and pipelines

A multi-clause function is one function value. Naming it anywhere refers to
the whole group:

<!-- chimi: fragment -->
```chimi
let labels = List.map(counts, describe)
let label = count |> describe
```

The pipeline rule applies to multi-clause functions unchanged:
`count |> describe` is `describe(count)`, and `count |> describe(style)` is
`describe(count, style)` ([pipelines](executable-kernel.md#pipelines)).

### Anonymous functions

Anonymous functions have one clause. Their parameters MAY be irrefutable
patterns:

<!-- chimi: fragment -->
```chimi
let total = List.fold(prices, 0, fn(sum, (_, price)) { sum + price })
```

To match several shapes in an anonymous function, write `case` in its body.
Multi-clause anonymous functions are not part of this specification (see
[Open questions](#open-questions)).

### Interaction with `loop`

Clauses do not change [repetition](repetition.md):

- `break` inside a clause body leaves only a `loop` in that body. A `break`
  that targets a loop in the caller crosses a function boundary and is
  `E_BREAK_OUTSIDE_LOOP`.
- A clause function MAY compute the next loop state. A step that can also
  finish returns an enum, and the loop body matches it:

<!-- chimi: fragment -->
```chimi
enum Step {
  Next(state: Search)
  Found(value: Int)
}

loop search = start {
  case advance(search) {
    Next(next) -> next
    Found(value) -> break value
  }
}
```

- Recursion through clauses follows the repetition rules. It suits structurally
  bounded work such as walking a tree. Chimi does not guarantee tail calls, so
  recursion over `..rest` is not a supported way to walk a long list. Use
  `loop` or a `List` function.

The `loop` state binding accepts an irrefutable pattern. The body's value is
matched against the same pattern before the next pass:

<!-- chimi: pending ts-5be727 ts-553107 -->
```chimi
fn sum(numbers: List<Int>) -> Int {
  loop (remaining, total) = (numbers, 0) {
    case remaining {
      [] -> break total
      [first, ..rest] -> (rest, total + first)
    }
  }
}
```

## Patterns

### Pattern forms

| Form | Example | Matches |
| --- | --- | --- |
| Wildcard | `_` | any value; binds nothing |
| Binding | `count` | any value; binds it |
| Typed binding | `count: Int`, `error: FileError` | see [Typed bindings](#typed-bindings-and-failure-type-patterns) |
| Literal | `0`, `-1`, `"orders"`, `True` | an equal value |
| Enum variant | `Circle(radius)`, `Value.Text(schema)`, `None` | that variant, then its payload by position |
| Struct | `Money(cents)` | every value of the struct, then its fields by position |
| Tuple | `(key, value)` | every tuple of that arity, then its elements by position |
| List | `[]`, `[a, b]`, `[first, ..rest]`, `[.., last]` | lists of the stated lengths, then elements by position |
| Alias | `[first, ..] as whole` | what the inner pattern matches; also binds the whole value |

Patterns nest without limit: `Some([("id", Json.Number(id)), ..])` is valid.

General rules:

- A lowercase name in a pattern always introduces a binding. It never compares
  against an existing value. To compare against a local value or a module
  constant, bind and use a guard: `found if found == expected`.
- A name MUST NOT be bound twice in one clause or arm (`E_DUPLICATE_LOCAL`).
  `fn same(x, x)` is not an equality test. Write `fn same(x, y) if x == y`.
- Bindings are visible in the guard and the body of their clause or arm.
- Patterns are pure. Matching never calls user code, needs a dependency, or
  fails.
- A labelled pattern such as `Money(cents: 2500)` is a syntax error (`S1001`)
  whose message says that patterns are positional, as construction is. A colon
  inside a pattern always introduces a type.

### Literal patterns

- `Int` literals use the expression literal syntax, including `_` separators.
  A negative literal pattern is `-` immediately followed by an integer literal.
- `Text` literals match by exact UTF-8 byte equality. This is the derived
  structural equality for `Text`. Matching performs no Unicode normalization.
- `True` and `False` form the complete `Bool` domain.
- `Float` values have no literal patterns. Compare them in a guard.
- `Int` and `Text` have unbounded domains. A match over them needs a catch-all.

Text prefix patterns (Gleam's `"GET " <> rest`) are not included. Prefix
splitting is a `std/text` function.

### Struct patterns

A struct pattern names the struct and lists one pattern per field in
declaration order, like construction. A struct has one shape, so a struct
pattern whose field patterns are irrefutable is irrefutable.

Struct patterns are valid only where the struct's fields are visible: inside
its defining module. Elsewhere they are `E_PRIVATE_STRUCT_CONSTRUCTOR`, for the
same reason external construction is. A wrong field count is `T4230`.

### Tuple patterns

A tuple pattern `(p1, ..., pn)` matches a tuple of arity `n`. Its elements
match by position. A tuple has one shape, so coverage treats a tuple as a
single constructor with `n` fields. Arity mismatch is `T4230`. The tuple arities
that exist are defined by `core-data-and-operators.md`.

`fn f((a, b))` takes one tuple parameter. `fn f(a, b)` takes two parameters.
The two are different functions.

### List patterns

A list pattern is written in brackets:

- `[]` matches the empty list.
- `[p1, ..., pn]` matches a list of exactly `n` elements.
- `[p1, ..., pk, ..rest]` matches a list of at least `k` elements and binds the
  remaining elements to `rest`.
- `[.., q1, ..., qm]` matches a list of at least `m` elements, matching the last
  `m` by position.
- `[p1, ..., pk, ..rest, q1, ..., qm]` matches at least `k + m` elements.

`..` without a name ignores the remaining elements. A list pattern MUST
contain at most one `..` (`L2021`). Every element pattern matches against the
list's element type.

The `..rest` spelling follows Gleam and Rust. It is also the spelling record
update uses, so `..` means "the rest of this value" everywhere.

#### List coverage

Coverage treats a list as a value with one constructor per length. For a
matrix whose longest fixed requirement is `L` elements, counting both exact
patterns and the fixed elements around a rest, the domain is the lengths
`0` through `L - 1` plus "at least `L`". Each row is expanded into those
classes before the usual specialization. Some consequences:

- `[]` and `[_, ..]` together cover every list.
- `[]`, `[_]`, and `[_, _, ..]` cover every list.
- `[_, ..]` alone misses `[]`. The witness is `[]`.
- `[a]`, then `[.., last]`, makes neither unreachable, because `[.., last]`
  also matches longer lists.

#### List cost model

Over the persistent vector (`L3`), matching MUST stay within these bounds:

- The length check is O(1) and happens before any element access.
- Each matched element costs one indexed read, which is O(log n) with a wide
  branching factor and effectively constant for realistic sizes.
- Binding `..rest` MUST be O(1) in time and allocation. The binding is a view
  that shares the source vector's storage, holding an offset and a length. It
  MUST NOT copy elements.
- A view keeps its source's storage alive. Code that keeps a small `rest` of a
  very large list retains the large list. Copying the elements into a new list
  releases it.

The `L3` representation therefore MUST support constant-time slicing views.

### Typed bindings and failure-type patterns

A typed binding `name: Type` is the binding form that parameters already use.
Because a parameter list is a list of patterns, `fn report(error: TraceError)`
and a typed binding inside a pattern are the same construct. `_: Type` is the
typed wildcard.

Its meaning depends on the type of the value it matches:

- If that value has an ordinary type, `Type` is an annotation. It MUST equal
  the value's type, and the binding is irrefutable. A different type is
  `T4283`. Its message says that typed bindings do not test types at run time,
  and that only failure values can be selected by enum. In a clause head, the
  parameter's type is not known in advance, so the annotation instead joins the
  cross-clause unification, and a conflict is `T4281`.
- If that value is a failure-set value, such as the payload of a `Failure`
  pattern or a value bound from one, `Type` MUST name a failure-set member: a
  closed enum, one variant such as `StartError.NameTaken`, or a nominal struct
  used as a failure value. An enum pattern matches every variant of that enum
  present in the failure set. It is shorthand for one arm per variant, with
  all payloads ignored. A variant or struct pattern matches that one member.

A failure-type pattern replaces one arm per variant. The trace summarizer's
`main` currently lists every IO variant. With failure-type patterns it reads:

<!-- chimi: fragment -->
```chimi
import std/io.{FileError}
import std/io.{LineTooLong}
import std/io.{Utf8Error}

case files.process_utf8_lines(IO.path(path), empty(), add_line) {
  Success(state) -> finish_and_print(state)
  Failure(error: TraceError) -> report_trace(error)
  Failure(LineTooLong(_, _) as error) -> {
    console.write_error("trace summary input record is too large")
    Failure(error)
  }
  Failure(error: FileError) -> {
    console.write_error("trace summary file IO failed")
    Failure(error)
  }
  Failure(error: Utf8Error) -> {
    console.write_error("trace summary input is not UTF-8")
    Failure(error)
  }
}
```

The `LineTooLong` arm comes first because first-match order applies. The
`FileError` arm then covers only the variants that remain.

Rules:

- The bound name's type is the failure set restricted to the members the
  pattern names that are present. For an enum, that is the enum's present
  variants, and when every variant is present, that type is the enum itself.
  For a variant or a struct, it is that one member. A restricted value is accepted wherever the enum is expected, so
  `report_trace(error)` above type-checks even if the row omits some
  `TraceError` variants.
- Selective recovery subtracts every variant the pattern covers when the arm
  has no guard. A guarded failure-type arm subtracts nothing. These are the
  kernel's existing subtraction rules applied to each covered variant.
- A failure-type pattern whose type has no member in a known closed row is
  `T4224`. Against an open row, the pattern is permitted. Subtraction is a no-op
  for an instantiation that lacks the type.
- After `Failure(error)` binds a whole failure set, `case error` MAY use
  failure-type patterns. That `case` MUST still be exhaustive:

<!-- chimi: fragment -->
```chimi
case error {
  file_error: FileError -> describe_file_error(file_error)
  trace_error: TraceError -> describe_trace_error(trace_error)
}
```

- Failure-type patterns are compile-time sets of variant and struct tags. Lowering MUST
  expand them into ordinary variant tests. They MUST NOT introduce a runtime
  type test, type tag, or new Core operation.

Rationale for `name: Type`: it is the existing parameter syntax, Scala and
Kotlin use it for the same job (`case e: IOException`), and it needs no new
keyword. A colon inside parentheses never labels an argument in Chimi. It
always introduces a binding with its type.

### Alias patterns

`pattern as name` matches `pattern` and also binds the whole matched value to
`name`, as in Gleam. `as` binds less tightly than any other pattern form. The
alias has the matched value's type. Inside a `Failure` pattern, the inner
pattern narrows that type: `LineTooLong(_, _) as error` binds a failure-set
value whose only variant is `LineTooLong`, so `Failure(error)` re-raises
exactly that variant.

### Irrefutable patterns

A pattern is irrefutable when it matches every value of its type: `_`, a
binding, a typed binding over an ordinary type, a tuple or struct pattern whose
parts are all irrefutable, and an alias of an irrefutable pattern.

These positions require an irrefutable pattern:

- the parameters of a single-clause function;
- the parameters of an anonymous function;
- a `let` binding;
- a `loop` state binding.

A refutable pattern in one of these positions is `T4231`, with a witness and a
message that suggests `case` or, for a function, more clauses. `Success(...)`
and `Failure(...)` in any of these positions are `T4282`.

<!-- chimi: fragment -->
```chimi
let (name, port) = parse_address(raw)
```

### Range patterns

Chimi has no range patterns. Integer ranges are matched with guards:

<!-- chimi: pending ts-e43af0 -->
```chimi
fn grade(score) if score >= 90 {
  "A"
}
fn grade(score) if score >= 80 {
  "B"
}
fn grade(_) {
  "C"
}
```

Range patterns would only help coverage of `Int`, whose domain is unbounded
anyway, and they would add a second meaning to the `Range` value syntax.

## Record update

### Form

A record update copies a value and replaces the named fields:

<!-- chimi: fragment -->
```chimi
fn with_jobs(config: Config, jobs: Int) -> Config {
  { ..config, jobs: jobs }
}
```

The braces hold `..base` first, then one or more `field: value` entries. The
result has the base's type. The trace summarizer's `Summary` update becomes:

<!-- chimi: fragment -->
```chimi
{ ..state, records: state.records + 1, line: state.line + 1 }
```

instead of a ten-argument construction.

### Why update names fields

Construction is positional because a constructor call is a call, and Chimi
calls have no labels. An update is not a call. It changes some fields and keeps
the rest, so it must say which fields change. Field names are already the
identity for access (`state.records`). Update uses that same identity.

A positional update such as `Config(..base, jobs)`, which the CLI sketch
wrote, would make the reader count declaration positions to find which field
changes. A reordered declaration would then silently change another field when
the types agree. Naming the field turns that mistake into a type error.

The rule is: parentheses are positional and braces are named. Anonymous
records, `using` providers, and updates use `name: value` inside braces. Calls,
construction, and patterns never use labels inside parentheses.

### Rules

- The base MUST be a struct value or an anonymous record value, including a
  value of a structural interface type. Any other base is `T4284`.
- A struct base is valid only where the struct's fields are visible
  (`E_PRIVATE_STRUCT_CONSTRUCTOR`). Other modules use the module's public
  functions.
- Every named field MUST exist on the base's type (`T4205`) and MUST appear at
  most once (`T4211`).
- Each value MUST be compatible with the field's declared type (`T4203`). An
  update MUST NOT change the base's type. A type-changing update of a generic
  struct, such as `Box<Int>` to `Box<Text>`, is `T4203`. Construct the new
  value instead.
- An update MUST name at least one field. `{ ..base }` is `L2022`, with a fix
  that replaces it with `base`.
- Updates MUST NOT add fields to an anonymous record. A field that the base
  lacks is `T4205`.
- `..base` MUST come first and appear once. Anything else is a syntax error
  (`S1001`), and the fix moves the spread.
- The base evaluates first, then the field values left to right in source
  order. The failures and needs of an update are the union of its parts.
- The formatter MUST NOT reorder fields, because order is evaluation order.

Nested updates are ordinary nesting:

<!-- chimi: fragment -->
```chimi
{ ..state, stats: { ..state.stats, retries: state.stats.retries + 1 } }
```

Updating one method of a test fake uses the same form:

<!-- chimi: fragment -->
```chimi
let failing = { ..fake_payment, charge: fn(_) { Failure(PaymentError.Offline) } }
```

### Cost

An update allocates one new value and copies the unchanged fields. The cost is
proportional to the field count, not to the size of the field values, which
are shared. A compiler MAY reuse the base's storage when the base has no other
reference. That reuse MUST NOT be observable.

## Execution

- Clause selection and `case` matching have the same semantics. A compiler MAY
  compile patterns into a decision tree, because patterns are pure. It MUST
  preserve first-match order and evaluate guards only for clauses whose
  patterns matched, in clause order.
- A multi-clause function is one callable. Calling it is one call in tracing.
  Full tracing MUST record the selected clause's source identity on the match
  operation. It MUST NOT emit one call or span per clause.
- Lowering expresses clauses, failure-type patterns, and updates through
  existing Core matching and construction, plus list length, index, and view
  operations. They add no runtime tag or row object.

## Diagnostics

Existing codes reused with clause-specific messages:

| Code | Stage | Condition |
| --- | --- | --- |
| `T4231` | Type | Clauses do not cover every input, or a refutable pattern is in an irrefutable position |
| `T4232` | Type | A clause can never run |
| `T4233` | Type | An unguarded catch-all clause is not last |
| `T4234` | Type | A clause guard is not a pure `Bool` |
| `T4230` | Type | A pattern does not fit the value's type, including patterns against a generic parameter and wrong field, tuple, or payload counts |
| `T4224` | Type | A failure-type pattern names an enum, variant, or struct absent from a closed failure row |
| `T4205`, `T4211`, `T4203` | Type | An update names an unknown field, repeats a field, or gives a value of the wrong type |
| `E_DUPLICATE_LOCAL` | Name | A pattern binds one name twice |
| `E_PRIVATE_STRUCT_CONSTRUCTOR` | Name | A struct pattern or update is outside the struct's module |
| `E_DUPLICATE_SYMBOL` | Name | A non-function declaration shares a function's name |
| `S1001` | Syntax | A labelled pattern, a misplaced `..base`, or a positional update |

Proposed new codes. `T4260`–`T4264` are already proposed by the process
specifications, so this document uses `T4280`–`T4284` and `L2020`–`L2022`:

| Code | Stage | Condition | Fix |
| --- | --- | --- | --- |
| `E_SEPARATED_FUNCTION_CLAUSES` | Name | Same-named functions are not adjacent | Move the clause next to the others |
| `L2020` | Lowering | A later clause repeats `pub`, generics, the success annotation, or `needs` | Remove it, or move it to the first clause |
| `L2021` | Lowering | A list pattern has more than one `..` | None |
| `L2022` | Lowering | `{ ..base }` names no field | Replace with `base` |
| `T4280` | Type | Clauses have different parameter counts | None. Chimi has no arity overloading |
| `T4281` | Type | Clauses require different types for one parameter | Use an enum to model the alternatives |
| `T4282` | Type | `Success` or `Failure` in a parameter, `let`, or `loop` pattern | Match the operation with `case` |
| `T4283` | Type | A typed binding names a type other than the value's ordinary type | Remove the annotation, or match variants |
| `T4284` | Type | An update's base is not a struct or record | None |

## Conformance matrix

| ID | Scenario | Expected |
| --- | --- | --- |
| PC-01 | `area` over both `Shape` variants | Compiles; `fn(Shape) -> Int` |
| PC-02 | `area` with only the `Circle` clause | `T4231`; witness `area(Rectangle(_, _))` |
| PC-03 | `describe(count)` before `describe(0)` | `T4232` on `describe(0)` |
| PC-04 | `fn f(_) {..}` followed by `fn f(1) {..}` | `T4233` and `T4232` |
| PC-05 | `describe` with the guarded `Negative` clause | Compiles; `fn(Int) -> Text errors CountError` |
| PC-06 | Only guarded clauses over `Int` | `T4231` |
| PC-07 | Guard calls a function that needs a dependency | `T4234` |
| PC-08 | Clauses with one and two parameters | `T4280` |
| PC-09 | `fn f(0)` and `fn f("zero")` | `T4281` |
| PC-10 | `pub` on the second clause | `L2020`; fix removes it |
| PC-11 | Clauses separated by a struct declaration | `E_SEPARATED_FUNCTION_CLAUSES` |
| PC-12 | `route` with `needs` on the first clause, `db` used in two clauses | Compiles; one needs entry |
| PC-13 | `fn f<T>(0)` where the parameter is `T` | `T4230` |
| PC-14 | `fn f(Failure(e))` | `T4282` |
| PC-15 | `List.map(counts, describe)` and `count \|> describe` | Compiles; one function value |
| PC-16 | `break` in a clause body called from a loop | `E_BREAK_OUTSIDE_LOOP` |
| PC-17 | `fn same(x, x)` | `E_DUPLICATE_LOCAL` |
| LP-01 | `[]` and `[first, ..rest]` | Compiles; exhaustive |
| LP-02 | `[_, ..]` alone | `T4231`; witness `[]` |
| LP-03 | `[]`, `[_]`, `[_, _, ..]` | Compiles; exhaustive |
| LP-04 | `[a, ..x, ..y]` | `L2021` |
| LP-05 | `[a, b]` against `["orders", "7"]` | Runtime: binds `a = "orders"`, `b = "7"` |
| LP-06 | `[first, ..rest]` against a one-element list | Runtime: `rest` is `[]` |
| LP-07 | `[.., last]` against a 1,000,000-element list | Runtime: one indexed read; no copy |
| LP-08 | `sum` over 10,000,000 elements with the `loop` example | Runtime: constant stack; `rest` views allocate O(1) each |
| LP-09 | `Some([("id", Json.Number(id)), ..])` | Compiles; nested match binds `id` |
| TP-01 | `let (name, port) = parse_address(raw)` | Compiles |
| TP-02 | `let Some(value) = lookup(key)` | `T4231`; witness `None` |
| TP-03 | Tuple pattern of arity 3 against a pair | `T4230` |
| TP-04 | Struct pattern for a struct from another module | `E_PRIVATE_STRUCT_CONSTRUCTOR` |
| TP-05 | `Money(cents: 2500)` as a pattern | `S1001`; message says patterns are positional |
| TP-06 | `-1` literal pattern | Compiles; matches negative one |
| TP-07 | `"é"` against a decomposed `"é"` | Runtime: no match |
| FP-01 | The trace summarizer `main` with failure-type arms | Compiles; residual row matches the original |
| FP-02 | `Failure(error: FileError)` against a row without `FileError` | `T4224` |
| FP-03 | Generic recovery thunk with `Failure(error: FileError)` | Compiles; residual row `E - FileError` |
| FP-04 | The same arm with a guard | Compiles; nothing subtracted |
| FP-05 | `LineTooLong(_, _) as error` before `error: FileError` | Compiles; the `FileError` arm covers the remaining variants |
| FP-06 | `LineTooLong` arm after `error: FileError` | `T4232` |
| FP-07 | `case error` with `FileError` and `TraceError` arms after `Failure(error)` | Compiles; omitting one is `T4231` |
| FP-08 | `count: Text` matching an `Int` | `T4283` |
| FP-09 | Defect raised inside the operation | Runtime: bypasses every failure-type arm |
| RU-01 | `{ ..config, jobs: jobs }` | Compiles; type `Config` |
| RU-02 | `{ ..config, job: 4 }` | `T4205` |
| RU-03 | `{ ..config, jobs: 1, jobs: 2 }` | `T4211` |
| RU-04 | `{ ..config, jobs: "four" }` | `T4203` |
| RU-05 | `{ ..config }` | `L2022` |
| RU-06 | Update of an enum value | `T4284` |
| RU-07 | Update of a struct from another module | `E_PRIVATE_STRUCT_CONSTRUCTOR` |
| RU-08 | `Config(..base, jobs)` | `S1001`; fix suggests `{ ..base, jobs: jobs }` |
| RU-09 | Update whose second field value fails | Runtime: base and first field evaluated in order; the failure propagates and no updated value exists |
| RU-10 | Update of a fake provider's `charge` | Compiles; type is the interface |

## Implementation notes

Affected compiler stages, in pipeline order:

- **Syntax** (`compiler/chimi/src/syntax.rs`). `parse_pattern` gains tuple,
  list, `..`, typed binding, alias, and negative-literal forms. Function
  declarations accept patterns in parameter position and an optional guard
  before the body. Brace expressions accept a leading `..base`. Recovery for a
  labelled pattern or positional update emits teaching `S1001` messages.
- **Lowering** (`compiler/chimi/src/semantic.rs`). Adjacent same-named
  functions become one function whose body is a multi-subject case. That case
  keeps a clause origin, so diagnostics talk about clauses and point at clause
  heads. `Pattern` currently combines `path`, `literal`, `applied`, and
  `arguments` in one struct. It should become an enum with one variant per
  pattern form, so that a literal with arguments cannot be represented. This
  stage emits `L2020`–`L2022`.
- **Resolution** (`compiler/chimi/src/resolution.rs`). It assigns one symbol per
  clause group, emits `E_SEPARATED_FUNCTION_CLAUSES`, resolves struct patterns
  and update bases against field visibility, and checks duplicate pattern
  bindings.
- **Typing** (`compiler/chimi/src/typing.rs`). `infer_ordinary_case` and
  `pattern_row_is_useful` are reused. `MatchTag` gains tuple, struct, and
  list-length classes. List rows are expanded into length classes before
  specialization. Failure-type patterns are expanded into one row per present
  variant before usefulness, and the outcome path's subtraction iterates the
  expanded variants. Parameter-type unification across clauses reports `T4281`
  instead of a generic mismatch. Witness rendering for `T4231` is new.
- **Capability checking** (`compiler/chimi/src/capability.rs`). Destructuring a
  tuple or struct moves exclusive capabilities out of it by the existing
  transfer rules. `rest` views share storage, so a view of a list holding
  exclusive capabilities counts as a use of that list.
- **Executable Core** (`compiler/chimi/src/executable.rs`). Clauses lower to
  existing case Core. New Core operations are list length, list index, and list
  view. Failure-type patterns lower to variant tests. Updates lower to
  construction from field reads.
- **Backend and runtime** (`compiler/chimi/src/backend/`). They need the
  persistent vector's offset-and-length view (`L3`). A clause match records the
  selected clause in full tracing.
- **Formatter** (`compiler/chimi/src/formatter.rs`). It keeps clause groups
  without blank lines and never reorders clauses or update fields.
- **Inspection** (`compiler/chimi/src/inspection.rs`). It reports the group
  contract and clause position for any clause.

## Rejected alternatives

- **One header with a clause block**, like OCaml's `function`, Erlang's `fun`,
  or `pub fn area -> Int { (Circle(r)) -> ... }`. It needs a parameterless
  header form that nothing else in Chimi uses, and it reads as a `case` with
  extra syntax.
- **Only a single `case` in the body**, as Gleam does. That is the "one header
  followed by internal branching" shape `CONTEXT.md` lists to avoid for
  Pattern-Directed Clause.
- **Header items on every clause**, as Elixir requires for `def`. It repeats
  `pub`, generics, and contracts and invites disagreement between headers.
- **A separate signature line** without a body, as in Haskell. It is a new
  declaration form. Inferred contracts already give tooling and documentation
  the full signature. It can be added later if public APIs need it.
- **Partial clause functions** with a runtime "no clause matched" defect, as in
  Elixir and Erlang. That contradicts kernel exhaustiveness and turns a
  compile-time gap into a production crash.
- **Arity overloading** (`f/1` and `f/2`). One module-qualified name would
  denote several functions, and pipelines and function values would become
  ambiguous.
- **`...rest`** (JavaScript) for list rest. Gleam and Rust use `..`, and record
  update already uses `..`.
- **`Failure(FileError as error)`**. It reverses the parameter order of
  `name: Type`, and `as` is reserved for aliasing a pattern.
- **Positional record update**. See [Why update names fields](#why-update-names-fields).
- **`base with { jobs: 4 }`** (C#, OCaml). It reads well but adds a keyword.
  `{ ..base, jobs: 4 }` reuses record literal syntax and the `..` that patterns
  use, and JavaScript programmers already know its shape.

## Open questions

These do not block this specification. Each records a recommendation that the
specification already follows.

1. **Should anonymous functions have several clauses?** Elixir allows
   `fn 0 -> ...; n -> ... end`. Chimi would need a second clause syntax inside
   expressions. **Recommendation:** no, until a real program shows that
   `fn(x) { case x { ... } }` hurts. Named clause functions cover dispatch, and
   `case` is already the nearby form inside an expression.
2. **Should the first clause carry the whole header, or should a bodiless
   signature line exist?** Documentation that shows `describe(0)` as the first
   clause is less clear than a named signature. **Recommendation:** first
   clause only now. The documentation generator renders the inferred contract
   (`fn(Int) -> Text errors CountError`), not the first clause's patterns.
   Revisit when `T4` documentation exists.
