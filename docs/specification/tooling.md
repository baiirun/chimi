# Tooling

Status: implemented in part (Phase 9). This page is the contract for the
commands that sit around the compiler: the documentation generator, the linter and editor
references, rename and completion now, and more editor features, cross-compilation and debugging as they land.
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
  the same rule that makes examples (`testing.md`, "Documentation examples"). It is shown
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

Not yet: one item by name (`chimi doc std/option unwrap_or`), and writing a
whole project's documentation as static pages, which comes with the second
documentation part.

Implementation: `compiler/chimi/src/doc.rs` builds the documentation from the
type report (`TypeReport::documentation`); doc-comment lines are read by
`doc_examples.rs`. Tests: `compiler/chimi/tests/doc.rs` and the fixture
`tests/fixtures/doc-project`.

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

Parameters are never reported, because a callback or a fake often ignores
one its type requires, as in Go and Gleam.

Known gaps: a use is any reference, not reachability from public items and
tests, so a private function that only calls itself, two that only call each
other, or a chain only dead code uses, all count as used. Private structs,
enums, interfaces and `process name` items, and unused `needs` entries, are
not checked yet. When a module alias and a member import share a name, as
`import std/clock as Clock` with `import std/clock.{Clock}`, a qualified path
such as `Clock.Instant` uses the alias, as the compiler resolves it. A missing doc comment on a
public item, suggested fixes, and showing findings in the editor come with
the second linting part.

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
