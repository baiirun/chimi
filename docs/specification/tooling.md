# Tooling

Status: implemented in part (Phase 9). This page is the contract for the
commands that sit around the compiler: the documentation generator now, and
the linter, editor features, cross-compilation and debugging as they land.
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
