# Chimi Name and Module Resolution Prototype

> PROTOTYPE: this is disposable semantic research, not the production Chimi
> resolver.

## Question

Can file-based modules, explicitly named module imports, separate
type/value/module namespaces, module-private representations, and lexical
scopes give every resolvable name a stable identity while keeping external
origins visible at the call site?

## Run

From the repository root:

```sh
go -C prototypes/name-resolution run .
```

Run the executable scenario contract:

```sh
go -C prototypes/name-resolution run . --check
```

Benchmark resolution independently from parsing and lowering:

```sh
go -C prototypes/name-resolution run . --bench
```

## Working rules

- A file's repository-relative path is its stable module identity. Importers
  must explicitly choose every module source binding
  (`import modules/money as Money`).
- Aliased imports bind modules only. Explicit member imports may bind public
  types or constructors; functions cannot be imported unqualified.
- Types, values/constructors, and modules are separate namespaces, so a module
  and a type may both be named `Money`.
- Without an explicit member import, external values and types stay qualified
  (`Money.from_cents`, `Money.Money`).
- An explicitly imported type or constructor may be used without the module
  qualifier (`import modules/money.{Money}`), while behavior remains qualified.
- Declarations are private by default. `pub struct Money` publishes the type
  but not its implicit constructor or fields.
- Lexical parameters, lets, and pattern bindings resolve before module values.
- A selection whose base is an imported module resolves here. A selection on a
  runtime value remains deferred until type checking knows the receiver type.
- Pattern constructors resolve to stable module symbols; other pattern names
  introduce arm-local bindings.
- Generic parameters are declared explicitly after a function or data-type
  name and resolve in a declaration-local type namespace. Their value-type,
  failure-row, or needs-row kind is inferred from use. Unbound uppercase names
  are unresolved types, not implicit generics.

The provisional surface forms are:

```chimi
import billing/money as BillingMoney
import billing/money.{Money, NegativeAmount}
```

There are no bare module imports or wildcard member imports. A member list
naming a function is a diagnostic with a module-qualified replacement.

## Current evidence

- Ten focused scenarios cover explicitly qualified module use, qualified and
  explicitly imported types, explicit constructor imports, rejected function
  member imports, private struct construction, missing member injection,
  explicit alias collisions, distinct explicit aliases, declared generic
  parameters, and rejection of undeclared generics.
- All ten scenarios pass with stable symbol identities and stage-specific
  diagnostics.
- On the recorded `darwin/arm64` Go 1.25.5 host, resolving a generated module
  with 10,000 declarations takes 15.7 ms p95 and allocates about 2,783 bytes per
  declaration in the disposable Go layout.
