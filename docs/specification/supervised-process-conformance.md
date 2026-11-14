# Supervised-Process Conformance Lab

## Status

Lab design for `prog show ts-b9cbe1`, inside the
[first supervised-process kernel](../wayfinding/first-supervised-process-kernel.md).
It gates the connected implementation (`prog show ts-6ccbc9`) and the final
conformance and performance run (`prog show ts-1765ba`). Decisions made while
designing it are logged on the ticket.

The lab tests these contracts and changes none of them:

- [process and supervisor static contracts](process-and-supervisor-static-contracts.md)
- [process execution and mailboxes](process-execution-and-mailboxes.md)
- [process identity and routing](process-identity-and-routing.md)
- [supervisor policy and lifecycle](supervisor-policy-and-lifecycle.md)
- [time and outcomes](time-and-outcomes.md), for `Duration`, virtual time, and
  runtime deadlines
- [task and channel capabilities](task-and-channel-capabilities.md)
- the [supervised process runtime](../architecture/supervised-process-runtime.md)
  representation, including the coordinator decisions logged on
  `prog show ts-a6b43b`

Where a contract leaves a detail open, this document fixes it for the lab and
says so. The [open questions](#open-questions) list the choices that need the
user.

## Goals

- Each scenario falsifies one contract rule. A scenario names the wrong
  implementation it rejects. The composite forcing program is never the
  evidence for a rule.
- Expected outcomes and traces are exact. Virtual time and a single carrier
  thread make every run of a scenario produce the same transcript.
- Each evidence kind stays separate: static diagnostics, verified Core,
  runtime lifecycle, heap safety, observability, and performance.
- Scenarios unlock in the order the implementation lands, so each
  implementation commit turns a known set of scenarios green.

## Evidence kinds

`chimi-process-lab` is one native executable, modelled on
`chimi-concurrency-lab`
([concurrency kernel conformance baseline](../performance/concurrency-kernel-conformance-baseline.md)).
Each kind runs on its own:

| Kind | Scenario prefixes | Evidence |
| --- | --- | --- |
| `static` | `SP`, `SF` | Compiler diagnostics for real Chimi fixtures |
| `core` | `CO` | Verified Core invariants, checked through the compiler's inspection API |
| `runtime` | `MB`, `RT`, `SV`, `RA`, `ES`, `RR`, `SD` | Native execution of Chimi fixtures under virtual time, with exact outcomes and transcripts |
| `heap` | `CB`, `MF` | Copy-boundary safety and memory flatness, from fixtures with memory reports and from white-box C harnesses |
| `observability` | `OB`, `IP` | Trace modes, determinism, event fields, and backend support |
| `forcing` | `FP` | The dynamic-pool forcing program |
| `performance` | `PG` | Timing and memory gates in release builds |
| `all` | every prefix | Every kind, in the order above |

## File layout

```text
compiler/chimi/src/bin/process_lab.rs        CLI entry: chimi-process-lab
compiler/chimi/src/process_lab/mod.rs        run(kind, options) -> Report
compiler/chimi/src/process_lab/catalogue.rs  SCENARIOS table and CURRENT_STAGE
compiler/chimi/src/process_lab/transcript.rs telemetry -> canonical transcript
compiler/chimi/src/process_lab/fixture.rs    build, run, and compare one fixture
compiler/chimi/src/process_lab/static_kind.rs
compiler/chimi/src/process_lab/core_kind.rs
compiler/chimi/src/process_lab/heap_kind.rs
compiler/chimi/src/process_lab/observability_kind.rs
compiler/chimi/src/process_lab/performance.rs
compiler/chimi/tests/process-lab/<ID>-<slug>/app/main.chimi
compiler/chimi/tests/process-lab/<ID>-<slug>/expected.diagnostics
compiler/chimi/tests/process-lab/<ID>-<slug>/expected.outcome
compiler/chimi/tests/process-lab/<ID>-<slug>/expected.transcript
compiler/chimi/tests/process-lab/<ID>-<slug>/expected.memory
compiler/chimi/tests/process-lab/RA08-<slug>/expected.events
compiler/chimi/tests/process-lab/c/heap_boundary.c
compiler/chimi/tests/process-lab/c/kernel_faults.c
compiler/chimi/tests/process_lab.rs          cargo test that runs every kind
programs/supervised-pool/app/main.chimi      the forcing program (FP01)
programs/supervised-pool/README.md
docs/performance/supervised-process-conformance-baseline.md   written by ts-1765ba
```

- Every fixture is a project rooted at its scenario directory with the entry
  module `app/main`. Name labels in transcripts are therefore stable, for
  example `app/main.worker_b`.
- A scenario directory holds only the expected files its form needs.
- The white-box C harnesses follow the precedent of
  `compiler/chimi/tests/fixtures/native-list-runtime.c`: they include
  `runtime.c`, supply empty program data, and exit nonzero on the first failed
  check. A harness takes the scenario ID as its only argument, runs that
  scenario's checks, and prints `ok`. An unknown ID exits 2. The lab compiles
  it with AddressSanitizer when the C compiler supports it.
- `Cargo.toml` gains a `chimi-process-lab` binary with
  `required-features = ["native-backend"]`.
- Each kind module and `fixture.rs` land with the first stage that runs
  them (`static_kind.rs` and `fixture.rs` at U1, `core_kind.rs` at U2,
  `heap_kind.rs` at U3, `observability_kind.rs` at U5, `performance.rs` at
  U10; runtime fixtures run through `fixture.rs` alone). Until then an unlocked
  scenario of that kind fails with "runner has not landed".

## Harness entry points

```text
chimi-process-lab <kind> [--through <stage>] [--scenario <ID>]
                  [--backend <native|interpreter>] [--print-actual]
```

- `--through` defaults to `CURRENT_STAGE` from `catalogue.rs`. Scenarios
  whose unlock stage is at or below it must pass. Later scenarios report
  `locked` and do not run.
- `--scenario` runs one scenario, regardless of its kind.
- `--backend` selects the backend that executes fixtures. The default is
  `native`. With `interpreter`, every fixture of the runtime, heap,
  observability, and forcing kinds reports `unsupported` and does not run
  (see [Backends](#backends)). Static and Core scenarios, C harnesses, and
  checks do not depend on the backend.
- `--print-actual` prints the actual diagnostics, outcome, transcript, and
  memory reports next to the expected ones. The lab never writes expected files. Expected
  files are written by hand from the contracts and reviewed like code.

Rust shapes:

```rust
pub fn run(kind: Kind, options: &Options) -> Result<Report, String>;

pub struct Scenario {
	pub id: &'static str,          // "RR02"
	pub slug: &'static str,        // "removal-during-replacement-start"
	pub unlock: Stage,
	pub form: Form,                // Fixture, CHarness(&str), Check(fn), Gate(fn)
	pub exit_code: i32,            // 0 success, 1 expected failure, 70 defect
	pub arguments: &'static [&'static str], // one run per entry
	pub extra_events: &'static [&'static str], // e.g. ["task_terminal"]
}

pub fn project(telemetry: &str, extra_events: &[&str]) -> Result<Vec<String>, String>;
```

- `Check(fn)` is a Rust check for a scenario with no fixture of its own:
  CO01 to CO04 over verified Core, MF06 over built executables, OB04 over
  other fixtures' telemetry, and OB07 over the trace ABI.
- Each entry of `arguments` is one run, passed to `main(arguments)`.
  Empty means one run with no arguments. MF01, MF02, MF03, and MF05 run once
  per size. SV11 runs once per out-of-range value.
- `exit_code` applies to every run of an executed fixture. Static and Core
  scenarios do not run and keep 0.

`compiler/chimi/tests/process_lab.rs` runs each kind through the binary and
fails if any scenario at or below `CURRENT_STAGE` reports `fail`. Each
implementation commit in ts-6ccbc9 that completes a stage bumps
`CURRENT_STAGE` in the same commit. That commit must turn the stage's
scenarios green and keep every earlier stage green.

### Report

Each kind prints one JSON line:

```json
{"schema":"chimi.process-conformance.v1","kind":"runtime","stage":"U6","scenarios":[{"id":"MB01","status":"pass"}],"metrics":{},"gates":[]}
```

- `status` is `pass`, `fail`, `locked`, or `unsupported`. A `fail` entry
  carries a `detail` field with the first differing line. A `pass` entry
  carries a `detail` note when its evidence is weaker than specified, for
  example a C harness that ran without AddressSanitizer because the
  sanitized build failed.
- `gates` lists each performance gate as `metric`, `value`, `limit`, and
  `status`.
- The process exits nonzero if any unlocked scenario or gate fails.

## Expected files

### `expected.diagnostics`

One line per diagnostic, sorted by position:

```text
T4263 app/main.chimi:14:30
```

The set of codes and positions must match exactly. A fixture that should
compile has an empty file. Message text is not compared, except where a
scenario says the diagnostic names a capture or provider. Those scenarios add
a second column with the required substring.

### `expected.contracts`

SP08 and SP02. One line per function, `<Module>.<function>: <contract>`,
in the compiler's contract notation (the form inferred contracts and hover
use): generic parameters are written bare, and a failure row that holds every
variant of an enum is the enum's name. A module part that is a project module
path (it contains `/`, as in `app/main.serving`) names that project function's
inferred contract; any other names a standard function. SP02's file holds
`app/main.serving: fn() -> Spec<Request, Never> needs { store: Store }`: the
need surfaces at the enclosing function, and the spec carries none. The SP08
file is:

```text
Process.send: fn(Address<M>, M) -> Void errors SendError<M>
Supervisor.start: fn(Policy) -> Running errors StartError.NameTaken
Supervisor.start_process: fn(Running, Spec<M, E>, Restart) -> ManagedProcess<M> errors StartError
```

### `expected.outcome`

The runner outcome line. For `success` and `expected_failure` the line must
match byte for byte, for example:

```json
{"schema":"chimi.runtime.v1","outcome":"success","value":{"type":"PoolResult","fields":{"a":3,"b":30,"c":300,"stale":99,"rejected":1}}}
```

For a `defect`, the file holds the two compared fields, `outcome` and
`message`, and the scenario's `exit_code` is 70. A scenario with several
runs has one line per run, in `arguments` order.

Each run is `<executable> --chimi-runner --clock=virtual --trace=off
--telemetry-file=<path> --outcome-file=<path> --memory-report-file=<path> --
<argument>`, and must exit with the scenario's `exit_code`. Its telemetry
file must stay empty. `--clock=virtual` is on every run, not only the
transcript runs (U5 decision): a fixture that sleeps then gets the same
outcome and memory report in every mode. A transcript fixture's
`--trace=full` and `--trace=boundaries` runs must exit with the same code.

### `expected.memory`

Any executed fixture may have one (memory-flatness, copy-boundary, and
observability fixtures use it). One assertion per line over the
`--memory-report-file` reports of the scenario's `--trace=off` runs:

```text
equal scoped_peak_live_bytes
at_most scoped_peak_live_bytes 7168
exactly copied_values 21
```

`equal <field>` requires the top-level integer field to have the same value
in every run. `at_most <field> <limit>` requires it to be at most `limit` in
every run. `exactly <field> <value>` requires it to be `value` in every run
(U5 decision, for CB02, CB03, and OB01, whose counts are exact). Equality across sizes proves that memory does not grow with
uptime, but it also accepts reclamation in batches of k passes. MF05
therefore also bounds its peak below two passes' allocations on top of the
4 KiB full-pass minimum, so that only per-pass reclamation passes.

### `expected.events`

RA08 only (U6 decision). Its 1,000-defect storm has a transcript of over
4,000 lines, too long to write by hand, so the scenario states event counts
instead. The lab runs the fixture twice under `--trace=full`. The two telemetry
files must be byte-identical once `execution_id` is removed. The first run's
projected transcript is then counted by class, one line per class, in any
order:

```text
process_terminated cause=defected 1000
restart_attempted attempts_in_window=2 500
```

A class is the event name followed by those of its fields that are a
`cause`, `reason`, `outcome`, or `attempts_in_window`. Identities and times do
not split a class. Every class that occurs must be listed.

### `expected.transcript`

The canonical transcript. The lab builds it from runner telemetry with
`transcript::project`:

1. Run the executable with `--chimi-runner --clock=virtual --trace=full`.
2. Keep the process-family events, the virtual-time advance event, and the
   scenario's `extra_events`. Drop everything else.
3. Track the current virtual time from the advance events. Do not print the
   advance events themselves.
4. Render each kept event as one line:
   `@<virtual time> <event> <fields>`. The time uses the `Duration` text form
   (`0s`, `1ms`, `30.001s`). Fields appear in the fixed order of the table
   below as `key=value`. Identities are printed as `S<n>`, `M<n>`, `I<n>`,
   and `T<n>`. An absent value is `-`.
5. Compare the line lists exactly.

| Event | Fields, in order |
| --- | --- |
| `process_started` | `supervisor`, `membership`, `incarnation`, `name` (only if registered) |
| `admission_rejected` | `supervisor`, `reason` (`capacity_reached`, `name_taken`, `stopped`), `name` (only for `name_taken`) |
| `process_terminated` | `membership`, `incarnation`, `cause` (`returned`, `failed`, `defected`, `cancelled`, `forced`) |
| `restart_attempted` | `supervisor`, `membership`, `attempts_in_window` |
| `restart_limit_exceeded` | `supervisor` |
| `supervisor_escalated` | `supervisor`, `parent` (`S<n>` or `root`) |
| `membership_removed` | `membership` |
| `supervisor_stopped` | `supervisor` |
| `process_send` | `kind` (`named`, `direct`), `name` (only for `named`), `incarnation` (accepting incarnation or `-`), `outcome` (`accepted`, `full`, `unavailable`) |
| `task_terminal` (extra) | `task`, `parent`, `cause` (the task whose defect ended it, or `-`) |

- `admission_rejected` also takes `reason=start_failed`, the scheduler's
  `CHIMI_ADMISSION_START_FAILED`, which only the RA06 fault injection
  reaches.
- `supervisor_started` (trace kind 10) is dropped until U9 (coordinator
  decision 5 on `ts-6ccbc9`). U9 adds it to this table and to the
  transcripts.
- A kept process event's own `time_ns` must equal the time tracked from the
  advance events, and `sequence` must increase on every line. `project`
  rejects telemetry that breaks either rule, or that has a missing or
  unknown field on a kept event.

Every transcript scenario runs in three trace modes:

- `full` must equal `expected.transcript`.
- `boundaries` must equal `expected.transcript` with every `process_send`
  line removed. Lifecycle events are boundary events. Send events are motion
  events, like channel motion today.
- `off` must produce an empty telemetry file.

### Runner telemetry for process and clock events

The runner writes each `ChimiSchedulerTraceEvent` as one JSON line with the
envelope that `emit_scheduler_event` in `runtime.c` already uses for task and
channel events: `schema`, `event`, `sequence`, `computation`,
`execution_id`, `task_id`, `parent_task_id`, `cause_task_id`, and
`channel_id`, then `attributes` with `scheduler_sequence`. An identity of 0
is `null`. The process and clock families add these fields. `project`
reads them. `runtime.c` writes the clock family (ts-6ccbc9 decision 6) and,
from U5, the process family:

- The envelope's `capacity`, `queue_depth`, `waiter_depth`, and
  `discard_count` attributes stay on process and clock events, and are 0.
- Process family (3). The event names are the transcript table's names, in
  kind order, with `supervisor_started` as kind 10. The top level adds
  `supervisor_id`, `membership_id`, `incarnation_id`, and
  `parent_supervisor_id`. `task_id` is the incarnation's task on
  `process_started` and `null` otherwise. `cause_task_id` is `null`, because
  the process family's `cause` is a code, not a task. `attributes` adds:
  - `time_ns`, the event's `time`;
  - `cause`, the terminal cause word for `process_terminated`
    (`CHIMI_PROCESS_CAUSE_*`: `returned`, `failed`, `defected`,
    `cancelled`, `forced`), or the reason word for `admission_rejected`
    (`CHIMI_ADMISSION_*`: `capacity_reached`, `name_taken`, `stopped`,
    `start_failed`);
  - `name`, the label `<module path>.<identifier>`. `runtime.c` resolves the
    event's name ID to this label through the program's name table;
  - `address_kind`, `named` for `CHIMI_ADDRESS_NAMED` (1) or `direct` for
    `CHIMI_ADDRESS_DIRECT` (2);
  - `outcome`, `accepted`, `full`, or `unavailable` (`CHIMI_SEND_*`);
  - `attempts_in_window`;
  - `defect`, on a `process_terminated` whose cause is `defected`: the
    defect's message, or `Process stack overflow` when the body overflowed
    its stack (the scheduler marks that end with
    `CHIMI_PROCESS_DEFECT_STACK_OVERFLOW` in the event's `outcome`). It is
    `null` on every other event, and also for a defect the runtime did not
    see, such as a child task's during shutdown. Transcripts keep the
    prototype's fields and do not print it; `project` checks that it is
    `null` unless the cause is `defected` (U5 decision).

  A field the kind does not set is `null`, and `attempts_in_window` is 0.
- Clock family (4): `clock_advanced`, `clock_timer_started`,
  `clock_timer_fired`, `clock_timer_cancelled`, and
  `clock_deadlock_waiter`. `task_id` is the timer's or waiter's task, or
  `null`. The scheduler stores a waiter's request kind in the event's
  `cause` field, so `cause_task_id` is `null` and `attributes` adds
  `request_kind`, read from `cause` (0 except on `clock_deadlock_waiter`),
  and `time_ns`.

The projector rejects a kept or clock event whose keys differ from these
sets.

### Determinism obligations on the runtime

Exact transcripts need these runtime rules. ts-6ccbc9 implements them, and
the lab depends on them:

- One carrier thread. The ready queue is FIFO.
- A process woken by an accepted message joins the tail of the ready queue at
  the moment of acceptance.
- A new incarnation is registered and routable when it is created, and its
  first body run joins the tail of the ready queue.
- Supervisor decisions run in the scheduler between fiber resumes. A defect,
  its termination, the restart charge, and the replacement's creation happen
  in one step, with no fiber running in between.
- Supervisor, membership, and incarnation identities each use their own
  counter, starting at 1 in creation order, separate from task identities. A
  rejected start allocates no membership.
- An incarnation cancelled before its body first runs terminates with cause
  `cancelled`.

### Virtual time in fixtures

Every runtime fixture runs under `--clock=virtual`
([virtual time](time-and-outcomes.md#virtual-time)). Restart windows and
`shutdown_within` deadlines use the execution time source, so they follow
virtual time without a clock dependency on the supervisor.

Fixtures use one barrier to let every runnable process finish its work:

```chimi
fn settle() -> Void needs { clock: Clock } {
  Clock.sleep(Duration.milliseconds(1))
}
```

Virtual time advances only when nothing is runnable. When `settle` returns,
every process woken before the call has run until it parked, and virtual time
has moved forward exactly 1 ms. Fixtures cross a restart window with a longer
sleep, such as `Clock.sleep(Duration.seconds(30))`.

Races are forced by message order, not by timing. A helper process that holds
an exclusive `ManagedProcess` or the shareable `Running` performs a
supervisor operation when its message arrives. FIFO wakeup then fixes the
interleaving. RR02 shows the pattern.

## Backends

- Static and Core scenarios are backend-independent. They run once, through
  the shared frontend.
- Runtime, heap, observability, and forcing scenarios gate on the native AOT
  backend.
- The reference interpreter has no Task, Channel, or process runtime today.
  `execution.rs` defects with "requires the finite Task runtime". IP01
  requires the interpreter to reject process programs the same way, with
  "requires the supervised process runtime". The lab reports each runtime
  fixture on the interpreter (`--backend interpreter`) as `unsupported`,
  never as `pass`. IP01 itself always runs on the interpreter, whatever
  `--backend` names: it executes its fixture in process through the
  interpreter (`chimi run`'s path) and compares `expected.outcome` as for a
  native run, with exit code 70 for its defect.
- When the interpreter gains a scheduler, every runtime fixture must produce
  the same `expected.outcome` and `expected.transcript` on both backends, with
  no backend-specific expected files. See open question 1.

## Scenario catalogue

Each table lists the scenario, its unlock stage, and the wrong
implementation it rejects. [Catalogue data](#catalogue-data) gives each
scenario's form, exit code, runs, and extra events. Forms:

- **fixture**: a Chimi project with expected files.
- **C**: a white-box C harness under `tests/process-lab/c/`.
- **check**: a Rust check with no fixture of its own: CO01 to CO04 over
  verified Core, MF06 over built executables, OB04 over other fixtures'
  telemetry, and OB07 over the trace ABI.
- **gate**: a measured performance gate.

Before a runtime or heap scenario unlocks, ts-6ccbc9 breaks the guarded
mechanism on purpose and confirms that the scenario fails, as the prototype
did for each of its tests.

### Static: compile-pass

| ID | Unlock | Program | Expected |
| --- | --- | --- | --- |
| SP01 | U1 | The static-contract example, with two members of different message types in one policy | Compiles |
| SP02 | U2 | `Process.spec` inside `using { store: memory_store }` with a shareable provider | Compiles. The inferred contract of the enclosing function shows the `store` need, and the `Process.Spec` value carries none |
| SP03 | U1 | `case Supervisor.start_process(root, spec, Restart.Never)` matching `CapacityReached`, `NameTaken(_)`, and `Stopped` | Compiles |
| SP04 | U2 | `Running` sent to a process in a message and used there to start a sibling | Compiles |
| SP05 | U2 | `ManagedProcess` sent through a channel to another task and removed there | Compiles |
| SP06 | U2 | A body that captures an `Address` and a `Running` | Compiles |
| SP07 | U1 | `Process.spec(body, 1, Duration.zero)` and `Supervisor.restarts(0, Duration.seconds(1))` | Compiles. Zero deadline and zero limit are in range |
| SP08 | U1 | Rendered contracts of `Process.send`, `Supervisor.start`, and `Supervisor.start_process` | `errors SendError<M>`, `errors StartError.NameTaken`, and the three-variant row, exactly as in the static contract |
| SP09 | U1 | Two modules that each declare `process name counter` with different message types, both used from `app/main` | Compiles. The names are distinct declarations |

### Static: compile-fail

| ID | Unlock | Program | Code |
| --- | --- | --- | --- |
| SF01 | U2 | Body needs a provider that is not shareable | `T4260`, names the provider |
| SF02 | U2 | Body captures a `Channel.Receiver` | `T4260`, names the capture |
| SF03 | U2 | Body captures a `ManagedProcess` | `T4260`, names the capture |
| SF04 | U1 | `process name` whose message contains a `Task` handle | `T4261` |
| SF05 | U1 | `Process.spec` body whose message type contains a `Task` handle, with no `process name` | `T4261` |
| SF06 | U1 | `Process.register(spec, local_copy_of_counter)` | `T4262` |
| SF07 | U1 | `Process.register(spec, Supervisor.address(managed))` | `T4262` |
| SF08 | U1 | A helper `fn named(spec, name: Address<M>)` that calls `Process.register(spec, name)` | `T4262`. A parameter is not a direct reference |
| SF09 | U1 | `Process.spec(body, 0, Duration.seconds(5))` | `T4263` |
| SF10 | U1 | `Supervisor.policy(members, 0, restarts)` | `T4263` |
| SF11 | U1 | `Supervisor.restarts(-1, Duration.seconds(30))` | `T4263` |
| SF12 | U1 | A members list literal of 3 with a literal capacity of 2 | `T4263` |
| SF13 | U1 | `Process.spec(body, 1, Duration.seconds(-1))` | `T4263` (extension from the time contract) |
| SF14 | U1 | `Supervisor.restarts(1, Duration.zero)` | `T4263` (extension from the time contract) |
| SF15 | U1 | Two members register `counter` in one policy literal | `T4264` |
| SF16 | U1 | `Process.send(counter, CacheMessage.Clear)` | type mismatch |
| SF17 | U1 | Registering a name whose message type differs from the body's | type mismatch |
| SF18 | U1 | A `Process.Spec` placed directly in the members list | type mismatch |
| SF19 | U2 | `Supervisor.address(managed)` after `remove_process(root, managed)` | `T4241` |
| SF20 | U2 | Using a `Receiver` after sending it inside a process message | `T4241` |
| SF21 | U2 | `Task.start(fn() { Process.receive(inbox) })` inside a body | `T4240` |
| SF22 | U2 | Returning the inbox from a helper beyond the body | `T4240` |
| SF23 | U2 | `Process.send(other, Message.Forward(inbox))` | `T4243` |
| SF24 | U2 | A loop pass that removes a managed handle bound outside the loop | `T4250` |
| SF25 | U1 | A `start_process` case that omits `Stopped`, matched as `Failure(error) -> case error { ... }`: a `Failure(variant)` arm list that leaves a variant out lets it propagate, so coverage is checked on the failure value | non-exhaustive match (`T4231`) |
| SF26 | U1 | A `case Supervisor.start(policy)` arm `Failure(Supervisor.StartError.CapacityReached)` | unreachable arm: the variant is absent from the closed row, `T4224` (patterns FP-02) |

### Verified Core

| ID | Unlock | Check | Rejects |
| --- | --- | --- | --- |
| CO01 | U2 | Each `process name` lowers to a constant address node with the declaration's symbol identity. SP09's two `counter` names have different identities | Names keyed by text |
| CO02 | U2 | The second argument of every `Process.register` in Core is such a constant node | A register that accepts a computed address |
| CO03 | U2 | Process and supervisor operations are intrinsic calls with standard symbol identity. A user function `send` in a user module `process` lowers to an ordinary call | Runtime entry points reachable by name |
| CO04 | U2 | The Core verifier rejects a hand-built Core program that produces a `Process.Inbox` outside a body parameter | A verifier that trusts the frontend |

### Runtime: mailbox and delivery

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| MB01 | U5 | Main and a relay process both send to one process. The process records arrival order and reports it | Per-sender queues, or delivery not in acceptance order |
| MB02 | U5 | Capacity 2. The third send returns `Full(message)`, and main reads the amount back from the returned message. Transcript shows `outcome=full incarnation=-` | Ignored capacity, or a lost message on `Full` |
| MB03 | U5 | A body sends to a full peer from inside its own fiber, gets `Full`, and keeps running | A send that parks |
| MB04 | U5 | Declared member X sends to later member Y on X's first run, before Y's first run | Registration after the body starts |
| MB05 | U6 | `Crash` and then `Add(5)` are both accepted before the process runs. The replacement reports 0 | Unread messages carried into the replacement |
| MB06 | U5 | After the body returns, a send returns `Unavailable(message)` with the original value | Sends accepted by a terminated incarnation |
| MB07 | U5 | After the body ends with `Failure(E)`, sends return `Unavailable`, and main's outcome is `success` | A process failure that reaches the supervising function |
| MB08 | U7 | Removal begins with three queued messages. `Process.receive` returns all three, then cancellation ends the body. The body reports the drained values on a channel. Cause `cancelled` | The coordinator rule for `ts-a6b43b`: receive drains queued messages, then reports cancellation |
| MB09 | U7 | While draining, the body sends to its own named address and to a direct address of itself. Both return `Unavailable` | Routing kept during cleanup |
| MB10 | U6 | `Process.spec(body, n, ...)` with non-literal `n = 0` | Defect at the `Process.spec` call, message names the capacity. See open question 3 |
| MB11 | U5 | One body ends each way: return, `Failure`, defect. Main observes only lifecycle events and finishes with `success` | Terminal outcomes in the supervisor's failure channel |

### Runtime: routing

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| RT01 | U6 | A named address reaches the replacement. A direct address taken before the defect returns `Unavailable` forever. A new `Supervisor.address` reaches the replacement | Retargeted direct addresses, or names held after termination |
| RT02 | U6 | A `Restart.Never` member defects. Named sends then return `Unavailable` | A name that outlives its incarnation |
| RT03 | U7 | `start_process` with a held name returns `NameTaken("app/main.counter")`. Transcript shows `admission_rejected reason=name_taken name=app/main.counter` | Two holders of one name, or a text that differs from the trace label |
| RT04 | U7 | Two modules each declare `process name counter`. Both register at once. Labels are `app/left.counter` and `app/right.counter` | Name collision by text |
| RT05 | U7 | After `remove_process`, a new process registers the same name and receives named sends | A name kept after removal |
| RT06 | U7 | A second root's declared member takes a held name. `Supervisor.start` returns `NameTaken` after the members it already started are stopped | A partial root left running |
| RT07 | U7 | `Process.register` applied twice to one spec with a non-literal flow | Defect "spec already has a name" |
| RT08 | U5 | An address stored in a struct, sent in a message, and used by another process. Each send records its kind | Addresses that are not shareable values |

### Runtime: supervisor lifecycle and admission

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| SV01 | U5 | `Supervisor.start` of three declared members. `process_started` events appear in list order | Unordered initial start |
| SV02 | U7 | Capacity 2. A third start returns `CapacityReached` with an `admission_rejected` event. After one removal, a start succeeds | Waiting admission, or a slot kept after removal |
| SV03 | U6 | `OnDefect` members that return, fail, and are cancelled are never restarted. Each frees its slot, shown by a later start at capacity | Restart on any terminal end |
| SV04 | U6 | A `Restart.Never` member defects. No `restart_attempted`, and its slot is freed | Restart without `OnDefect` |
| SV05 | U7 | A member holding `Running` calls `Supervisor.stop`. Main's `Supervisor.wait(root)` returns `Void`. The calling member terminates `cancelled` as part of the stop | A stop that waits for its own caller |
| SV06 | U7 | After `stop`, `start_process` returns `Stopped`. `remove_process` and a second `stop` return `Void` and emit nothing | Errors or duplicate events on a stopped supervisor |
| SV07 | U7 | A member uses a shared `Running` to start a sibling dynamic process | `Running` bound to one task |
| SV08 | U7 | A `ManagedProcess` sent through a channel to a task and removed there | Managed handles bound to their creator |
| SV09 | U9 | A declared nested supervisor starts its members after its parent's earlier members, and `stop` stops it in reverse order with its subtree | Nested supervisors as flat members |
| SV10 | U9 | `start_supervisor` and `remove_supervisor` on a dynamic nested supervisor | Dynamic nested membership, which the prototype did not model |
| SV11 | U6 | Non-literal out-of-range values defect at the call that receives them: capacity at `Process.spec`, limit and window at `Supervisor.restarts`, members over capacity at `Supervisor.policy`, negative `shutdown_within` at `Process.spec` | Late or missing range checks |
| SV12 | U7 | `remove_process` through a `Running` that does not own the handle | Defect naming both supervisors |

### Runtime: restart accounting

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| RA01 | U6 | One member defects. Its replacement starts with fresh state, and the peer keeps its state. Transcript order is `process_terminated`, `restart_attempted attempts_in_window=1`, `process_started` | Peers restarted, state kept, or the restart charged after the start |
| RA02 | U6 | `restarts(2, 30s)`. A defects at 0s and B at 10.001s. A's second defect, at 29.999s, is the third in the window and escalates | A limit per member instead of per supervisor, or a window that drops a restart early |
| RA03 | U6 | `restarts(1, 30s)`. Defect, sleep 31 s, defect. Both restart, and each shows `attempts_in_window=1` | A window that never rolls |
| RA04 | U6 | `restarts(2, 30s)`. Defects at 0s, 10.001s, and 30s all restart, the third with `attempts_in_window=2`: at exactly `within` after it, the 0s restart is outside the window, as in the prototype, and the 10.001s one still counts | Off-by-one at the window edge, or a tumbling window that forgets every restart once the oldest expires |
| RA05 | U6 | `restarts(0, 30s)`. The first defect escalates while main is runnable: a child task sends `Crash`, then wakes main through a channel | A free first restart, or an escalation that ends only parked root tasks |
| RA06 | U7 | C harness: the replacement's start fails by injected fault. The attempt shows in `attempts_in_window`, and the next defect counts it | Uncharged failed replacements |
| RA07 | U8 | A body starts a child task that sleeps, then defects. The child's `task_terminal` (cancelled, so `cause=-`) appears before the replacement's `process_started` (`extra_events = ["task_terminal"]`) | Overlap of a child task with the replacement |
| RA08 | U6 | 1,000 defect cycles, with a 31 s sleep after every second one. The lab runs it twice and compares telemetry byte for byte, ignoring `execution_id`. It also checks the counts of each event kind (`expected.events`) | Nondeterminism under a restart storm |

RA06 as built (U7): `kernel_faults.c` drives the scheduler's C ABI with C
process bodies and links the bundled `libchimi_fiber.a`. The scheduler
allocates each fiber stack with one `PROT_NONE` anonymous `mmap`, so the
harness defines `mmap`, which the static archive binds to, and fails the next
such call. The replacement then fails to start as it would without stack
memory: attempt 1 shows `admission_rejected reason=start_failed`, attempt 2
starts, and the member's next defect is attempt 3 under `restarts(3, 30s)`.
The scheduler gains no fault-injection entry point.

An escalation that reaches the root ends the program with the defect
`Supervisor.RestartLimitExceeded` (exit code 70), whatever main is doing
when it happens. The scheduler ends every root task that is parked or
runnable with its restart-limit code, and the runner reports that code as
this defect (U6 decision). Main is parked in `Supervisor.wait` in ES01 and
ES05, parked in a sleep in RA02, and runnable (just woken by a channel send)
in RA05.

### Runtime: escalation

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| ES01 | U6 | The root exceeds its limit while main is parked in `Supervisor.wait`. Exact transcript below. The outcome is a defect with message `Supervisor.RestartLimitExceeded`, exit code 70 | Peers left running, a recorded refused attempt, or a `wait` that returns |
| ES02 | U9 | A nested supervisor declared `OnDefect` exceeds its limit. The parent records one `restart_attempted` for it, and its declared members start fresh. Peers of the nested supervisor keep their state | Escalation that skips the parent |
| ES03 | U9 | The same with `Restart.Never`. The subtree stays stopped, and the parent and its other members continue | Restart of a `Never` supervisor |
| ES04 | U9 | Nested escalation exceeds the parent's own limit. The parent escalates, then the root terminates the program | A chain that stops at one level |
| ES05 | U6 | An escalating supervisor with dynamic members stops them newest first, then declared members in reverse, before `supervisor_escalated` | Escalation in another order |

Expected transcript for ES01. Members `counter` and `peer` are declared with
`OnDefect`, under `restarts(1, 30s)`. Main sends `Crash` to `counter`,
calls `settle`, sends `Crash` again, and calls `Supervisor.wait(root)`:

```text
@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.counter
@0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.peer
@0s process_send kind=named name=app/main.counter incarnation=I1 outcome=accepted
@0s process_terminated membership=M1 incarnation=I1 cause=defected
@0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1
@0s process_started supervisor=S1 membership=M1 incarnation=I3 name=app/main.counter
@1ms process_send kind=named name=app/main.counter incarnation=I3 outcome=accepted
@1ms process_terminated membership=M1 incarnation=I3 cause=defected
@1ms restart_limit_exceeded supervisor=S1
@1ms process_terminated membership=M2 incarnation=I2 cause=cancelled
@1ms supervisor_escalated supervisor=S1 parent=root
```

### Runtime: removal races

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| RR01 | U7 | Removing a parked process: `process_terminated cause=cancelled`, then `membership_removed`. The name is free when `remove_process` returns | Removal that returns before termination |
| RR02 | U7 | Removal while the replacement exists but has not run. Exact transcript below | A replacement that survives removal, or an uncharged attempt |
| RR03 | U7 | Removal begins, and the body defects while it drains a queued `Crash`. Cause `defected`, no `restart_attempted`, then `membership_removed` | Restart of a membership under removal |
| RR04 | U7 | Removing a membership whose body already returned. `membership_removed` only, with no second `process_terminated` | Double termination |
| RR05 | U8 | Removal while the target is parked in `Clock.sleep(Duration.seconds(60))` inside a message handler. The sleep is interrupted, the cause is `cancelled`, and virtual time does not advance to the sleep's deadline. Pins open question 2 | A removal that waits for a sleeping handler |
| RR06 | U8 | `stop` begins. The newest member drains a queued request and removes an older sibling. The sibling terminates once, then `membership_removed`, and `stop` skips it | A sibling terminated twice, or removal blocked by an ongoing stop |

RR02 pattern. P is named `p` with `OnDefect`. Q is named `q` with
`Restart.Never`, and its message carries `Running` and P's `ManagedProcess`.
R is unnamed with `OnDefect`. The root uses `restarts(2, 30s)`:

```chimi
let managed_p = Supervisor.start_process(root, spec_p, Supervisor.Restart.OnDefect)
let managed_q = Supervisor.start_process(root, spec_q, Supervisor.Restart.Never)
Process.send(p, PMessage.Crash)
Process.send(q, QMessage.Remove(root, managed_p))
settle()
let managed_r = Supervisor.start_process(root, spec_r, Supervisor.Restart.OnDefect)
Process.send(Supervisor.address(managed_r), PMessage.Crash)
settle()
Supervisor.stop(root)
```

P wakes first and defects. The kernel creates I3 at the tail of the ready
queue, behind Q. Q runs and removes P before I3 ever runs:

```text
@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.p
@0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.q
@0s process_send kind=named name=app/main.p incarnation=I1 outcome=accepted
@0s process_send kind=named name=app/main.q incarnation=I2 outcome=accepted
@0s process_terminated membership=M1 incarnation=I1 cause=defected
@0s restart_attempted supervisor=S1 membership=M1 attempts_in_window=1
@0s process_started supervisor=S1 membership=M1 incarnation=I3 name=app/main.p
@0s process_terminated membership=M1 incarnation=I3 cause=cancelled
@0s membership_removed membership=M1
@1ms process_started supervisor=S1 membership=M3 incarnation=I4
@1ms process_send kind=direct incarnation=I4 outcome=accepted
@1ms process_terminated membership=M3 incarnation=I4 cause=defected
@1ms restart_attempted supervisor=S1 membership=M3 attempts_in_window=2
@1ms process_started supervisor=S1 membership=M3 incarnation=I5
@2ms process_terminated membership=M3 incarnation=I5 cause=cancelled
@2ms process_terminated membership=M2 incarnation=I2 cause=cancelled
@2ms supervisor_stopped supervisor=S1
```

`attempts_in_window=2` for R proves that the interrupted attempt for P still
consumed budget.

### Runtime: shutdown and forced termination

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| SD01 | U5 | Two declared and two dynamic members. `stop` terminates dynamic members newest first, then declared members in reverse, then `supervisor_stopped` | Forward or mixed order |
| SD02 | U8 | `shutdown_within = Duration.zero` with three queued messages. Cause `forced`. No drained value reaches the body's report channel, and `fragments_live` is 0 at exit | Draining past a zero deadline, or leaked unread messages |
| SD03 | U8 | `shutdown_within = 5s` with three queued messages. The body drains all three, cause `cancelled`, and virtual time does not move | A deadline that waits when cleanup is done |
| SD04 | U8 | C harness: a body that parks again after cancellation is forced at `now + shutdown_within`. Virtual time jumps to the deadline. Cause `forced`. The body runs no code after the forced reply | Fiber destruction while parked (it aborts the process), or code that runs after forcing |
| SD05 | U8 | Each member owns a sleeping child task. During `stop`, each member's children terminate before the next member's `process_terminated` (`extra_events = ["task_terminal"]`) | Children left running across members |
| SD06 | U8 | `stop` begins. The oldest member, stopped last, drains a message and sends to a named sibling that is still running. The send returns `Unavailable` | Routing removed member by member instead of for the whole subtree first |
| SD07 | U8 | `OnDefect` members ended by `cancelled` and by `forced` are not restarted | Shutdown treated as a defect |
| SD08 | U7 | A member calls `Supervisor.stop` on its own root. The stop completes and the member ends `cancelled` | Self-deadlock |
| SD09 | U5 | A second `stop` returns `Void` and emits nothing | Duplicate stop events |

### Heap: copy-boundary safety

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| CB01 | U5 | Main sends a list of 1,000 Ints, then runs 10,000 loop passes that allocate and drop garbage. The process then reports the list's sum correctly | A message that points into the sender's reclaimed region |
| CB02 | U5 | A message whose graph has 2^20 paths and 21 distinct values. The memory report shows `copied_values = 21` | A copy that ignores sharing |
| CB03 | U5 | `Full` and `Unavailable` return a value equal to the original, usable by the sender. `fragments_live = 0` at exit | A lost or leaked failed send |
| CB04 | U7 | A process puts a reply on a buffered channel, and main removes the process before receiving. The received value is intact | A channel value that points into a destroyed heap |
| CB05 | U8 | A body reads `Task.result` of a child after the child's heap is destroyed | A task result that points into a destroyed heap |
| CB06 | U3 | C harness: a graph in heap A that points into heap B. Fragment copy and region pass both return the foreign-pointer status and do not follow the pointer | Following a pointer into another heap |
| CB07 | U5 | C harness: `Process.send` with a foreign pointer in the message. The sender defects with a message that names both owners. The receiver's mailbox is unchanged, and no `process_send` is recorded | A foreign pointer that reaches a mailbox, or a defect charged to the receiver |
| CB08 | U6 | A body whose provider holds a closure keeps working after a restart destroyed the first incarnation's heap | Providers shared between incarnations |

### Heap: memory flatness

These scenarios compare exact counts from `--memory-report-file`. The memory
report gains these fields in ts-6ccbc9: `heap_chunks_live`,
`heap_chunk_bytes_peak`, `fragments_live`, `fragment_bytes_peak`,
`process_heap_peak_value_bytes` (the maximum over all incarnations),
`copied_values`, `copied_bytes`, and `trace_events_constructed`. Fixtures
take their size from `main(arguments)`. As built in `runtime.c` (U5):

- `runtime.c` has no chunks: an allocation is its unit of heap memory.
  `heap_chunks_live` is the live allocations at exit, in every heap and
  fragment, and `heap_chunk_bytes_peak` is `peak_footprint_bytes`.
- `fragments_live` is the fragments live at exit, and `fragment_bytes_peak`
  is `peak_fragment_footprint_bytes`.
- `process_heap_peak_value_bytes` is the largest peak of live payload bytes
  of any incarnation's heap.
- `copied_values` counts the values `Process.send` copied into messages, and
  `copied_bytes` their payload bytes. Other copies (task captures and
  results, channel messages, each incarnation's copy of its spec) do not
  count, so CB02 measures one message.
- `trace_events_constructed` counts the telemetry events the runner built,
  its own and the scheduler's.

MF01 rounds N down to whole batches of 64 (9,984 for 10,000), so that both
runs repeat one batch shape; a final partial batch changes the incarnation's
peak by a few bytes without any growth. OB01 runs its own fixture at U5,
with the forcing program's ingredients (named and direct sends of every
outcome, a dynamic member, a stop); FP01 adds the same check at U10.

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| MF01 | U5 | One process receives N Int messages in batches of 64, for N = 10,000 and N = 1,000,000. `process_heap_peak_value_bytes` and `heap_chunks_live` at exit are equal for both N | Memory that grows with uptime |
| MF02 | U10 | N restart cycles, for N = 100 and N = 10,000, with the window rolled by virtual sleeps. `heap_chunks_live` and `fragments_live` at exit are equal | Leaks per restart |
| MF03 | U10 | N `start_process` and `remove_process` cycles, for N = 100 and N = 10,000. Equal `heap_chunks_live`, and no live memberships at exit | Leaks per membership |
| MF04 | U5 | 64 queued Int messages hold less than 256 KiB of `fragment_bytes_peak` | One 64 KiB chunk per small fragment. Coordinator decision 6 on `ts-a6b43b` requires small-fragment slabs |
| MF05 | U3 | A program that uses tasks but no processes runs a 1,000,000-pass loop with equal peak live bytes at 10,000 passes | Loop reclamation still disabled under `CHIMI_REQUIRES_SCHEDULER` (`prog show ts-da83af`) |
| MF06 | U5 | Build three programs: no concurrency, tasks only, and processes. Only the process program links process-kernel symbols, and only the last two link `libchimi_fiber.a`. As built, the process kernel is the runtime variant compiled with `CHIMI_PROCESS_KERNEL`, whose `chimi_rt_process_entry` only process programs define; `chimi_fiber_abi_version` marks the scheduler archive. The scheduler's own process code is part of `libchimi_fiber.a` and is not split out | Pay-for-play linking broken |

### Observability

| ID | Unlock | Scenario | Rejects |
| --- | --- | --- | --- |
| OB01 | U5 | The forcing program under `--trace=off` reports `trace_events_constructed = 0` and an empty telemetry file | Events built and then dropped |
| OB02 | U5 | `boundaries` mode has lifecycle events and no `process_send` | Send events in boundary mode |
| OB03 | U5 | `full` mode records `kind`, `name`, `incarnation`, and `outcome` on every send | Missing routing fields |
| OB04 | U6 | Every transcript fixture runs twice. Telemetry is byte-identical once `execution_id` is removed | Nondeterministic scheduling or identities |
| OB05 | U5 | Process, task, and channel events share one strictly increasing `sequence` | A separate sequence for process events |
| OB06 | U6 | A fixture with 30 s windows and 5 s deadlines finishes in less than 2 s of wall time | Real waiting under virtual time |
| OB07 | U5 | The trace ABI reports `CHIMI_FIBER_ABI_VERSION` 6 (5 until the capability-violation hook joined the scheduler config). Process events carry dedicated `supervisor`, `membership`, and `incarnation` fields, separate from `task_id` | Identities carried in task fields |
| IP01 | U5 | `chimi run` on a process fixture defects with "requires the supervised process runtime" | Silent divergence between backends |

As built at U6:

- OB04 covers every transcript fixture whose unlock stage is at or below
  `CURRENT_STAGE`: two `--trace=full` runs of one build, with the scenario's
  first argument when it has runs. A fixture's own run makes the second full
  run on the executable it built and keeps the result for OB04 in the lab
  process. OB04 builds only the fixtures that process has not run, so
  `chimi-process-lab all` (which `tests/process_lab.rs` runs once) builds each
  fixture once.
- OB06 runs 2m35s of virtual time (five restart cycles of 31 s, with 5 s
  deadlines). The lab's 60 s limit on every run fails a runtime that waits for
  real. The default suite asserts no wall-clock bound, because the machine
  may be loaded; the 2 s bound belongs to the performance gates
  (`ts-1765ba`).

### Catalogue data

`catalogue.rs` must match this table, and a unit test checks that it does.
A scenario not listed is a `fixture` with exit code 0, one run with no
arguments, and no extra events. `Runs` lists each run's argument.

| ID | Form | Exit code | Runs | Extra events |
| --- | --- | --- | --- | --- |
| CO01–CO04 | check | 0 | - | - |
| MF06 | check | 0 | - | - |
| OB04 | check | 0 | - | - |
| OB07 | check | 0 | - | - |
| PG01–PG10 | gate | 0 | - | - |
| RA06 | C `kernel_faults.c` | 0 | - | - |
| SD04 | C `kernel_faults.c` | 0 | - | - |
| CB06 | C `heap_boundary.c` | 0 | - | - |
| CB07 | C `heap_boundary.c` | 0 | - | - |
| MB10 | fixture | 70 | - | - |
| RT07 | fixture | 70 | - | - |
| SV11 | fixture | 70 | `capacity`, `limit`, `window`, `members`, `shutdown_within` | - |
| SV12 | fixture | 70 | - | - |
| RA02 | fixture | 70 | - | - |
| RA05 | fixture | 70 | - | - |
| ES01 | fixture | 70 | - | - |
| ES04 | fixture | 70 | - | - |
| ES05 | fixture | 70 | - | - |
| IP01 | fixture | 70 | - | - |
| MF01 | fixture | 0 | `10000`, `1000000` | - |
| MF02 | fixture | 0 | `100`, `10000` | - |
| MF03 | fixture | 0 | `100`, `10000` | - |
| MF05 | fixture | 0 | `10000`, `1000000` | - |
| RA07 | fixture | 0 | - | `task_terminal` |
| SD05 | fixture | 0 | - | `task_terminal` |

## Forcing program

FP01 is `programs/supervised-pool`. It unlocks at U10. Three named workers
run under one root with capacity 3. One worker defects and restarts while its
peers keep their state. The program also forces one fail-fast admission, one
stale direct address, one message lost with a defect, one removal, and the
final stop. Each of those rules has its own scenario above. FP01 proves that
they compose.

### Program

Source spelling follows the current contracts and stays provisional.
`Process.register` takes each `process name` directly, because passing it
through a helper parameter is `T4262` (SF08).

```chimi
import std/channel as Channel
import std/channel.{Receive, Sender}
import std/clock as Clock
import std/clock.{Clock}
import std/duration as Duration
import std/process as Process
import std/process.{Inbox, SendError}
import std/supervisor as Supervisor
import std/supervisor.{StartError}

pub enum WorkerMessage {
  Add(amount: Int)
  Crash
  Report(reply: Sender<Int>)
}

pub process name worker_a: WorkerMessage
pub process name worker_b: WorkerMessage
pub process name worker_c: WorkerMessage

struct PoolResult {
  a: Int,
  b: Int,
  c: Int,
  stale: Int,
  rejected: Int,
}

fn worker(inbox: Inbox<WorkerMessage>) -> Void {
  loop total = 0 {
    case Process.receive(inbox) {
      WorkerMessage.Add(amount) -> total + amount
      WorkerMessage.Crash -> total / 0
      WorkerMessage.Report(reply) -> {
        Channel.send(reply, total)
        total
      }
    }
  }
}

fn settle() -> Void needs { clock: Clock } {
  Clock.sleep(Duration.milliseconds(1))
}

fn reply_value(received: Receive<Int>) -> Int {
  case received {
    Receive.Received(value) -> value
    Receive.Drained -> -1
  }
}

pub fn main() -> PoolResult needs { clock: Clock } {
  let pool = Supervisor.policy([], 3, Supervisor.restarts(2, Duration.seconds(30)))
  let root = Supervisor.start(pool)
  let a = Supervisor.start_process(
    root,
    Process.spec(worker, 8, Duration.seconds(1)) |> Process.register(worker_a),
    Supervisor.Restart.OnDefect,
  )
  let b = Supervisor.start_process(
    root,
    Process.spec(worker, 8, Duration.seconds(1)) |> Process.register(worker_b),
    Supervisor.Restart.OnDefect,
  )
  let c = Supervisor.start_process(
    root,
    Process.spec(worker, 8, Duration.seconds(1)) |> Process.register(worker_c),
    Supervisor.Restart.OnDefect,
  )
  let rejected = case Supervisor.start_process(
    root,
    Process.spec(worker, 8, Duration.seconds(1)),
    Supervisor.Restart.Never,
  ) {
    Success(extra) -> {
      Supervisor.remove_process(root, extra)
      0
    }
    Failure(error) -> case error {
      StartError.CapacityReached -> 1
      StartError.NameTaken(_) -> 0
      StartError.Stopped -> 0
    }
  }

  Process.send(worker_a, WorkerMessage.Add(1))
  Process.send(worker_b, WorkerMessage.Add(10))
  Process.send(worker_c, WorkerMessage.Add(100))
  settle()

  let b_first = Supervisor.address(b)
  Process.send(b_first, WorkerMessage.Crash)
  Process.send(worker_b, WorkerMessage.Add(20))
  settle()

  Process.send(worker_a, WorkerMessage.Add(2))
  Process.send(worker_b, WorkerMessage.Add(30))
  Process.send(worker_c, WorkerMessage.Add(200))
  let stale = case Process.send(b_first, WorkerMessage.Add(99)) {
    Success(_) -> 0
    Failure(error) -> case error {
      SendError.Full(_) -> -1
      SendError.Unavailable(message) -> case message {
        WorkerMessage.Add(amount) -> amount
        WorkerMessage.Crash -> -2
        WorkerMessage.Report(_) -> -2
      }
    }
  }

  let replies = Channel.bounded(3)
  let reply_sender = replies.sender
  let reply_receiver = replies.receiver
  let reply_completion = replies.completion
  Process.send(worker_a, WorkerMessage.Report(reply_sender))
  Process.send(worker_b, WorkerMessage.Report(reply_sender))
  Process.send(worker_c, WorkerMessage.Report(reply_sender))
  let total_a = reply_value(Channel.receive(reply_receiver))
  let total_b = reply_value(Channel.receive(reply_receiver))
  let total_c = reply_value(Channel.receive(reply_receiver))
  Channel.close(reply_completion)

  Supervisor.remove_process(root, c)
  Supervisor.stop(root)
  PoolResult(total_a, total_b, total_c, stale, rejected)
}
```

### Expected outcome

Exit code 0, and this outcome line:

```json
{"schema":"chimi.runtime.v1","outcome":"success","value":{"type":"PoolResult","fields":{"a":3,"b":30,"c":300,"stale":99,"rejected":1}}}
```

- `a = 3` and `c = 300`: the peers kept their state across B's defect.
- `b = 30`: the replacement started from 0. `Add(20)` was accepted by I2
  behind `Crash` and lost with it.
- `stale = 99`: the direct address to I2 returned the unsent message.
- `rejected = 1`: the fourth start failed fast at capacity 3.

### Expected transcript

Under `--clock=virtual --trace=full`:

```text
@0s process_started supervisor=S1 membership=M1 incarnation=I1 name=app/main.worker_a
@0s process_started supervisor=S1 membership=M2 incarnation=I2 name=app/main.worker_b
@0s process_started supervisor=S1 membership=M3 incarnation=I3 name=app/main.worker_c
@0s admission_rejected supervisor=S1 reason=capacity_reached
@0s process_send kind=named name=app/main.worker_a incarnation=I1 outcome=accepted
@0s process_send kind=named name=app/main.worker_b incarnation=I2 outcome=accepted
@0s process_send kind=named name=app/main.worker_c incarnation=I3 outcome=accepted
@1ms process_send kind=direct incarnation=I2 outcome=accepted
@1ms process_send kind=named name=app/main.worker_b incarnation=I2 outcome=accepted
@1ms process_terminated membership=M2 incarnation=I2 cause=defected
@1ms restart_attempted supervisor=S1 membership=M2 attempts_in_window=1
@1ms process_started supervisor=S1 membership=M2 incarnation=I4 name=app/main.worker_b
@2ms process_send kind=named name=app/main.worker_a incarnation=I1 outcome=accepted
@2ms process_send kind=named name=app/main.worker_b incarnation=I4 outcome=accepted
@2ms process_send kind=named name=app/main.worker_c incarnation=I3 outcome=accepted
@2ms process_send kind=direct incarnation=- outcome=unavailable
@2ms process_send kind=named name=app/main.worker_a incarnation=I1 outcome=accepted
@2ms process_send kind=named name=app/main.worker_b incarnation=I4 outcome=accepted
@2ms process_send kind=named name=app/main.worker_c incarnation=I3 outcome=accepted
@2ms process_terminated membership=M3 incarnation=I3 cause=cancelled
@2ms membership_removed membership=M3
@2ms process_terminated membership=M2 incarnation=I4 cause=cancelled
@2ms process_terminated membership=M1 incarnation=I1 cause=cancelled
@2ms supervisor_stopped supervisor=S1
```

The `boundaries` transcript is the same 24 lines without the 12
`process_send` lines, which leaves 12 lines. `off` mode writes no telemetry.
FP01 also runs OB04's double run and OB01's zero-event check on this program.

## Performance gates

### Derivation

Gates come from the prototype measurements in the
[supervised process runtime](../architecture/supervised-process-runtime.md#measurements)
record (Apple M1, one thread, single runs). The prototype drove the kernel
from Rust. The connected runtime adds generated C, a real `case` over the
message, the trace-mode checks, and the runner. Each absolute gate is
therefore about 6 to 13 times the prototype figure, rounded. That leaves
room for that overhead and for slower CI hosts, and it still fails on an
asymptotic regression or a lost fast path. The concurrency lab used about 19
times for its rendezvous gate. These gates are tighter because the prototype
already measured the chosen representation.

Debug builds use 10 times each release time limit, as the concurrency lab
does. Memory and count gates are the same in both profiles.

### Method

- Time comes from differential runs: the lab runs a workload at sizes N1 and
  N2 and computes `(t(N2) - t(N1)) / (N2 - N1)`. Process start, runner setup,
  and trace-file setup cancel out.
- Each timing is the p95 of 7 samples, as in the concurrency lab.
- Memory comes from one run per size, from the memory report and from the
  resident-size probe that the concurrency lab already uses.
- All workloads run under `--clock=virtual`, so restart windows and deadlines
  cost no wall time.

### Gates

| ID | Metric | Workload | Prototype | Release gate |
| --- | --- | --- | ---: | ---: |
| PG01 | ns per message, trace off | One process, Int messages, batches of 64, N1 = 10,000, N2 = 1,000,000 | 316 | < 2,000 |
| PG02 | Trace overhead on PG01 | `boundaries` and `full` against `off` | n/a | `boundaries` < 1.25x, `full` < 5x |
| PG03 | µs per restart cycle | Defect, terminate, restart, first park of the replacement. N1 = 1,000, N2 = 10,000, with a limit above N2 | 3.9 | < 40 |
| PG04 | µs per process to start and first park, and to stop | 1,000 dynamic processes | 5.1 start, 2.8 stop | < 50 start, < 30 stop |
| PG05 | Scaling of PG04 | Per-process cost at 1,000 divided by the cost at 100 | 1.28 | < 2.0 |
| PG06 | ns per message by payload | Text 4 KiB; List of 1,000 Ints; the 21-value DAG from CB02 | 190; 13,351; 402 | < 2,000; < 70,000; < 5,000 |
| PG07 | Resident memory growth | PG01 at N2 against N1 | flat | < 1 MiB |
| PG08 | Parked resident memory | 1,000 parked processes with 64 KiB stacks | n/a | < 64 MiB |
| PG09 | Cold start p95 | FP01 executable, trace off | n/a | < 25 ms |
| PG10 | Trace-off events | PG01 at N2 | 0 | exactly 0 |

The executable sizes of FP01 and of a process-free program are recorded
without a gate. MF06 already guards pay-for-play linking.

### Regression reporting

- Every gate result appears in the `performance` report's `gates` list, with
  its value and limit. A failed gate fails the lab.
- ts-1765ba records the first full release run in
  `docs/performance/supervised-process-conformance-baseline.md`, with the
  date, host, toolchain, and every metric. The format follows the
  concurrency baseline.
- `chimi-process-lab performance --baseline <report.json>` compares a run
  with a saved report. A metric more than 1.5 times its baseline value prints
  a warning line. A warning does not fail the run, because hosts differ. A
  warning on the baseline host becomes a new decision ticket, as ts-1765ba
  requires for every failure.

## Unlock order

Each stage is one or more commits in ts-6ccbc9. A stage is complete when its
scenarios pass and `CURRENT_STAGE` names it.

| Stage | Lands | Unlocks |
| --- | --- | --- |
| U0 | Lab skeleton: catalogue, transcript projector with unit tests on hand-written telemetry, report, and the cargo test. Every scenario is `locked` | none |
| U1 | `std/process` and `std/supervisor` types and functions, `process name` declarations, typing, and the literal range checks | SP01, SP03, SP07–SP09, SF04–SF18, SF25, SF26 |
| U2 | Capability checking for specs, inboxes, and managed handles, and lowering to verified Core | SP02, SP04–SP06, SF01–SF03, SF19–SF24, CO01–CO04 |
| U3 | Per-owner heaps, regions, and fragments in `runtime.c`, with loop reclamation on in every tier. The concurrency lab stays green | CB06, MF05 |
| U4 | Copy on transfer for `Task.start`, `Task.result`, and `Channel.send` | none |
| U5 | The process kernel: `Process.spec`, `send`, `receive`, `Supervisor.start`, `start_process`, `stop`, the trace family, ABI version 5 (now 6), and the memory-report fields | MB01–MB04, MB06, MB07, MB11, RT08, SV01, SD01, SD09, CB01–CB03, CB07, MF01, MF04, MF06, OB01–OB03, OB05, OB07, IP01 |
| U6 | Restart accounting and escalation at the root. Needs `Duration` and `--clock=virtual` from ts-f31a20 | MB05, MB10, RT01, RT02, SV03, SV04, SV11, RA01–RA05, RA08, ES01, ES05, CB08, OB04, OB06 |
| U7 | Admission, names, removal, `Supervisor.wait`, drain on cancellation | MB08, MB09, RT03–RT07, SV02, SV05–SV08, SV12, RA06, RR01–RR04, SD08, CB04 |
| U8 | `shutdown_within` deadlines, forced termination, and structured child tasks inside processes | RA07, RR05, RR06, SD02–SD07, CB05 |
| U9 | Declared and dynamic nested supervisors | SV09, SV10, ES02–ES04 |
| U10 | `programs/supervised-pool` and the performance harness | FP01, MF02, MF03, PG01–PG10 |

Two scenarios moved stage (coordinator decisions on `ts-6ccbc9`). MF06
moved from U3 to U5, because a process program cannot be built natively
before the kernel lands. Its no-concurrency and tasks-only half is
`native_only_a_task_program_links_the_fiber_runtime` in `tests/cli.rs`.
CB04 moved from U4 to U7, because it needs `remove_process`. Its task
analogue is
`native_a_value_a_finished_task_put_on_a_channel_survives_its_heap` in
`tests/cli.rs`. U4 therefore unlocks no scenario: copy on transfer is
already in `runtime.c`, and the task and channel tests in `tests/cli.rs`
guard it.

ts-1765ba runs `chimi-process-lab all` in release and debug builds, writes
the baseline record, and opens a decision ticket for every failure instead of
changing a contract.

## Scenario counts

| Category | Count |
| --- | ---: |
| Static compile-pass (SP) | 9 |
| Static compile-fail (SF) | 26 |
| Verified Core (CO) | 4 |
| Mailbox and delivery (MB) | 11 |
| Routing (RT) | 8 |
| Supervisor lifecycle and admission (SV) | 12 |
| Restart accounting (RA) | 8 |
| Escalation (ES) | 5 |
| Removal races (RR) | 6 |
| Shutdown and forced termination (SD) | 9 |
| Copy-boundary safety (CB) | 8 |
| Memory flatness (MF) | 6 |
| Observability (OB) and backend support (IP) | 8 |
| Forcing program (FP) | 1 |
| Performance gates (PG) | 10 |
| **Total** | **131** |

Four scenarios use C harnesses: RA06, SD04, CB06, and CB07.

## Decisions this lab fixes

These details are not settled in the contracts. The lab fixes them, and the
implementation must match:

- Process identities use their own counters, starting at 1, and a rejected
  start allocates no membership.
- An incarnation cancelled before its first run ends `cancelled`.
- Lifecycle events are boundary events. `process_send` is a full-mode event.
- A restart exactly `within` after a counted restart is outside the window.
- A removal requested during `stop` terminates its target at once. `stop`
  then skips that member (RR06).
- `Process.receive` after cancellation returns queued messages until the
  mailbox is empty, then cancellation ends the body. This is the coordinator
  decision on `ts-a6b43b`. It replaces the prototype's "cleanup never
  receives" behavior.
- `NameTaken` text and trace labels use the form `<module path>.<identifier>`.

## Open questions

The coordinator resolved all five on `ts-6ccbc9` and accepted each
recommendation (`docs/decisions.md`, "Processes and runtime"). The clock
slice for open question 2 is `Clock.sleep(Duration)` with a `clock: Clock`
need closed by the runner, and the runner flag `--clock=real|virtual`
(default `real`).

1. **Should the reference interpreter run processes?** The time contract
   already asks the interpreter to model timers and virtual time, which needs
   a scheduler.
   *Recommendation:* gate ts-6ccbc9 on native only. Keep IP01 and the
   `unsupported` status. Add a deterministic single-queue scheduler to the
   interpreter in a later ticket. The same expected files then apply to both
   backends, so the interpreter becomes a second oracle at no extra fixture
   cost.

2. **Which parked operations does process shutdown interrupt?** Under virtual
   time, computation takes no time. A body that is not parked can never miss
   a non-zero deadline. If cancellation interrupts every park, Chimi source
   can reach `forced` only with `shutdown_within = Duration.zero` (SD02). The
   non-zero path is then tested only in the C harness (SD04).
   *Recommendation:* cancellation interrupts every park, as task cancellation
   does. The one exception is `Process.receive` while queued messages remain.
   Keep SD04 in C. The alternative is to let `Clock.sleep` ignore process
   cancellation during drain. That would make deadlines reachable from
   source, but it would give processes and tasks different cancellation
   rules.

3. **Where does a non-literal mailbox capacity below 1 defect?** The mailbox
   contract says "when the specification is started". The later static
   contract says "at the call that receives it", which is `Process.spec`.
   *Recommendation:* `Process.spec`, as MB10 and SV11 expect. The defect then
   points at the line that computed the bad value. Update the mailbox
   contract to match.

4. **Should the routing contract and the architecture record follow the
   drain rule?** Both still say that cleanup never processes the mailbox. The
   coordinator decision lets the body drain queued messages after
   cancellation. Routing still rejects new sends, so no message is accepted
   and then discarded unread. Only messages accepted before shutdown are
   drained.
   *Recommendation:* reword both documents to "cleanup accepts no new
   messages, and the body may drain the ones already accepted". MB08, MB09,
   and SD03 test that wording.

5. **Should the root supervisor and nested supervisors emit a start event?**
   The prototype emits none, because a supervisor has no incarnation. The
   transcripts above therefore first show `S1` inside `process_started`, and
   a supervisor with no members leaves no trace until it stops.
   *Recommendation:* add `supervisor_started` with `supervisor` and `parent`
   fields when nested supervisors land (U9). Until then, keep the prototype
   set so that U5 to U8 match the reference transcript.
