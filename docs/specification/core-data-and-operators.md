# Core Data and Operators

## Status

This is the normative contract for Chimi's core value operations: boolean
logic, structural equality, hashing, structural ordering, the numeric model,
tuples, the `Void` value, the `Never` type, and integer ranges
(`prog show ts-ff2d8f`). It closes gaps G2, G3, G4, G5, G30, G32, L4, and L6 from
the [language completion plan](../plans/language-completion-plan.md), and sets
the representation expectations for L2 (`Decimal`).

Implementation status (Phase 1, epic `ep-a6f138`): boolean logic is
implemented (`ts-fd46b6`). Numbers are implemented (`ts-a28bb7`) except
`Decimal`, `==` on numbers, and `Float` map keys, which wait for equality.
The other sections are implementation targets tracked by `ts-95bffa`
(equality, hashing, ordering) and `ts-5be727` (tuples, `Void`, `Never`,
ranges); `Decimal` is phase 2. Chimi remains free to break. This is an
implementation target.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative. Examples
use the current source dialect with positional construction.

## Goals

- Compound conditions read as one expression, not as nested `case`.
- Any plain data can be compared with `==`, used as a `Map` key or `Set`
  element, and sorted, with no declaration, import, or trait.
- A comparison that cannot mean anything, such as comparing two functions,
  fails at compile time and names the field that prevents it.
- Every arithmetic result is either a correct value or a defect with context.
  No operation silently wraps, saturates, or produces a value that breaks
  equality.
- Money and other exact quantities have a type that is not `Float`.
- The forms are familiar to someone who has used Python, Rust, Swift, Kotlin,
  or TypeScript.

These rules follow the recorded decisions: arguments are positional, there is no
local mutation, and equality, hashing, and ordering are derived structurally
with plain generics. A non-structural order is an explicit comparator value.

## Scope

This document specifies:

- the `and`, `or`, and `not` operators;
- the derived **equatable** and **ordered** value properties;
- `==`, `!=`, `<`, `<=`, `>`, `>=`, the `Ordering` enum, and comparators;
- derived hashing as a runtime obligation;
- `Int`, `Float`, the `%` operator, numeric literals, conversions, and parsing;
- representation expectations for `std/decimal`;
- equality and order of `Text`, lists, maps, sets, options, and `Json.Value`;
- tuples, the `Void` value expression, the `Never` type, and `Range`;
- operator precedence for every operator in the language.

It does not specify list patterns (G15), multi-clause functions (L1), record
update (G8), debug rendering (G10), bit operations, big integers, or the full
API of `std/int`, `std/float`, `std/order`, or `std/decimal`. Standard function
names in this document are provisional unless a rule states the name. Their
behavior is normative.

## Value properties

### Equatable and ordered

The compiler derives two structural properties for every type, the same way it
derives the shareable and transferable properties in
[task and channel capability contracts](task-and-channel-capabilities.md#concurrent-value-properties):

- An **equatable** type supports `==`, `!=`, and derived hashing. It can be a
  `Map` key or a `Set` element.
- An **ordered** type supports `<`, `<=`, `>`, `>=`, and `Order.compare`. Every
  ordered type is equatable. Not every equatable type is ordered.

These are execution properties, not interfaces. A program never declares,
implements, or imports them. Tooling MUST show them on hover and in inspection
output, and every diagnostic that rejects one MUST name the field, payload,
element, or capture that prevents it.

The derivation walks the same structure as the shareable derivation but stops
at different leaves:

| Type | Equatable | Ordered |
| --- | --- | --- |
| `Int`, `Bool`, `Text`, `Void` | yes | yes |
| `Float` | yes | yes |
| `Range` | yes | yes |
| `Decimal`, `Json.Number` | yes | yes |
| `Never` | yes | yes |
| struct, enum, tuple | when every field or payload is | when every field or payload is |
| `List<T>`, `Option<T>` | when `T` is | when `T` is |
| anonymous record | when every field is | no |
| `Map<K, V>` | when `K` and `V` are | no |
| `Set<T>` | when `T` is | no |
| `Json.Value` | yes | no (it contains `Map`) |
| `Process.Address<M>` | yes, by routing identity | no |
| failure set `A \| B` | when every member is | no |
| function value | no | no |
| interface value or provider record | no (its fields are functions) | no |
| `Task<T, F>`, `Channel.Sender<T>`, `Channel.Receiver<T>`, `Channel.Completion<T>` | no | no |
| opaque standard type | as published in module metadata | as published in module metadata |

Rules:

- A struct or enum whose fields are hidden by `pub` without field access MUST
  still expose its derived properties to other modules. A module that needs a
  canonical equality, such as cents that are never negative, MUST normalize
  values when it constructs them.
- A generic struct or enum is equatable or ordered per instantiation:
  `Box<Int>` is ordered, `Box<fn() -> Int>` is neither.
- Recursive types derive their properties as the greatest fixed point: a
  recursive enum is equatable unless some non-recursive component is not.
- An opaque standard type MUST state both properties in its module metadata.
  Private representation MUST NOT grant either property silently.
- Records are not ordered because they have no declaration order. Ordering by
  field name would invent an order the program never wrote.
- Maps and sets are not ordered because they have no element order. Sorting a
  list of maps needs an explicit comparator.
- Capabilities are not equatable. Comparing two receivers would expose runtime
  identity that the capability model keeps private. `Process.Address` is a
  routing identity that programs store in registries, so it is equatable by
  identity. A named address equals another named address with the same
  `process name` declaration. A direct address equals another direct address
  to the same incarnation. A named address never equals a direct address, even
  when both currently route to the same incarnation.
- A failure set of several members is equatable so that a projected
  `Failure(error)` value can be compared, with another value of the set or
  with a value of one of its member types, which widens to the set type for
  the comparison. It is not ordered: its members
  have no declaration order. A failure set of one member is that member's
  type and keeps its properties.

### Generic code

A generic function MAY use `==` or `<` on a value of a generic parameter. The
use infers a requirement on that parameter:

```chimi
fn same_pair<T>(pair: (T, T)) -> Bool {
  let (left, right) = pair
  left == right
}
```

Tooling displays the inferred contract as:

```text
fn<T: equatable>((T, T)) -> Bool
```

Rules:

- The requirement is part of the inferred callable contract. It propagates
  through calls exactly as failure and needs rows do.
- Each instantiation MUST satisfy the requirement, whether a call instantiates
  `T` or a generic function is used as a value of a specific function type
  (for example, passed as an argument). An instantiation with a type that lacks
  the property is rejected with `T4305`. A requirement that starts in a
  standard function, such as `Order.compare` or `Map.empty`, labels the call
  inside the callee that created it. The diagnostic
  names the parameter, the operator use inside the callee that created the
  requirement, and the component of the argument type that lacks the property.
- Source code has no spelling for the requirement. Annotations stay optional,
  and an explicit header `fn same_pair<T>(...)` does not need to mention it.
- The requirement MUST NOT change runtime representation or calling
  convention. Equality and comparison work on runtime-tagged values, so generic
  code needs no dictionary argument.
- An inference variable that keeps an equatable or ordered requirement but is
  never solved follows the existing ambiguous-type rule.

This is not a trait system. Programs cannot define new properties, implement
them for a type, or override the derived behavior. See
[open question 2](#open-questions) for function-type annotations.

## Equality

`a == b` and `a != b` take two operands of the same equatable type and produce
`Bool`.

```chimi
struct Money {
  cents: Int
}

enum Status {
  Pending
  Paid(amount: Money)
}

const settled = Paid(Money(500)) == Paid(Money(500))      // True
const changed = Paid(Money(500)) != Status.Pending         // True
const pair_match = (1, "one") == (1, "one")                // True
```

Rules:

- Both operands MUST have the same type. Chimi has no implicit conversion, so
  `1 == 1.0` is rejected with `T4302`, and `1 == "1"` is rejected with
  `T4302`.
- Each operand type MUST be equatable, or the expression is rejected with
  `T4300`.
- The left operand evaluates first, then the right operand, then the
  comparison. A failure, defect, park, or cancellation in an operand behaves as
  it does for any call argument.
- The comparison itself MUST be pure and total. It never fails, defects, parks,
  needs a provider, or emits a boundary trace event.
- `a != b` MUST equal `not (a == b)`.
- Equality MUST be an equivalence relation: reflexive, symmetric, and
  transitive for every value of every equatable type. The `Float` rules below
  exist to keep this true.

Structural equality is defined per type:

| Type | Two values are equal when |
| --- | --- |
| `Int`, `Bool`, `Void` | they are the same value |
| `Float` | they are numerically equal; `-0.0 == 0.0` |
| `Text` | their UTF-8 byte sequences are identical |
| struct | they are the same nominal type and every field is equal |
| enum | they are the same enum type and the same variant, and every payload field is equal |
| tuple, anonymous record | every element or field is equal |
| `List<T>` | they have the same length and equal elements at every index |
| `Option<T>` | both are `None`, or both are `Some` with equal payloads |
| `Map<K, V>` | they have the same key set and equal values for each key |
| `Set<T>` | they have the same elements |
| `Range` | they have equal normalized start and end bounds |
| `Decimal`, `Json.Number` | they denote the same exact number |

Equality never compares identity. Two separately built lists with equal
elements are equal. A compiler MAY short-circuit when both operands are the same
runtime object, because the result is unobservable.

Structural equality replaces `Text.equal`. `Text.equal` MAY remain as an
ordinary function with the same result as `==`.

## Hashing

Every equatable value has a derived hash. Source code cannot call it in this
version. An implementation whose maps use only the internal total order need
not compute hashes at all; the rules below then constrain whichever hashing a
later `Set` or `Map` representation adds. It exists so that `Map` and `Set` can use any equatable key.

Rules:

- Equal values MUST have equal hashes. In particular, `-0.0` and `0.0` hash
  equally, `Decimal` values `1.5` and `1.50` hash equally, and maps with equal
  entries hash equally regardless of insertion history.
- The hash of a map or set MUST NOT depend on storage or insertion order.
- The runtime MAY seed hashes per execution. Programs cannot observe hash
  values, so seeding is unobservable.
- `Map` and `Set` MAY use hashing, an internal total order over all equatable
  values, or both. The internal order is not the source-level ordered property
  and MUST NOT be observable through enumeration. `Debug.render` is not
  enumeration: it MAY print entries in the internal order, because it is a
  diagnostic text form that equal maps must render identically. The rule in
  [immutable maps](trace-data-and-processing.md#immutable-maps) that `Map` does
  not promise enumeration order still holds.
- Hashing and equality MUST handle any nesting depth that value construction
  allows without exhausting the host stack.

## Ordering

### Comparison operators

`<`, `<=`, `>`, and `>=` take two operands of the same ordered type and produce
`Bool`. They evaluate like `==`.

Structural order is defined per type:

| Type | Order |
| --- | --- |
| `Int` | numeric |
| `Float` | numeric; `-0.0` and `0.0` compare equal |
| `Bool` | `False < True` |
| `Void` | one value, equal to itself |
| `Text` | lexicographic by UTF-8 bytes |
| struct | lexicographic by field, in declaration order |
| enum | by variant declaration order, then lexicographic by payload field |
| tuple | lexicographic by element |
| `List<T>` | lexicographic by element; a proper prefix is less |
| `Option<T>` | `None < Some(_)`, then by payload |
| `Range` | lexicographic by normalized start, then end |
| `Decimal`, `Json.Number` | numeric |

```chimi
struct Version {
  major: Int,
  minor: Int,
  patch: Int,
}

const newer = Version(1, 10, 0) > Version(1, 9, 7)        // True
const earlier = (2, "b") < (2, "c")                        // True
const shorter = [1, 2] < [1, 2, 0]                         // True
```

Rules:

- Structural order MUST be a total order consistent with equality: exactly one
  of `a < b`, `a == b`, and `a > b` holds.
- Reordering struct fields or enum variants changes the derived order. It is a
  breaking change, as it already is for positional construction.
- The standard `Option` declares `None` before `Some`. Its derived order
  follows that declaration.
- Comparison and equality operators are non-associative. `a < b < c` and
  `a == b == c` are rejected with `S1006`. Write `a < b and b < c`.

### The `Ordering` enum

`std/order` declares:

```chimi
pub enum Ordering {
  Less
  Equal
  Greater
}
```

`Order.compare(a, b)` returns the structural `Ordering` of two values of the
same ordered type. It is an ordinary function value, so it can be passed where a
comparator is expected.

### Comparators

A **comparator** for `A` is any function value of type
`fn(A, A) -> Ordering`. Comparators express every order that is not the
structural one: descending order, ordering by one field, case-insensitive text,
or an order over a type that is not ordered.

```chimi
import std/order as Order
import std/order.{Equal, Ordering}

struct User {
  name: Text,
  age: Int,
}

fn by_age_then_name(left: User, right: User) -> Ordering {
  case Order.compare(left.age, right.age) {
    Equal -> Order.compare(left.name, right.name)
    other -> other
  }
}

const youngest_first = List.sort_by(users, by_age_then_name)
const oldest_first = List.sort_by(users, Order.reverse(by_age_then_name))
const by_name = List.sort_by(users, Order.by(fn(user) { user.name }))
```

`std/order` MUST provide functions equivalent to:

| Function | Behavior |
| --- | --- |
| `Order.compare(A, A) -> Ordering` | structural order; `A` MUST be ordered |
| `Order.reverse(comparator) -> comparator` | swaps `Less` and `Greater` |
| `Order.by(fn(A) -> B) -> comparator for A` | compares the extracted keys structurally; `B` MUST be ordered |
| `Order.then(first, second) -> comparator` | uses `second` only when `first` returns `Equal` |
| `Order.min(A, A) -> A`, `Order.max(A, A) -> A` | structural; returns the left operand when equal |

`std/list` MUST provide `List.sort(List<A>) -> List<A>`, where `A` is ordered,
and `List.sort_by(List<A>, comparator) -> List<A>`. Both sorts MUST be stable.

A comparator MAY have failures, needs, and effects. A function that accepts a
comparator propagates them as `Map.update` propagates its callback's rows. A
comparator that is not a total preorder makes the sort result an unspecified
permutation of its input. It MUST NOT cause a defect, lose elements, or
duplicate elements.

Comparison operators never accept a comparator. Chimi has no operator
overloading.

## Boolean logic

### Spelling

The boolean operators are the keywords `and`, `or`, and `not`:

```chimi
fn is_success(status: Int) -> Bool {
  status >= 200 and status < 300
}

fn should_retry(attempt: Int, error: HttpError) -> Bool {
  attempt < 5 and not is_permanent(error)
}
```

Rules:

- `and`, `or`, and `not` are reserved words and cannot be identifiers.
- Each operand MUST have type `Bool`. Chimi has no truthiness, so
  `count and ready` with `count: Int` is rejected with `T4304`.
- `&&`, `||`, and a prefix `!` are not operators. The lexer recognizes them
  only to report `S1004` with a source edit that replaces them with `and`,
  `or`, or `not`. `!=` remains the inequality operator.

### Short-circuit evaluation

`a and b` means exactly:

```chimi
case a {
  True -> b
  False -> False
}
```

`a or b` means exactly:

```chimi
case a {
  True -> True
  False -> b
}
```

`not a` means `case a { True -> False  False -> True }`.

Consequences, which an implementation MUST preserve:

- `b` runs only when the result depends on it. When `b` does not run, it emits
  no events, takes no provider calls, and does not park.
- If an operand fails, the failure leaves the expression through the ordinary
  failure channel. `a and b` exposes the union of the failure rows of `a` and
  `b`, even though `b` may not run.
- The needs and effects rows are the union of both operands' rows. Static
  contracts do not depend on which branch runs.
- An operand MAY park, for example on `Channel.receive`. The expression waits
  as any `case` subject waits.
- Cancellation, defects, and parking follow the `case` rules. `and` and `or`
  add no new terminal behavior.
- For capability flow, `b` is a conditional branch. A transfer inside `b` is a
  possible transfer, as it is inside one `case` arm. The loop rule `T4250`
  applies to it.
- Inside a case guard, both operands MUST be pure, as the guard rule already
  requires.

### Precedence

`not` binds more loosely than comparison, so `not a == b` means
`not (a == b)`. `and` binds more tightly than `or`. Boolean operators bind more
loosely than `|>`, so a pipeline result can be combined directly:

```chimi
let ready = items |> List.is_empty and not draining
// parsed as: (items |> List.is_empty) and (not draining)
```

The full table is in [operator precedence](#operator-precedence).

## Numbers

### `Int`

`Int` is a signed 64-bit two's complement integer, from -2^63 to 2^63 - 1.

| Operation | Result |
| --- | --- |
| `a + b`, `a - b`, `a * b` | the exact result, or a defect if it is outside `Int` |
| `-a` | the exact negation, or a defect for -2^63 |
| `a / b` | the quotient truncated toward zero; a defect if `b == 0` or for `-2^63 / -1` |
| `a % b` | `a - (a / b) * b`; the sign follows `a`; a defect if `b == 0` |

```chimi
let quotient = -7 / 2       // -3
let remainder = -7 % 2      // -1
let positive = 7 % -2       // 1
```

Rules:

- Overflow and division by zero MUST be defects, not expected failures.
  Putting a failure variant on every `+` would make every arithmetic
  expression fallible, and the failure would almost always be a programmer
  fault. A defect crashes the bounded unit of work with context, as principle 6
  requires.
- The defect context MUST name the operator and its source location, and
  SHOULD include both operand values. The message names the operator and the
  operands, as in ``integer overflow in `+`: 9223372036854775807 + 1``; the
  defect outcome's `origin_id` locates the operation in source.
- `-2^63 % -1` is 0. It MUST NOT defect, even though the matching division
  overflows.
- Code that expects overflow uses `std/int` functions that return a value
  instead: `Int.add_checked(a, b) -> Option<Int>`, and the `_checked` forms of
  the other operators. `Int.modulo(a, n)` returns a result with the sign of
  `n`, for clock arithmetic and bucket indexes; `Int.modulo(a, 0)` defects.
  Wrapping arithmetic for hash functions belongs to the later bit-operations
  work.
- When both operands are literals or folded constants and the operation
  overflows or divides by zero, the compiler SHOULD warn with `T4312`. A
  folded constant is an expression built only from literals and operators;
  a `let` binding is not folded through. The warning covers `Int` operations
  only; a `Float` operation that always defects is not required to warn. It MUST
  still compile the operation so that it defects at run time. Constant folding
  MUST NOT change observable behavior. `chimi check` reports the warning and
  succeeds.

### `Float`

`Float` is an IEEE 754 binary64 number restricted to **finite values**. Every
`Float` value is a finite number or negative zero. NaN and infinity are not
`Float` values.

| Operation | Result |
| --- | --- |
| `a + b`, `a - b`, `a * b`, `a / b` | the IEEE 754 result rounded to nearest, ties to even |
| `a % b` | the truncated remainder (C `fmod`); the sign follows `a` |
| `-a` | the negation |
| any operation whose IEEE result is NaN or infinite | a defect |
| `a / 0.0`, `a % 0.0` | a defect |

Rules:

- An operation that would produce NaN or an infinity MUST defect with a
  context that names the operator. Gradual underflow and rounding to zero are
  not defects.
- `-0.0` is a value. It compares equal to `0.0` and hashes equally.
  `Float.to_text` keeps the sign and returns `"-0.0"`. Debug rendering
  normalizes it to `0.0`, because equal values MUST render the same.
- Equality and order on `Float` are numeric. Because NaN cannot exist, they
  are a total order and an equivalence relation, and a `Float` can be a `Map`
  key, a `Set` element, or a struct field compared with `==`.
- `Float` has no literal patterns, as the
  [patterns spec](patterns-and-function-clauses.md#literal-patterns) states. A
  `Float` literal in a pattern is rejected with `T4313`. Compare in a guard
  instead.
- Functions that leave the finite domain return failures. For example,
  `Float.parse("inf")` fails, and a later `Float.square_root(x)` MUST fail
  for negative `x`.

This departs from IEEE 754 defaults. [Open question 1](#open-questions) records
the choice.

### No mixed arithmetic

Every arithmetic and comparison operator requires both operands to have the
same type. `1 + 2.0` is rejected with `T4302`, and the diagnostic MUST suggest
`Float.from_int`. `+` is not defined for `Text`. Use `Text.append`.

### Numeric literals

Integer literals:

- decimal: `0`, `42`, `1_000_000`;
- hexadecimal: `0x1F`, `0xFF_FF`;
- binary: `0b1010_0001`.

Float literals:

- a decimal point with at least one digit on each side: `0.5`, `3.0`,
  `1_000.25`;
- an exponent, with or without a decimal point, whose sign MAY be `+` or `-`:
  `1e9`, `2.5e-3`, `1e+5`, `6.02E23`.

Rules:

- `_` MAY appear only between two digits.
- `1.`, `.5`, and octal forms are not literals. A decimal literal with a
  leading zero, such as `017`, is rejected with `S1001` rather than read as
  either octal or decimal.
- A `.` that follows a digit starts a fraction only when a digit follows it. So
  `0..10` is a range and `point.x` stays a field access.
- An integer literal outside `Int` is rejected with `S1002`. Hexadecimal and
  binary literals are not reinterpreted as two's complement:
  `0xFFFF_FFFF_FFFF_FFFF` is out of range.
- A float literal whose value rounds to an infinity is rejected with `S1003`.
  A literal that is not exactly representable rounds to the nearest `Float`.
- Negative numbers are unary `-` applied to a literal. Lowering MUST fold `-`
  applied directly to an integer literal before the range check, so
  `-9223372036854775808` is valid. Whitespace between `-` and the literal
  does not prevent folding. The same rule applies to literal patterns
  ([patterns spec](patterns-and-function-clauses.md#literal-patterns)). A grouped operand, as in
  `-(9223372036854775808)`, is not folded and is out of range.

### Conversions and parsing

There are no implicit numeric conversions. `std/int` and `std/float` MUST
provide functions equivalent to the following. They are ordinary standard
modules, so a program writes `import std/int as Int` and
`import std/float as Float` before it calls them. The `Float` type itself is
built in, next to `Int`.

| Function | Outcome |
| --- | --- |
| `Float.from_int(Int) -> Float` | rounds to nearest, ties to even; exact up to 2^53 |
| `Float.truncate`, `Float.floor`, `Float.ceiling`, `Float.round` `(Float) -> Float` | `round` rounds half away from zero |
| `Float.to_int(Float) -> Int` | fails with `NotInteger` or `OutOfRange` |
| `Int.to_text(Int) -> Text` | canonical decimal |
| `Float.to_text(Float) -> Text` | the shortest text that parses back to the same value; it always contains `.` or `e` |
| `Int.parse(Text) -> Int` | fails with `Empty`, `InvalidDigit`, or `OutOfRange` |
| `Float.parse(Text) -> Float` | fails with `Empty`, `Invalid`, or `OutOfRange` |

Parsing accepts the decimal literal grammar without `_`, with an optional
leading `-`. Unlike source literals, parsed text MAY have leading zeros
(`"007"` is 7), because external input often does. `Float.parse` also accepts
integer text such as `"7"`. It rejects whitespace, a leading `+`, `nan`, and
`inf`. Text with no digits, such as `"-"`, fails with `InvalidDigit` from
`Int.parse` and `Invalid` from `Float.parse`; only `""` is `Empty`. A parse
failure is an expected failure, because parsing reads external input.

`Float.to_int` takes a value that the caller has already rounded. Rounding is a
separate, visible step. The same `NotInteger` and `OutOfRange` variants exist
for `Json.number_to_int` today.

The failure types are `ParseError` in `std/int` (`Empty`, `InvalidDigit`,
`OutOfRange`), and `ParseError` (`Empty`, `Invalid`, `OutOfRange`) and
`ConversionError` (`NotInteger`, `OutOfRange`) in `std/float`. The variants
carry no fields. Because both `std/float` types own an `OutOfRange`, that
variant is named through its type, as in `Float.ParseError.OutOfRange`.

`Float.to_text` renders positional notation for decimal exponents from -5 to
15 and scientific notation otherwise, as in `0.00001`, `1e-6`,
`1000000000000000.0`, and `1e16`. Among the shortest texts that parse back, it
picks the one closest to the value, and the larger on an exact tie. Outcome
JSON renders a `Float` with the same text.

### `Decimal`

`std/decimal` declares the opaque type `Decimal` for exact base-10 quantities
such as money. It is a standard primitive, not a user struct, so its equality
is numeric rather than representational.

```chimi
import std/decimal as Decimal
import std/decimal.{Decimal, Rounding}

const price = Decimal.of(1999, 2)                 // 19.99
const total = price * Decimal.of(3, 0)            // 59.97
const share = Decimal.divide(total, Decimal.of(4, 0), 2, Rounding.HalfEven)
const same = Decimal.of(150, 2) == Decimal.of(15, 1)   // True
```

Representation expectations:

- A `Decimal` is a signed coefficient and a non-negative scale. It is exact.
  It MUST support at least 38 significant decimal digits and scales from 0 to
  38.
- `Decimal.of(coefficient, scale)` builds `coefficient × 10^-scale`. It
  defects when `scale` is outside the supported range.
- Equality, hashing, and order are numeric: `1.50 == 1.5`. The scale is kept
  for `Decimal.to_text`, so `Decimal.to_text(Decimal.of(150, 2))` is
  `"1.50"`. Debug rendering drops trailing zeros, because equal values MUST
  render the same, so the same value renders as `1.5`.
- `+`, `-`, unary `-`, `*`, and the comparison operators apply to two
  `Decimal` operands. The result of `+` and `-` has the larger scale. The
  result of `*` has the sum of the scales.
- A result that exceeds the supported precision or scale MUST defect.
- `/` and `%` on `Decimal` are rejected with `T4306`. Division needs a result
  scale and a rounding mode, so it is `Decimal.divide(a, b, scale, rounding)`.
  Division by zero defects.
- `Decimal.parse(Text) -> Decimal` fails on invalid text. It accepts the
  decimal literal grammar without `_` and without an exponent.
- `Decimal` has no literal syntax. `Decimal.of` is total, so constants need no
  failure handling.
- `Json.number_to_decimal(Number) -> Decimal` converts exactly or fails with
  `OutOfRange`. `Decimal` has no conversion from `Float`. `Decimal.to_float`
  rounds to nearest.

`Decimal` is the one library type that the operator rules name. The compiler
knows it as a standard primitive, in the same way that it knows `Map`.

## Text

`Text` is always valid UTF-8, so byte order and Unicode scalar order are the
same order.

- `==` compares UTF-8 bytes. Two texts are equal exactly when they contain the
  same Unicode scalar sequence. Equality performs no normalization, case
  folding, or locale handling: `"é"` written as one scalar is not equal to
  `"e"` followed by a combining accent.
- `<` compares UTF-8 bytes lexicographically. This matches `Json.encode` key
  order and is independent of host locale.
- Locale-aware collation, case-insensitive order, and normalization are
  comparators and functions in `std/text`, not the structural order.

## Collections and JSON

- `List<T>`, `Option<T>`, `Map<K, V>`, and `Set<T>` derive their properties
  from their type arguments, as the property table states.
- `Map<K, V>` and `Set<T>` MUST accept any equatable key or element type. The
  first-implementation restriction to `Text` keys in
  [immutable maps](trace-data-and-processing.md#immutable-maps) is lifted by
  this contract. A `Map` whose key type is not equatable is rejected with
  `T4300` at the use that fixes the key type.
- `Json.Value` is equatable and not ordered. Two `Json.Number` values are equal
  when they denote the same exact decimal number, so `Json.decode("1.0")`
  equals `Json.decode("1")` and `Json.decode("1e2")` equals
  `Json.decode("100")`. `Json.encode` still re-encodes each number with its
  validated source spelling, so equal values MAY encode differently.
- `Json.Number` is ordered by numeric value. Comparing it MUST be exact and
  MUST NOT round through `Float`.

## Tuples

A tuple is an anonymous, ordered product of two or more values.

```chimi
fn min_and_max(values: List<Int>) -> (Int, Int) {
  // ...
}

let (low, high) = min_and_max([3, 1, 4])
let counts = Map.from([("ok", 3), ("failed", 1)])

case request_line {
  ("GET", path) -> fetch(path)
  (method, _) -> reject(method)
}
```

Rules:

- A tuple expression is `(e1, e2, ...)` with at least two elements. A trailing
  comma is allowed after the last element.
- A tuple type is `(T1, T2, ...)` with at least two elements.
- `(e)` is a grouped expression, not a tuple. `(e,)` and `()` are rejected with
  `S1005`. The `()` diagnostic suggests `Void`.
- Tuples are structural. Two tuple types are the same when they have the same
  arity and the same element types in order.
- Tuples have no element access syntax such as `pair.0`. Elements are read with
  patterns.
- Tuple patterns, `let` destructuring, and coverage are specified in
  [tuple patterns](patterns-and-function-clauses.md#tuple-patterns) and
  [irrefutable patterns](patterns-and-function-clauses.md#irrefutable-patterns).
  In summary, a tuple pattern `(p1, p2, ...)` MUST have the subject's arity
  (`T4230`), and `let` accepts only an irrefutable pattern (`T4231`), so
  `let (low, high) = pair` is valid and `let (0, x) = pair` is not.
- Tuples derive equatable, ordered, shareable, and transferable from their
  elements. Order is lexicographic by element.
- A `case` over several subjects (`case a, b { ... }`) is not a tuple.
  `case (a, b) { ... }` matches one tuple subject.

Use a struct when the elements have names that matter to readers. A linter MAY
suggest a struct for tuples with more than three elements.

## `Void`

`Void` is both the type and its only value. The value expression is `Void`:

```chimi
let console = {
  write_line: fn(_line) { Void },
}

loop _ = Void {
  handle(Channel.receive(jobs))
  Void
}
```

Rules:

- `Void` in expression position has type `Void`. The parser MUST accept it
  where it currently reports "Void is a type, not a value".
- `Void` is also a pattern that matches the value. `_` matches it too.
- `break` with no value still means `break Void`.
- An empty block remains invalid and MUST NOT stand in for `Void`.
- `Void` is equatable, ordered, shareable, and transferable.

## `Never`

`Never` is the type of an expression that never produces a success value. It
has no values.

```chimi
fn reject(reason: Text) -> Never {
  Failure(RequestError.Invalid(reason))
}

fn route(request: Request) -> Response {
  case request.method {
    "GET" -> fetch(request)
    other -> reject(other)
  }
}
```

These expressions have type `Never`:

- a `loop` that no `break` targets;
- `break`, which leaves the enclosing loop instead of producing a value;
- `Failure(error)`, whose success type is `Never`;
- a call whose success type is `Never`;
- a block whose final expression has type `Never`;
- a `case` whose every arm has type `Never`.

Rules:

- `Never` is compatible with every expected type. When the arms of a `case`
  join, a `Never` arm adds nothing to the result type.
- A function that declares `-> Never` MUST have a body of type `Never`. A body
  that can produce a value is rejected with `T4308`.
- A function that ends only by failing has success type `Never` and keeps its
  failure row. `Never` says nothing about failures, defects, or cancellation.
- An inferred success type MAY be `Never`, and tooling displays it.
- In a failure row, `Never` is the empty failure set. `errors Never` means the
  same as no `errors` row, and `A | Never` is `A`.
- `Never` MAY appear in any type position. A parameter of type `Never` makes a
  function that cannot be called, and a `List<Never>` can only be empty.
- `Never` lives in the type namespace. The constructor
  `Supervisor.Restart.Never` lives in the value namespace and does not
  conflict with it.

The repetition spec calls the type of a loop with no `break` "unconstrained".
`Never` is that type with a name.

## `Range`

`Range` is a built-in type for a run of consecutive integers.

```chimi
let indexes = 0..List.length(items)      // 0, 1, ..., length - 1
let dice = 1..=6                         // 1, 2, 3, 4, 5, 6
let empty = 5..2                         // no elements
let window = List.slice(items, 2..5)
```

Rules:

- `start..end` is the half-open range from `start` up to, not including,
  `end`. `start..=end` is the inclusive range and means exactly
  `start..(end + 1)`.
- Both bounds MUST be `Int`, or the expression is rejected with `T4311`.
- `start..=end` defects when `end` is the largest `Int`, because the
  normalized end overflows.
- A range with `end <= start` is empty. Creating it is not a defect.
- Equality and order are structural on the normalized `(start, end)` pair.
  `0..3 == 0..=2`. Two empty ranges with different bounds are not equal.
- `Range` is equatable, ordered, shareable, and transferable.
- `std/range` MUST provide `Range.start`, `Range.end` (exclusive),
  `Range.length`, and `Range.contains`. `Range.length` returns 0 for an empty
  range and defects when the count exceeds `Int`.
- `..` is non-associative: `a..b..c` is rejected with `S1006`.
- `..` in a list pattern (G15, `[head, ..rest]`) is a pattern form, not a
  range. Chimi has no range patterns. The
  [patterns spec](patterns-and-function-clauses.md#range-patterns) matches
  ranges of `Int` with guards.

Ranges are values. They do not add iteration syntax. Counted repetition uses
`loop` or standard higher-order functions that take a `Range`.

## Operator precedence

From tightest to loosest. Operators on one row have equal precedence.

| Level | Operators | Associativity |
| --- | --- | --- |
| 1 | call `f(x)`, member access `a.b` | left |
| 2 | unary `-` | prefix |
| 3 | `*`, `/`, `%` | left |
| 4 | `+`, `-` | left |
| 5 | `..`, `..=` | non-associative |
| 6 | `==`, `!=`, `<`, `<=`, `>`, `>=` | non-associative |
| 7 | `\|>` | left |
| 8 | `not` | prefix |
| 9 | `and` | left |
| 10 | `or` | left |
| 11 | postfix `using` | postfix |

Rules:

- Levels 1, 2, 3, 4, 6, 7, and 11 keep their current relative order. Levels 5,
  8, 9, and 10 are new.
- A line break MAY come before any binary operator, as it can today before
  `+` and `|>`.
- The canonical formatter keeps the parentheses a program writes. A linter MAY
  suggest parentheses where `and` and `or` mix.

## Diagnostics

All codes are new. This contract reserves `S1003` to `S1006` and `T4300` to
`T4319`. Syntax-stage diagnostics use `S` codes because the shared envelope
MUST NOT blur stages.

| Code | Stage | Condition | Message and help |
| --- | --- | --- | --- |
| `S1002` | syntax | An integer literal, after folding a leading `-`, is outside `Int`, in an expression or a pattern. | "integer literal is outside the `Int` range" |
| `S1003` | syntax | A float literal rounds to an infinity, in an expression or a pattern. | "float literal is outside the finite `Float` range" |
| `S1004` | syntax | `&&`, `\|\|`, or prefix `!`. | "Chimi spells this `and`" (or `or`, `not`), with a replacement edit |
| `S1005` | syntax | `()` in expression or type position, or a one-element tuple `(e,)`. | "`()` is not a value; use `Void`", or "a tuple needs at least two elements" |
| `S1006` | syntax | A second non-associative operator at the same level, as in `a < b < c`, `a == b == c`, or `a..b..c`. | "comparisons do not chain; write `a < b and b < c`" |
| `T4300` | type | An operand of `==` or `!=`, or a `Map` key or `Set` element type, is not equatable. | Names the path to the offending component, such as "field `handler` of `Route` is a function" |
| `T4301` | type | An operand of `<`, `<=`, `>`, `>=`, or an argument of `Order.compare` or `List.sort`, is not ordered. | Names the component; once `std/list` provides `List.sort_by`, suggests a comparator with it |
| `T4302` | type | The operands of a binary arithmetic, equality, or comparison operator have different types. | For `Int` and `Float`, suggests `Float.from_int` or `Float.to_int` |
| `T4303` | type | An arithmetic operator, including unary `-`, is applied to a type other than `Int`, `Float`, or `Decimal`. When an operand is not numeric, `T4303` is reported instead of `T4302`, whichever side it is on. | For `Text` and `+`, suggests `Text.append`. For `Duration`, `Instant`, or `Timestamp`, names the `Duration` or `Clock` function to use, such as `Duration.add` |
| `T4304` | type | An operand of `and`, `or`, or `not` is not `Bool`. | "Chimi has no truthiness; compare explicitly" |
| `T4305` | type | A generic instantiation violates an inferred equatable or ordered requirement. | Labels the callee's operator use, the generic parameter, and the offending component |
| `T4306` | type | `/` or `%` on `Decimal`. | Suggests `Decimal.divide` with a scale and rounding mode |
| `T4308` | type | A function declared `-> Never` has a body that can produce a value. | Labels the value-producing expression |
| `T4311` | type | A range bound is not `Int`. | |
| `T4312` | type, warning | Constant operands overflow or divide by zero. | "this operation always defects" |
| `T4313` | type | A `Float` literal pattern. | Suggests a guard |

`T4307`, `T4309`, `T4310`, and `T4314` to `T4319` are reserved for this
contract's follow-up work.
A later stage SHOULD suppress diagnostics caused only by one of these errors,
for example a `T4203` on the result of a rejected `==`.

## Conformance matrix

Each row is one test. "Pass" means the program compiles with no diagnostics.
"Fail" names the required diagnostic code. "Run" names the required terminal
outcome.

### Boolean logic

| ID | Scenario | Expected |
| --- | --- | --- |
| B1 | `status >= 200 and status < 300` with `status: Int` | Pass; type `Bool` |
| B2 | `not a == b` | Pass; parses as `not (a == b)` |
| B3 | `a or b and c` | Pass; parses as `a or (b and c)` |
| B4 | `xs \|> List.is_empty and ready` (`List.is_empty` arrives with `std/list`; a local `is_empty` helper stands in until then) | Pass; parses as `(xs \|> List.is_empty) and ready` |
| B5 | `False and consume(sender)` where `consume` writes to a channel | Run: success `False`; no send event in a `full` trace |
| B6 | `True or fail_now()` where `fail_now` returns `Failure(E.X)` | Run: success `True`; the inferred failure row still contains `E.X` |
| B7 | `True and fail_now()` | Run: expected failure `E.X` |
| B8 | `ready and Channel.receive(rx) == Received(1)`, with a sender in a sibling task | Run: parks, then succeeds |
| B9 | `a and b` where `b` needs `payment` | Pass; the needs row contains `payment` |
| B10 | `1 and True` | Fail `T4304` |
| B11 | `a && b`, `a \|\| b`, `!a` | Fail `S1004` with replacement edits |
| B12 | a guard `x if x > 0 and is_valid(x)` where `is_valid` calls a provider | Fail with the existing guard-purity diagnostic |
| B13 | a loop pass with `done or sent(Channel.send(outbox, captured_rx))`, where `captured_rx` is an exclusive `Receiver` bound outside the loop and `sent` maps the send to `Bool` | Fail `T4250` |

### Equality and hashing

| ID | Scenario | Expected |
| --- | --- | --- |
| E1 | `Paid(Money(5)) == Paid(Money(5))` | Run: `True` |
| E2 | `Paid(Money(5)) == Pending` | Run: `False` |
| E3 | `[1, 2] == [1, 2]` built separately | Run: `True` |
| E4 | `{a: 1, b: "x"} == {b: "x", a: 1}` | Run: `True` |
| E5 | maps built by inserting the same entries in different orders compare with `==` | Run: `True` |
| E6 | `Map<(Int, Text), Int>` put, then get with an equal, separately built key | Run: `Some(value)` |
| E7 | `Set` of a user enum with payloads deduplicates equal elements | Run: size counts distinct values |
| E8 | `route_a == route_b` where `Route` has a function field | Fail `T4300`; the message names `handler` |
| E9 | `rx_a == rx_b` on channel receivers | Fail `T4300` |
| E10 | `let keyed: Map<fn() -> Int, Int> = Map.empty()`, and a parameter or field declared `Map<fn() -> Int, Int>` | Fail `T4300` at each written type |
| E11 | `1 == 1.0` | Fail `T4302` |
| E12 | `a == b == c` | Fail `S1006` |
| E13 | `same_pair((f, g))` with function values | Fail `T4305`, labelling `left == right` in `same_pair` |
| E14 | `same_pair((Box(1), Box(1)))` with `struct Box<T>` | Run: `True` |
| E15 | `Json.decode("1.0") == Json.decode("1")` | Run: `True` |
| E16 | `Json.decode("{\"a\":[1,2]}") == Json.decode("{ \"a\": [1, 2.0] }")` | Run: `True` |
| E17 | `"é" == "e\u{301}"` | Run: `False` |
| E18 | a list nested one million levels deep compared with an equal copy | Run: `True`; no host stack overflow |
| E19 | `a != b` for every pair in E1 to E7 | Run: the negation of `==` |
| E20 | a `Map` keyed by `0.0`, looked up with `-0.0` | Run: `Some(value)` |

### Ordering and comparators

| ID | Scenario | Expected |
| --- | --- | --- |
| O1 | `Version(1, 10, 0) > Version(1, 9, 7)` | Run: `True` |
| O2 | `(2, "b") < (2, "c")` | Run: `True` |
| O3 | `[1, 2] < [1, 2, 0]` | Run: `True` |
| O4 | `None < Some(0)` | Run: `True` |
| O5 | `False < True` | Run: `True` |
| O6 | enum `Low`, `High` declared in that order: `Low < High` | Run: `True` |
| O7 | `"Z" < "a"` and `"a" < "é"` | Run: `True` for both |
| O8 | `Order.compare(3, 3)` | Run: `Equal` |
| O9 | `List.sort` on structs ordered by declaration order, with equal elements | Run: sorted, and equal elements keep input order |
| O10 | `List.sort_by(users, Order.reverse(by_age_then_name))` | Run: descending |
| O11 | `List.sort_by` with a comparator that always returns `Less` | Run: a permutation of the input; no defect |
| O12 | `List.sort_by` with a failing comparator | Run: expected failure propagates; the failure is in the inferred row |
| O13 | `{a: 1} < {a: 2}` | Fail `T4301` |
| O14 | `map_a < map_b` | Fail `T4301` |
| O15 | `List.sort(list_of_json_values)` | Fail `T4301`, naming `Map` inside `Json.Value` |
| O16 | `a < b < c` | Fail `S1006` |
| O17 | `Order.min(x, y)` with `x == y` | Run: returns `x` |

### Numbers

| ID | Scenario | Expected |
| --- | --- | --- |
| N1 | `-7 / 2`, `-7 % 2`, `7 % -2` | Run: `-3`, `-1`, `1` |
| N2 | `9223372036854775807 + 1` | Warn `T4312`; Run: defect naming `+` |
| N3 | `x / 0` with `x` from input | Run: defect "integer division by zero" |
| N4 | `x % 0` | Run: defect |
| N5 | `min / -1` with `min = -9223372036854775808` | Run: defect |
| N6 | `min % -1` | Run: `0` |
| N7 | `-9223372036854775808` literal | Pass |
| N8 | `9223372036854775808` and `0xFFFF_FFFF_FFFF_FFFF` | Fail `S1002` |
| N9 | `1_000`, `0x1F`, `0b101` | Run: `1000`, `31`, `5` |
| N10 | `1.`, `.5`, `1__0`, `_1` | Fail with syntax diagnostics |
| N11 | `0..10` lexes as a range | Pass |
| N12 | `1e309` | Fail `S1003` |
| N13 | `0.1 + 0.2 == 0.3` | Run: `False` |
| N14 | `1.0 / 0.0` from input values | Run: defect |
| N15 | `x * x` overflowing to infinity | Run: defect |
| N16 | `0.0 - 0.0 == -0.0`, and `Float.to_text(-0.0)` | Run: `True`, `"-0.0"` |
| N17 | `1 + 2.0` | Fail `T4302`; help mentions `Float.from_int` |
| N18 | `"a" + "b"` | Fail `T4303` |
| N19 | `Int.parse("42")`, `Int.parse("")`, `Int.parse("4x")`, `Int.parse("99999999999999999999")` | Run: `42`, then failures `Empty`, `InvalidDigit`, `OutOfRange` |
| N20 | `Float.parse("nan")`, `Float.parse("1e400")` | Run: failures `Invalid`, `OutOfRange` |
| N21 | `Float.to_int(2.5)`, `Float.to_int(1e20)`, `Float.to_int(Float.round(2.5))` | Run: `NotInteger`, `OutOfRange`, `3` |
| N22 | `Float.parse(Float.to_text(x)) == x` for sampled finite `x` | Run: `True` |
| N23 | `case x { 1.5 -> ... }` with `x: Float` | Fail `T4313` |
| N24 | `Decimal.of(150, 2) == Decimal.of(15, 1)` and `Decimal.to_text(Decimal.of(150, 2))` | Run: `True`, `"1.50"` |
| N25 | `Decimal.of(1999, 2) * Decimal.of(3, 0)` | Run: renders `"59.97"` |
| N26 | `price / Decimal.of(4, 0)` | Fail `T4306` |
| N27 | `Decimal.divide(one, zero, 2, Rounding.HalfEven)` | Run: defect |

### Tuples, `Void`, `Never`, and `Range`

| ID | Scenario | Expected |
| --- | --- | --- |
| T1 | `let (low, high) = (1, 9)` | Pass; `low: Int`, `high: Int` |
| T2 | `case pair { (0, name) -> ... (_, name) -> ... }` | Pass; exhaustive |
| T3 | `case pair { (0, name) -> ... }` | Fail with the existing non-exhaustive diagnostic |
| T4 | `let (a, b, c) = (1, 2)` | Fail `T4230` |
| T5 | `let (0, x) = pair` | Fail `T4231` |
| T6 | `(1,)` and `()` | Fail `S1005`; the `()` help suggests `Void` |
| T7 | `(1)` | Pass; type `Int` |
| T8 | a tuple of an `Int` and a shareable record sent through a channel | Pass |
| T9 | `Map.from([("ok", 3), ("failed", 1)])` | Run: map with two entries |
| V1 | `write_line: fn(_line) { Void }` provides an interface method `fn(Text) -> Void` | Pass |
| V2 | `loop _ = Void { ... Void }` | Pass |
| V3 | `Void == Void` | Run: `True` |
| V4 | `{}` as a body | Fail with the existing empty-block diagnostic |
| R1 | `fn reject(reason: Text) -> Never { Failure(Invalid(reason)) }` used in one `case` arm whose other arm is `Response` | Pass; the case type is `Response` |
| R2 | `fn bad() -> Never { 1 }` | Fail `T4308` |
| R3 | a function whose body is a `loop` with no `break` | Pass; inferred success type is `Never` |
| R4 | `Never` used as a type alongside `Supervisor.Restart.Never` | Pass; no name conflict |
| G1 | `0..3 == 0..=2` | Run: `True` |
| G2 | `Range.length(5..2)` | Run: `0` |
| G3 | `0..=max` where `max` is the largest `Int` | Run: defect |
| G4 | `0..1.5` | Fail `T4311` |
| G5 | `0..n + 1` | Pass; parses as `0..(n + 1)` |
| G6 | `a..b..c` | Fail `S1006` |

## Implementation notes

Non-normative. The stages follow the pipeline in the
[executable kernel](executable-kernel.md#non-normative-implementation-model).

- **`compiler/chimi/src/syntax.rs`**
	- Lexer: add `==`, `!=`, `%`, `..`, `..=`, the keywords `and`, `or`, `not`,
	  float literals, `0x` and `0b` literals, and recovery tokens for `&&`,
	  `||`, and a bare `!` (`S1004`). `=` alone stays `TokenKind::Equal`, which
	  `let` uses. A `.` after digits starts a fraction only when a digit
	  follows.
	- Parser: replace the chain `parse_using` → `parse_pipeline` →
	  `parse_comparison` → `parse_additive` with `parse_using` → `parse_or` →
	  `parse_and` → `parse_not` → `parse_pipeline` → `parse_comparison` →
	  `parse_range` → `parse_additive`. `parse_comparison` today accepts a
	  chain of operators in one loop. It and `parse_range` must accept one
	  operator and report `S1006` on a second.
	- `parse_primary`: accept `Void` as a literal instead of calling
	  `error_and_bump`. Distinguish `GroupedExpression` from a new
	  `TupleExpression` by the first top-level comma, and report `S1005` for
	  `()` and `(e,)`. Add tuple types. Tuple patterns and `let` patterns come
	  from the patterns spec.
- **`compiler/chimi/src/semantic.rs`**: extend `BinaryOperator` with `Equal`,
  `NotEqual`, `Remainder`, `And`, `Or`, `Range`, and `RangeInclusive`, and
  `UnaryOperator` with `Not`. Add `Literal::Float` and `Literal::Void`. Fold
  unary minus on an integer literal before the `S1002` range check.
- **`compiler/chimi/src/resolution.rs`**: add the built-in types `Float`,
  `Never`, and `Range` next to `Bool`, `Int`, and `Text`. The `Float` and
  `Range` functions, `std/order`, `std/decimal`, and `std/set` are embedded
  standard modules that a program imports, as
  [standard modules](standard-modules-and-constants.md#the-prelude) requires.
  They are not built-in module qualifiers or `standard.rs` entries.
- **`compiler/chimi/src/typing.rs`**
	- Replace the `Int`-only typing of `Less` to `GreaterEqual` with an
	  operator table keyed by operand type.
	- Compute equatable and ordered with the traversal that
	  `capability.rs` uses for `PropertyDerivation`. Record the path to the
	  first offending component for `T4300`, `T4301`, and `T4305`.
	- Attach property requirements to generic parameters and inference
	  variables, check them at each instantiation, and show them in inspection.
	- Make `Never` the bottom type in unification and in `case` joins, and type
	  `Failure(...)`, `break`, and loops with no `break` as `Never`.
	- Replace the hard-coded `Text` key check for `Map` with the equatable
	  check.
- **`compiler/chimi/src/capability.rs`**: treat the right operand of `and` and
  `or` as a conditional branch. Tuples derive shareable and transferable like
  records.
- **`compiler/chimi/src/executable.rs`**: lower `and`, `or`, and `not` to
  `CoreOperation::Match` on `Bool`, keeping the source node identity for
  tracing. Lower `!=` to `not` applied to `==`, and `..=` to `..` with a
  checked `+ 1`. Lower tuples to a built-in tuple constructor or to a
  positional `Record`. `Never` needs no runtime representation. Add
  `CoreLiteral::Float` and `CoreLiteral::Void`.
- **`compiler/chimi/src/execution.rs`**: implement structural equality,
  comparison, and hashing over interpreter values without recursion on the
  host stack. Add `Float` arithmetic with the finite-result check. `%` must
  return 0 for `-2^63 % -1`, so it cannot use `checked_rem` directly. The
  existing `checked_integer` defect path covers the other `Int` cases.
- **`compiler/chimi/runtime/native/runtime.c`**: add `VALUE_FLOAT`, a tuple
  representation, and `chimi_value_equal`, `chimi_value_compare`, and
  `chimi_value_hash` with an explicit work stack. `MapNode` is keyed by
  `const char *key` today. Re-key it on `ChimiValue *` with the internal total
  order or with hashing. Compare `VALUE_JSON_NUMBER` by canonical decimal
  form: sign, digits without leading or trailing zeros, and exponent. Do not
  convert through `double`.
- **`compiler/chimi/src/standard.rs`**: `StandardConstructor` lists
  `OptionSome` before `OptionNone`. Derived order must follow the `Option`
  declaration order in this contract (`None` first), not that enum's
  discriminants.
- **`compiler/chimi/src/formatter.rs`**: spacing for the new operators,
  tuples, and `let` patterns.
- **Tests**: turn each conformance row into a fixture under
  `prototypes/golden/valid` or `prototypes/golden/invalid`, or into a compiler
  test that runs through the same type, Core, and runtime boundaries as
  ordinary programs.

## Rejected alternatives

- **Equality and ordering traits with derive annotations.** The recorded
  decision is plain generics with no traits. Derived properties give the same
  safety for structural data with no declarations.
- **Equality on every type, with identity for functions.** It would let
  programs observe closure and capability identity and would make `Map` keys
  depend on allocation.
- **IEEE `==` for `Float` (NaN is not equal to itself).** Structural equality
  would stop being an equivalence relation, so a struct with a NaN field would
  not equal itself and could not be found in a `Set`.
- **Total-order `Float` equality with NaN as a value (Java `Double.compare`).**
  It keeps NaN, which JSON cannot encode and which services almost never mean
  to produce. It also makes `NaN == NaN` true, which surprises IEEE users as
  much as it helps.
- **`Int` overflow as an expected failure.** Every arithmetic expression would
  gain a failure variant that callers handle by re-raising it.
- **Wrapping `Int` arithmetic.** It hides bugs in money and counting code.
- **Floor division and floor modulo for `/` and `%`.** Python uses them, but C,
  Java, JavaScript, Go, Rust, Swift, and Kotlin truncate. `Int.modulo` covers
  the floor case by name.
- **Locale-aware or normalized `Text` equality.** It depends on host data and
  Unicode versions, which breaks reproducible hashing and `Json.encode` order.
- **Ordering records by field name.** It invents an order the program never
  wrote.
- **Element access `pair.0`.** It collides with float lexing in `pair.0.1`, and
  patterns already read elements.
- **`()` as the `Void` value.** The repetition spec already writes `break Void`,
  and `()` next to tuples suggests a zero-element tuple that Chimi does not
  have.

## Open questions

These are vision-level choices. The contract above already uses the
recommended answer, so it stays usable as written.

1. **Should `Float` exclude NaN and infinity?** Recommended: yes, as specified.
   Services exchange JSON, which has neither value, and a NaN in derived
   equality breaks `Map`, `Set`, and `==` on structs. The cost is that
   scientific code that relies on IEEE special values needs a separate type,
   for example a future `std/ieee` type with explicit total-order comparators.
2. **Do explicit function types need a spelling for the equatable and ordered
   requirements?** Recommended: not yet. Requirements are inferred and shown
   by tooling. When generic interface methods (G11) need to state one, use the
   Go-style form `fn<T: equatable>`, restricted to the two compiler-derived
   properties so that it does not become a trait system.
3. **Keywords or symbols for boolean logic?** Recommended: `and`, `or`, `not`,
   as specified. Python uses them, the use-case sketches already do, and
   `not a == b` reads as intended. `&&`, `||`, and `!` are equally familiar,
   from C-family languages, so `S1004` gives a precise fix for anyone who types
   them.
4. **Should `Decimal` get operators and a literal?** Recommended: operators
   yes, as specified, because money code with `Decimal.add` chains is hard to
   read. No literal for now: `Decimal.of(1999, 2)` is total, and a suffix such
   as C#'s `19.99m` can be added once real programs show constants are common.
