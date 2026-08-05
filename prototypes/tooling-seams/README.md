# Chimi Formatting, Diagnostics, and Inspection Prototype

> PROTOTYPE: this is disposable tooling research, not the production Chimi
> formatter or language server.

## Question

Can the accepted lossless CST and source-mapped semantic representation support
canonical formatting, stage-correct diagnostics, and cursor inspection through
small interfaces that do not couple tools to parser or type-checker internals?

## Run

From the repository root:

```sh
go -C prototypes/tooling-seams run .
```

The TUI presents four scenarios. Use `s` for the original source, `f` for the
formatter result, `d` for unified diagnostics, and `i` for syntax and semantic
inspection at the scenario's selected source position.

Check the scenario invariants noninteractively:

```sh
go -C prototypes/tooling-seams run . --check
```

## Proposed seams

- Formatting consumes the lossless syntax result and produces either complete
  canonical text or the unchanged source plus syntax diagnostics. It never
  guesses a rewrite for malformed source.
- Diagnostics share a stable envelope—stage, code, severity, primary span,
  secondary labels, and optional edits—while each compiler stage owns the
  meaning and production of its diagnostics.
- Inspection accepts a source byte offset and composes the containing CST path
  with source-mapped semantic facts. Tools do not search compiler internals or
  derive semantic identity from formatted text.

The formatter itself is intentionally small and incomplete. Its purpose is to
validate that trivia-preserving CST tokens are sufficient and that formatting
can remain a separate consumer. Canonical layout policy requires its own corpus
and decisions before a real implementation.
