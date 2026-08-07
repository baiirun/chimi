# Synchronous Effects and Host Capabilities

## Status

This is the first normative post-kernel extension. It specifies only the
synchronous I/O behavior required by `chimi-trace-summary`. Chimi remains free
to break, but implementations of this milestone MUST preserve these semantic
distinctions unless a later decision explicitly supersedes them.

## Purpose

The executable kernel makes data inputs, success values, expected failures, and
dependency needs independently visible. A real CLI also interacts with files
and console streams. Dependency needs alone cannot describe that behavior: a
`FileSystem` identifies which implementation a caller must supply, while `io`
states that calling the operation may interact with the external world.

Chimi therefore adds an inferred effect row to callable contracts. The row
makes observable behavior inspectable after dependencies are provided without
wrapping ordinary source expressions in an `Effect` value.

## Scope

This contract defines:

- one built-in synchronous effect identity, `io`;
- effect-row syntax, inference, composition, and callable compatibility;
- the distinction between data, failures, needs, effects, and runner policy;
- ownership of official filesystem and console capabilities;
- a scope-owning resource operation for incremental file processing;
- root provision, setup validation, and synchronous cancellation boundaries;
- diagnostics and inspection obligations.

It does not define async or suspension effects, concurrency, general algebraic
effect handlers, effect elimination, user-defined effects, raw file handles,
lifetime syntax, mutable state effects, networking, clocks, or durability.
Those concepts MUST NOT be inferred from `io` merely because they may later
compose with it.

## Callable Contract

Every callable has five independent dimensions:

```text
data inputs -> success type
errors expected-failure-row
needs dependency-row
effects effect-row
```

The first source spelling places effects after needs:

```chimi
fn load(path: Path) -> Text
  needs { files: FileSystem }
  effects { io } {
  files.load(path)
}
```

Implemented functions infer failures from their bodies and MAY omit success,
needs, and effects annotations; the compiler infers those uniformly for public
and private code. Function-value types and interface methods MAY state failure
and effect rows. Empty rows are omitted from ordinary source and exposed as
empty collections by inspection tools. In this example, tooling reports the
`FileError` failure row inherited from `FileSystem.load`.

`io` is a compiler-known effect identity, not a value, type, module, interface,
dependency name, failure, or runner option. It does not require an import. The
first implementation MUST reject unknown effect identities rather than
silently creating user effects.

Generic parameters used in effect position have effect-row kind:

```chimi
fn apply<A, B, E>(
  value: A,
  operation: fn(A) -> B effects E,
) -> B effects E {
  operation(value)
}
```

One generic parameter MUST NOT be used as both a value type and an effect row.
The compiler MAY infer the kind from use, following the existing failure- and
needs-row rules.

## Composition Rules

Calling a function unions the callee's effect row into the caller's inferred
row. Blocks, pipelines, partial application, returned functions, and ordinary
function composition preserve the same relationship. Duplicate effects
normalize to one stable identity.

For the descriptive shapes below:

```text
f: fn(A) -> B errors F1 needs R1 effects X1
g: fn(B) -> C errors F2 needs R2 effects X2
```

their sequential composition has:

```text
fn(A) -> C
errors union(F1, F2)
needs union(R1, R2)
effects union(X1, X2)
```

The transformations remain independent:

- `using` removes successfully supplied dependency needs and MUST NOT remove
  effects or failures;
- failure matching removes only handled failure variants and MUST NOT remove
  effects or needs;
- passing application arguments changes only data inputs;
- runner tracing, execution identity, cancellation, and limits MUST NOT enter
  any callable row;
- there is no source form in this milestone that handles or removes `io`.

A case guard MUST have success type `Bool` with empty failure, needs, and effect
rows. Provisioning its dependencies or recovering its failures does not make an
I/O-performing guard pure.

## Callable Compatibility

An implementation is compatible with a function or interface contract only if
its data inputs and success type match and each observable row is no wider than
the declared contract. It MAY expose fewer expected failures, fewer dependency
needs, or fewer effects. It MUST NOT expose an effect absent from the expected
contract.

Calls through an interface use the interface method's declared effect row. A
pure in-memory provider may satisfy an interface that permits `io`, but viewing
that provider through the interface remains conservatively `io`. Provision is
a dependency transformation, not effect specialization. A direct call through
a separately typed pure function retains its narrower empty effect row.

## Host Capability Ownership

The official standard library owns the structural `FileSystem` and `Console`
interfaces, their path and error values, and safe module-qualified functions.
Application code MAY define alternative providers structurally. These
interfaces are dependencies because callers choose their implementations.

The native runtime owns concrete OS operations, file descriptors, stream
access, and cleanup on success, expected failure, defect, and cancellation. It
MUST NOT make provider lookup ambient: execution receives a resolved provider
plan for every remaining root need.

The runner supplies application arguments as ordinary `List<Text>` data. It
also selects or accepts the root provider plan, validates that every declared
need has a compatible provider before computation starts, and supplies trace,
identity, cancellation, and limit configuration separately. A missing or
incompatible provider is an execution-setup failure, not an expected value from
the program body.

The compiler owns effect-row kinds, inference, compatibility, inspection, and
root metadata. A standalone target MAY expose `io` at its root when its selected
runtime supports that effect. Unlike dependency needs, supported effects are
not removed by the runner; they remain truthful properties of the executable.

This extends the kernel's self-contained empty-needs root: `using` MAY still
close every need in source, but a post-kernel root MAY retain needs that the
runner's typed provider plan closes. The runner MUST NOT supply undeclared
dependencies or reinterpret effect names as providers.

## Synchronous Files and Console

The accepted general data-processing API is specified by
[trace data and processing](trace-data-and-processing.md) (`prog show
ts-2ac2de`). The first filesystem boundary MUST own resource scope. Its contract
is:

```chimi
interface FileSystem {
  process_utf8_lines: fn<A, F, R, X>(
    Path,
    A,
    fn(A, Text) -> A errors F needs R effects X,
  ) -> A
    errors F + FileError + Utf8Error
    needs R
    effects X + { io }
}

interface Console {
  write_line: fn(Text) -> Void
    errors ConsoleError
    effects { io }
}
```

Callback failures, needs, and effects propagate independently; filesystem and
UTF-8 failures are added; and `io` remains visible.

`process_utf8_lines` opens and closes the file internally. The callback receives
text, not a raw handle, and therefore cannot retain the resource. The runtime
MUST close the resource before returning success, propagating an expected
failure, surfacing a defect, or completing cancellation. A cleanup failure
MUST NOT be swallowed.

For this first scope, a close failure replaces a pending success or expected
failure with the capability's expected `FileError.Close` value; the displaced
outcome is retained as structured suppressed context. A pending defect or
cancellation remains primary, and the close failure is attached as suppressed
runtime context. Full tracing MUST expose both the body outcome and cleanup
failure. This precedence keeps defects and cancellation distinct while ensuring
that a failed cleanup cannot be mistaken for a clean resource boundary.

Raw handles and a general `with_resource` abstraction are rejected for this
milestone because preventing a handle from escaping would require a resource,
region, capture, or lifetime model not otherwise earned by the program.

## Cancellation

Cancellation remains a runtime outcome, not an expected failure variant and
not a member of the effect row. A synchronous host call MUST check cancellation
before entering the OS operation and after regaining control. Cleanup completes
before cancellation propagates.

This milestone does not promise that every blocking system call is immediately
interruptible. The runtime MUST document the maximum known uninterruptible
boundary for each host operation. If a system call returns an interruption and
cancellation is active, the runtime propagates cancellation; otherwise the
operation follows its documented retry or `FileError`/`ConsoleError` policy.

## Representative Program

The first CLI root can expose the full shape directly:

```chimi
pub fn main(arguments: List<Text>) -> Void
  needs {
    files: FileSystem,
    console: Console,
  }
  effects { io } {
  TraceSummary.run(arguments)
}
```

Tooling reports the inferred `UsageError`, `FileError`, and `ConsoleError`
failure variants; implemented-function syntax does not restate them.

An embedding caller can instead supply in-memory providers with `using` at any
expression boundary. Arguments stay data in both cases; no `Arguments` service
is invented merely to preserve the old zero-input kernel root.

## Diagnostics and Inspection

The compiler MUST diagnose at the owning source range when:

- an unknown name appears in an effect row;
- one generic is constrained as both an effect row and another kind;
- a function or provider exposes effects wider than its expected contract;
- a case guard exposes `io` even after its failures and needs are handled;
- a target cannot execute an effect exposed by the sealed root.

Representative messages are:

```text
case guards must be pure and cannot expose effects { io }
provider method write_line exposes effects { io }, expected {}
generic E cannot be both a value type and an effect row
target native-cli cannot execute root effects { unsupported_effect }
```

For example, supplying the dependency in this guard does not erase the
operation's effect:

```chimi
interface FeatureFlags {
  enabled: fn(Text) -> Bool effects { io }
}

fn choose(value: Int, host_flags: FeatureFlags) -> Int
  needs { flags: FeatureFlags } {
  case value {
    current if flags.enabled("new") using { flags: host_flags } -> current,
    current -> current,
  }
}
```

The guard is invalid with `case guards must be pure and cannot expose effects
{ io }`. Separately, an annotation such as `fn invalid() effects { network }`
is invalid in this milestone because `network` is not a declared built-in
effect, and passing `Console.write_line` where `fn(Text) -> Void` is required
is invalid because the actual callable exposes a wider effect row.

Missing or incompatible root providers are runner setup diagnostics and MUST be
reported before `computation_started`. Tooling inspection and generated
documentation MUST display the complete inferred effect row beside success,
errors, and needs. Structured diagnostics retain the existing type/setup stage
distinction and source ownership.

## Implementation Model

The typed representation adds an interned effect row parallel to, but distinct
from, failure and needs rows. Callable contracts and row-generic substitution
carry all four non-input dimensions without introducing a user-visible
computation wrapper. Executable lowering retains the normalized effect row on
functions and the root so targets and tooling can validate it; ordinary `io`
calls still lower through runtime-owned host capability boundaries.

Effect inference is semantic and MUST NOT depend on whether native lowering
later inlines, specializes, or replaces a host provider. Runtime tracing records
the operations that actually execute but does not define their static effect
types.

## Rejected Alternatives

- **`effects { FileSystem, Console }`:** rejected because these are dependency
  interfaces whose implementations are selected by callers, not behavior
  properties.
- **Needs imply effects:** rejected because `using` would make external behavior
  disappear from the callable shape and a dependency can have pure or effectful
  implementations.
- **I/O failures imply effects:** rejected because failure values describe how
  an operation may complete, not whether it interacts externally.
- **An explicit `Effect<...>` source wrapper:** rejected because every function
  is already an executable computation and direct syntax can carry the row.
- **Raw file handles first:** rejected until Chimi has a proven safe lifetime or
  resource-scope model.
- **General effect handlers:** rejected because synchronous host I/O needs
  inference and execution boundaries, not continuation manipulation.

## Verification

The accepted implementation MUST prove:

1. pure calls retain an empty effect row;
2. filesystem and console calls infer `io` independently of their needs and
   failures;
3. sequencing and higher-order calls union and substitute effect rows;
4. `using` changes only needs, and failure recovery changes only failures;
5. pure providers can narrow interface implementations without making calls
   through an `io`-permitting interface statically pure;
6. case guards reject any remaining effect;
7. root arguments arrive as data, root provider validation precedes execution,
   and runner configuration never appears in the callable contract;
8. file resources close on success, failure, defect, and cancellation;
9. strict compiler, formatter, inspection, native-oracle, and malformed-source
   gates remain intact.

The throwaway state model that established the row independence is captured on
branch `codex/synchronous-effects-prototype`; it is evidence, not a production
dependency. Production compiler implementation is tracked by `prog show
ts-5b0547`.
