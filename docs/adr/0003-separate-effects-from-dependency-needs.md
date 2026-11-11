---
status: superseded by ADR-0004
---

# Separate effects from dependency needs

Chimi callable contracts track effects in a row distinct from dependency needs
and expected failures. `FileSystem` and `Console` are dependencies because a
caller selects their implementations; `io` is an effect because it describes
external interaction that remains true after those dependencies are supplied.
Calls union effect rows, `using` removes only needs, and failure recovery removes
only failures. The first effect is built-in synchronous `io`; Chimi does not add
an explicit `Effect` wrapper, user-defined effects, or general handlers in this
slice.

This separation preserves explicit local reasoning. Deriving effects from needs
would make I/O disappear after provision, while deriving effects from failures
would confuse execution behavior with completion values. The runtime and runner
own concrete host providers, resource cleanup, cancellation boundaries, and
execution configuration, but the compiler owns the inspectable effect contract.

The first resource API owns its scope and passes line values to a callback
instead of exposing a raw file handle. This avoids committing Chimi to lifetime
or region syntax before a concrete use case requires it.
