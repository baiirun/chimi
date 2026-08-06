The Last Programming Language
- BEAM durability/reliability
- Rust performance and safety
- JS + Gleam ergonomics (simple, pipeable, pattern matching, errors as values, pattern matched function overloads instead of control booleans, docs with code, no null sentinels, interface-based, procedural, enums/choices can carry values, expression-oriented, matching including expressions, etc.)
- functions and "services/structs/whatever" define error cases, parameters, AND requirements
- Effects
- Explicit vs implicit type systems?
- Modules/namespaces as first-class mechanism. Don't want arbitrary named imports. Must use module.
- Built-in tracing/logging/observability mechanism. Should be pippable instead of macros/decorators.
- How are failable expressions represented? `try X`? `?` operator?
- Pluggable concurrency (actor, event loop, task queues, channels, etc.)
	- + concurrency helpers like semaphors, mutexes, async data structures, etc.
- Meta-language? What if agents could make the language more powerful ad-hoc via some kind of pluggable mechanisms? What would they even do?
- Would be interesting to make everything in the language pluggable, from type systems, to runtime behaviors (like async), to compiler components, to tooling, etc.
- golang-like built-in tooling and performance
- LSPs
- General purpose
- need to figure out package manager :notlikethis:
- generics or nah
- Logs/traces built into language with exporters. Internals are also logs so agents can debug runtime 

## Try the Current Kernel

From the repository root:

```sh
cargo run -q -p chimi -- run prototypes/golden/valid/dependencies_and_execution.chimi
```

The command executes the checkout forcing program and writes JSON Lines for
every traced operation followed by the terminal outcome. Format source without
modifying its file:

```sh
cargo run -q -p chimi -- format compiler/chimi/tests/fixtures/format_messy.chimi
```

`format` writes canonical source to stdout. Malformed source is returned
unchanged with syntax diagnostics and a failing status. `check` and `run`
accept either one self-contained source file or an explicit project root and
entry module through `--root <project-dir> <entry-module>`.

Request versioned machine-readable diagnostics with:

```sh
cargo run -q -p chimi -- check --json compiler/chimi/tests/fixtures/invalid_field_type.chimi
```

The JSON document keeps compiler stages distinct and includes explicit module,
path, span, label, and fix fields. A successful check returns an empty
`diagnostics` array.
