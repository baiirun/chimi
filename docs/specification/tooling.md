# Tooling

Status: implemented in part (Phase 9). This page is the contract for the
commands that sit around the compiler: the documentation generator and its
static pages, the linter, the post-mortem trace viewer, the `--target` flag
of `chimi build`, and editor references, rename, completion, inlay types and
hover documentation now, and more editor features and building for other
targets as they land.
The formatter, `chimi check` and `chimi test` keep their own pages
(`testing.md` for the runner and doc-comment examples).

## Documentation: `chimi doc`

Purpose: show what a module offers, as the compiler understands it
(CONTEXT.md, "Compiler-Derived Documentation"), so a reader sees the
failures and needs a function has even where its source leaves them to
inference.

```text
chimi doc [--json] <path>                      the `main` module of one file
chimi doc [--json] --root <project> <module>   a module of a project
chimi doc [--json] std/<module>                a module of the standard tree
chimi doc --html <directory> <path>            static pages for a file's modules
chimi doc --html <directory> --root <project> <module>
                                               static pages for a project
chimi doc --html <directory> std               static pages for the standard tree
```

Rules:

- It MUST list every public item of the module in source order, and MUST
  NOT list private items or `test` items.
- A function MUST show the contract the compiler derived for it: its
  parameter names as written, the inferred parameter and result types, and
  its `errors` and `needs` rows when they are not empty. Its generic
  parameter list is shown as written. A clause group's parameters are
  patterns, so it shows only their types, as `pub fn area(Shape) -> Int`.
- A struct, enum, interface, native type or `process name` MUST show its
  declaration as written, with its members' own doc comments, since its
  fields, variants and members are its contract. A constant shows its
  declaration; one whose declaration spans lines shows only its name and
  type, because a long initializer is not its contract. Declarations print
  with `\n` line ends whatever the source uses.
- An item's doc comment is the `///` run directly before the line it starts,
  or before plain `//` lines directly above it, the same rule that makes examples (`testing.md`, "Documentation examples"). It is shown
  as written, examples included, indented four spaces under the signature.
- The module's summary is a `///` run at the top of the file that a blank
  line separates from what follows, so it documents no item.
- A module that does not type-check MUST fail with its diagnostics and print
  no documentation, so the output never shows a contract the compiler did not
  derive. A project module that cannot be read fails naming the file; a
  `std/` path the standard tree lacks fails with `no module`.
- `--json` prints one object, `{"module", "summary", "items": [{"kind",
  "name", "signature", "doc"}]}`, with `null` for a missing summary or doc.
  `kind` is one of `fn`, `struct`, `enum`, `interface`, `const`, `type`,
  `process name`.

- `--html <directory>` MUST write static pages into the directory, creating
  it: `index.html`, which lists the modules in path order with the first
  paragraph of each summary, and one page per module at its module path, as
  `lib/geo.html`, showing what the terminal form shows. For a file or a
  project the modules are the project's own ones that the entry module
  reaches, never `std/`; for `std` they are every module of the standard
  tree, and naming one `std/` module is a usage error. Links between pages are relative, and each page
  carries its own styles, so the directory can be opened from disk or served
  as it is. Nothing is printed, and nothing is written when the project does
  not type-check.
- On a page, doc comments are read as a small part of Markdown: blank lines
  separate paragraphs, `- ` lines make a list (an indented line continues an
  entry), fences make a code block (closed by a fence at least as
  long), and backticks mark inline code. Anything
  else shows as written, and all text is HTML-escaped.

Not yet: one item by name (`chimi doc std/option unwrap_or`), links from a
type in a signature to its declaration, and search.

Implementation: `compiler/chimi/src/doc.rs` builds the documentation from the
type report (`TypeReport::documentation`, and `TypeReport::documentation_of`
for many modules); doc-comment lines are read by `doc_examples.rs`, and
`doc_site.rs` renders the pages. Tests: `compiler/chimi/tests/doc.rs`, the
fixture `tests/fixtures/doc-project`, and the unit tests in `doc_site.rs`.

## Linting: `chimi lint`

Purpose: point out code that compiles but that a maintainer would want to
clean up. The compiler enforces correctness; the linter guides conventions
and maintainability (design-philosophy.md), so lint findings are never
compile errors and `chimi check` does not run them.

```text
chimi lint [--json] <path>                      one file
chimi lint [--json] <directory>                 every module under it
chimi lint [--json] --root <project> <module>   a module and what it imports
```

Rules:

- `chimi lint` MUST lint only the project's own modules, never `std/`. It
  exits 0 with no findings and 1 with findings, printing them to standard
  error, or with `--json` to standard output in the `chimi check --json`
  form.
- A project that does not type-check MUST report its compile errors and no
  lint findings, since a finding on code that does not compile misleads.
- Each finding is a warning with the stage `lint` and the rule's name as its
  code.
- A name that starts with `_` is unused on purpose: no rule reports it.
- There is no configuration file and no comment that silences a finding,
  as the formatter has no options: a finding is fixed, or the name gets a
  leading `_`.

| Rule | Reports |
| --- | --- |
| `unused-import` | a module alias no code in the module uses, as `Alias.member` or `Alias.Type`, and a member import (`import m.{Name}`) the module never names, alone or as `Name.Variant` |
| `unused-private` | a private function or constant nothing references; a runner provider (`host`) the runner binds by name is exempt |
| `unused-binding` | a `let` binding or a pattern binding (in `case`, a destructuring `let`, a clause head) that its scope never reads |
| `missing-doc` | a public item with no `///` doc comment, which `chimi doc`, hover and completion would show with no explanation; `main` is exempt, since the entry point is the program, not an API |

Parameters are never reported, because a callback or a fake often ignores
one its type requires, as in Go and Gleam.

Known gaps: a use is any reference, not reachability from public items and
tests, so a private function that only calls itself, two that only call each
other, or a chain only dead code uses, all count as used. Private structs,
enums, interfaces and `process name` items, and unused `needs` entries, are
not checked yet. When a module alias and a member import share a name, as
`import std/clock as Clock` with `import std/clock.{Clock}`, a qualified path
such as `Clock.Instant` uses the alias, as the compiler resolves it.
Suggested fixes (code actions) are not offered yet.

In the editor, the language server MUST publish the same findings for the
project's own modules as warnings beside the compiler's diagnostics, and
none while the project has errors.

Implementation: `compiler/chimi/src/lint.rs` (`TypeReport::lint`), over the
name resolution, which records each local binding's name range. Tests:
`compiler/chimi/tests/lint.rs`, which also keeps the standard tree and the
example programs lint-clean.

## Editor: find references and rename

Purpose: let an editor list every place a symbol is written and rename it
everywhere, across the modules of the project that owns the file. The
language server (`chimi-lsp`) answers `textDocument/references` and
`textDocument/rename` from the latest analysis of that project, including
unsaved edits.

Rules:

- The symbol is the one whose name is under the cursor, at a reference or
  at a declaration. A struct's type and its constructor are one symbol, as
  are a type and a value declared by one item.
- Its occurrences MUST be its declaration (a local's binding, an item's
  name, every clause head of a multi-clause function, a field or variant
  name), every reference that resolves to it, every member import of it
  (`import lib.{Point}`), and for a struct every constructor call that builds
  it. Each occurrence covers only the name: in `Lib.total`, only `total`.
- References MUST leave the declaration out when the editor asks for that.
- A rename MUST cover every occurrence, and MUST be refused when:
  - the symbol is a standard or built-in one, or a module alias;
  - the new name is not an identifier, or changes the leading case, since
    case decides whether a name is a type or a value;
  - the renamed project would have more errors than before, which catches a
    collision with another name in scope;
  - in the renamed project the symbol would not be written at exactly the
    renamed places, which catches a shadowing that still type-checks, such
    as a parameter renamed to the name of a constant its function reads.
- A refused rename is an error reply that says why, never a partial edit.

Not yet covered: renaming a struct field or an enum variant through its uses,
because field accesses are resolved by the type checker, not by name
resolution.

Implementation: `compiler/chimi/src/occurrences.rs`
(`IncrementalTypeReport::occurrences`), `Workspace::references` and
`Workspace::rename` in `compiler/chimi-lsp/src/workspace.rs`. Tests: the
workspace unit tests and `compiler/chimi-lsp/tests/protocol.rs`.

## Editor: completion

Purpose: after a module alias and a dot, offer the names the imported module
makes public, so its surface is visible without leaving the file. The
language server answers `textDocument/completion`, triggered by `.`.

Rules:

- After `Alias.`, or `Alias.` and the start of a name, where `Alias` is a
  module alias of the file (`import lib as Lib`), the items MUST be the
  module's public items in source order: their names, kinds, the signatures
  `chimi doc` prints (a function's with its derived contract) and their `///`
  doc comments as Markdown. The editor filters them by what is typed.
- The alias MUST be looked up in the file's latest analysis, so a `Lib.`
  that does not parse yet still completes.
- After anything else the answer MUST be empty. A path such as
  `point.Lib.` is not an alias, and `Lib.` inside a comment or a text
  literal, closed or still being typed, is not code.

Not yet covered: names in scope where no alias comes first (locals, the
module's own items and imported members), and a struct's fields after a
value and a dot, which needs the value's type.

Implementation: `IncrementalTypeReport::alias_members` (in
`compiler/chimi/src/doc.rs`) and `Workspace::completion`. Tests: the
workspace unit tests and `compiler/chimi-lsp/tests/protocol.rs`.

## Editor: inlay types and documentation on hover

Purpose: show what the compiler inferred where the source leaves it out,
and an item's documentation where it is used.

Rules:

- `textDocument/inlayHint` MUST give, for each `let` that does not state a
  type, the inferred type after the binding's name, as `: List<Int>`, for
  the lines the editor asks about. An annotated `let` gets none.
- A binding whose type is not fully inferred, because of a type error or an
  unsolved type, MUST get no hint: a wrong type misleads more than none.
- Hover MUST add the item's `///` doc comment under the contract or type it
  shows, after a rule, at a reference to the item in any module and at its
  declaration. An item without a doc comment shows only its contract.

Not yet covered: hints for pattern bindings (a destructuring `let`, a `case`
arm) and for parameters of anonymous functions, and doc comments of fields
and variants.

Implementation: `compiler/chimi/src/hints.rs`
(`IncrementalTypeReport::let_type_hints`), `doc_comment_at` in
`compiler/chimi/src/doc.rs`, and `Workspace::inlay_hints` and
`Workspace::hover`. Tests: the workspace unit tests and
`compiler/chimi-lsp/tests/protocol.rs`.

## Post-mortem traces: `chimi trace show`

Purpose: answer "what went wrong in that run" from the telemetry a run
already wrote (`runtime-output-channels.md`), without rerunning it or reading
raw JSON Lines.

```text
chimi trace show [--all] <trace> [<path> | <directory> | --root <project> <entry>]
chimi trace show [--all] - ...                 the trace on standard input
```

Rules:

- The trace is `chimi.runtime.v1` JSON Lines: a run's telemetry, optionally
  followed by its outcome record (`cat telemetry.jsonl outcome.jsonl`), or a
  `chimi test --trace` file. Any other record, or a line that is not a JSON
  object, MUST fail naming its line, and print nothing.
- For each `execution_id`, in the order first seen, the report MUST give the
  outcome (`none recorded` without one; a failure's message and cause when
  the record has them), the number of events, and the computation tree.
  An outcome record belongs to the execution whose event came just before
  it.
- In the tree, each operation shows its kind, its source location and its
  outcome when that is not success. An operation that did not succeed and
  none of whose children failed is marked `<- started here`, since that is
  where the failure began; one that started but never completed says
  `(did not finish)`. A dependency call shows as
  `dependency <requirement>.<method>`.
- By default only operations that did not succeed are listed, with each
  parent's succeeded children folded into a count, so the failure path
  stays readable in a long run. `--all` lists every operation.
- Indentation stops growing 24 levels deep, and deeper lines carry
  `[depth N]`, so a deep recursion in the program gives a report linear in
  the trace's size. The tree is walked without recursion, so no depth
  overflows the viewer.
- With the program's source (the same file, directory or project and entry
  the run was built from; a directory for `chimi test <directory>` traces),
  a location is `path:line:column`; without it, `file <id> at <offset>`.
  The source is loaded only to name places, so it need not type-check.
  When a place in the trace is not in the given source, the report still
  prints and a warning on standard error says the source is likely not the
  one the run was built from, since its other locations may be wrong too.
  This catches a missing module or an offset past a file's end, not a
  different program whose modules happen to cover the same places: the
  trace carries no fingerprint of its source.
- After the tree come the process incarnations that ended other than by
  returning, with their cause and defect message, and the tasks that never
  ended, with the channel a parked task waits on (from the
  `channel_receive_parked` or `channel_send_parked` event after its
  `task_parked`): the usual reasons a concurrent run stops or hangs.

Not yet: timing (the telemetry carries none), channel and clock event
detail, linking a process's spans to its entry in the process list, telling
apart two runs concatenated under the same `--execution-id` (their events
merge), and a browsable or interactive view. Chimi has no REPL
(decisions.md, 2026-11-17): exploration happens in tests and runnable doc
examples.

Implementation: `compiler/chimi/src/trace_view.rs` (`render_trace`) and
`LoadedProject::source_locator`. Tests: `compiler/chimi/tests/trace_show.rs`.

## Cross-compilation: `chimi build --target`

Purpose: name the platform a build is for, so that building for another
platform than the one running `chimi` is one flag once it is supported.

```text
chimi build <path> [-o <executable>] [--target <triple>]
chimi build --root <project> <entry> [-o <executable>] [--target <triple>]
```

Rules:

- `<triple>` is a target triple, as `x86_64-unknown-linux-gnu` or
  `aarch64-apple-darwin`, in the spelling Rust and LLVM use. Two triples
  name the same target when their architecture, operating system,
  environment and object format agree: the vendor is ignored, as are
  `darwin` against `macosx` and a macOS version, so `x86_64-linux-gnu`,
  `x86_64-pc-linux-gnu`, `arm64-apple-darwin` and `aarch64-apple-macosx14.0`
  name the hosts they look like. A partial triple, such as `x86_64` alone,
  names a bare-metal target and is refused as another target.
- The host's triple MUST build exactly as a build without `--target`.
- Any other target MUST fail with exit status 1 before compiling anything,
  naming the target, what building for it needs (the Chimi runtime compiled
  for it, and a linker and C library for it) and the host it can build for.
  A string that is not a target triple fails naming the host as an example.
- `-o` and `--target` may come in either order, each at most once.

Not yet: building for another target. That needs a runtime pack per target
(the C runtime and fiber library built for it) and a linker and C library for
it, such as clang with lld and a sysroot, and Cranelift's code generators for
the other architectures, which this build leaves out.

Implementation: `check_build_target` in `compiler/chimi/src/backend/native.rs`
and `chimi build` in `main.rs`. Tests: `compiler/chimi/tests/build_target.rs`.
