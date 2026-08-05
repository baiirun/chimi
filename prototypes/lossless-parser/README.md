# Chimi Lossless Parser and CST Prototype

> PROTOTYPE: this is disposable parser research, not the production Chimi
> frontend. Go is a tactical native host for meaningful timings and simple
> distribution; it does not select the permanent compiler language.

## Question

Does a tree-independent event parser feeding an immutable, semantic-free green
CST preserve every source byte, recover useful local structure from malformed
Chimi, provide precise byte locations through a lightweight cursor, and remain
within the accepted initial syntax-performance envelope?

## Run

From the repository root:

```sh
go -C prototypes/lossless-parser run .
```

The first screen explains what to review. Browse fixtures with `n` and `p`, then
use `s` for source, `t` for the recovered CST, `d` for syntax diagnostics, and
`j`/`k` to scroll. The TUI shows actual parser results rather than proposed
workload definitions.

Each fixture is labeled as syntax-valid, invalid syntax rejected by this
parser, or syntax-valid source expected to be rejected later by semantic/type
checking. This prototype reports only syntax diagnostics; it does not simulate
diagnostics from compiler stages that do not exist yet.

Check all golden fixtures noninteractively:

```sh
go -C prototypes/lossless-parser run . --check
```

Run the syntax-relevant performance workloads:

```sh
go -C prototypes/lossless-parser run . --bench
```

## Prototype shape

The lexer emits exact source slices for identifiers, literals, punctuation,
comments, whitespace, newlines, and invalid text. The parser reads significant
tokens but emits all tokens through a flat event stream:

```text
START(kind) | TOKEN(index) | MISSING(expected) | FINISH
```

A separate sink folds those events into immutable green nodes and tokens.
Unexpected consumed input is nested beneath `ERROR` nodes. Expected but absent
syntax becomes a zero-width `MISSING_*` token, while diagnostics remain a
separate collection. Concatenating token text always reconstructs the input.

`core.Cursor` is a lightweight red view: it retains only the root and a child
path, computes absolute byte offsets from green text lengths, navigates to child
nodes, and can return to its parent without putting offsets or parent pointers
in the green tree.

The parser initially reparses whole files. The benchmark output therefore
records full-reparse behavior for editor edits but makes no reused-node claim.
Incremental subtree replacement remains a later optimization driven by the
accepted performance contract.

## Deliberate limits

- No semantic AST, name resolution, or type checking.
- No formatter or typed syntax-facade generator yet; those consume the accepted
  CST boundary in following tickets.
- Benchmarks report total parser allocation from Go's allocator, live retained
  heap after GC for a 64-file result set, and a separate structural green-tree
  estimate. These are prototype-host measurements rather than portable budgets.
- Cold process startup and semantic workloads remain in the broader performance
  contract and cannot be measured by this parser alone.
