# Standard Modules, Module Constants, and Text Rendering

## Status

Normative contract for `prog show ts-5e95b8`. It covers gaps G1, G7, G9, G10,
and G20 from the [standard library map](../plans/standard-library-map.md) and
resolves the module-constants decision `prog show ts-f47931`. Module
constants (G7) are implemented, except result caching, the build report beyond
the evaluation budgets (which `chimi.build.v1` records as `constant_limits`),
and `process name` (which the language does not have yet). Text interpolation (G9)
is implemented on the existing intrinsics (migration step 2). Debug rendering
(G10) is implemented: `std/debug` declares `render`, `render_indented`, and
`crash` in source (step 5) over the existing intrinsics; the runner's
bounded rendering is not, and the one registry render override
(`Duration`, as canonical text) is hand-built in both renderers. The standard tree (G1)
is implemented for `std/option`, `std/int`, `std/text`, `std/list`, and
`std/order` (migration steps 3 and 4, `ts-0ba63e`): the embedded tree, `native
fn`, the registry in `operations.rs` with runtime-known declarations, and the
`std-tests` harness, `--std-dir`, and `chimi std check`. `native type`, the
generated `operations.h`, and moving interpolation to `std/text.concat` are not, and C
bindings (G20) are not implemented. Source spelling is provisional, as
in the other specifications.

The words MUST, MUST NOT, SHOULD, and MAY are normative. Examples use
positional construction and calls (`ts-835fc0`).

## Goals

- Write the standard library as ordinary `.chimi` source. Keep the native core
  small, and let application code use none of it directly.
- Make adding a standard function one source edit when it needs no runtime
  support, and three cross-checked edits when it does.
- Ship the standard library inside the compiler and the runtime inside the
  executable, so a build needs no installed package tree.
- Give modules exported values that the compiler evaluates once, with no
  program initialization phase, no initialization order, and no failure at
  startup.
- Build messages without `Text.append` chains, using syntax a reader of
  JavaScript, Kotlin, or Dart already knows.
- Render any value as text for assertions, defect context, and debugging,
  without leaking authority or inventing a trait system.

## Example

<!-- chimi: check -->
```chimi
// std/duration.chimi
pub struct Duration {
  nanoseconds: Int
}

pub const zero = Duration(0)

pub fn seconds(count: Int) -> Duration {
  Duration(count * 1_000_000_000)
}
```

<!-- chimi: check -->
```chimi
// app/worker.chimi
import std/duration as Duration
import std/debug as Debug

const workers = 8
const job_timeout = Duration.seconds(30)

pub fn describe_status(status: Int) -> Text {
  "payments returned ${status} after ${Debug.render(job_timeout)}"
}
```

`describe_status(503)` returns
`payments returned 503 after 30s`, because `Duration` has a registry render
override (see [Debug rendering](#debug-rendering-g10)).

## Standard modules in Chimi (G1)

### Source tree and embedding

- The standard library is a tree of `.chimi` files in the compiler repository
  at `compiler/chimi/std/`. The file `compiler/chimi/std/list.chimi` defines
  the module `std/list`.
- The compiler MUST embed every file of that tree in its own executable at
  compiler build time. The standard library is versioned with the compiler.
  A compiler release has exactly one standard library, and the build does not
  read it from the filesystem.
- The project loader MUST resolve an import whose path starts with `std/`
  against the embedded tree, and every other import against the workspace.
- A workspace file whose module path starts with `std/` is an error (`N3102`).
  The prefix belongs to the compiler.
- The compiler MUST load only the standard modules that the build imports,
  directly or transitively.
- A compiler developer MAY pass `--std-dir <path>` to read the standard tree
  from disk instead. The build report MUST say that it used an override.
  No environment variable changes where the standard library comes from.
- Diagnostics, inspection, and go-to-definition MUST show standard source with
  real spans. Editors receive it as read-only documents named
  `chimi-std:///std/<path>.chimi`.
- A module under `std/internal/` MAY be imported only by other standard
  modules (`N3103`). It holds shared native cores that are not public API.

The native backend links only reachable Core nodes, so an executable contains
the standard functions it calls and nothing else.

### The prelude

Every module sees a fixed set of names without an import:

- the core types `Int`, `Text`, `Bool`, `List`, and `Void`, which have literal
  or keyword forms and remain compiler built-ins;
- the `Option` type and its `Some` and `None` constructors, declared as an
  ordinary enum in `std/option`. The declaration lists `None` before
  `Some`, so derived ordering puts `None` first;
- the outcome constructors `Success` and `Failure`, which are language forms.

The prelude MUST NOT bind any function or module qualifier. The built-in
`List`, `Text`, and `Int` namespaces are removed. Code that calls
`List.length` imports `std/list as List`, as Principle 4 requires for every
module qualifier.

### Native declarations

A standard module implements runtime-backed behavior with two declaration
forms. Each is an ordinary declaration with the `native` modifier and no body:

<!-- chimi: pending ep-a6f138 ts-facbbe -->
```chimi
// std/map.chimi
pub native type Map<K, V>

pub native fn empty<K, V>() -> Map<K, V>
pub native fn get<K, V>(map: Map<K, V>, key: K) -> Option<V>
pub native fn put<K, V>(map: Map<K, V>, key: K, value: V) -> Map<K, V>
native fn reduce_entries<K, V, S, F, R>(
  map: Map<K, V>,
  initial: S,
  step: fn(S, K, V) -> S errors F needs R,
) -> S errors F needs R

pub fn update<K, V, F, R>(
  map: Map<K, V>,
  key: K,
  change: fn(Option<V>) -> V errors F needs R,
) {
  put(map, key, change(get(map, key)))
}

pub fn size<K, V>(map: Map<K, V>) -> Int {
  reduce_entries(map, 0, fn(count, _key, _value) { count + 1 })
}
```

Rules for `native fn`:

- Its **operation id** is the qualified name of the declaration, such as
  `std/map.put`. Source does not spell the id, so a declaration cannot bind
  the wrong operation.
- It MUST annotate every parameter and the success type. It MUST state its
  complete `errors` and `needs` rows, because there is no body to infer them
  from (`T4320`). It is the only function header that writes an `errors`
  row.
- It MAY be generic, MAY take function values, and MAY declare failure-row and
  needs-row generics. A native operation that calls a function argument
  composes that function's failures and needs, as `Map.update` does today.
- It MAY be private. A module SHOULD keep a native operation private when a
  Chimi wrapper can validate arguments or give a better contract.
- It MUST NOT have a body (`S1102`).

Rules for `native type`:

- It declares an opaque nominal type with a runtime-owned representation. Its
  id is its qualified name, such as `std/map.Map`.
- It has no fields and no constructors. Values come only from native
  operations.
- Its concurrent value property (shareable, transferable, exclusive, or
  parent-local) comes from the operation registry. Inspection and generated
  documentation MUST show it. For example, `Map<K, V>` is shareable when `K`
  and `V` are, and `std/channel.Receiver<T>` is exclusive.

The registry MAY mark a std type that the runtime knows, either a `native
type` or a runtime-known declaration, with two more properties:

- **Constant-representable.** The type's values are inert and shareable: they
  hold no runtime resource and act on nothing until a runtime operation
  receives them. A constant MAY hold such a value. `Duration`,
  `Process.Spec`, `Supervisor.Member`, `Supervisor.Policy`, and
  `Supervisor.Restarts` carry this mark. A task handle, channel endpoint, or
  direct process address never does.
- **Render override.** Debug rendering uses a registry-named function instead
  of the generic format. `Duration` renders as its canonical text, such as
  `30s`.

Everything else in a standard module is ordinary Chimi: structs, enums,
interfaces, constants, and functions. Error enums such as `FileError` and data
types such as `Json.Value` MUST be declared in source, not built in.

### Who may declare native items

- A module from the embedded standard tree MAY declare `native` items.
- A vendored module MAY declare them only under the
  [native bindings](#native-bindings-for-std-and-vendored-modules-g20) rules.
- Any other module that declares a `native` item is an error (`N3101`).

Application code therefore cannot reach a runtime entry point except through a
standard or vendored function that someone wrote and reviewed. This keeps
Principle 7: no unsafe keyword, package, or unchecked operation exists for
ordinary code.

### The operation registry

The compiler holds one operation registry, in
`compiler/chimi/src/operations.rs`. Each entry records:

- the operation id;
- its arity and the runtime representation of each argument, for a cheap
  consistency check against the source signature;
- its class:
  - `pure`: deterministic and creates no runtime resources, for example
    `std/list.get`, `std/text.concat`, and `std/debug.render`;
  - `runtime`: needs the scheduler or creates runtime resources, for example
    `std/task.start` and `std/channel.bounded`;
  - `host`: interacts with the operating system, for example
    `std/io.write_stdout`;
- the interpreter implementation, a Rust function over runtime values;
- the native runtime symbol, `chimi_op_` followed by the id with each `/` and
  `.` replaced by `_`, such as `chimi_op_std_map_put`.

The registry also lists **runtime-known declarations**: source declarations
the runtime constructs or inspects, such as `std/io.FileError.Open` and
`std/option.Option.Some`. The compiler MUST check that each one exists with
the expected payload arity, and MUST give the runtime its constructor id
through the program data.

The registry is the single source of truth. The compiler MUST verify all of
these during every build that loads the affected module, and the standard
test suite MUST verify them for the whole tree:

- every `native fn` names a registered operation (`N3104`);
- every registered operation has exactly one `native fn` declaration;
- the declared arity matches the registry;
- every runtime-known declaration exists with the registered shape (`N3105`).

`chimi std check` runs the whole-tree verification without building a
program.

### Host operations stay behind providers

A `host` operation MUST be private (`N3106`). Its module exposes it only
through a private constant that holds a provider record:

<!-- chimi: pending ts-975ca3 ep-a6f138 -->
```chimi
// std/io.chimi
pub interface Console {
  write_line: fn(Text) -> Void errors ConsoleError
  write_error: fn(Text) -> Void errors ConsoleError
}

pub enum ConsoleError {
  WriteFailed(stream: Text, message: Text)
}

native fn write_stdout(line: Text) -> Void errors ConsoleError
native fn write_stderr(line: Text) -> Void errors ConsoleError

const system_console = {
  write_line: write_stdout,
  write_error: write_stderr,
}
```

The runner's root provider plan names `std/io.system_console` by qualified
identity. No source outside `std/io` can name it, so importing `std/io` grants
no authority, and every host interaction still flows through `needs`.

### Mapping to the interpreter and the native runtime

- Resolution gives a `native fn` symbol the kind `Intrinsic` carrying its
  operation id. Executable Core keeps that kind. Core is otherwise unchanged:
  a call to a native operation is an ordinary call node.
- The reference interpreter dispatches an intrinsic call through the
  registry's interpreter implementation.
- The native backend gives each registered operation a dense index. The
  generated header `compiler/chimi/runtime/native/operations.h` declares the C
  symbols in index order. A compiler test MUST fail when the header is out of
  date. Program data stores the index, and `CHIMI_SYMBOL_INTRINSIC` calls
  through the runtime's operation table.
- The capability checker identifies special operations by operation id, for
  example `std/task.start`, instead of by a `StandardFunction` variant.

### What a new standard function costs

| Change | Edits |
| --- | --- |
| A function written in Chimi | One: the `.chimi` source |
| A new primitive | Three: the `native fn` declaration, the registry entry with its interpreter implementation, and the C implementation. The build checks each against the others. |
| A new runtime-known enum | The source declaration plus one registry line |

This replaces today's five edits to `standard.rs`, `resolution.rs`,
`typing.rs`, `execution.rs`, and `runtime.c`. Hand-built signatures in
`typing.rs` and the `$`-named generics in `StandardGeneric` disappear,
because signatures come from source.

### Testing the standard library before a test runner exists

Until `test` declarations exist (G16), the standard library uses
expected-output programs:

- Each standard module has a directory `compiler/chimi/std-tests/<module>/`.
  Each `.chimi` file there is a program with `pub fn main()`, and a sibling
  `.expected` file holds its exact standard output and terminal outcome.
- A Rust integration test runs every program through both the interpreter and
  the native backend. It MUST fail when either output differs from the
  expected file, or when the two backends differ from each other.
- The same test MUST fail when a registered operation is reached by no test
  program, so every primitive has differential coverage.
- Programs print values with `Debug.render`, so an expected file is readable
  without per-type printing code.

When G16 lands, each program converts to `test` declarations in the same
directory, and the coverage rule carries over.

## Native bindings for std and vendored modules (G20)

Design only. Nothing in the first standard port depends on it.

### Form

A C binding is a `native fn` or `native type` whose implementation lives in C
source owned by the declaring package, not in the compiler's registry. The
declaration form is the same, so callers cannot tell a C-backed operation from
a runtime one.

### Who may declare bindings

- The standard tree MAY ship C bindings, for example `std/crypto` over a
  vendored hashing library.
- A **vendored module** is a module of a package that the workspace manifest
  lists as vendored and grants `native`. Packages and manifests are gap T5,
  so until T5 exists only the standard tree can declare bindings.
- Application modules MUST NOT declare bindings (`N3101`), and a grant MUST
  NOT apply to a workspace's own application modules.

### Linking is pay-for-play

- A binding package lists its C sources, include directories, and any
  operating-system libraries it requires.
- The native backend compiles each binding package into its own static archive
  with the same `cc`, caches it by the hash of its sources and flags, and adds
  it to the link only when Core reaches one of its operations from `main`.
- Vendored C source is preferred over system libraries. When a required
  operating-system library is linked dynamically, the build report MUST list
  it, as Principle 15 requires.
- An executable that reaches no binding links no binding code.

### Safety boundary

C code is trusted code. Chimi cannot contain a C fault, so the boundary limits
what reaches C and what C may do:

- Binding code MUST use only the binding API in
  `compiler/chimi/runtime/native/chimi_binding.h`. That API builds and reads
  Chimi values, returns success, an expected failure, or a defect, checks
  cancellation, and runs blocking calls on a blocking pool so that scheduler
  threads never block.
- A foreign resource is a `native type` value backed by a runtime-owned handle
  with a release function. Chimi code never sees an address, a length paired
  with a pointer, or a raw descriptor.
- Binding code MUST NOT keep a pointer to a Chimi value after the call
  returns, and MUST NOT call Chimi code except through the binding API.
- A binding operation is always class `host` or `runtime`. The compiler never
  runs it at build time, and the tracing runtime records each call as a host
  boundary.
- The reference interpreter cannot call C. A program that reaches a binding
  operation under the interpreter fails setup with `E5204`, which names the
  operation. Running such programs in the interpreter would need a later
  shared-library bridge.
- A C fault such as a segmentation violation ends the whole process. It is
  not a Chimi defect. The build report lists every linked binding package so
  that this risk is visible.

## Module constants (G7, `ts-f47931`)

### Decision

A module exports values with `const`. The compiler evaluates every constant at
build time and stores the result in the executable's program data. Module-level
`let` is removed.

Three designs were compared:

| Design | Failure or defect during initialization | Order and cycles | Concurrency | Startup |
| --- | --- | --- | --- | --- |
| `pub let`, evaluated lazily at first reference | Happens in whichever task touched it first; later readers need a poisoned state | Discovered at run time | Needs a once-cell with synchronization on every read path | Cost spread over first uses |
| `pub let`, evaluated eagerly before `main` in dependency order | The program fails before `main` with no caller to handle it | Needs an init order across modules, as in Go | Safe after init | Grows with every constant |
| `const`, evaluated by the compiler | A compile error at the constant | Found by the compiler with the full path | Values are immutable data in the executable | None |

Build-time evaluation wins on every row. Its cost is that initializers must be
deterministic and need nothing from the environment, which is what a
module-level value should be under Principle 4. A different keyword marks the
different rule: a block `let` runs every time its block runs, while a `const`
runs once, when the program is built.

### Syntax and visibility

<!-- chimi: fragment -->
```chimi
pub const max_attempts = 5
pub const default_backoff: Backoff = Backoff(Duration.seconds(1), 2, Duration.seconds(30))
const greeting = "hello"
```

- `const` is valid only at module level. Inside a block it is an error
  (`S1103`). A module-level `let` is an error (`L2101`), and the diagnostic
  carries an edit that replaces `let` with `const`.
- A constant is private unless marked `pub`. A public constant is a value
  outside its module and MUST be used qualified, as `Retry.max_attempts`.
  A member import MUST NOT bind a constant, by the same rule that forbids
  binding functions.
- The type annotation is optional. The inferred type MUST be fully determined:
  a constant cannot be generic, so `const nothing = None` is an error
  (`T4324`) and needs an annotation such as `const nothing: Option<Int> = None`.

### Allowed initializers

An initializer is any expression that satisfies all of these rules:

1. Its failure row is empty (`T4321`). A constant cannot fail, because no
   caller exists to handle the failure.
2. Its needs row is empty (`T4322`). A constant cannot depend on a provider,
   because no `using` boundary encloses it.
3. Its type is **constant-representable** (`T4323`): a shareable value whose
   every reachable part is `Int`, `Float`, `Decimal`, `Text`, `Bool`, `Void`,
   a `Range`, a struct, enum, record, tuple, list, set, or map of such parts, a
   named process address, a std type that the registry marks
   constant-representable, or a function value whose captures and `using`
   providers are themselves constant-representable. A task handle, channel
   endpoint, direct process address, or other runtime capability is not
   constant-representable. The diagnostic names the field or capture that
   prevents it.
4. Its evaluation calls only `pure` operations (`E5201`). The diagnostic names
   the operation and the call chain that reached it.

Rule 4 is the restricted context that the host-capabilities contract requires
any such restriction to justify. The justification is that build-time
evaluation has no scheduler and no operating system to act on. Rules 1 to 3
catch most violations at type checking. Rule 4 catches code that creates and
discards a runtime resource, such as a channel built inside a block.

A constant MAY call any function, including functions from other modules and
other constants, as long as the evaluation obeys the rules.

### Evaluation

- The compiler MUST evaluate every constant in every module that the build
  loads, including constants that nothing references, so a broken constant
  fails the build whether or not it is used.
- Evaluation uses the reference interpreter on executable Core. It is
  deterministic: the same source and compiler produce the same value, so
  builds stay reproducible.
- A defect during evaluation is a compile error (`E5202`). It reports the
  defect message, the constant, and the span of the operation that failed.
- Evaluation of one constant MUST stop after 50 million executed Core
  operations, at a call that would make more than 10 000 function and closure
  calls active at once, or at a value larger than 16 MiB serialized, whichever
  comes first (`E5203`). A constant that another constant's evaluation reaches
  starts its own budgets. These limits are provisional and are recorded in the
  build report.
- The serialized size of a value is what the native table stores for it: one
  24-byte `CHIMI_CONSTANT_VALUES` row, 4 bytes of `CHIMI_IDS` per part it refers
  to (8 for a named or keyed part), the bytes of its text, and its parts. A part
  that a value refers to twice is charged twice, because the table and the
  interpreter's loaded copy hold a copy per reference. Evaluation measures every
  value it copies out of a binding, so a value that doubles each step stops near
  the budget instead of being built first; the final value is measured again,
  and a constant whose value is too large is not stored.
- A constant whose evaluation reaches itself, directly or through functions
  and other constants, is a cycle (`E5205`). The diagnostic lists every
  constant and function on the path. Constants that only name each other, as
  in `const a = b` and `const b = a`, never get a type, so type checking
  reports the cycle (still `E5205`) instead of evaluation.
- Each broken constant reports one diagnostic. A constant that fails only
  because a constant it uses failed reports nothing more.
- Results are cached by the hash of the constant's transitive Core and the
  compiler version. Standard constants are cached once per compiler.

### At run time

- A reference to a constant loads its value. There is no initialization phase
  before `main`, no ordering between modules, and no failure at startup.
- Constants are visible from every task, process, and restarted incarnation,
  and are the same value in each.
- Chimi has no reference identity, so the compiler MAY share one stored value
  or inline copies of a small one. Neither choice is observable.
- A constant reference produces no trace events beyond those a literal would
  produce. Its evaluation happened at build time and emits no runtime events.

### Native program data

- The native backend MUST emit each constant as statically initialized data in
  `program_data.c`: the table `CHIMI_CONSTANT_VALUES`, whose entries are
  scalars, text as ids into the read-only string table, lists and maps as
  static arrays of entry ids, and nominal values with their constructor ids.
  Children precede their parent.
- The runtime MAY decode the table into runtime values before the entry runs.
  Decoding cannot fail, emits no events, and costs time linear in the stored
  data, so it is not an initialization phase in the sense above.
- Static values are immortal. Reclamation scopes and any future collector MUST
  NOT free them, and a value built at run time MAY point to them. Decoded
  values are allocated outside every reclamation scope for this reason.
- A function value in a constant stores the declared function's symbol, or
  for a closure its `Closure` node and its captured constant values. A function
  configured with `using` also stores each provider as a requirement name and a
  constant value, which a call sees as if the `using` boundary enclosed it.
- `CHIMI_SYMBOL_BINDING` and the interpreter's `CoreSymbolKind::Binding`, which
  re-evaluate an expression on each reference, are replaced by a constant
  symbol kind that loads the stored value.

### Constants and `process name`

- A `process name` declaration stays a separate declaration. Its identity is
  the declaration itself, and registration requires that identity statically.
- A named address is constant-representable. A constant MAY hold one, or a
  struct or list of them:

  <!-- chimi: fragment -->
  ```chimi
  pub process name primary_cache: CacheMessage
  pub process name backup_cache: CacheMessage

  pub const caches = [primary_cache, backup_cache]
  ```

  In program data, a named address is the declaration's id.
- `Process.register(spec, name)` still requires a direct reference to the
  declaration. Passing a constant that holds one is still `T4262`.
- A direct address comes from a running incarnation, so it is not
  constant-representable.

### Migration of existing module bindings

Every module-level `let` in the corpus today holds a pure value: a provider
record of functions, a configured function, or data. The `L2101` edit
replaces `let` with `const`, and a one-time codemod applies that edit across
`prototypes/` and `programs/`. The formatter does not rewrite it. Most
bindings need nothing more. A binding still fails when its initializer breaks
an initializer rule: a type that is not fully determined needs an annotation
(`T4324`), and a value that is not constant-representable (`T4323`) or that
calls a non-`pure` operation (`E5201`) moves into a function.

The common `T4324` case is a fake provider such as
`const fake = { read: fn(path) { 40 } }`. A module-level `let` let a later
`using` decide the type of `path`; a constant is not generic, so only its own
initializer and annotation decide it, and the parameter needs an annotation:
`fn(path: Text) { 40 }`.

## Text interpolation (G9)

### Syntax

A text literal MAY contain interpolations written `${expression}`:

<!-- chimi: run "-L 7,7 / git blame a.txt exited 128: fatal" -->
```chimi
fn blame_argument(line: Int) -> Text {
  "-L ${line},${line}"
}

fn failure_message(path: Text, code: Int, stderr: Text) -> Text {
  "git blame ${path} exited ${code}: ${stderr}"
}

pub fn main() -> Text {
  "${blame_argument(7)} / ${failure_message("a.txt", 128, "fatal")}"
}
```

- `${` opens an interpolation and the matching `}` closes it. The lexer
  counts braces and lexes nested text literals, so
  `"names: ${Text.join(names, ", ")}"` is valid once `std/text` provides `Text.join` (phase 2).
- `\$` writes a literal `$`. A `$` not followed by `{` is literal text.
- An interpolation MUST contain an expression (`S1101` for `${}`). Text
  literals cannot span lines, and an interpolation MUST NOT contain a line
  break (`S1101`).
- A text pattern is a literal and MUST NOT interpolate. `"a${x}" -> ...` in a
  case arm is one `S1101` for the whole pattern; match the text with a binding
  and a guard instead.
- Interpolation never applies to `{` alone. JSON text such as
  `"{\"account\":\"a\"}"` keeps its meaning, which is why the `{name}` form
  used in one sketch was rejected.

`${}` was chosen over a formatting function because a message is read far more
often than it is written, and inline names read in order. It was chosen over
other interpolation spellings because JavaScript, Kotlin, Dart, Groovy, and
shells already use it. Python's prefixed `f"{x}"` and Swift's `"\(x)"` are
equally clear but less widespread.

### Typing

- An interpolated expression MUST have type `Text`, `Int`, or `Float`
  (`T4325`). `Text` is inserted unchanged. `Int` is inserted in canonical
  decimal form, the same text as `Int.to_text`, and `Float` in its canonical
  form, the same text as `Float.to_text` (G5 defined it, so `Float` joined the
  list). An expression whose type is still open is taken to be `Text`.
- Every other type is an error. The diagnostic suggests
  `${Debug.render(value)}` for debugging output, or a named function for
  user-facing text. Interpolation never chooses a user-facing rendering for a
  struct or enum, because that choice belongs to the program.
- Interpolated expressions are ordinary expressions. They evaluate left to
  right. Their failures and needs join the enclosing contract as any other
  subexpression does.
- A literal with no interpolation is a plain `Text` value, as today.

### Lowering

- The CST keeps the literal losslessly as a text-start token, text chunks,
  interpolation open and close tokens around ordinary expression nodes, and a
  text-end token.
- Lowering turns an interpolated literal into one call of the language-known
  pure operation `std/text.concat` on a list of parts. An `Int` part is first
  passed to `std/int.to_text`, and a `Float` part to `std/float.to_text`. The
  program does not need to import any of these modules. Until migration step 4
  the call is a left-to-right chain of the existing `Text.append` intrinsic.
- Width, padding, and number formatting are functions, not a format
  mini-language: `Text.pad_start`, `Text.join`, and later the numeric modules.

## Debug rendering (G10)

### Surface

`std/debug` declares three pure native operations:

<!-- chimi: pending ep-a6f138 -->
```chimi
pub native fn render<T>(value: T) -> Text
pub native fn render_indented<T>(value: T) -> Text
pub native fn crash(message: Text) -> Never
```

`crash` ends the unit of work with a defect carrying `message`, for a broken
precondition that Chimi source checks ([conventions](standard-library-conventions.md#absence-failure-and-defect)).

`render` and `render_indented` accept any value, never fail, and need
nothing. `render` produces one line. `render_indented` puts each field, element, and entry on its own line
with two-space indentation, for assertion diffs. Each part ends with `,`, as
the formatter writes multi-line literals:

<!-- chimi: fragment -->
```chimi
{
  items: [
    Money {
      cents: 1,
    },
  ],
  status: Status.Declined(
    "x",
  ),
}
```

### Format

Each kind of value renders like the declaration or literal that describes it:

| Value | Rendering |
| --- | --- |
| `Int` | `-42` |
| `Text` | `"say \"hi\"\n"`, quoted with Chimi escapes: `\"`, `\\`, `\n`, `\r`, `\t`, `\$` before `{`, and `\u{1b}` for other control characters |
| `Float` | `-0.5`, `1e20`; the `Float.to_text` form, except that `-0.0` and `0.0` both render as `0.0` |
| `Decimal` | `1.5`; the canonical normalized form, so `1.50` and `1.5` both render as `1.5` |
| `Bool` | `True`, `False` |
| `Void` | `Void` |
| Struct | `Money { cents: 2500 }`, fields in declaration order |
| Enum variant without payload | `PaymentError.GatewayUnavailable` |
| Enum variant with one field | `PaymentError.Declined("card")` |
| Enum variant with several fields | `FileError.Open(path: Path { text: "a.txt" }, message: "denied")` |
| Prelude variants | `Some(3)`, `None`, without the type name |
| Anonymous record | `{ cents: 1, name: "a" }`, fields sorted by name |
| List | `[1, 2, 3]` |
| Tuple | `(1, "a")` |
| `Range` | `0..3`, by its normalized bounds, so `0..=2` renders the same |
| Map | `Map { "a": 1, "b": 2 }`, entries in an internal canonical order that is not stable across releases |
| Set | `Set { 1, 2 }`, items in the same order as map keys |
| `Json.Number` | `Json.Number(1)`; the canonical normalized number, so `1.0` and `1` both render as `1` |
| Declared function | `<fn checkout>` |
| Anonymous function | `<fn>` |
| Named process address | `<Address counter>` |
| Direct process address | `<Address direct>` |
| Std type with a registry render override | `30s` for a `Duration` |
| Other `native type` | `<Receiver>`, the type's simple name |

Rules:

- Type, constructor, and field names are simple names, not module-qualified.
  Generic arguments are not shown.
- A function's captures are never shown. A capability or other runtime handle
  never shows its contents. Rendering a value therefore never reveals or
  copies authority.
- Fields of an opaque struct from another module are shown. Debug rendering is
  a diagnostic aid, and Rust's derived `Debug` makes the same choice. The
  output format is not a stable contract: it MAY change between compiler
  releases, and code MUST NOT parse it. Tests compare values with derived
  equality, not with rendered text.
- Two values that are equal under derived structural equality MUST render to
  the same text.
- Chimi values are immutable and cannot form cycles, so rendering always ends.

### Users

- `std/test` assertions render both sides with `render_indented` in failure
  messages.
- The runner renders an expected failure that escapes `main`, and the value
  context of a defect, with a **bounded** rendering: the same format, cut
  after 4096 bytes and ended with `… (truncated)`. `Debug.render` itself is
  never truncated.
- Structured logs and traces keep attributes as typed values, as Principle 9
  requires. An exporter that needs text MAY use the debug format.

### Runtime metadata

Rendering needs type, constructor, and field names at run time. The native
program data MUST contain those names for every nominal type reachable from
`main`. Today's program data already holds them: each constructor's type,
variant, and field names, and each symbol's name for declared functions.
Until `std/io` moves to source (step 7), a `Path` is text at run time and
renders as quoted text, and a `Json.Value` variant renders with its type's
simple name, as in `Value.Number(Json.Number(1))`. The cost is a few bytes per name, and defect context needs the names
even in programs that never call `Debug.render`.

## Diagnostics

All codes below are new. This specification owns `T4320`–`T4339` and the
blocks `S1101`–`S1119`, `L2101`–`L2119`, `N3101`–`N3119`, and `E5201`–`E5219`,
chosen to avoid codes in use (`S1001`–`S1002`, `L2001`, `N3001`–`N3002`,
`T4001`–`T4250`, `E5001`–`E5008`) and codes other specifications claimed
(`T4260`–`T4264`, `T4280`–`T4284`, `T4300`–`T4319`, `L2020`–`L2023`).

| Code | Stage | Rejected program |
| --- | --- | --- |
| `S1101` | syntax | `${}` with no expression, an unterminated interpolation, or a line break inside one |
| `S1102` | syntax | A `native fn` with a body, or `native` on a declaration other than `fn` or `type` |
| `S1103` | syntax | `const` inside a block |
| `L2101` | lowering | A module-level `let`; carries an edit to `const` |
| `N3101` | name | A `native` declaration outside the standard tree or a vendored module with a `native` grant |
| `N3102` | name | A workspace module whose path starts with `std/` |
| `N3103` | name | An import of `std/internal/...` from outside the standard tree |
| `N3104` | name | A `native fn` or `native type` with no registered operation or type |
| `N3105` | name | A runtime-known declaration that is missing or has the wrong payload arity |
| `N3106` | name | A `pub native fn` whose operation class is `host` |
| `T4320` | type | A `native fn` with an unannotated parameter or success type, or a declared arity that differs from the registry |
| `T4321` | type | A constant initializer with a non-empty failure row |
| `T4322` | type | A constant initializer with a non-empty needs row |
| `T4323` | type | A constant whose type is not constant-representable; names the field or capture |
| `T4324` | type | A constant whose type is not fully determined |
| `T4325` | type | An interpolated expression whose type is not `Text`, `Int`, or `Float` |
| `E5201` | execution setup | Constant evaluation reached a `runtime` or `host` operation; names the operation and call chain |
| `E5202` | execution setup | A defect during constant evaluation |
| `E5203` | execution setup | Constant evaluation exceeded its operation, call depth, or size budget |
| `E5204` | execution setup | The interpreter reached a C binding operation |
| `E5205` | execution setup | A cycle among constants; lists the path |

## Conformance scenarios

| Scenario | Expected |
| --- | --- |
| `import std/list as List` then `List.length([1, 2])` | `2` under both backends |
| `List.length([1])` with no import | unresolved module `List` |
| A workspace file `std/list.chimi` | `N3102` |
| `native fn now() -> Int` in `app/clock.chimi` | `N3101` |
| `pub native fn length<T>(list: List<T>) -> Int { 0 }` in std | `S1102` |
| A std `native fn` whose qualified name is not in the registry | `N3104` |
| A registry entry with no declaration | `chimi std check` fails |
| `pub native fn write_stdout(...)` in `std/io` | `N3106` |
| Importing `std/io` and naming `IO.system_console` | private symbol |
| Root needs `Console`; runner supplies `std/io.system_console` | runs; trace shows host boundary |
| `pub const zero = Duration(0)` used as `Duration.zero` from another module | compiles; value loaded from program data |
| `import std/duration.{zero}` | member imports cannot bind values |
| `let limit = 32` at module level | `L2101` with an edit to `const` |
| `const limit = 32` inside a function body | `S1103` |
| `const config = Config.load(path)` where `load` can fail | `T4321` |
| `const now = clock.now()` with `clock` from `needs` | `T4322` |
| `const jobs = Channel.bounded(8)` | `T4323` naming `Sender` |
| `const total` whose block calls `Channel.bounded(1)` and then returns `5` | `E5201` naming `std/channel.bounded` |
| `const first = first_or_defect([])`, where the function defects on an empty list | `E5202` with the defect span |
| `const a = f()` with `fn f() -> Int { a }` | `E5205` listing `a -> f -> a` (two modules cannot name each other's constants, because their imports would form an import cycle, which the project loader rejects first) |
| `const nothing = None` | `T4324` |
| `const counted = count(9_999)`, where `count` recurses once per step down to `0` | compiles; loads `9999` under both backends |
| `const counted = count(10_000)` with the same `count` | `E5203` naming 10000 calls |
| `const spun = spin(0)` with `fn spin(n: Int) -> Int { spin(n + 1) }` | `E5203` |
| A loop that builds `Node(value, value)` or `Text.append(value, value)` from its previous value 40 times | `E5203` within seconds |
| `const configured = timestamp using { clock: fake }`, where `fake` is a constant provider | compiles; calls through `fake` under both backends |
| `const caches = [primary_cache, backup_cache]` over `process name` declarations | compiles; sends through `caches` route by name |
| `Process.register(spec, caches_first)` where `caches_first` is a constant | `T4262` |
| A constant referenced from two processes and a restarted incarnation | same value in each; no initialization events in the trace |
| `"-L ${line},${line}"` with `line = 7` | `-L 7,7` |
| `"cost: \${amount}"` | `cost: ${amount}` |
| `"{\"a\": 1}"` | unchanged JSON text |
| `"${}"` | `S1101` |
| `"state: ${state}"` where `state` is an enum | `T4325` suggesting `Debug.render` |
| `"${load(path)}"` where `load` returns `Text` and can fail | compiles; the failure joins the enclosing row |
| `Debug.render(PaymentError.Declined("card"))` | `PaymentError.Declined("card")` |
| `Debug.render(Money(2500))` | `Money { cents: 2500 }` |
| `Debug.render({ charge: fake_charge })` | `{ charge: <fn fake_charge> }` |
| `Debug.render(receiver)` | `<Receiver>` |
| A 1 MiB list escaping `main` as a failure payload | runner output truncated at 4096 bytes |
| Each registered operation | reached by at least one `std-tests` program; backends agree |
| A program reaching a C binding, run with the interpreter | `E5204` |
| A native build reaching no binding operation | links no binding archive |

## Implementation notes

Affected stages, in pipeline order:

- **Lexer and parser** (`syntax.rs`): `native` and `const` keywords;
  interpolation tokens inside text literals with brace counting; `native fn`
  and `native type` declaration nodes without bodies.
- **Lowering** (`semantic.rs`): constant declarations; `L2101` for
  module-level `let`; interpolated literals as `std/text.concat` calls.
- **Project loading** (`project.rs`): an embedded standard source reader
  behind the existing `SourceReader` trait; the `std/` prefix check; the
  `--std-dir` override.
- **Resolution** (`resolution.rs`): standard modules become ordinary loaded
  modules. `index_standard_modules`, the built-in `Int`, `List`, and `Text`
  namespaces, and `StandardSymbol` lookups are removed. The prelude binds
  `Option`, `Some`, and `None` from `std/option`. Native declarations resolve
  to `Intrinsic(operation id)`.
- **Typing** (`typing.rs`): native contracts come from annotations. The
  hand-built contracts for each `StandardFunction` and the `StandardGeneric`
  parameters are removed. New: constant rules `T4321`–`T4324` and
  interpolation typing `T4325`.
- **Capability checking** (`capability.rs`): match operation ids instead of
  `StandardFunction::TaskStart` and `StandardFunction::ChannelSend`; derive
  native type properties from the registry.
- **Executable Core** (`executable.rs`): a constant symbol kind; build-time
  evaluation through the interpreter with the class check, budgets, cycle
  path, and cache.
- **Interpreter** (`execution.rs`): registry dispatch for intrinsics; constant
  loads; the debug renderer.
- **Native backend** (`backend/native.rs`, `runtime/native/runtime.c`): the
  generated `operations.h`; operation table dispatch; static immortal constant
  data; name metadata; binding archives linked by reachability.
- **Formatter and inspection** (`formatter.rs`, `inspection.rs`): format
  interpolations and native declarations; show operation class, native type
  properties, and constant types.

`standard.rs` is deleted at the end of the migration.

## Migration plan

Each step leaves the full test suite and both backends passing.

1. **Constants.** Add `const`, the constant symbol kind, build-time
   evaluation, and static program data. Apply the `L2101` codemod to the
   corpus. This step does not depend on standard modules in source and closes
   `ts-f47931`.
2. **Interpolation.** Add the lexer tokens and `T4325`. Until step 4, lower to
   the existing `Text.append` and `Int.to_text` intrinsics.
3. **Embedded standard tree and native declarations.** Add
   `compiler/chimi/std/`, the embedded reader, `native` declarations, the
   registry, `operations.h`, the `std-tests` harness, and `chimi std check`.
   Existing intrinsics keep resolving through `standard.rs` while modules move
   over one at a time.
4. **`std/list`, `std/text`, `std/int`, `std/option`.** Port the built-in
   namespaces and `Option`. Add the codemod that inserts imports for
   `List`, `Text`, and `Int`. Move interpolation lowering to
   `std/text.concat`.
5. **`std/debug`.** Add `render` and `render_indented`, name metadata, and
   bounded rendering in the runner. Switch `std-tests` expected files to
   rendered output.
6. **`std/map` and `std/json`.** `Map` and `Json.Number` become native types.
   `Json.Value`, `DecodeError`, and `NumberError` become source enums listed
   as runtime-known declarations.
7. **`std/io`.** `Path`, `FileError`, `ConsoleError`, `Utf8Error`, and
   `Console` move to source, with host operations behind private provider
   constants and the runner's provider plan naming them. `FileSystem` keeps
   its hand-built interface until G11 allows a generic method in a source
   interface. It is the last entry left in `standard.rs`.
8. **`std/task` and `std/channel`.** `Task`, `Sender`, `Receiver`, and
   `Completion` become native types with registry properties. The capability
   checker switches to operation ids.
9. **Removal.** After G11, port `FileSystem` and delete `standard.rs`.
10. **C bindings (G20).** Add `chimi_binding.h`, binding archives, and the
    build report when the first binding-backed module (`std/crypto` or
    `std/tls`) is scheduled, and vendored grants when T5 adds manifests.

Steps 1 and 2 belong to phase 1 of the
[language completion plan](../plans/language-completion-plan.md), and steps 3
to 9 to phase 2. Step 5 MAY move ahead of step 4 as a hand-built intrinsic if
phase 1 needs debug rendering sooner, then port in step 5.

## Open questions

These need a decision from the language owner. Each has a recommendation, and
nothing above waits on them.

1. **Redacting secrets in debug output.** Debug rendering shows every field,
   so a password inside a config struct reaches assertion messages and defect
   context. *Recommendation:* add a `Secret<T>` native type in `std/secret`
   that renders as `<redacted>` in debug output and traces, and make
   `std/config` return secrets as `Secret<Text>`. Decide with `std/config`.
2. **C bindings outside the standard library.** Allowing vendored packages to
   ship C code trades Principle 7's guarantee for practical drivers (SQL,
   compression). *Recommendation:* allow it only for packages that the
   workspace manifest vendors and grants `native` explicitly, and list every
   such package in the build report. Prefer standard modules for anything
   most services need, such as TLS and crypto.
3. **A standard library versioned apart from the compiler.** Embedding ties
   every standard change to a compiler release. *Recommendation:* keep the
   standard library embedded through phase 9, and revisit only if T5 packages
   create real pressure to update standard modules without a new compiler.
