# Chimi Type and Operation Contract Prototype

> PROTOTYPE: this is a disposable semantic probe, not a production type
> checker or stable Chimi specification.

## Question

Can Chimi infer one coherent callable shape across ordinary values, expected
failures, environmental requirements, structural providers, higher-order
composition, pipelines, selective recovery, and retry without requiring a
visible computation wrapper or propagation syntax?

The pure `contracts` package models the proposed rules. The terminal shell
shows source-shaped scenarios, the resulting contract, diagnostics, and each
channel transformation. It deliberately does not parse the displayed snippets;
the existing grammar, CST, lowering, and name-resolution prototypes own those
earlier compiler boundaries.

## Run

From the repository root:

```sh
go -C prototypes/type-contracts run .
```

Check every scenario non-interactively:

```sh
go -C prototypes/type-contracts run . --check
```

## Working rules

- Every expression has a success type, a normalized row of expected-failure
  variants, and a normalized row of named requirements.
- Ordinary calls automatically union unhandled failure and requirement rows
  into the caller. There is no propagation operator.
- Failure rows retain variant identities internally. Tooling prints a whole
  error enum when every variant remains and prints individual variants after
  selective handling.
- An outcome `case` removes the failure variants it handles. Unmatched failure
  variants bubble automatically; ordinary enum cases remain exhaustive.
- Higher-order transformations can change success while preserving unknown
  failure and requirement rows.
- `Retry.with(...)` (historical; now `Retry.run(operation, policy)` in the
  time-and-outcomes spec) is a unary transformation. It preserves the operation's
  final expected failures and adds requirements used by its policy, such as
  `Clock`.
- Requirement rows use lexical names and interface types. Composition unions
  compatible names and rejects the same name with incompatible interfaces.
- `using` may discharge any subset of a computation's needs at an arbitrary
  expression boundary; unresolved needs remain visible in the result.
- Providers satisfy requirement interfaces structurally. Provision removes
  only successfully matched requirements. A method implementation may expose
  fewer failures or needs than its interface promises, but never more.
- A runtime root is valid only when its requirement row is empty.
- A failed `using` check leaves the need unresolved for analysis but marks it as
  already explained, preventing a redundant missing-dependency diagnostic at a
  later runtime root.
- Defects and cancellation are deliberately outside these callable channels;
  the runner prototype will model them as runtime outcomes.

## Current evidence

Seventeen scenarios cover pure inference, automatic bubbling,
failure/requirement union, selective recovery, higher-order preservation,
pipeable retry, partial `using` boundaries, structural provision, missing
dependencies, provider mismatch, ordinary enum exhaustiveness, guarded and
unguarded fallbacks, unreachable arms, variant destructuring, struct fields,
and incompatible pipelines.

## Accepted verdict

The reviewed kernel uses one inferred callable shape with success, expected
failures, and named needs. Calls bubble unhandled failures and union needs;
failure cases remove only named handled variants; `using` discharges needs at
arbitrary boundaries; structural providers may narrow channels; runtime roots
must be closed; pipelines are ordinary unary application; and ordinary closed
variants retain exhaustive pattern matching with catch-all and reachability
checks. Nominal struct construction and variant destructuring enforce their
declared data shapes.
