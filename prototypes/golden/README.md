# Golden Chimi Source Corpus

> PROTOTYPE: these files are design inputs, not supported Chimi programs yet.

## Question

What is the smallest coherent Chimi surface that makes values, functions,
expected errors, dependency requirements, and cold execution compose without
hidden imports or an Effect-style library API?

The corpus is intended to become acceptance input for the disposable compiler
prototype. It deliberately chooses concrete working notation so the notation
can be judged in complete programs instead of prose.

## Working notation

- A source file defines its module. There is no module header.
- Module bindings are always explicit: `import billing/money as Money` binds
  the path to the source name `Money`. Chimi never derives a namespace from a
  path basename. Explicit member imports may bind public types or constructors,
  but never functions. Calls from other modules therefore stay qualified, such
  as `Checkout.submit`.
- `fn` defines a cold computation parameterized by its inputs. Named and
  anonymous functions are values with the same function shape. A definition
  may omit its success type; the compiler infers it from the body.
- Generic functions and data types declare their parameters explicitly after
  the declaration name, such as `fn transform<A, B>(...)`. The compiler infers
  whether each parameter is a value type, failure row, needs row, or effect row from its
  use; an undeclared type-like name is a diagnostic rather than an implicit
  generic.
- Applying a function constructs an invocation. It executes only as part of a
  runtime root; ordinary calls inside a function compose in direct style.
- `value |> transform` applies a unary transformation to `value`. The right
  side is an ordinary function value, so `operation |> Retry.with(policy: ...)`
  is equivalent to `Retry.with(policy: ...)(operation)`: retry configuration is
  partially applied and the operation is not a distinguished argument field.
  Every type-compatible unary function is pipeable. Multi-argument calls do
  not gain automatic partial application or a special pipeline argument slot;
  an API intended for pipelines explicitly returns a unary function.
- Every expression and function has separate inferred success, expected-error,
  requirement, and effect types. This separation is part of the semantic model
  and tooling shape; it is not mandatory surface annotation.
- A normal call binds its successful value. An unhandled expected error stops
  the remaining computation and joins the caller's inferred error row.
- A direct `case` over a fallible expression handles its complete outcome with
  `Success(value)` and `Failure(error)` patterns. Producing `Failure(error)`
  constructs a failed computation outcome and places that ordinary error value
  back into the failure channel. An ordinary value implicitly succeeds;
  `Success(value)` is the equivalent optional explicit outcome constructor.
  These computation outcomes remain distinct from explicitly qualified,
  storable `Result.Success` and `Result.Failure` data.
- Bodyless and higher-order function types use the provisional `errors E`
  notation when their error contract cannot be inferred from an implementation.
  Chimi has no expected-error throwing.
- `struct` declares a named nominal product type. Its fields and implicit
  constructor are visible inside the defining module. Publishing a struct
  exposes its type name but does not expose its representation.
- Struct representation visibility is all-or-nothing rather than per-field.
  Other modules use explicitly published accessor and update functions; Chimi
  does not automatically generate public setters.
- `enum` declares a named nominal sum type. Each member is a variant and may
  carry its own data. Pattern matching over an enum is exhaustive.
- `{ field: value }` constructs an anonymous transparent structural
  record value.
  It is useful for open data and for satisfying small interfaces without
  declaring a nominal struct.
- Anonymous records and expression blocks must be nonempty. Bare `{}` is
  invalid rather than an empty record or a no-op block.
- `Void` is the explicit success type for a computation that completes without
  producing data. It has no source literal or public constructor. `Option<T>`
  represents optional data; Chimi has no nullable value.
- `needs { name: Interface }` is part of a function's type. Requirement
  names are lexical inside the function and may be supplied at intermediate
  `using` boundaries or in the final execution configuration.
- `operation using { name: provider }` supplies any subset of an operation's
  needs at an arbitrary expression boundary. The resulting operation retains
  only unresolved needs; the runtime root rejects a nonempty remainder.
- `effects { io }` describes observable synchronous IO independently from the
  dependency that performs it. `using` removes supplied needs but leaves effects
  unchanged; handling an expected failure removes only that failure channel.
- A runnable program exposes a conventional root function whose ordinary
  dependencies are closed with `using`. The toolchain or embedding host invokes
  that root and supplies operational execution configuration; Chimi source does
  not call `Runtime.run`.
- Interfaces are structural records of functions. Conformance is implicit and
  does not add methods to a type or alter name lookup.
- Every block is an expression. Its final expression is the block's value; a
  block does not need a local `return` to produce that value. A block must
  contain at least one expression.
- `case` is the only branching expression. It accepts one or more subject
  expressions, requires exhaustive patterns, and permits expression blocks as
  branch bodies. There is no standalone `if` expression.
- `if` appears only in a case guard. Guards may use expressions proven pure by
  the compiler; they may not invoke work with failures, dependencies, or effects.
- Calls sequence normally. Suspension is a runtime implementation detail;
  detached or concurrent execution requires a separate explicit operation.
- Public and private contracts use the same inference rules. A written `-> T`
  constrains only the success type; errors are still inferred unless separately
  constrained. `EXPECT-SHAPE` comments show the complete contract that tooling
  must be able to display.
- Pure functions use the same model. The compiler may evaluate, inline, or fuse
  requirement-free deterministic calls without allocating runtime nodes.

The `.chimi` extension and the spellings `struct`, `enum`, `needs`,
`errors`, `Void`, `Success`, `Failure`, and `|>` are candidates, not settled
decisions.

## Corpus

Positive cases:

- `valid/values_and_variants.chimi` — immutable data and exhaustive matching
- `valid/functions_as_values.chimi` — closures, partial application, and
  higher-order functions
- `valid/expressions_and_matching.chimi` — block values, guarded patterns,
  multiple case subjects, and expression-valued branches
- `valid/modules/money.chimi` and `valid/modules/use_money.chimi` — file-based
  modules, qualified calls and types, explicit type imports, module-private
  struct representations, inferred success types, and automatic error
  propagation
- `valid/dependencies_and_execution.chimi` — structural dependency contracts,
  named requirements, cold function invocation, and runtime wiring
- `valid/composition.chimi` — preservation of error and requirement shapes
  through higher-order composition
- `valid/worker_forcing_example.chimi` — one job, retry as a separate
  transformation, dependency union, and tracing at execution

Negative cases:

- `invalid/non_exhaustive_case.chimi`
- `invalid/private_struct_construction.chimi`
- `invalid/private_struct_field_access.chimi`
- `invalid/ignored_value.chimi`
- `invalid/non_exhaustive_outcome_case.chimi`
- `invalid/missing_dependency.chimi`
- `invalid/interface_shape_mismatch.chimi`
- `invalid/observable_case_guard.chimi`
- `invalid/standalone_if.chimi`
- `invalid/empty_braces.chimi`
- `invalid/void_value.chimi`
- `invalid/implicit_module_alias.chimi`

Each negative file contains the diagnostic and source expression the compiler
must identify. Diagnostic codes are provisional but stable identifiers are a
requirement for tooling.

## Deliberately unresolved

- How an execution outcome is deliberately stored or transported as ordinary
  data; the default callable model does not return a `Result` wrapper.
- The exact resource, cancellation, concurrency, and defect model.

Type aliases and `newtype`-like declarations are deliberately excluded from the
first kernel because no accepted corpus case requires them.
