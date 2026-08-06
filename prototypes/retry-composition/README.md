# Retry Composition Prototype

> PROTOTYPE: this fixture asks whether retry can be expressed entirely through
> thunks and ordinary function composition.

Run from the repository root:

```sh
cargo run -q -p chimi -- run --root prototypes/retry-composition app/main
```

The program pipes a failing thunk through a partially applied `Retry.with`
function. The library calls the thunk once, handles its expected failure, and
calls the same thunk again. The final expected failure proves exhaustion
without introducing retry syntax, compiler behavior, or a runtime intrinsic.

## Verdict

Retry itself is ordinary composition. `Retry.with(Retry.once())` remains
polymorphic until the pipe supplies an operation, then returns a thunk with the
operation's success, failure, and `needs` channels. Full tracing shows the
original operation's required dependency called twice.

This prototype does not define a generic selective-retry predicate. In the
current kernel, a generic failure row such as `E` describes a set of possible
variants; it is not a value type that can also be passed to `fn(E) -> Bool`.
Relating a recovered failure value to an open failure row is a separate
type-system question, not grounds for retry-specific runtime behavior.
