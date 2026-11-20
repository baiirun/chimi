# Archived Linear issues: decisions and key logs

The Linear workspace hit the free plan's issue limit on 2026-11-16, so the
oldest closed CHI issues were archived on 2026-11-17 to make room. Archived
issues are not deleted: they stay searchable in Linear (turn on "show
archived") and can be restored. They carry the label `to-archive`, so
`label:to-archive` finds all 59 of them (CHI-1 to CHI-65, closed issues only). This file keeps the decisions and the
important log entries from each archived issue in the repo, so nothing depends
on Linear.

Almost all of these issues were migrated from the old prog tracker, so the
dates in the logs are from August 2026, when the early prototypes were built.
Several prototypes were written in Go before Rust was chosen as the compiler
host, and some early syntax choices were later changed. Where a decision has
since been superseded, the entry says so; the current rule always lives in
[decisions.md](../decisions.md), the [specification](../specification/), and
the ADRs. Each entry gives the CHI number, the old prog id, what the issue
decided or delivered, and where the lasting record lives.

## Selective failure handling (epic CHI-1, ep-24aa4a)

The epic set the semantics for handling some failures from an inferred,
open failure set while letting the rest bubble up. It kept automatic
bubbling, composable functions, and errors as ordinary values, and avoided
`?`, throw, raise, and retry-specific syntax. Retry stays ordinary function
composition.

- **CHI-2 (ts-c99c23) Selective handling over open failure rows.** One
  generic failure-set parameter means the same thing in both places: in
  `fn() -> A errors E`, `E` is the open set of failure variants, and in
  `fn(E) -> B`, the same `E` is the ordinary value union of those variants. No
  `FailureOf<E>` or wrapper enums. Capturing a failure turns it into an
  ordinary value, which is matched exhaustively; to pass unmatched failures on,
  a function writes `_ -> Failure(error)`. Shorthand for that forwarding is to
  be added only if real code shows it is repetitive.
- **CHI-3 (ts-c45b33) Caller-facing recovery form.** Byron's verdicts:
  recover at the call site by matching directly on a live outcome with `case`;
  selected `Failure(pattern)` arms remove those variants and the rest bubble.
  Reusable recovery is an ordinary function that takes a thunk and returns a
  thunk, so it composes with `|>`. No `Recovery.with` library API, no
  contextual partial functions, no new recovery syntax. When every original
  variant is handled, the original failure set is removed but failures raised
  by recovery arms remain. Prototype: branch `codex/prototype-selective-recovery`.
- **CHI-4 (ts-52159f) Inference and lowering.** Result failures = input
  failures, minus variants fully covered by unguarded `Failure` patterns, plus
  failures produced by the arms. Guarded arms do not subtract (a false guard
  lets the failure bubble). Live-outcome cases must cover success exhaustively
  but may omit failures. A `Failure` pattern for a variant the subject cannot
  produce is a compile error that names the variant and lists the real ones.
  Leftover failures after partial recovery are not warnings. Core has no
  Recover node: recovery lowers to an ordinary Match; an unmatched failure
  propagates unchanged; defects and cancellation bypass recovery. Failure-set
  generics are compile-time only, with no runtime wrapper or allocation.
- **CHI-11 (ep-05cd68)** is the implementation epic for the above; its
  children are listed under "Implementation" below.

## First connected compiler and the executable kernel (CHI-7 and older children)

CHI-7 (ep-703bbc) closed once source-to-CST, semantics, resolution, typing,
executable core, tooling, incrementality, native AOT, artifact reuse, and
direct integer lowering were all implemented. The prototype work that came
before it (children of the open map issues CHI-5 and CHI-6) recorded these
decisions:

- **CHI-15 (ts-08dd56) Lossless parser research.** Recommended a
  never-failing event parser feeding an immutable lossless green CST, a typed
  syntax facade, and a separate compact semantic representation; start with
  whole-file reparsing and handwritten recursive descent, and benchmark before
  adding incremental parsing. Research: docs/research/lossless-parser-architectures.md.
- **CHI-17 (ts-8ec777) Typed pipeline research.** Recommended that the typed
  core track success, expected failures, and requirements as separate inferred
  rows; providing dependencies and composing errors transform the rows; only a
  fully provided computation may run; defects and cancellation are separate
  runtime causes. Research: docs/research/typed-semantic-pipeline-precedents.md.
- **CHI-16 (ts-70701e) Compiler prototype foundation.** Byron named Rust's
  build latency as a core objection, so the foundation criteria separate the
  compiler's own speed from how fast it is to rebuild and iterate on.
  Shortlist was Rust with Rowan or cstree, TypeScript, and Go with custom
  green trees. The answer recorded on the issue: the prototype's host language
  is a tactical choice, not a Chimi design decision, and the lasting
  foundation is host-independent (lossless lexer, never-failing event parser,
  immutable lossless CST with a typed facade, compact semantic representation,
  resolved and typed core, and a separately designed runtime boundary).
- **CHI-18 (ts-ffa0aa) Golden source corpus.** Byron's reviews shaped the
  early language: functions are cold direct-style computations (no source
  `Computation<T>`, no `await`); nominal products are field-only `struct`
  declarations with implicit constructors and closed alternatives are `enum`
  with payloads; anonymous records are structural; blocks and case branches
  are expressions; case has guards and multiple subjects, and guards must be
  pure. Normal calls propagate failures into the caller's inferred error row;
  `Failure(error)` re-enters the failure channel. Struct fields and
  constructors are module-private by default; publishing a struct exposes only
  its type; representation visibility is all or nothing; no generated setters.
  `case` is the only branching expression: `if` appears only as a case-arm
  guard (still the rule in the specification).
- **CHI-19 (ts-39cad7) Names and modules.** The module namespace is never
  inferred from a file name. Plain `import` binds only the module; `as` sets an
  alias; `.{...}` member lists may import public types or enum constructors but
  not functions (diagnostics suggest `Module.function`); no wildcard imports.
  Module, type, and value namespaces are separate.
- **CHI-20 (ts-556783) CST-to-semantic lowering.** Lowering drops trivia and
  punctuation, keeps byte spans, keeps explicit error and needs channels inside
  function types while leaving omitted annotations for inference, normalizes
  pipelines to calls marked as pipeline origin, and represents malformed code
  as explicit Invalid nodes (an incomplete pipeline keeps its input rather
  than silently dropping the pipe).
- **CHI-21 (ts-6f54dc) Lossless parser and CST.** Accepted: lossless lexer,
  tree-independent event parser, immutable semantic-free green CST, zero-width
  missing tokens, ERROR recovery nodes, separate syntax diagnostics, and red
  cursor views. The Go prototype's memory overhead was treated as a layout
  problem, not a reason to reject the design.
- **CHI-22 (ts-7109ff) Kernel validation.** An integration audit over all the
  prototypes found the kernel coherent and ready to specify, but not yet an
  end-to-end compiler; the resolver-to-typing and typing-to-core handoffs were
  left as implementation work.
- **CHI-23 (ts-892f0b) Frontend performance contract.** Byron kept all ten
  workload scenarios (cold start, typical and large files, malformed braces,
  incomplete pipelines, prefix and middle edits, retained CST memory, local
  and high-fanout semantic edits). Targets: 16 ms p95 for syntax feedback and
  100 ms p95 for local semantic feedback on a recorded reference machine.
- **CHI-24 (ts-92334c) Formatting, diagnostics, inspection.** The formatter
  works on the lossless CST and refuses to rewrite syntax-invalid source.
  Diagnostics share one stage-aware, source-mapped envelope but keep syntax and
  type errors distinct. Cursor inspection joins the CST path with semantic
  facts through source spans.
- **CHI-25 (ts-929d55) Lexical and surface grammar.** Newlines separate
  top-level items, block items, declaration members, and case arms, and are
  soft inside delimited lists and continued contracts (no semicolons). Bare
  `{ name: value }` is an anonymous record and other braces are blocks; Byron
  rejected a `record { }` prefix. Empty braces are invalid. `Void` is written
  in success types, has no literal, and is distinct from `Option<T>`.
  *Superseded:* the early pipe rule (`x |> f(a)` means `f(a)(x)`, with APIs
  such as `Retry.with` returning unary functions) was replaced on 2026-11-11
  by "`|>` passes its left side as the first argument" (see decisions.md).
- **CHI-26 (ts-b47a2e) Types and operation contracts.** Contracts track
  success, variant-level expected failures, and named requirements
  separately; calls union the channels. Byron renamed the keyword `requires`
  to `needs`. `using` provides dependencies as a postfix expression and can
  discharge needs partially; a provider may narrow failures and needs but may
  not change inputs or success, or add channels. Closed enums are matched
  exhaustively with `_` or a binding as catch-all; a guarded fallback does not
  close the set; arms after a catch-all are unreachable; payload arity and
  struct fields are checked.
- **CHI-27 (ts-d7eaf3) Executable semantics and runner boundary.** Outcomes
  are Success, ExpectedFailure, Defect, and Cancelled; retry reruns only
  selected expected failures, never defects or cancellation. Every event
  carries a computation context that the runtime owns and propagates across
  child work, retries, and dependency calls; process and task labels are
  optional debugging names (BEAM-informed). Chimi needs no hot code upgrade; a
  future runtime may deploy a new version beside the old one and drain old
  work, without migrating live state. Research:
  docs/research/runtime-computation-event-context.md.
- **CHI-28 (ts-5073cc) Normative kernel specification.** Byron accepted
  docs/specification/executable-kernel.md. Generic parameters are bound
  explicitly (`fn map<A, B, E, R>`), with their kind inferred from use.
  Implementation failure rows are inferred; `errors` syntax constrains
  function-type values.
- **CHI-29 (ts-5c7c94) Decision inventory.** Aliases and newtype-like
  declarations are out of the first kernel (structs, enums, and records cover
  every program; a new named-data form needs a concrete use case). Chimi
  source does not call `Runtime.run`: a conventional root function closes its
  dependencies with `using`, and the toolchain or host supplies tracing,
  identity, cancellation, and resource policy. `Failure(error)` builds a
  failed outcome, ordinary expressions succeed implicitly, and stored Result
  data is `Result.Success` / `Result.Failure`. Inventory:
  docs/specification/executable-kernel-decision-inventory.md.
- **CHI-30 (ts-73855d) Representation handoffs.** Byron accepted
  docs/architecture/compiler-representation-handoffs.md: immutable snapshots
  with stable-ID side tables; `check` always yields a partial typed program
  for tooling; an opaque VerifiedRoot exists only when everything is valid
  and root needs are closed; lowering accepts only VerifiedRoot.
- **CHI-31 (ts-acb78f) Implementation roadmap.** Byron accepted
  docs/plans/first-compiler-implementation-roadmap.md (milestones M0 to M8,
  thin end-to-end slice first, backend choice as a separate evidence gate).
- **CHI-32 (ts-0846e5) Compiler host.** Rust is the bootstrap compiler host
  (ADR 0001). Go rebuilt faster, but Rust had better frontend latency, about
  a quarter of the peak memory, and a far smaller binary. Constraints: the
  compiler ships as one executable with no installed runtime, host details
  must not leak into compiler representations, and self-hosting is the
  eventual goal. Spike evidence: branch `codex/compiler-host-spike`.

### Building the compiler (milestones M1 to M7, children of CHI-7)

These logs record how each milestone landed. The code and tests are the
lasting record; the points worth keeping are below.

- **CHI-34 (ts-3ce62b) Thin source-to-execution slice (M1).** The first real
  path ran source through every representation (SourceSnapshot,
  SemanticProgram, ResolvedProgram, TypedProgram, VerifiedRoot, CoreProgram)
  to structured execution. Invariants kept since: stable compact IDs, exact
  source spans on origins, diagnostics owned by their stage, and only a
  VerifiedRoot may lower.
- **CHI-38 (ts-e87da7) Lossless frontend (M2).** Replaced M1's narrow parser
  with the accepted lossless lexer, event parser, green CST, and recovery.
  A fixed-seed suite of 2,048 malformed inputs checks termination, exact
  round trip, and bounded diagnostics. Syntax IDs are scoped to one parse
  snapshot and promise nothing across edits. On an Apple M1, typical files
  parsed in 1.25 ms p95 and 115 KB editor reparses in about 2.5 ms p95.
- **CHI-35 (ts-737964) Modules, names, declared generics (M3).** File
  modules, explicit imports, visibility, namespaces, stable symbols, and
  explicit generic binders; undeclared generics are rejected.
- **CHI-37 (ts-b79f17) Types, failures, needs, patterns (M4).** Body
  inference with fixed-point propagation across forward references,
  rank-one polymorphic function references, `using` as a provision scope with
  structural provider checks, real Success/Failure outcome semantics with
  selective removal of handled variants, full usefulness analysis for
  exhaustiveness and unreachable arms, and pure Bool guards. `E5002` fires
  only for unresolved needs on an exported `main`.
- **CHI-36 (ts-b70040) Seal, lower, execute (M5).** Retry is verified as
  ordinary polymorphic thunk composition with no syntax, compiler operation,
  or runtime intrinsic. `--root` project loading rejects missing modules,
  cycles, symlink escapes, and duplicate files. The kernel validator reached
  end-to-end-compiler = true.
- **CHI-33 (ts-1bf9f2) Formatter, diagnostics, inspection (M6).** `chimi
  format` (non-mutating; malformed input returns the original text plus
  syntax diagnostics), `chimi check --json` with the versioned
  `chimi.diagnostics.v1` envelope, and `chimi.inspection.v1` cursor
  inspection.
- **CHI-39 (ts-fa8494) Incrementality and performance (M7).** A
  CompilerSession reuses snapshots and invalidates by reverse imports. At 256
  modules: clean build 155 ms p95, no-op 0.2 ms, local edit 35 ms, fan-out
  edit 80 ms.

## Choosing the backend (children of CHI-8)

- **CHI-40 (ts-f75d35) Comparison harness.** An opaque BackendProgram, an
  interpreter oracle, and a five-workload JSON Lines harness (success,
  dependencies, retry exhaustion, defect, cancellation).
- **CHI-41 (ts-47988d) Native AOT spike.** Cranelift, one native function per
  Core node, linked with a static C runtime. Matched the oracle byte for byte;
  about 73 KB executables, under 2 ms cold start, about 1.4 MiB peak memory,
  only libSystem as a dynamic dependency. Results:
  docs/performance/native-aot-m8-spike.md.
- **CHI-42 (ts-8717b3) Embedded bytecode spike.** One opcode per Core node and
  a C dispatcher. Also exact; size, startup, memory, and speed came out
  effectively tied with native because the shared runtime dominated. Results:
  docs/performance/embedded-bytecode-m8-spike.md.
- **CHI-43 (ts-95a81c) Permanent execution architecture.** Native AOT with a
  linked Chimi runtime (ADR 0002); Cranelift is the first code generator;
  bytecode stays a possible future portability option. Tracing is chosen by
  the caller in three modes: off (a real fast path that builds no events),
  boundaries (computation lifecycle and dependency calls), and full (every
  operation).
- **CHI-44 (ts-a5f862) `chimi build`.** Per-module Cranelift objects plus an
  entry object and an embedded runtime archive, with a versioned build report
  and manifest.
- **CHI-45 (ts-c19aa2) Trace-off fast path** and **CHI-46 (ts-636302) trace
  cost.** Off skips all events but keeps terminal outcomes. Measured
  boundaries at 1.7 to 2.0 times off, and full at 17 to 21 times off; this fed
  the later decision that the 5x gate applies to boundaries only.
- **CHI-48 (ts-d31a46) Artifact reuse.** BLAKE3 content keys over everything
  that affects an artifact (Core, target, lowering revision, runtime ABI, C
  compiler); an unchanged rebuild took about 74 ms against 294 ms cold.
- **CHI-49 (ts-02d20b) Direct integer lowering.** Pure integer expressions
  lower straight to Cranelift SSA; trace-off time for the workload fell from
  12.7 us to 175 ns.
- **CHI-9 (ep-15f401)** and **CHI-47 (ts-fe05bc)** were canceled as
  duplicates of the backend epic and of CHI-44.

## The first useful CLI (epic CHI-10, ep-0738db)

The epic closed with a maintained Chimi program, a real host boundary,
deterministic trace aggregation, typed failures, optional observability,
bounded native memory, a single-executable build, exact golden outputs, and
recorded performance.

- **CHI-50 (ts-1c0afb)** defined `chimi-trace-summary` as the forcing program
  and filed every gap it exposed.
- **CHI-51 (ts-2ac2de) Trace data surface.** Byron accepted all six
  scenarios: a closed `Json.Value` with an explicit `Null`; decoding by
  ordinary pattern matching; an immutable `Map<K, V>` with unspecified
  enumeration order; scope-owned UTF-8 line processing with bounded buffering;
  JSON objects encoded with keys in ascending UTF-8 byte order; and failures
  never emit a partial summary. Spec: docs/specification/trace-data-and-processing.md.
- **CHI-52 (ts-41796d)** implemented std/json and std/map (persistent AVL).
- **CHI-53 (ts-65a567)** shipped programs/chimi-trace-summary, a 200 KB
  executable that links only libSystem.
- **CHI-54 (ts-8cec2f) Synchronous effects and host capabilities.** Accepted
  a separate inferred effect row with built-in `io`, with FileSystem and
  Console as caller-selected needs, IO failures as values, and tracing,
  cancellation, and limits as runner configuration. *Superseded:* ADR 0003
  and then ADR 0004 removed user-facing effect rows; needs stay.
- **CHI-58 (ts-5b0547)** implemented that effect row; it was later removed
  under CHI-72 (ts-4bdb73) and ADR 0004.
- **CHI-55 (ts-e2d9a9)** added `main(List<Text>)`, explicit std/io
  capabilities, bounded UTF-8 line streaming, typed host failures, and exit
  codes. **CHI-56 (ts-eaae0e)** separated application stdout and stderr from
  the versioned `chimi.runtime.v1` telemetry and outcome files, with tracing
  off by default.
- **CHI-57 (ts-da83af) Bounded native memory.** Callback-scoped reclamation
  held 100,000 records at about 30 KB peak. A later note (2026-11-10) warns
  that per-pass collection is unsafe when a pass hands a value to the
  scheduler (a channel send), because the collection scope was global across
  fibers; loops shipped with a bounded stack only until an escape rule
  existed.
- **CHI-59 (ts-445d99), CHI-64 (ts-ebdedf), CHI-65 (ts-67abf0) Allocation
  work.** Callback churn fell 75%; environment clones were 92% of remaining
  allocation and were cut 45%; dense environment slots (ABI 6) cut another
  63%. Output stayed byte-identical throughout, and an integration budget of
  under 110 KB per record guards regressions.

## Implementation of selective recovery (epic CHI-11, ep-05cd68)

CHI-60 (ts-4ff914, Core and runtime erasure checks), CHI-61 (ts-7b3fa5,
compiler fixtures), CHI-62 (ts-befdeb, failure-set row and value projection),
and CHI-63 (ts-ee3a96, documentation) implemented the CHI-2 to CHI-4 contract
with no runtime row wrappers and no recovery-specific Core operation. They
carry no log comments beyond their descriptions.
