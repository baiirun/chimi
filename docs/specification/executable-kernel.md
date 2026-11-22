# Chimi Executable Kernel Specification

## Status

This document specifies Chimi's first executable language kernel. It is the
normative consolidation of the accepted golden corpus and prototype verdicts.
Chimi remains free to break; this is an implementation target, not a backwards-
compatibility promise.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY describe normative
requirements. Examples use the first kernel's source dialect.

## Purpose

The kernel defines the smallest coherent language that lets programmers compose
ordinary values, functions, expected failures, environmental dependencies, and
observable execution while retaining explicit local reasoning and a lossless,
tooling-oriented compiler frontend.

It is designed to support a first real compiler. It does not define the
production runtime architecture or the complete future Chimi language.

## Scope

The kernel specifies:

- files, modules, imports, visibility, and name identity;
- nominal structs, closed enums, anonymous records, and core value types;
- functions, generics, expressions, blocks, pipelines, and pattern matching;
- success, expected-failure, and dependency-needs channels;
- structural dependency interfaces and provision with `using`;
- root execution, evaluation order, outcomes, and structured events;
- lossless parsing, semantic lowering, diagnostics, formatting, inspection, and
  frontend performance obligations.

It does not specify aliases, newtypes, nullability, exceptions, standalone
conditionals, macros, mutation, resource ownership, structured concurrency,
supervision, durability, a permanent backend, an artifact format, package
management, deployment, or interoperability.

## Source Files and Lexical Model

- Chimi source files use the `.chimi` extension in the first kernel.
- Source is UTF-8. Identifiers are ASCII letters or `_`, followed by ASCII
  letters, digits, or `_`.
- `//` begins a line comment. Comments and all whitespace are source trivia and
  MUST remain losslessly represented in the CST.
- Decimal integer literals MAY contain `_` separators. Text literals use double
  quotes and backslash escaping and cannot span lines.
- A text literal accepts exactly these escapes: `\"` (quote), `\\` (backslash),
  `\n` (line feed), `\r` (carriage return), `\t` (tab), `\$` (dollar sign, so
  `\${` is literal text rather than an interpolation), and `\u{X}`, where `X`
  is one to six hex digits naming a Unicode scalar value (`\u{e9}` is `é`). Any
  other escape, a `\u` escape without its braces or digits, and a `\u` escape
  naming a surrogate or a value above `10FFFF` are rejected with `S1001` at the
  escape. A backslash cannot escape a line break. `Debug.render` writes text
  with these escapes, so its output for a text is a literal for the same text.
- Line breaks are syntactically significant between declarations, block items,
  data members, and case arms. They are soft inside delimited lists and around
  continued function contracts.
- Semicolons are not part of the kernel.

The parser MUST consume the entire file even when the source is malformed.

## Modules and Visibility

Each source file defines one module. Its workspace-relative path is its stable
module identity. Source files MUST NOT contain module headers.

A module's items are imports, `struct`, `enum`, `interface`, and `fn`
declarations, and `const` and `pub const` declarations. A module-level `let`
is rejected with `L2101`, whose edit replaces `let` with `const`; see
[standard modules and constants](standard-modules-and-constants.md). An expression at file scope is not a module
item and MUST be rejected with `S1001`; the parser still consumes it so the CST
stays lossless.

A module import chooses its source binding explicitly:

<!-- chimi: fragment -->
```chimi
import billing/money as BillingMoney
```

The compiler MUST NOT derive `BillingMoney` from the path. External functions
remain qualified through this binding:

<!-- chimi: fragment -->
```chimi
BillingMoney.from_cents(500)
```

An explicit member import MAY bind public types and enum constructors:

<!-- chimi: fragment -->
```chimi
import billing/money.{Money, NegativeAmount}
```

Member imports MUST NOT bind functions. Bare module imports and wildcard member
imports are invalid.

Imports MUST NOT form a cycle. The project loader rejects a cycle while reading
modules from disk, and the compiler rejects one among sources it is given with
`E_IMPORT_CYCLE` at the import that closes it, listing the path, as in
`import cycle: app/main -> lib/other -> app/main`.

Module, type, and value names occupy separate namespaces. Every resolved
external reference carries a stable module-qualified symbol identity even when
an explicit member import permits an unqualified source spelling.

Declarations are private unless prefixed with `pub`. Publishing a struct
exposes its nominal type but does not expose its fields or its struct literal.
External construction and field access therefore use public module functions.
A struct literal of another module's struct is
`E_PRIVATE_STRUCT_CONSTRUCTOR`, and reading a field there is
`E_PRIVATE_STRUCT_FIELD`.
Publishing an enum exposes its variants as constructors.

## Data Types

### Structs

`struct` declares a nominal product:

<!-- chimi: check -->
```chimi
struct Money {
  cents: Int
}
```

Fields have declared types. A struct is built by field name, in braces:

<!-- chimi: fragment -->
```chimi
Money { cents: 2500 }
```

A struct literal MUST name every field exactly once, in any order, each with a
value compatible with the field's type. A missing field is `T4215`, a field
the struct does not declare is `T4205`, and a field named twice is `T4211`.
The values evaluate in source order, left to right, whatever the declaration
order. Field names are the identity for construction as for access
(`value.cents`), so reordering a struct's fields changes no construction site.
`..base` inside a struct literal is `S1001`; a copy with some fields changed is
a [record update](patterns-and-function-clauses.md#record-update),
`{ ..base, cents: 0 }`. A struct without fields is built with empty braces,
`Marker {}`.

Positional construction such as `Money(2500)` is `T4216`, whose fix writes the
struct literal (in brackets when the call is a `case` subject), and so is a
labelled call such as `Money(cents: 2500)`. A struct's constructor is not a function value: `let make = Money` and
`value |> Money` are `T4216` too; write `fn(cents) { Money { cents: cents } }`.
Braces with field names after anything that is not a struct of the module,
such as an enum variant, a function, a field, or a local or parameter that
shares a struct's name, are `T4217`. Another module's struct is
`E_PRIVATE_STRUCT_CONSTRUCTOR`, since its fields are private to its module. Function calls and enum variant
constructors stay positional.

A struct literal after a bare name or a qualified name is read as one wherever
an expression is, except in a `case` subject, where `{` opens the arms and
an arm may start with a typed binding `name: Type`. There it needs
parentheses: `case (Money { cents: 0 }) == total { ... }`.

Struct representation is all-or-nothing; the kernel has no per-field
visibility modifiers.

### Enums

`enum` declares a nominal closed sum:

<!-- chimi: check -->
```chimi
enum PaymentError {
  Declined(reason: Text)
  GatewayUnavailable
}
```

Each variant is a constructor. Construction is positional in the variant's
declared field order: `PaymentError.Declined("card")`. Payload
field order is part of the enum's public shape. Variant patterns MUST use the
declared payload arity and match payload fields by the same position.

A variant without payload is written bare, with no parentheses, in both
expressions and patterns: `PaymentError.GatewayUnavailable`, `None`,
`Supervisor.Restart.OnDefect`. The bare name is the value itself.
`PaymentError.GatewayUnavailable()` is accepted as the same value. The
formatter keeps whichever form the source uses, because it works on syntax
alone and cannot tell a fieldless variant from a zero-argument call; a linter
MAY suggest the bare form. A variant constructor with fields is a function
of its fields, so `let make = Some` and `value |> Some` are valid and
`make(value)` constructs as `Some(value)` does. A struct's constructor is not;
see [Structs](#structs).

### Anonymous records

An anonymous record is a transparent structural product:

<!-- chimi: fragment -->
```chimi
{
  charge: fn(total) { Receipt("test") },
}
```

Records MUST contain at least one field. Records provide open local data and
structural interface implementations; they do not create nominal identity.

A record type is written `{ name: Type, ... }` wherever a type is written,
including parameters, returns, fields, type arguments, and function types:
`fn sum(point: { x: Int, y: Int }) -> Int`. It denotes the records with exactly
those field names and field types, so field order does not matter. A record type
MUST have at least one field, and a field name MUST occur once (`T4211`). The
formatter writes a record type on one line.

### Absence and completion

Chimi has no `Nil`, `null`, or nullable value. Optional data uses `Option<T>`.

`Void` is the success type of an operation that completes without data. It is
also that type's only value, written `Void`, as
[core data](core-data-and-operators.md#void) specifies. An empty block is
invalid and MUST NOT stand in for completion.

Aliases and newtype-like declarations are not part of the first kernel.

## Generics

Generic function and data declarations bind their parameters explicitly after
the declaration name:

<!-- chimi: check -->
```chimi
struct Box<T> {
  value: T
}

fn transform<A, B>(value: A, operation: fn(A) -> B) -> B {
  operation(value)
}
```

An unqualified type-like name that is neither a declared generic parameter nor
a resolvable nominal or built-in type is a diagnostic. The compiler MUST NOT
silently create generics from unresolved uppercase identifiers.

The compiler infers each generic parameter's kind from use. Ordinary type
positions constrain value types; `errors` positions constrain failure rows;
`needs` positions constrain dependency rows. Inconsistent uses are diagnostics.
A generic used in both an ordinary type position and an `errors` position is a
failure set: the ordinary position denotes its error-value union and the
`errors` position denotes the same set as a failure row. This is one generic
identity with two compile-time projections, not two independently inferred
parameters. The value projection is valid wherever an ordinary type is, type
arguments of other generic types included, so `fn capture<T, E, R>(operation:
fn() -> T errors E needs R) -> Result<T, E>` binds `Failure(error)` to `E` and
returns `Err(error)` as a `Result<T, E>`. A generic that the body raises with
`Failure(error)` is a failure set too, as `E` of `Result.get` is. A failure set
is instantiated only with a failure-set value: an enum, a variant, or a set of
them. The full rules are in
[time and outcomes](time-and-outcomes.md#projection-into-type-arguments).

A call instantiates each generic parameter of its callee. When no argument,
result context, or later use ever constrains an instantiation, as in
`is_none(nothing())` with `fn nothing<T>() -> Option<T> { None }` or an unused
`Map.empty()`, the instantiation is erased rather than rejected. This is the
ambiguous-type rule. It is sound because values are uniform at run time and
every type-directed operation (arithmetic, field access, literal and
constructor patterns) constrains the type or reports a type error, so no
evaluated operation depends on the erased type. Its only inhabitants
are values that never mention it, such as `None` or an empty collection.
Execution setup MUST NOT report a missing type fact (`E5006`) for an erased
instantiation.

An inferred contract is generic in each type its body leaves open, as if the
declaration named a generic parameter there. With `fn wrapper() { nothing() }`,
`wrapper` returns `Option<T>` for any `T`, so `count(wrapper())` and
`flag(wrapper())` may use it at `Option<Int>` and `Option<Bool>`; with
`fn pass(value) { value }`, each call returns its argument's type. Each
reference instantiates the open types afresh, and an instantiation nothing
constrains is erased by the ambiguous-type rule.

The first kernel supports declaration-scoped parametric polymorphism. It does
not require higher-rank types, explicit kind annotations, specialization
syntax, or user-defined variance.

## Functions and Callable Contracts

Functions are values. Named and anonymous functions share the same callable
model and functions MAY return functions.

Every callable contract contains three independent channels:

```text
data inputs -> success type errors expected-failure-row needs dependency-row
```

For example:

<!-- chimi: fragment -->
```chimi
fn submit(cart: Cart) -> Order needs {
    payment: Payment,
    inventory: Inventory,
} {
  // implementation
}
```

Tooling displays the inferred complete shape as:

```text
fn(Cart) -> Order errors CheckoutError
needs { payment: Payment, inventory: Inventory }
```

The implementation MAY omit its success annotation and infer it from the body.
Inference applies uniformly to public and private code. Tooling MUST expose the
complete inferred callable contract.

Implemented function headers infer their expected-failure rows in the first
kernel. Function-type annotations use `fn(...) -> ... errors ... needs ...` to
state the contract expected from a function value. Higher-order contracts MUST
preserve relationships expressed through their declared generic type, failure-
row, and needs-row parameters.

A function value passed as a call argument MAY expose fewer failures and fewer
needs than the parameter's function type declares, as a `using` provider's
method may: `run(fn() { 3 })` is accepted for `run(f: fn() -> Int errors
Oops)`. Its inputs and success type still match exactly, and a failure or need
outside the declared rows is rejected with `T4203`. A failure-row or needs-row
parameter that only function arguments' rows bind is their union, so
`both(zero, failing)` with `both<E>(f: fn() -> Int errors E, g: fn() -> Int
errors E)` binds `E` to `failing`'s row. A row parameter that another position
fixes exactly, such as the row of a function a callback receives, stays that
row, and every function argument's row must fit within it.

## Expressions

Every function body and nested block is an expression. A block contains one or
more ordered items, and its final expression produces the block's value:

<!-- chimi: fragment -->
```chimi
let value = {
  let intermediate = compute()
  transform(intermediate)
}
```

Blocks MUST NOT be empty. A block-local `return` is not part of the kernel.
A block's last item MUST be an expression: a block that ends with a `let`,
plain or destructuring, is rejected with `L2023`, because a `let` names a value
and does not produce one. The fix is to end the block with the value, or to use
the bound expression itself as the block's last item.

Calls and block items evaluate left to right. Member access and calls bind more
tightly than unary, multiplicative, additive, and comparison operations.
Pipelines apply after those ordinary expressions; postfix `using` applies to
the resulting operation.

Calling a function composes its invocation into the surrounding computation.
Execution begins only when the external runner invokes the root. A compiler MAY
evaluate, inline, or fuse deterministic requirement-free calls without changing
their observable semantics.

### Pipelines

<!-- chimi: fragment -->
```chimi
value |> transform
value |> combine(other, more)
```

are exactly:

<!-- chimi: fragment -->
```chimi
transform(value)
combine(value, other, more)
```

The rule is syntactic. When the right side of `|>` is written as a call,
`callee(arguments...)`, the left operand becomes that call's first argument.
Any other right side, such as a name, a member path, or a parenthesized
expression, is a unary function applied to the left operand. Chimi MUST NOT
fall back from one reading to the other: `value |> make(config)` is always
`make(value, config)`, even when `make(config)` would return a function. To
apply a function that a call returns, parenthesize the call:

<!-- chimi: fragment -->
```chimi
value |> (make(config))
```

Chimi has no partial application, and `|>` does not create one. APIs that take
a subject put it first, so a value flows through a chain of ordinary calls:

<!-- chimi: fragment -->
```chimi
orders
|> List.filter(is_open)
|> List.map(summarize)
|> Text.join(", ")
```

Lowering rewrites each pipeline, in both forms, to the call it denotes, so the
call's evaluation order, arity checks, and diagnostics apply unchanged: the
callee expression evaluates first, then the left operand, then the remaining
arguments from left to right. `x |> (make(c))` therefore evaluates `make(c)`
before `x`.

The right side of `|>` is parsed at the tighter precedence of comparison and
arithmetic, so `x |> f(a) + 1` means `x |> (f(a) + 1)`, a unary application
that is usually a type error. Parenthesize the pipeline instead:
`(x |> f(a)) + 1`.

### Case expressions

`case` is the kernel's only branching expression:

<!-- chimi: fragment -->
```chimi
case state, request {
  Suspended(reason), _ -> Denied(reason)
  Active, Transfer(amount) if amount.cents <= 0 -> Denied("invalid")
  Active, _ -> Allowed
}
```

Ordinary matching over closed values MUST be exhaustive. `_` and an unguarded
binding pattern are catch-alls. An unguarded catch-all MUST be last. A guarded
catch-all does not prove exhaustiveness. Unreachable arms are diagnostics.

`if` is valid only on a case-arm guard. A guard MUST be pure: its failure row
and its needs row MUST both be empty. Chimi has no standalone `if`
expression in the first kernel.

## Expected Failures

Expected errors are ordinary typed values carried in a callable's separate
failure row. Chimi has no exception throwing for expected failures.

An ordinary expression implicitly constructs a successful computation outcome:

<!-- chimi: fragment -->
```chimi
Receipt("receipt-1")
```

`Success(value)` is the optional explicit equivalent. Programmers are never
required to wrap ordinary successful values.

`Failure(error)` constructs a failed computation outcome:

<!-- chimi: fragment -->
```chimi
Failure(PaymentError.Declined("expired"))
```

It places the error value in the expected-failure channel. It does not perform
untyped stack unwinding.

An unhandled failure stops the remaining local sequence and joins the caller's
inferred failure row without propagation syntax. A fallible expression can be
matched directly:

<!-- chimi: fragment -->
```chimi
case payment.charge(total) {
  Success(receipt) -> receipt
  Failure(error) -> Failure(CheckoutError.PaymentFailed(error))
}
```

These patterns inspect the same outcome constructors. A failure case MAY handle
selected variants; unmatched failure variants continue bubbling. This differs
from ordinary enum matching, which remains exhaustive.

A multi-subject `case` MAY match any of its subjects this way. Coverage then
applies per combination of outcomes, as specified in
[patterns and function clauses](patterns-and-function-clauses.md#outcome-patterns-in-a-multi-subject-case).

Failure rows retain variant identities. Tooling MAY display a whole error enum
when every variant remains and individual variants after selective handling.

Selective recovery is ordinary higher-order function composition. It requires
no recovery keyword, wrapper, or propagation operator:

<!-- chimi: fragment -->
```chimi
fn recover_declined<E>(operation: fn() -> Receipt errors E) {
  fn() {
    case operation() {
      Success(receipt) -> receipt
      Failure(Declined(reason)) -> Receipt(reason)
    }
  }
}

let configured = operation |> recover_declined
```

Here `E` is a failure set. The returned thunk exposes the residual row
`E - PaymentError.Declined`. Instantiating `E` with `PaymentError` therefore
leaves only `PaymentError.Offline`. An unguarded failure pattern MUST subtract
every variant it fully covers. A guarded pattern MUST NOT subtract that variant,
because the guard can reject a matching value. A pattern that cannot occur in
a known closed failure row is a diagnostic; a pattern against an open row is
permitted and subtraction is a no-op when an instantiation lacks that variant.

Binding `Failure(error)` projects the failure set into an ordinary value.
Subsequent ordinary `case error` matching MUST be exhaustive; the live outcome
case alone permits omitted failure variants to bubble. Residual rows MUST
remain visible in inferred contracts and structured inspection.

Failure-set projections and row subtraction are compile-time only. Lowering
MUST use ordinary outcome matching, and MUST NOT introduce a `Recover` Core
operation, runtime row object, allocation, or tag. Unmatched expected failures
propagate unchanged, while defects and cancellation bypass recovery arms.

A storable `Result<T, E>` is ordinary data in `std/result`, specified in
[time and outcomes](time-and-outcomes.md#storable-outcomes-and-failure-set-types).
Its `Ok` and `Err` constructors remain distinct from the live outcome
constructors `Success` and `Failure`.

Defects and cancellation are not expected failures and MUST NOT appear in
callable failure rows.

## Dependency Needs and Interfaces

`needs` declares environmental dependencies separately from data inputs,
success, and failure:

<!-- chimi: fragment -->
```chimi
fn checkout(cart: Cart) needs {
  payment: Payment,
  inventory: Inventory,
} {
  // payment and inventory are lexical values here
}
```

Each need has a lexical name and an interface type. Composition unions
compatible needs. Reusing one name with incompatible interface types is a
diagnostic.

An `interface` is a structural record of function contracts:

<!-- chimi: fragment -->
```chimi
interface Payment {
  charge: fn(Money) -> Receipt errors PaymentError
}
```

Conformance is implicit. Implementing an interface does not attach methods to a
nominal type and does not alter name lookup. A provider method MAY expose fewer
failures or needs than its interface permits, but MUST NOT expose more or change
the required data inputs or success type.

A member MAY be generic. Its generic parameters follow `fn` and are scoped to
that member; the interface's own parameters stay fixed by the need's type:

<!-- chimi: check -->
```chimi
interface Store<K> {
  get: fn<A>(K, A) -> A
}
```

Each call of `store.get` instantiates `A` afresh, as a call of a generic
function does. A generic function type MUST appear only as the whole type of
an interface member (`T4360` elsewhere); the kernel still has no higher-rank
function types. A member generic MUST NOT reuse a name of the interface's own
generics.

A provider for a generic member MUST be generic over each of the member's
parameters, so its provider is a generic function declared with explicit
parameters, such as `fn keep<K, A>(key: K, fallback: A) -> A`. A function fixed
to one type, a lambda, a provider that compares or orders a value of a member
parameter, or a record update that puts such a function in a generic member is
a conformance error (`T4213` for a provider, `T4203` for an update).

## Provision with `using`

Postfix `using` supplies named providers at any expression boundary:

<!-- chimi: fragment -->
```chimi
let checkout = submit(cart) using {
  payment: fake_payment,
}
```

Successfully supplied needs are removed from the resulting callable contract;
unsupplied needs remain. The provider record MUST be nonempty.

A provider shape mismatch leaves the need unresolved for analysis but marks the
mismatch as explained. A later closed-root check SHOULD NOT emit a redundant
missing-dependency diagnostic for that same need.

## Program Entry and Execution

A runnable program exports a conventional `main` root:

<!-- chimi: fragment -->
```chimi
pub fn main() {
  Checkout.submit(cart) using {
    payment: Stripe.gateway(client),
    inventory: Warehouse.inventory(database),
  }
}
```

After provision, every need that remains on `main` MUST be one that the
runner's typed provider plan closes, such as `clock: Clock` or a `Console`
need. Any other need MUST be provided with `using`. The runner MUST NOT supply
undeclared dependencies; see
[host capability ownership](synchronous-effects-and-host-capabilities.md#host-capability-ownership). Chimi source
does not call `Runtime.run`.

The toolchain or embedding host invokes `main`. It supplies operational
execution configuration—such as tracing export, execution identity,
cancellation, and resource policy—through an external runner boundary. Such
configuration is not an ordinary program dependency map.

The runtime representation distinguishes four terminal outcomes:

- success with a value;
- expected failure with an error value;
- defect with invariant-failure context;
- cancellation with cancellation context.

Retry is ordinary higher-order composition over one operation. A retry policy
MAY retry selected expected-failure variants. It MUST NOT retry defects or
cancellation. Exhaustion preserves the final expected failure value.

## Observable Execution

The external runner selects one of three tracing modes:

- `off` emits and exports no runtime events;
- `boundaries` emits computation lifecycle and dependency-call events;
- `full` additionally emits paired events for every executed semantic
  operation.

Tracing mode does not change program evaluation or its terminal outcome. In
`off` mode, the runtime MUST bypass event construction, buffering,
serialization, and exporter calls rather than constructing events and dropping
them afterward. Error values, defects, and cancellation still reach the caller
through the ordinary terminal outcome.

Every canonical event produced by boundary or full tracing contains or can
losslessly recover:

- deterministic sequence within one execution;
- a stable low-cardinality computation kind;
- a unique trace or execution ID;
- span and parent-span causality;
- semantic node or local operation identity;
- source identity;
- typed dynamic attributes.

Dynamic identifiers MUST NOT be embedded in the computation kind. Structured
computation context propagates across runtime-owned child work, retries, and
dependency calls. Process or task labels MAY aid debugging but MUST NOT define
or replace computation context.

Exporters MAY encode context with repeated fields, interning, or context deltas.
They MUST NOT invent or heuristically reconstruct computation identity.

## Compiler and Tooling Contract

### Parsing and CST

The lexer preserves the exact source slice for every token, including trivia
and invalid text. The parser emits structural events independently from the
tree representation. A tree sink constructs an immutable, semantic-free green
CST.

Parsing always returns a CST and syntax diagnostics. Missing expected syntax is
represented by a zero-width missing token. Unexpected consumed syntax remains
inside an explicit error node. Recovery MUST make progress and SHOULD resume at
the smallest useful enclosing boundary.

Concatenating non-missing CST token text MUST reproduce the source exactly.

### Semantic lowering and resolution

Lowering produces a compact source-mapped semantic representation. It removes
formatting trivia, separates block steps from their final value, normalizes
pipelines to calls while retaining their origin, and preserves `using` as an
explicit provision boundary.

Malformed forms remain explicit invalid semantic nodes; lowering MUST NOT
invent valid meaning for damaged source.

Resolution assigns stable identities in the module, type, and value namespaces.
It distinguishes declared generic parameters from nominal types and resolves
pattern constructors before type checking.

The concrete resolved-to-typed and typed-to-executable representations are
implementation contracts specified separately from this source-language
document.

### Diagnostics

Every diagnostic uses a shared envelope containing:

- originating compiler stage;
- stable diagnostic code;
- severity;
- primary source span;
- optional secondary labels;
- optional source edits.

The common envelope MUST NOT blur syntax, lowering, name, type, or execution-
setup errors. Later stages SHOULD suppress diagnostics caused solely by an
already-reported earlier error.

### Formatting and inspection

Chimi has one canonical, non-configurable formatter. It consumes the lossless
CST. When syntax damage prevents a complete safe rewrite, it MUST return the
source unchanged with syntax diagnostics.

Cursor inspection composes the containing CST path with source-mapped semantic
facts. Tools MUST NOT derive semantic identity from formatted text.

## Performance Contract

The following correctness invariants do not depend on benchmark hardware:

- every source byte remains represented;
- malformed parsing consumes all input and terminates;
- diagnostics and missing markers have valid source spans;
- malformed regions do not cause unbounded diagnostic cascades;
- future clean and incremental executions produce structurally equivalent
  results.

Performance reports record implementation revision, build mode, operating
system, architecture, CPU, core count, and memory. The implementation measures
cold startup, valid and malformed parsing, editor edits, retained CST storage,
local semantic invalidation, and high-fanout invalidation.

The provisional experience targets are 16 ms p95 for typical full-file syntax
and editor reparses and 100 ms p95 for local semantic diagnostics on the
recorded reference machine. These are design constraints, not portable claims.

The implementation MUST NOT claim incremental semantic performance until an
incremental semantic engine exists.

## Non-Normative Implementation Model

A conforming first compiler is expected to use this phase structure:

```text
source
  -> lossless CST + syntax diagnostics
  -> source-mapped semantic representation
  -> resolved semantic representation
  -> typed representation with success/failure/needs contracts
  -> executable core
  -> backend/runtime execution
```

The CST is authoritative for source-preserving tools. The semantic and later
representations are derived and retain stable source identity. Compiler stages
own their diagnostics while sharing the diagnostic envelope.

This phase structure is normative at its semantic boundaries, not in its host
language, allocation strategy, object layout, or choice of permanent backend.

## Verification

Run the combined executable validation:

```sh
python3 prototypes/kernel-validation/main.py --check
```

Every component probe MUST pass. Resolved semantic expressions now feed type
inference, and typed expressions reach executable lowering only through a
verified root, so the validator MUST report:

```text
specification-ready=true
end-to-end-compiler=true
```

The files under `prototypes/golden/` are the acceptance corpus for this
specification. A future implementation SHOULD convert each expectation comment
into an automated compiler assertion without making those comments part of the
language.

Compiler tests MUST also cover a generic recovery thunk, guarded
non-subtraction, closed impossible patterns, exhaustive matching after failure
capture, concrete residual specialization, structured inspection, and runtime
bypass for defects and cancellation. These checks MUST execute through the
same type, Core, and runtime boundaries used by ordinary programs.

## Non-Normative Sources

The [decision inventory](executable-kernel-decision-inventory.md) links each
contract area to its prototype and research evidence. Go and Python are
disposable prototype hosts and impose no implementation requirement.
