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

Compile the same program and its Chimi runtime into one native executable:

```sh
cargo run -q -p chimi -- build \
  prototypes/golden/valid/dependencies_and_execution.chimi \
  -o target/checkout
target/checkout --chimi-runner \
  --trace=boundaries \
  --telemetry-file=target/checkout.trace.jsonl \
  --outcome-file=target/checkout.outcome.jsonl \
  --
```

Native executables leave stdout and stderr to the application. Runner telemetry
and the terminal outcome are optional, versioned JSONL exports written only to
their explicitly configured files.

Build the maintained trace summarizer:

```sh
cargo run -q -p chimi -- build \
  --root programs/chimi-trace-summary \
  app/main \
  -o target/chimi-trace-summary
```

Summarize the checkout execution produced above:

```sh
cat target/checkout.trace.jsonl target/checkout.outcome.jsonl \
  | target/chimi-trace-summary -
```

See `programs/chimi-trace-summary/README.md` for input rules and runner options.

`build` prints a versioned `chimi.build.v1` report and writes the same report
as `target/chimi/checkout/build.json`. The report identifies the Cranelift
backend and runtime ABI, separately cacheable module, metadata, and runtime
artifacts, remaining dynamic system dependencies, and whether the target is
fully static. Fully static output is reported only when the produced executable
actually has no dynamic dependencies.

Build artifacts use a content-addressed cache under `target/chimi/.cache`.
The report's `cache.hits` and `cache.misses` arrays show exactly which module
objects, entry object, metadata object, and runtime archive were reused. A Core
change invalidates its owning module object and whole-program metadata without
forcing unrelated modules or the versioned runtime archive to rebuild.

Request versioned machine-readable diagnostics with:

```sh
cargo run -q -p chimi -- check --json compiler/chimi/tests/fixtures/invalid_field_type.chimi
```

The JSON document keeps compiler stages distinct and includes explicit module,
path, span, label, and fix fields. A successful check returns an empty
`diagnostics` array.

Inspect the compiler facts at a UTF-8 byte offset with:

```sh
cargo run -q -p chimi -- inspect prototypes/golden/valid/dependencies_and_execution.chimi 1535
```

The versioned `chimi.inspection.v1` document connects the containing lossless
syntax, token, resolved declaration origin, inferred type and callable
channels, and reachable executable origin. Offsets and ranges are zero-based
UTF-8 bytes; lines and byte columns are one-based.
