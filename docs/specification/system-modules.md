# System Modules

## Status

This is the normative contract for the Phase 6c system modules of the
[language completion plan](../plans/language-completion-plan.md) (CHI-209):
the modules a command-line tool or a service uses to reach its operating
system. It covers runner-provided capabilities, `std/path`, `std/env`,
`std/fs`, and `std/signal` (CHI-246 to CHI-249), and `std/exec` (CHI-250) as
far as it has landed: starting, waiting for and stopping programs. Its pipes
and captured output, and the cleanup of children at exit, come next.
`std/terminal` and `std/cli` (CHI-251, CHI-252) add their sections as they
land.

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
| `std/fs.Files` | `files` | `std/fs.host` |
| `std/signal.Signals` | `signals` | `std/signal.host` |
| `std/net.Network` | `network` | `std/net.system` ([network modules](network-modules.md#stdnet)) |
| `std/exec.Commands` | `commands` | `std/exec.system` |

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
- `join` MUST append `part`, without its leading `/`s, with exactly one `/`
  between the two, even when `part` starts with `/`; an empty `part` leaves the
  path as it is, and an empty path gives `part` without its leading `/`s. Joining never replaces the base path, unlike Rust's
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

## std/fs

```text
pub interface Files {
  open: fn(Path) -> Reader<FsError> errors FsError
  create: fn(Path, WriteMode) -> Writer<FsError> errors FsError
  metadata: fn(Path) -> Metadata errors FsError
  list: fn(Path) -> List<Text> errors FsError
  create_directory: fn(Path) -> Void errors FsError
  remove: fn(Path) -> Void errors FsError
  rename: fn(Path, Path) -> Void errors FsError
}
pub enum WriteMode { Replace, Append, New }
pub enum Kind { File, Directory, Special }
pub struct Metadata                      // read with kind, size, modified
pub enum FsError {
  NotFound(path), PermissionDenied(path), AlreadyExists(path),
  NotADirectory(path), IsADirectory(path), NotEmpty(path), Closed(path),
  Other(path, message),
}

// needs { files: Files }, failing with FsError
pub fn read(path) -> Bytes            pub fn write(path, bytes) -> Void
pub fn read_text(path) -> Text        pub fn write_text(path, text) -> Void
pub fn open(path) -> Reader<FsError>  pub fn append(path, bytes) -> Void
pub fn metadata(path) -> Metadata     pub fn create(path, mode) -> Writer<FsError>
pub fn list(path) -> List<Text>       pub fn create_directory(path) -> Void
pub fn remove(path) -> Void           pub fn create_directories(path) -> Void
pub fn rename(from, to) -> Void

pub fn kind(metadata) -> Kind         pub fn metadata_of(kind, size, modified) -> Metadata
pub fn size(metadata) -> Int          pub fn modified(metadata) -> Timestamp

// test-only (T4401 from main)
pub fn temporary_directory(use: fn(Path) -> T) -> T
pub fn files_within(root: Path) -> <a Files record>
```

- Every failure MUST name the path the operating system refused. The runner's
  provider maps `errno` to the variants on both backends (`ENOENT`, `EACCES`
  or `EPERM`, `EEXIST`, `ENOTDIR`, `EISDIR`, `ENOTEMPTY`); anything else is
  `Other` with the system's message. A path holding U+0000 names no file and
  is `Other(path, "path contains a NUL byte")`.
- `open` and `create` give streams ([byte streams](byte-streams.md)) built with
  `Stream.reader` and `Stream.writer`, so `Stream.copy`, `Stream.read_all`, and
  `Resource.scoped(fn() { Fs.open(path) }, Stream.stop, use)` work on files. A
  read gives chunks of at most 64 KiB. Opening a directory fails with
  `IsADirectory`. A read after `Stream.stop` or a write after `Stream.close`
  fails with `Closed`, never reaching another file: a handle carries a
  generation that closing advances.
- A file that is not a regular file (a pipe, a terminal, a socket) is read and
  written without blocking other tasks on the native backend: the descriptor is
  made non-blocking and a read or write that would block waits with
  `CHIMI_REQUEST_IO_WAIT`. Opening a named pipe still blocks until its other
  end opens, as `open(2)` does.
- `read` and `write` hold the whole file in memory and have no size limit,
  since they are for local files the program chose; `Stream.read_all` with a
  limit is the bounded read. `write` empties the file and writes it in place:
  it is not atomic.
- `create` with `Replace` empties or creates the file, `Append` writes after its
  end, and `New` fails with `AlreadyExists` rather than touch an existing file.
- `metadata` follows symbolic links, so `Kind` has no link variant; `Special`
  is a device, socket, or named pipe. `modified` is a `Timestamp`, so on the
  interpreter, which has no clock, `Fs.modified` defects; `kind` and `size`
  work on both.
- `list` returns the entry names without `.` and `..`, in ascending byte order,
  and fails with `Other` when a name is not UTF-8.
- `remove` removes a file, a symbolic link, or an empty directory (`NotEmpty`
  otherwise); there is no public recursive remove. `rename` replaces a file at
  `to`.
- `create_directories` creates the missing directories from `path` upward and
  stops at the first that exists, so it never touches a directory above them
  (as Rust's `create_dir_all` does). A file in the way fails with
  `AlreadyExists`, or `NotADirectory` when it is above the leaf.
- `temporary_directory(use)` creates a new directory under `$TMPDIR` (else
  `/tmp`), with its symbolic links resolved, runs `use` with it, and removes it
  and everything in it however `use` ends, through `Resource.scoped`. Removal
  does not follow symbolic links out of it.
- `files_within(root)` is the real provider confined to the absolute `root`:
  it refuses, with `PermissionDenied`, a relative path and any path whose
  normalized text is not `root` or under it. A test supplies it with
  `using { files: Fs.files_within(directory) }`. It checks text, so a
  symbolic link inside `root` that points out of it is followed.

## std/signal

```text
pub enum Signal { Hangup, User1, User2 }   // SIGHUP, SIGUSR1, SIGUSR2
pub interface Signals {
  next: fn(List<Signal>) -> Signal
}

// needs { signals: Signals }
pub fn next(wanted: List<Signal>) -> Signal
```

- `next(wanted)` MUST return a pending signal from `wanted` at once, the first
  in `wanted`'s order when several are pending, and otherwise wait, letting
  other tasks run, until one arrives. It defects when `wanted` is empty. It is a
  cancellation point, so it races a timeout or a channel inside `Select.first`
  through `Select.task`.
- Each signal is pending at most once: repeats that arrive before a waiter
  takes it coalesce, as the operating system's do. A signal that arrives while
  no task waits for it MUST be kept until one asks for it, and it is never
  dropped by a waiter that is cancelled before it takes it.
- One signal MUST wake one waiter, the one that has waited longest among those
  that want it. Several tasks that must all hear a signal are served by one
  task that waits and a broadcast channel.
- The runner's provider installs the handlers for all three signals when it
  is built, before `main`'s body runs, and never removes them. A program whose
  `main` does not need `signals` keeps the operating system's default actions,
  so SIGHUP ends it. A program that needs `signals` catches all three, even
  those it never waits for, so SIGUSR1 sent to a program that only waits for
  SIGHUP is kept, not fatal.
- SIGINT and SIGTERM are not `Signal` variants: the first one drains and stops
  every program (application lifecycle, G22), and a later one ends it at once.
  A SIGHUP never changes that.
- On the native backend the handler sets one bit per signal and wakes the
  scheduler through the stop signals' pipe; a task waits with
  `CHIMI_REQUEST_SIGNAL_WAIT`. That wait is not a host operation in flight, so
  it never holds virtual time. Under the real clock a program whose tasks all
  wait for signals waits for one instead of reporting a deadlock; under the
  virtual clock, with no injected signal to come, it is a deadlock that names
  `Signal.next`.
- The runner's `--signal=` option takes `HUP`, `USR1`, and `USR2` under the
  virtual clock, as many times as needed (at most 64). Each is latched at its
  instant with a `clock_signal_received` event carrying the signal number; one
  due after `main` ends is never latched and never moves time.
- The interpreter installs no handlers: `next` with the runner's provider
  defects there, saying it requires the native runtime. A test supplies a fake
  `{ next: fn(wanted) { ... } }` with `using`, on both backends.

## std/exec

```text
pub interface Commands {
  start: fn(Command) -> Child errors ExecError
}
pub struct Command { ... }                 // built with the functions below
pub enum Stdio { Inherit, Null }
pub struct Child { ... }                   // wait, terminate, kill, release
pub enum Status { Exited(code: Int), Signaled(signal: Int) }
pub enum ExecError {
  NotFound(program: Text), PermissionDenied(program: Text),
  InvalidDirectory(path: Path), Closed(program: Text),
  Failed(program: Text, message: Text),
}

pub fn command(program: Text, arguments: List<Text>) -> Command
pub fn variable(command: Command, name: Text, value: Text) -> Command
pub fn environment(command: Command, variables: Map<Text, Text>) -> Command
pub fn directory(command: Command, path: Path) -> Command
pub fn stdin(command: Command, stdio: Stdio) -> Command   // and stdout, stderr
pub fn program(command: Command) -> Text
pub fn arguments(command: Command) -> List<Text>

// needs { commands: Commands }
pub fn start(command: Command, use: fn(Child) -> T) -> T
pub fn run(command: Command) -> Status

pub fn wait(child: Child) -> Status
pub fn terminate(child: Child) -> Void
pub fn kill(child: Child) -> Void
pub fn succeeded(status: Status) -> Bool
pub fn child(wait, terminate, kill, release) -> Child  // for fakes
pub fn host_for_tests() -> Commands                      // test-only
```

- A `Command` is a value; building one MUST start nothing. `command` inherits
  the parent's environment, working directory and standard streams.
  `variable` sets one variable over what the command inherits, and a later
  value for a name wins; `environment` replaces the inherited environment
  with exactly its map. A variable name that is empty or holds `=`, or any
  text holding U+0000, is a defect when the command starts.
- There is no shell. The arguments MUST reach the program exactly as given. A
  program name without a `/` is looked up on the parent's `PATH` (on
  `/usr/bin:/bin` when it is unset or empty), before the child is created, so
  `NotFound` and `PermissionDenied` are reported by the start itself; a
  directory on `PATH` named like the program is passed over. A name with a
  `/` is found from the parent's working directory, even when `directory`
  gives the child another. A working directory that does not exist, is not a
  directory or cannot be entered is `InvalidDirectory`, also reported by the
  start. A file the kernel cannot run (no `#!` line, wrong format) is
  `Failed`, and `Failed` carries only the operating system's words.
- `start(command, use)` is the only way to hold a `Child`: when `use` returns,
  fails or is cancelled, a child still running MUST be killed with SIGKILL and
  reaped, so no child and no zombie outlives the `start` that made it. A
  `Child` used after that answers `Closed` from `wait`. The child's own
  children are not stopped: a program that leaves its children running when
  it dies leaves them to outlive the `start` (process groups are decided
  with the graceful release in part 4).
- `wait` MUST let other tasks run while the child runs, and MUST return the
  same `Status` when asked again. It is a cancellation point. A nonzero exit
  is a `Status`, never an `ExecError`. `terminate` sends SIGTERM and `kill`
  SIGKILL; both do nothing once the child has been waited for, so a signal
  never reaches a reused process number.
- The child starts with an empty signal mask and the actions `exec` leaves.
  A signal the runtime handles is back at its default action in the child:
  SIGINT and SIGTERM always, and SIGHUP, SIGUSR1 and SIGUSR2 when the program
  needs `signals`, even if the program itself inherited one as ignored (as
  Go's `os/exec` does for signals a program has asked for). Any other signal
  the program inherited as ignored stays ignored, so a program without
  `signals` run under `nohup` passes SIGHUP's on. The child stays in the parent's process group,
  and no file the runtime opens for itself (the telemetry, outcome, profile
  and memory reports) stays open in it.
- On the native backend `std/exec` is a binding package
  (`runtime/native/bindings/exec`). It spawns with `posix_spawn` and waits
  with `chimi_binding_wait_child` (binding ABI 2), which issues
  `CHIMI_REQUEST_CHILD_WAIT`: the scheduler only looks at the child (`waitid`
  with `WNOWAIT`) and the binding reaps it. The runner's SIGCHLD handler,
  installed before the first spawn, wakes an idle scheduler through the stop
  signals' pipe. A task waiting for a child is a host operation in flight, so
  it holds virtual time.
- The interpreter cannot call C bindings: the runner's provider and
  `host_for_tests` defect there (`E5204`). A test supplies a fake with
  `using`, built with `Exec.child`, on both backends; `program` and
  `arguments` let a fake check what it was asked to run.

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
- **A `Files` interface of seven members.** Whole-file reads and writes,
  `create_directories`, and the error mapping are Chimi over them, so a fake is
  seven functions and both backends share the rest. The host core is a small
  set of private natives returning status tuples; new modules do not extend
  the runtime's older `HOST_*` table.
- **Real files in tests are confined, not ambient.** The test plan never
  closes `files`; `temporary_directory` and `files_within` are test-only, so a
  test reaches real files only inside a directory it owns.
- **`next(wanted)` over a subscription value.** A one-function interface keeps
  a fake to one closure, and `Select.task` already composes a waiting call with
  timeouts and channels, so `std/signal` adds no receiver type.
- **Latch, one waiter per signal.** One pending bit per signal matches what the
  kernel can promise (it coalesces too), and handing each signal to one waiter
  keeps a signal that lands while the loop is busy reloading. Broadcasting to
  every current waiter would drop it.
- **Handlers from the provider, default actions otherwise.** Installing them
  when the need is closed means no startup window where SIGHUP still kills a
  program that asked for it, and a program that did not ask keeps the
  behavior `nohup` and process managers expect. Whether an unwatched SIGHUP
  should drain like SIGTERM is left open; the familiar default stands.
- **`std/exec` as a binding with a scheduler wait.** Starting a child is a
  posix_spawn call and its pipes are descriptors, which the binding API
  already serves; the one new primitive is the child-exit wait, which only
  looks at the child, so a waiter cancelled between its wake and its retry
  loses no exit status. Scoped-only children follow `Resource.scoped`, and
  the names avoid `with_` as the conventions ask.
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
| The runner closes `files` on `main` on both backends | `std-tests/fs/runner_provider.chimi`; `tests/host_modules.rs` `the_runner_provides_the_file_system_to_main` |
| Reading, writing, appending, streaming past one chunk, listing, renaming, and each error variant on both backends | `tests/host_modules.rs` `the_runner_provides_the_file_system_to_main` |
| Metadata follows links; `create_directories` accepts existing directories and stops at a file | `tests/host_modules.rs` `metadata_reads_kind_size_and_modification_time` (native) |
| A waiter wakes; signals coalesce; the first listed pending signal wins; a cancelled `next` in a `Select.first` race takes nothing; an unwanted signal stays latched | `tests/std_signal.rs` `signals_wake_waiters_coalesce_and_wait_their_turn` |
| One signal wakes the oldest waiter | `tests/std_signal.rs` `each_signal_wakes_the_oldest_waiter` |
| SIGTERM still drains after a SIGHUP (the second stop signal's rule is unchanged, `tests/signals.rs`) | `tests/std_signal.rs` `a_hangup_reloads_and_a_later_terminate_still_drains`, `stop_signals_keep_their_meaning_after_a_hangup` |
| A waiter with nothing to come is a deadlock under the virtual clock; injected signals after `main` change nothing; `--signal=` rules; empty `wanted` defects | `tests/std_signal.rs` `a_waiter_with_no_signal_coming_is_a_deadlock`, `a_signal_due_after_main_ends_changes_nothing`, `the_signal_option_takes_the_three_signals_under_the_virtual_clock_only` (also the 64 cap), `waiting_for_no_signal_defects` and, for the interpreter, `the_interpreter_needs_the_native_runtime_to_wait` |
| Real SIGHUPs reload a waiting service; the handlers cover all three signals from the first line; without `signals` SIGHUP ends the program | `tests/std_signal.rs` `a_service_reloads_on_each_hangup`, `a_program_that_needs_signals_survives_them_from_its_first_line`, `a_program_without_signals_is_ended_by_a_hangup` |
| The interpreter defects on a wait; a fake on both backends; `T4400` without one | `tests/std_signal.rs` `the_interpreter_needs_the_native_runtime_to_wait`, `a_test_supplies_signals_as_a_fake` |
| `temporary_directory` is removed after the test; `files_within` refuses paths outside it and relative ones; a fake; `T4400` without one; `T4401` from `main` | `tests/host_modules.rs` `a_test_uses_real_files_in_a_temporary_directory_or_a_fake` |
| A status for an exit code or a signal; arguments pass unchanged; a second `wait` answers the same | `tests/std_exec.rs` `a_child_reports_how_it_ended` |
| `NotFound`, `PermissionDenied` and `InvalidDirectory` before the child starts; `PATH` lookup; variables and a replaced environment; a working directory; `Stdio.Null`; a relative program with another working directory; `Failed` for a file the kernel cannot run | `tests/std_exec.rs` `a_command_that_cannot_start_fails_with_its_reason` |
| A directory on `PATH` named like the program is passed over | `tests/std_exec.rs` `a_directory_on_path_is_not_the_program` |
| Other tasks run while one waits for a child, and the exit wakes the wait without waiting for a recheck; two tasks waiting for one child both get its status | `tests/std_exec.rs` `a_child_wait_lets_other_tasks_run_and_ends_at_the_exit`, `a_child_is_stopped_and_never_outlives_its_start` |
| `kill` and `terminate`; a child left running, or whose wait was cancelled, is killed and reaped when its `start` ends; `Closed` after release | `tests/std_exec.rs` `a_child_is_stopped_and_never_outlives_its_start` |
| A child wait holds virtual time | `tests/std_exec.rs` `a_child_wait_holds_virtual_time`; the scheduler side in `runtime/chimi-fiber/src/scheduler/child_tests.rs` |
| A child keeps SIGHUP ignored when the parent inherited it ignored | `tests/std_exec.rs` `a_child_keeps_the_signals_its_parent_ignores` |
| A fake on both backends; the runner closes `commands` on `main`; `T4401` from `main`; `T4400` without one; the interpreter defects | `tests/std_exec.rs` `a_fake_stands_in_for_real_programs_on_both_backends`, `main_gets_real_programs_by_declaring_the_need_and_nothing_else_does`, `the_interpreter_cannot_start_real_programs` |
