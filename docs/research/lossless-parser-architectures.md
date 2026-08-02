# Lossless Parser Architectures for Chimi

Research date: 2026-08-01

## Question

Which parser and syntax-tree architecture best supports Chimi's requirements for canonical formatting, language-server tooling, useful trees for malformed input, lowering into semantic representations, and a high-performance compiler frontend?

## Recommendation

Adopt a **compiler-owned, Rowan/Roslyn-style syntax pipeline**, without yet committing to the Rowan library or a host language:

```text
source snapshot
  -> lossless lexer
  -> tree-independent event parser
  -> immutable green CST
  -> red cursor and generated typed syntax facade
  -> compact, source-mapped semantic representation
  -> resolved and typed core
```

The important decision is the boundary between representations, not a particular parser package:

1. The lexer classifies every source byte, including comments, whitespace, and invalid text.
2. The parser emits structural events and diagnostics rather than constructing a particular tree type directly. Parsing always returns a tree plus diagnostics; malformed input is not a failed parse.
3. The lossless CST is semantic-free, immutable, and scoped to one file. Its leaf text reproduces the original input exactly, including malformed input.
4. A lightweight cursor layer adds parents and absolute offsets without storing them in every immutable node. A typed syntax facade exposes convenient node APIs, but every accessor tolerates incomplete input.
5. Lowering discards punctuation and trivia, desugars surface forms, and produces a compact semantic representation with explicit links back to source spans. Name resolution and types live here or later, never on CST nodes.
6. Batch compilation may release CSTs after lowering. Editor sessions may retain green roots and reuse unchanged subtrees.

Start the prototype with handwritten recursive descent for declarations and patterns plus Pratt parsing for expressions. Emit a flat event stream into a tree sink so the parser algorithm and tree storage can change independently. This follows rust-analyzer's current architecture, where the parser is independent of token and tree representations and returns a tree with a list of errors rather than failing ([rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html#cratesparser)). Revisit the algorithm if Chimi's eventual grammar exposes ambiguity or recovery cases that make a generated parser materially better.

For the first CST prototype:

- Represent ordinary trivia as explicit leaves. This gives the simplest auditable round-trip invariant; compact leading/trailing trivia storage can be benchmarked later.
- Preserve consumed but unexpected input beneath `ERROR` nodes.
- Permit zero-width `MISSING` tokens for expected syntax that recovery inserts, while keeping detailed diagnostics in a separate collection. Both Tree-sitter and Roslyn use queryable missing syntax to describe incomplete programs ([Tree-sitter error nodes](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html#the-error-node), [Roslyn syntax model](https://learn.microsoft.com/en-us/dotnet/csharp/roslyn-sdk/work-with-syntax#syntax-trees)).
- Generate the typed syntax facade from a small declarative CST schema instead of hand-maintaining parallel node definitions. rust-analyzer uses this approach because the useful programmatic tree shape and the grammar needed by a parser are not necessarily the same ([Ungrammar rationale](https://rust-analyzer.github.io/blog/2020/10/24/introducing-ungrammar.html)).

Design immutable subtree replacement and sharing into the CST API now, but initially reparse whole files after edits. Add region-based incremental parsing only when measurements justify its complexity. rust-analyzer supports heuristic block reparsing yet reports that full reparsing is often already fast enough for IDE workloads ([rust-analyzer incremental reparse](https://rust-analyzer.github.io/book/contributing/syntax.html#incremental-reparse)).

## Precedents

### rust-analyzer and Rowan

This is the closest architectural match for Chimi. Its syntax trees preserve whitespace and comments, remain lossless for invalid input, contain no name-resolution or type information, and are resilient enough to recover multiple fragments from malformed source ([syntax design goals](https://rust-analyzer.github.io/book/contributing/syntax.html#design-goals)). The tree is split into immutable green storage, cursor-like syntax nodes with parent and offset context, and typed wrappers over raw syntax nodes. The frontend then lowers syntax into a more compact, non-full-fidelity representation ([tree overview](https://rust-analyzer.github.io/book/contributing/syntax.html#overview), [typed syntax facade](https://rust-analyzer.github.io/book/contributing/syntax.html#ast)).

Its parser emits start-node, finish-node, token, and error events through a tree sink. That separation is valuable for Chimi because parser recovery, CST storage, and lightweight indexing can evolve independently. The syntax layer is also deliberately independent from the language server and semantic database, allowing syntax-only tooling to remain available when a project does not build ([rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html#cratessyntax)).

Rowan shows concrete performance techniques rather than a requirement to use its exact implementation: compact immutable nodes, text lengths rather than absolute positions in green nodes, structural sharing, and transient cursor views. Its own documentation describes it simply as a generic lossless syntax-tree library ([Rowan crate](https://docs.rs/rowan/latest/rowan/)).

The limitation is that rust-analyzer's incremental reparsing is heuristic rather than a general incremental parsing algorithm. That is acceptable for Chimi's first prototype and is evidence against paying for sophisticated incrementality before benchmarks demand it.

### Roslyn

Roslyn validates the same broad architecture at compiler-and-IDE scale. Its immutable syntax trees are full fidelity: nodes, tokens, whitespace, comments, directives, skipped input, and missing tokens can reproduce the parsed source. New trees reuse unchanged internal nodes, and immutability makes snapshots safe to analyze concurrently ([Roslyn syntax model](https://learn.microsoft.com/en-us/dotnet/csharp/roslyn-sdk/work-with-syntax#syntax-trees)). `WithChangedText` performs an incremental parse for small changes and otherwise falls back to a full parse ([`SyntaxTree.WithChangedText`](https://learn.microsoft.com/en-us/dotnet/api/microsoft.codeanalysis.syntaxtree.withchangedtext)).

Roslyn also exposes syntax, symbols, binding, diagnostics, and emission as distinct compiler phases. Formatting uses syntax trees while navigation and other features use symbols or semantic models ([compiler API model](https://learn.microsoft.com/en-us/dotnet/csharp/roslyn-sdk/compiler-api-model#compiler-pipeline-functional-areas)). That supports Chimi's decision to share one syntactic truth between the compiler and tooling while keeping semantic state outside the CST.

Roslyn is a precedent, not a size target. Its public object model and mature incremental machinery are much larger than the first Chimi kernel needs.

### Tree-sitter

Tree-sitter is the strongest alternative when incremental editor parsing is the primary goal. It is a generated GLR parser designed to update a concrete syntax tree on every keystroke and return useful structure despite errors ([Tree-sitter introduction](https://tree-sitter.github.io/tree-sitter/)). Given an edited old tree, reparsing creates a new tree that shares unchanged structure; trees are cheap to copy across threads ([incremental editing](https://tree-sitter.github.io/tree-sitter/using-parsers/3-advanced-parsing.html#editing), [concurrency](https://tree-sitter.github.io/tree-sitter/using-parsers/3-advanced-parsing.html#concurrency)). Its `ERROR` and zero-width `MISSING` nodes are a useful recovery model.

It is not the best canonical frontend for Chimi's first kernel. Tree-sitter's grammar directly controls the runtime CST shape, while Chimi benefits from independently evolving its recognition grammar and its tooling API. Tree-sitter also treats whitespace as an `extra` by default, and grammar authors decide which extras become visible rules ([grammar DSL](https://tree-sitter.github.io/tree-sitter/creating-parsers/2-the-grammar-dsl.html), [writing a Tree-sitter grammar](https://tree-sitter.github.io/tree-sitter/creating-parsers/3-writing-the-grammar.html#using-extras)). The documented contract emphasizes nodes and source ranges, not a self-contained, byte-for-byte transformation tree. **Inference:** using it canonically would therefore require Chimi-specific fidelity and lowering conventions. This does not make Tree-sitter unsuitable for Chimi tooling.

Tree-sitter should therefore be a performance and recovery benchmark, and possibly a later editor-compatibility grammar, rather than a second source of truth beside the compiler parser.

### SwiftSyntax

SwiftSyntax corroborates the value of one source-accurate tree feeding tooling, formatting, and compiler lowering. Swift's parser project tests that syntax nodes round-trip every input byte even for broken source, models unknown, unexpected, and missing syntax as errors, is used by `swift-format`, and lowers syntax nodes into the compiler AST through ASTGen ([Swift parser update](https://forums.swift.org/t/update-on-the-new-swift-parser/60470)). This is especially relevant to Chimi's combination of canonical formatting, malformed-source tooling, and a separate semantic tree.

## Performance contract for the prototype

"Fast" must be split into independently measured workloads:

- Cold lex-and-parse throughput for valid and malformed files.
- Peak bytes allocated per source byte for the green CST.
- CST traversal and CST-to-semantic lowering time.
- Full-file reparse latency after representative one-character edits.
- Incremental reparse latency and reused-node ratio if an incremental strategy is added.
- End-to-end editor latency from text edit to updated syntax diagnostics.

The golden corpus should assert these invariants before optimization:

1. Concatenating real CST leaves exactly reproduces every input byte.
2. Parsing never aborts because of a syntax error and always consumes the input.
3. Every error and missing marker has a valid source location.
4. Lowering valid source is deterministic; malformed syntax produces bounded diagnostics rather than a crash.
5. Full and incremental reparses, if both exist, produce structurally equivalent CSTs and diagnostics.
6. Formatting valid source is idempotent and preserves comment contents.

The executable workload contract lives in
[`prototypes/frontend-performance`](../../prototypes/frontend-performance/README.md).
Its provisional 16 ms syntax and 100 ms local-semantic targets describe the
desired editor experience on a recorded reference machine. Portable throughput
and memory thresholds must wait for the implementation language, benchmark
hardware, and measured native prototypes rather than importing another
project's numbers.

## Prototype evidence

The disposable [`prototypes/lossless-parser`](../../prototypes/lossless-parser/README.md)
implements the recommended event-parser, green-tree, and red-cursor boundaries
in Go. Its current corpus checks establish that:

- all 23 golden and focused recovery fixtures reproduce their input byte for
  byte and have internally consistent cursor offsets and parent paths;
- missing record punctuation, missing record values, incomplete pipelines, and
  missing closing braces each recover with one localized diagnostic while
  preserving the remaining source;
- full reparses of representative 57–115 KB editor workloads remain below the
  provisional 16 ms p95 target on the recorded `darwin/arm64` Go 1.25.5 host;
- retained parse results currently occupy about 26.5 source bytes per input
  byte, while the green-structure model alone estimates about 24.2 bytes per
  input byte; and
- total transient allocation is roughly 300 bytes per input byte, exposing
  allocation churn as a primary optimization question even though latency is
  currently acceptable.

These measurements validate the representation boundaries, not the current
Go object layout. Compact kind identifiers, packed child storage, arena-like
construction, and discarding lexer/parser intermediates after the sink are the
obvious comparisons for a production-oriented frontend prototype.

## Remaining uncertainties

- **Prototype foundation:** host language and whether to reuse Rowan, build a smaller green tree, or wrap another library.
- **Recovery representation:** when a missing construct should be absent, represented by a zero-width token, or represented by a larger recovery node.
- **Brace disambiguation:** bare anonymous records and expression blocks share `{ ... }`. The real parser prototype should test malformed and incomplete forms such as `{ name`, `{ name:`, and `{ expression` for recovery quality, diagnostic stability, and measurable parser cost before treating the contextual lookahead as settled.
- **Trivia ownership:** explicit trivia leaves versus compact leading/trailing trivia on tokens; both can be lossless but have different traversal and formatter costs.
- **Incremental granularity:** full-file, delimiter-bounded subtree, or general incremental parsing. This should follow edit-latency measurements.
- **Stable identity:** whether semantic queries need node identity that survives edits, rather than file-and-range source pointers.
- **Source encoding:** whether source is guaranteed UTF-8 and whether all offsets are bytes, scalar values, or both.
- **Formatter recovery:** whether canonical formatting rejects malformed files, formats only intact regions, or formats a recovered tree.

None of these uncertainties blocks the recommended representation boundaries. They are the measurements and design choices the parser, lowering, formatting, and performance prototype tickets should resolve.
