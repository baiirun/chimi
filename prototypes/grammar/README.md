# Chimi Grammar Prototype

> PROTOTYPE: this is a disposable grammar probe, not a Chimi specification or
> compiler implementation.

## Question

Can one small, explicit lexical and surface grammar recognize the complete
golden corpus, reject the deliberately unsupported standalone `if`, preserve
all source text for a future CST, and recover around useful structural
boundaries without adding syntax the kernel does not exercise?

The prototype makes one consequential working choice: line breaks are tokens.
They separate declarations, block items, data members, and case arms, but are
soft inside delimited lists and around continued function contracts. Anonymous
records and expression blocks both use braces; `name:` after the opening brace
selects a record, while other contents select a block. This avoids mandatory
semicolons and extra record syntax.

Both forms must be nonempty, so `{}` is rejected. `Void` is recognized only as
an explicit type for successful completion without data; it is not a literal
or nullable value.

The pipeline operator passes its left operand as the first argument.
`value |> transform` means `transform(value)`, and `value |> combine(other)`
means `combine(value, other)`. The parser does not give retry or any other API
special treatment, and Chimi does not partially apply missing arguments. (This
prototype originally specified unary-only pipelines; the executable kernel's
Pipelines section supersedes that.)

`operation using { dependency: provider }` is a postfix expression that may
supply any subset of an operation's dependency needs before the runtime root.
The provider list is a nonempty record, so an empty `using {}` is invalid under
the same brace rule as other records and blocks.

Imported types use the same explicit module origin as imported values:
`Money.Money` names type `Money` from the module explicitly bound as `Money`.
A module import must declare that binding with `as`; Chimi never derives a
namespace from the path basename. Imports do not inject type or value members
into local scope.

An explicit member import may bring a public type or constructor into its
corresponding namespace, such as `import modules/money.{Money}`. Functions
cannot be member-imported: behavior stays module-qualified at call sites. A
bare `import modules/money` is invalid because it names neither a module binding
nor any members.

Generic function and data declarations bind their parameters explicitly after
the declaration name, such as `fn transform<A, B>(...)`. The parser does not
infer generic declarations from otherwise unresolved uppercase names.

## Run

```sh
python3 prototypes/grammar/main.py
```

Use `--all` for a non-interactive corpus summary:

```sh
python3 prototypes/grammar/main.py --all
```

The checker is intentionally smaller than the later lossless-parser prototype.
It recognizes surface structure and reports locations, but it does not build a
CST, recover after errors, resolve names, or perform semantic checks.

## Recovery boundaries

The later event parser should preserve unexpected input beneath error nodes and
resume at the smallest enclosing structural boundary:

- the next line-starting declaration keyword at file scope;
- a line break, comma, or closing brace in struct, enum, and interface bodies;
- a line break or closing brace between block items and case arms;
- a comma or matching closing delimiter in parameter, argument, list, record,
  pattern, and type-argument lists;
- `->` within a damaged case arm.

An expected but absent delimiter may become a zero-width missing token. Recovery
must always consume input or insert one missing token, so malformed source
cannot trap the parser in a loop.

## Files

- `grammar.ebnf` — the proposed lexical and surface grammar
- `model.py` — a pure lexer and throwaway recognizer for the corpus
- `main.py` — a terminal browser over the corpus analyses

## Expected corpus result

Every fixture is syntactically accepted except `invalid/standalone_if.chimi`,
`invalid/empty_braces.chimi`, `invalid/empty_tuple.chimi`, and
`invalid/implicit_module_alias.chimi`. The other invalid fixtures are
deliberately well-formed programs whose errors belong to later semantic phases.

## Deliberately absent

The draft does not add aliases, newtypes, semicolons, block comments,
floating-point literals, multiline strings, standalone conditionals, nullable
values, or other syntax not exercised by the kernel. Source is UTF-8, while
identifiers remain ASCII in this prototype; broader identifier rules can be
decided when a real use case demands them.
