---
status: accepted
---

# Keep execution properties out of callable types

Chimi callable types expose data inputs, success values, expected failures, and
dependency needs, but no user-facing effect row. I/O, suspension, resource
access, and observability remain ordinary execution behavior: resolved Core may
derive compact compiler-private summaries when code generation, target
validation, optimization, or tooling has a concrete use for them. These facts
are not source syntax, generic row parameters, caller-handled channels, or
purity claims. This supersedes ADR-0003 because a public effect system adds
pervasive type and higher-order composition machinery without improving how
callers provide dependencies or handle value-level failures.
