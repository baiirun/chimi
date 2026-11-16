# System Modules

## Status

This is the normative contract for the Phase 6c system modules of the
[language completion plan](../plans/language-completion-plan.md) (CHI-209):
the modules a command-line tool or a service uses to reach its operating
system. It covers runner-provided capabilities, `std/path`, and `std/env`
(CHI-246, CHI-247). `std/fs`, `std/signal`, `std/exec`, `std/terminal`, and
`std/cli` (CHI-248 to CHI-252) add their sections as they land.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative. The
modules follow the [standard library conventions](standard-library-conventions.md)
and the [host capability rules](host-capabilities-and-execution-properties.md).
Targets are macOS and Linux, so paths follow POSIX.

## Goals

- A function that reaches the operating system says so in its signature, with
  a `needs` entry, and a test replaces that access with a fake through `using`,
  as it already does for `clock`.
- Each module's capability is a small interface written in Chimi, so a fake
  is a record of a few functions and the rest of the module is ordinary Chimi
  over them.
- No ambient authority: no public function hands out the host's provider, so
  the only code that reads the real environment is code whose needs reach
  `main`.

## Runner-provided capabilities

A standard module MAY declare an interface that the runner closes on `main`.
Each one has a **fixed need name**, listed with it in the registry
(`operations.rs` `RUNNER_PROVIDERS`):

| Interface | Need name | Provider |
| --- | --- | --- |
| `std/env.Environment` | `environment` | `std/env.host` |

Rules:

- A need on `main` of such an interface under its fixed name MUST be closed by
  the runner. Under any other name it is an ordinary need, so `main` fails with
  `E5002` unless it supplies it with `using`. One name per authority keeps every
  function that reads it on the same row entry, as `clock: Clock` does.
- The runner closes the need by evaluating the module's private provider
  function once, before `main`'s body runs, and binding the result as if
  `main` were called `using { <need>: <provider>() }`. A `using` inside `main`
  that supplies the same need takes precedence for the code it encloses.
- The provider function and the host operations it is built from MUST be
  private (`N3106` for host operations), so no code can take the capability
  without declaring the need.
- The test provider plan does not close these needs: a test that reaches one
  without supplying it with `using` is rejected with `T4400`, so a test never
  reads the machine by accident.
- A constant cannot reach one: its initializer would need a provider
  (`T4322`), and constant evaluation also rejects a host operation by name.
- A host operation checks cancellation before it enters the operating system,
  as the host capability rules require.

The older capabilities `Console` and `FileSystem` keep their rules (any need
name, built into the compiler) until `std/io` is rewritten over these
modules.

## std/path

```text
pub native type Path

pub fn from(text: Text) -> Path
pub fn to_text(path: Path) -> Text
pub fn join(path: Path, part: Text) -> Path
pub fn is_absolute(path: Path) -> Bool
pub fn parts(path: Path) -> List<Text>
pub fn name(path: Path) -> Option<Text>
pub fn stem(path: Path) -> Option<Text>
pub fn extension(path: Path) -> Option<Text>
pub fn parent(path: Path) -> Option<Path>
pub fn replace_extension(path: Path, extension: Text) -> Path
pub fn normalize(path: Path) -> Path
```

- `Path` MUST be the one path type of the standard library. `std/io`'s `Path`
  (returned by `IO.path`) is the same type: when `std/path` is loaded,
  `import std/io.{Path}` names `std/path.Path`. At run time a `Path` is its
  text on both backends; equality and order are the text's.
- Every function works on the text alone and MUST NOT consult the file system.
- `join` MUST append `part` with exactly one `/` between the two, even when
  `part` starts with `/`. Joining never replaces the base path, unlike Rust's
  `Path::join` and Python's `/`: a name read from input stays inside the
  directory it was joined to, and `..` is removed only by `normalize`. This is
  Go's `filepath.Join` and Node's `path.join` without their cleaning.
- `parts` lists the components, with `"/"` first for an absolute path, and
  leaves out empty components and `.`.
- `name` is the last component, ignoring a trailing `/`; it is `None` when
  there is none or it is `..`.
- `extension` is the text after the last `.` of `name`; it is `None` when there
  is no `.`, nothing follows it, or the only `.` starts the name (`.bashrc`).
  `stem` is `name` without `.` and the extension.
- `parent` removes the last component: `/` for `/etc`, `.` for a single
  relative name, and `None` for `/`, `.`, and the empty path.
- `normalize` is Go's `filepath.Clean`: repeated `/` and `.` are removed, `..`
  removes the name before it, a leading `..` of a relative path stays, one
  directly under `/` is dropped, and the empty path is `.`. Because it reads
  only the text, it can disagree with the operating system across a symbolic
  link.

## std/env

```text
pub interface Environment {
  variables: fn() -> List<(Text, Text)>
  current_directory: fn() -> Option<Path>
}
pub enum EnvError { NoCurrentDirectory }

pub fn get(name: Text) -> Option<Text>          needs { environment: Environment }
pub fn all() -> Map<Text, Text>                 needs { environment: Environment }
pub fn current_directory() -> Path              needs { environment: Environment }
                                                // fails with EnvError
```

- The runner's provider lists the process environment in the order the
  operating system keeps it (`environ`) and leaves out a variable whose name
  or value is not UTF-8. Its `current_directory` is `None` when the working
  directory was removed, cannot be read, or is not UTF-8.
- `get` MUST return the first value of `name`, and `all` MUST keep the first
  value of a name the operating system holds twice, as C's `getenv` reads it.
  Names are case-sensitive; an empty value is `Some("")`.
- There is no function that sets or removes a variable: the environment is
  shared by the whole process, Chimi has no mutation, and a child process gets
  its own variables from `std/exec`.
- Program arguments stay `main`'s `List<Text>` parameter. Ending with a chosen
  exit status belongs to the program entry contract, decided with `std/cli`
  (CHI-252), not to this module: an immediate exit would skip
  `Resource.scoped` releases and the supervisor drain.

## Decisions

- **Interfaces in source, fixed need names, private providers.** The advisor
  review and the existing `clock` and `Random` rules agree: one name per
  authority, a fake per interface, no ambient provider.
- **The smallest interface.** `Environment` has two members; `get` and `all`
  are Chimi over `variables`, so a fake is two functions and both backends
  share one implementation of lookup.
- **One `Path` type,** moved to `std/path`, rather than a second type beside
  `std/io`'s or plain `Text`: the conventions already treat a path as a
  nominal boundary.
- **`join` appends.** Replacing on an absolute part is a known source of
  path-traversal bugs, and Chimi prefers the rule a reader can predict.

## Conformance

| Rule | Evidence |
| --- | --- |
| Lexical path functions, including their edge cases, on both backends | `std-tests/path/lexical.chimi` |
| `std/io`'s `Path` is `std/path`'s | `std-tests/path/lexical.chimi` imports both |
| The runner closes `environment` on `main` on both backends | `std-tests/env/runner_provider.chimi`; `tests/host_modules.rs` `the_runner_provides_the_environment_to_main` |
| Values, missing and empty variables, case, and the working directory | `tests/host_modules.rs` `the_runner_provides_the_environment_to_main` |
| A test supplies a fake; the first value wins; `NoCurrentDirectory`; `T4400` without one | `tests/host_modules.rs` `a_test_supplies_the_environment_as_a_fake` |
| Only the fixed need name is closed | `tests/host_modules.rs` `only_the_environment_need_name_is_closed_by_the_runner` |
