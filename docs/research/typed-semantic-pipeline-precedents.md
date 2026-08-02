# Typed Semantic Pipeline Precedents

## Question

What should Chimi's first semantic pipeline learn from existing models for typed success values, expected failures, environmental requirements, inference, composition, and executable operations?

This report is scoped to the first executable language kernel. It does not choose production concurrency, durability, resource management, or a permanent runtime.

## Executive recommendation

Give Chimi's typed semantic representation a first-class executable computation with three independently tracked dimensions:

```text
Computation<Success, Failures, Requirements>
```

`Computation` is only a working internal name. Chimi source need not expose that constructor or call everything an effect. Surface syntax can remain direct and expression-oriented while lowering makes the computation explicit.

For the first kernel:

- Represent `Failures` as a closed set or row of named expected-failure variants.
- Represent `Requirements` as a closed set or row of named capability or service identities.
- Infer both dimensions through ordinary composition and make the inferred contract inspectable.
- Treat defects and cancellation as runtime causes, not members of the expected-failure set.
- Allow execution only after requirements have been discharged by typed providers.
- Implement failure handling, retry, and provision as typed transformations of a computation. Do not build general algebraic-effect handlers yet.
- Preserve an explicit semantic distinction between an ordinary `Result` value and suspended executable work. Retrying a `Result` cannot rerun the work that produced it.
- Design the typed core to carry rows and row variables, but initially expose only closed rows plus compiler-declared polymorphic combinators. General user-authored row polymorphism can wait for evidence from the corpus.

This borrows the useful contract shape from Effect and ZIO, inference discipline from Koka, and requirement-discharge rule from Effekt and Unison without adopting any precedent's terminology or full abstraction stack.

## Precedents

### Ordinary `Result` values: Rust and Gleam

Rust defines `Result<T, E>` as an enum whose `Ok(T)` and `Err(E)` cases are ordinary values. Its `map`, `map_err`, `and_then`, and `or_else` operations show that success and error types can transform independently; `?` is syntax for extracting `Ok` or returning `Err` from the enclosing function ([Rust `Result` documentation](https://doc.rust-lang.org/std/result/)). Gleam similarly provides `Result(success, error)` and pipe-friendly `map`, `map_error`, `try`, and `try_recover` functions ([Gleam `result` module](https://hexdocs.pm/gleam_stdlib/gleam/result.html)).

This model is a strong baseline for Chimi's claim that expected failures are values. It is transparent, pattern-matchable, and needs no special runtime. It also keeps absence separate through a different optional type.

It is not sufficient for the agreed operation model. Once a `Result` exists, the work that produced it has already happened. A retry combinator applied to that value can only inspect the same result again; it cannot rerun an external operation. Requirements such as `Clock` or `Mailer` also do not appear in `Result<T, E>`.

**Constraint for Chimi:** keep an ordinary result type available for data-level success/failure, but represent retryable or externally executable work separately. The golden corpus must distinguish these two cases rather than treating every failure as either a `Result` or an implicit exception.

### Typed executable descriptions: Effect and ZIO

Effect describes `Effect<Success, Error, Requirements>` as an immutable, lazy description that does not act until interpreted by its runtime. The three parameters separately track the success value, expected error, and contextual dependencies ([Effect type](https://www.effect.website/docs/v3/getting-started/the-effect-type)). Providing a service removes it from the requirements parameter, and attempting to run an unprovided effect is a type error ([Effect services](https://www.effect.website/docs/v3/requirements-management/services)). Retry composes a schedule with an existing effect, reruns it on failure, and propagates the final failure if the policy is exhausted ([Effect retry](https://www.effect.website/docs/v3/error-management/retrying)).

Effect also separates typed expected errors from the broader runtime cause. Its matching API distinguishes ordinary failure, defect, and interruption when the full cause is inspected ([Effect matching](https://www.effect.website/docs/v3/error-management/matching)). ZIO states the same boundary directly: `ZIO[R, E, A]` describes a workflow that requires `R`, may fail with `E`, or succeed with `A`, while defects and interruption are not promises made by `E` ([ZIO core type](https://zio.dev/reference/core/zio/), [typed-error guarantees](https://zio.dev/reference/error-management/typed-errors-guarantees), and [`Cause`](https://zio.dev/reference/core/cause)).

This family demonstrates that Chimi's desired composition is coherent:

- construction and execution are separate;
- success, expected failure, and requirements can be inferred together;
- retry wraps one job operation rather than owning worker lifecycle;
- providers transform a computation into one with fewer requirements;
- runtime defects remain observable without polluting every callable contract.

Its main warning is ergonomic rather than semantic. In a host language without native support, nearly every operation is routed through one highly generic library type and a large combinator vocabulary. Chimi can encode the same information directly in its semantic model and choose a smaller, native surface.

**Constraint for Chimi:** the typed core needs a suspended executable form and a distinct runtime `Exit` or `Cause` representation. The source language does not need a universally visible `Effect` wrapper.

### Inferred type-and-effect rows: Koka

Koka puts possible effects in function types and infers both ordinary types and effect rows. Its row-polymorphic system automatically combines effects during composition and gives effect-eliminating forms, such as exception handling, types that describe what remains. The original design reports principal type-and-effect inference and proves that omitting an exception effect rules out an unhandled exception ([Koka row-polymorphic effect paper](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/koka-effects-2013.pdf)).

Koka demonstrates that a language can retain ordinary call syntax while the compiler tracks more than input and output types. The extra contract does not have to be represented as a user-visible container around each value. It also shows why sets alone eventually become limiting: higher-order combinators need a way to preserve unknown effects from callbacks, which is the job of row variables.

The full Koka system is beyond the first kernel. General row polymorphism, effect elimination, state interaction, and principal inference introduce substantial type-system work before the corpus has established Chimi's needs.

**Constraint for Chimi:** store failures and requirements as explicit rows in the typed representation and let call composition combine them. Reserve a row-variable representation so built-in higher-order combinators have honest signatures, but do not commit the first surface language to general effect polymorphism.

### Requirements and handlers: Effekt and Unison

Effekt explicitly reads an effect annotation as a requirement: a computation returning `Double / {exc}` needs the calling context to provide the `exc` capability. A call is rejected when its requirement is unavailable ([Effekt effect safety](https://effekt-lang.org/docs/concepts/effect-safety)). Effekt further distinguishes an open requirement, whose implementation is still chosen by the context, from a captured capability whose handler has already been fixed lexically ([Effekt captures](https://effekt-lang.org/tour/captures)).

Unison similarly writes abilities in function types, infers a function's requirements as the union of requirements used by its body, and rejects a call when the surrounding context does not provide the required ability. Handling an ability removes that requirement and gives its operations an implementation ([Unison abilities and handlers](https://www.unison-lang.org/docs/language-reference/abilities-and-ability-handlers/)).

These models are especially relevant to Chimi's `provide` operation: providing is a semantic transformation, not merely a runtime lookup in a string-keyed container. They also expose a boundary Chimi should not cross accidentally. General algebraic handlers receive or manipulate continuations and can implement exceptions, generators, nondeterminism, and other control flow. That is much more power and implementation complexity than typed service provision requires.

**Constraint for Chimi:** model a requirement by resolved type identity and make provision remove that identity from the contract. Keep provider composition specialized in the first kernel; do not make general resumable handlers a prerequisite.

## Comparison

| Model | Success and expected failure | Requirements | Execution | Composition lesson for Chimi |
| --- | --- | --- | --- | --- |
| Rust/Gleam `Result` | Ordinary closed variant value | Function parameters or ambient mechanisms | Eager ordinary evaluation | Best baseline for explicit failure data, mapping, recovery, and pattern matching; cannot by itself rerun work |
| Effect/ZIO computation | Separate success and typed failure channels | Third type dimension | Lazy description interpreted at a runtime boundary | Closest behavioral fit for retry, provision, execution, and defects outside expected failures |
| Koka effect rows | Value type plus inferred effect row | Effects accumulate through calls | Direct-style function execution | Shows that native syntax can hide the container and that higher-order inference eventually needs row variables |
| Effekt/Unison abilities | Return value plus required operations; errors may be modeled as an ability | Named capabilities checked and discharged by handlers | Only fully handled programs execute | Strong rule for typed provision; general handlers are deliberately deferred |

## Recommended semantic contract

The following notation is descriptive, not proposed Chimi syntax:

```text
Comp<A, F, R>
```

- `A`: success value type.
- `F`: normalized row of expected-failure variants.
- `R`: normalized row of unresolved capability requirements.

Name resolution must turn every failure constructor and requirement into a stable symbol identity before type inference. This prevents two modules' identically spelled names from being merged accidentally.

The first core transformations should have explicit, testable rules:

```text
sequence:
  Comp<A, F1, R1>
  (A -> Comp<B, F2, R2>)
  --------------------------------
  Comp<B, union(F1, F2), union(R1, R2)>

map_failure:
  Comp<A, F1, R> and (F1 -> F2)
  --------------------------------
  Comp<A, F2, R>

recover H:
  Comp<A, union(H, Frest), R1>
  (H -> Comp<A, Fhandler, R2>)
  --------------------------------
  Comp<A, union(Frest, Fhandler), union(R1, R2)>

retry:
  Comp<A, F, R1> and Policy<F, R2>
  --------------------------------
  Comp<A, F, union(R1, R2)>

provide P:
  Comp<A, F, union(P, Rrest)>
  Provider<P, Fprovider, Rprovider>
  --------------------------------
  Comp<A, union(F, Fprovider), union(Rrest, Rprovider)>
```

These rules surface two details that prose tends to hide:

1. A retry policy may itself require capabilities. Delay needs a clock; jitter may need randomness. Retry should not secretly access either.
2. Constructing a provider may itself fail or require other capabilities. Provision is therefore more than subtracting a name from `R`.

The executable root should require an empty requirement row. Its dynamic result should preserve at least:

```text
Success(A)
ExpectedFailure(F)
Defect(diagnostic context)
Cancelled(cancellation context)
```

Only the first two belong to the callable's expected success/failure contract. The latter two belong to runtime causality and structured execution events.

## Consequences for the compiler pipeline

The research suggests the following minimum semantic stages after CST construction:

1. **Semantic AST:** discard formatting detail while retaining source spans and distinct syntax for pure values, failure construction, capability use, handlers/combinators, providers, and the execution boundary.
2. **Name resolution:** assign identities to values, types, failure constructors, capability interfaces, and provider implementations.
3. **Contract inference:** infer the success type, failure row, and requirement row for every expression and callable; check annotations against the inferred contract.
4. **Typed elaboration:** lower pipelines and ergonomic failure syntax into a small set of typed core transformations such as `Pure`, `Sequence`, `Fail`, `Request`, `Recover`, `Retry`, and `Provide`.
5. **Closure checks:** reject non-exhaustive failure handling where exhaustiveness is promised and reject execution with unresolved requirements.
6. **Prototype execution:** interpret the typed core and emit a structured event for each core operation, preserving source spans and parent-child causality.

The typed core should be inspectable. A prototype command or test fixture should be able to print an expression's normalized `A`, `F`, and `R`, its lowered core, and its dynamic exit. That gives later syntax decisions evidence instead of requiring the surface notation to expose all compiler machinery.

## Required golden-corpus cases

The first source corpus should contain small positive and negative programs for:

1. An ordinary pure value with empty failure and requirement rows.
2. A pure validation returning an explicit `Result`, contrasted with a suspended fallible operation.
3. Two sequential operations whose success types differ and whose failure and requirement rows combine.
4. Mapping a vendor failure into a domain failure.
5. Recovering one failure variant while leaving another visible in the inferred contract.
6. Retrying only a selected failure and showing the policy's `Clock` requirement.
7. Providing one capability and showing it disappear from the inferred requirements.
8. A missing-provider diagnostic at the execution boundary.
9. A defect that does not appear in the expected-failure row but does appear in the runtime exit and event stream.
10. A higher-order combinator that preserves the callback's unknown failures and requirements, initially via a compiler-declared signature.
11. The compact worker forcing example, assembled from the same primitives rather than worker-specific language semantics.

Each positive case should record expected resolved names, inferred contract, lowered core, runtime exit, and execution events. Each negative case should record the expected diagnostic and source span.

## Uncertainties to resolve by prototype

- **Surface distinction:** whether suspended work is visibly constructed in Chimi syntax or direct-style function bodies are elaborated into computations.
- **Failure representation:** whether a computation's failure row contains variant constructors directly, a single closed error type, or both through normalization.
- **Pure failure:** whether pure validation should conventionally return `Result` while externally executable work uses the failure channel, and how syntax prevents the two from feeling inconsistent.
- **Row scope:** whether closed rows plus compiler-known polymorphism are sufficient for the first corpus, or whether user-authored retry/recovery helpers immediately require general row variables.
- **Provider identity and lifetime:** whether capabilities are nominal interfaces, module values, or another construct, and whether provider acquisition/release belongs in the first semantic kernel.
- **Error handling syntax:** whether pattern matching, a propagation operator, pipelines, or block syntax offers the clearest surface while lowering to the same typed core.
- **Retry observability:** which retry decisions become mandatory execution events without tying semantic typing to the later production tracing design.
- **Defect boundary:** which conditions the prototype runner classifies as defects and how much source/cause context the typed core must preserve.

## Decision gist

Chimi's first typed core should model executable work as a native computation with separately inferred success, expected-failure, and requirement rows; provision and error combinators transform those rows, only fully provided computations run, and defects remain a separate observable runtime cause.
