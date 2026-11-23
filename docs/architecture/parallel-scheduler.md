# Parallel Scheduler (R4)

## Status

Design note. Ticket: CHI-267. It records what running Chimi on more than one
thread takes. Steps 1 to 3 are done: `--carriers=<n>` runs processes on n
threads around one kernel lock, and the default stays one carrier
(decisions.md, 2026-11-21). Step 4 is planned below ("Step 4 plan"):
4a to 4c are done (fibers move between threads, and an idle carrier steals
from a sleeping one); 4d to 4f split the kernel lock.

## Why

Today one carrier thread runs every task and process. A service that is busy
on one core leaves the others idle, and a long pure computation in one process
delays every other process. The language already promises that safety does
not depend on one thread (task-and-channel-capabilities.md, "Goals"), so this
is an implementation limit, not a language one.

## What already helps

- **No shared heap.** Each task and process owns its heap. Everything that
  crosses owners (messages, captures, results, broadcast copies) is copied
  into a fragment first (supervised-process-runtime.md, "The heap as built in
  `runtime.c`"). Two carriers running two owners never touch the same
  allocation.
- **Capabilities are owner-checked.** A Receiver or Completion carries an
  owner token, and the channel registry checks it on every operation. That
  check does not assume one thread.
- **All cross-owner traffic goes through scheduler requests.** A fiber never
  reaches another fiber's state directly. It parks with a request, and the
  scheduler answers it. That gives one place to put synchronisation.

## What blocks it today

1. **Process-wide globals in `runtime.c`.** Step 1 moved the per-fiber and
   per-carrier ones into one `Carrier` struct ("Runtime state by owner"), but
   there is still exactly one carrier, and the shared tables (handle counts,
   files, bindings) are not yet safe to touch from two threads.
2. **One `Scheduler` value owns everything.** The run queue, timers, the
   reactor, the channel registry, the process kernel and the task table are
   fields of one struct, driven by one loop (`Scheduler::drive`). The fiber
   crate's thread-locals (the active yielder, the scope state) assume that
   loop runs on the thread that owns them.
3. **Virtual time and transcripts.** The virtual clock and the lab's event
   transcripts rely on one global order of events. Tests compare that order
   exactly.
4. **Handle counts.** Step 2 made them one locked table with a release
   queue per carrier ("Handle counts" below). The queue is still flushed only
   by a fiber's next request on that carrier.

## Recommended shape

Shard by owner, then steal. This is the BEAM's model, adapted to Chimi's
owner heaps.

- **N carriers, one per core by default.** Each carrier runs its own copy of
  the scheduling loop over the tasks and processes it currently owns, with its
  own run queue, timers and `runtime.c` per-carrier state. The per-carrier
  state moves from file-scope statics into one struct that the carrier passes
  around, or into C11 `_Thread_local`.
- **A fiber runs on one carrier at a time.** Its heap goes with it, so a heap
  is never touched by two threads at once. That keeps the allocator and the
  collector single-threaded, which is where most of the runtime's code is.
- **Cross-carrier requests become messages.** A send, a monitor notice, a
  supervisor action or a release whose target lives on another carrier is
  queued to that carrier's inbox (a lock-free MPSC queue) and handled there.
  The channel registry and the process kernel are split by owner along with
  everything else, or start out behind one lock and are split later, guided
  by measurement.
- **Work stealing at park points only.** An idle carrier may take a parked
  or runnable fiber from a busy one between runs, never mid-run. Moving a
  fiber means moving its heap's ownership, which is one pointer.
- **Determinism stays available.** `--clock=virtual` and the lab run with one
  carrier, so transcripts stay exact. Parallel runs would promise outcomes,
  not interleavings. "Ordering rules under several carriers" checks every
  spec rule that names an order across owners.

## Runtime state by owner

Step 1 sorted every writable global in `runtime.c` into three groups. The test
`compiler/chimi/tests/runtime_state.rs` compiles the runtime and fails on any
writable static that is neither the carrier nor on its shared list, and on any
function-local static, so new state gets an owner when it is added.

- **Fiber context** (`FiberContext`, the `fiber` field of the carrier): the
  heap the running fiber allocates in and its allocation target. It follows
  the fiber, so `scheduler_request` saves the whole struct before a park and
  restores it on resume.
- **Carrier** (`Carrier`, reached through `carrier`): what one thread mutates
  while it runs fibers. That is the collector's and the copier's scratch
  (`collecting_*`, the mark, copy and pending stacks, `copy_counting` and the
  foreign owner of the last failed copy), the allocator's caches (small blocks,
  young chunks, the deferred footprint, `fs_read`'s buffer), and the counters
  behind the memory report (`heap_totals`, `copied_*`,
  `capture_transfer_probes`, `unscoped_allocated_bytes`,
  `process_heap_peak_value_bytes`, `scope_totals`). The counters are signed
  per-carrier deltas, since an allocation made on one carrier can be freed on
  another; the report sums them (PR 5a), and with several carrier threads its
  peaks are process-wide ones kept within 80 KiB per carrier (PR 6b).
  Carrier 0 (`carrier_first`) is a static, and `carriers_create` allocates
  the others before any carrier thread starts. `carrier` is each thread's
  initial-exec thread-local pointer, set by `carrier_thread_enter`.
- **Shared** (file-scope globals): state every carrier sees.
  - Heaps and constants: `main_heap`, `static_area`, `next_heap_owner` (an
    atomic counter), `shared_values`, `constant_values`.
  - `next_mark_epoch`. Mark epochs are stamped into allocation headers that
    move with a heap, so a per-carrier counter could repeat an epoch that a
    stale header still holds, and treat a dead allocation as marked. It is one
    process-wide atomic counter, so every pass anywhere gets a fresh epoch.
    A carrier takes 4,096 epochs from it at once (`take_mark_epoch`, step 3
    PR 6a): a loop pass collects, and one fetch-add per pass bounced the
    counter's cache line between carriers on every pass.
  - The handle-count table (`handle_table`, "Handle counts" below). Its
    release queue is carrier state (`released_handles`).
  - Tables whose ids travel inside values, each behind its own lock: open
    files (`fs_file_lock`), test logs (`test_log_lock`), process defects
    (`process_defect_lock`), and binding handles (`binding_handle_lock`). A
    binding call holds each open handle among its arguments for its whole
    length, so a close by any task only marks the handle closed and the
    last holder releases its state. A read or write
    takes a file's descriptor for the length of its system call, and a close
    during one leaves the descriptor to the last user to close, so the
    number is never reused under the system call. A read or write that
    waits for readiness gives the descriptor back first, so, as with one
    carrier, a close during the wait leaves the reactor watching a number
    that may be reused; `fs_close` does not withdraw the wait.
  - The operation profile's rows (`profile_rows`), allocated once at startup
    and counted with relaxed atomic adds.
  - The `random` provider's one seeded stream, behind `random_lock`. It is
    deterministic only on one carrier.
  - Telemetry: `trace_lock` is held for each whole event, so sequence numbers
    follow file order; span ids come from one atomic counter.
  - Standard input (`standard_input_lock`, held while a reader takes one
    line's bytes) and the console queues (`console_lock`, held by a writer
    except while it parks). Every signal field: signal handlers write these,
    so they must never sit behind a carrier or thread-local pointer, and the
    handler installers run once through `pthread_once`.

Rules for the steps that run more than one carrier:

- `carrier` is a thread-local, and a compiler may keep a thread-local's
  address, or the thread pointer it comes from, in a register across a
  call, so a fiber that moved threads would use the old thread's state.
  Since step 4 PR 4b `carrier` reads the thread pointer afresh at every use,
  and the fiber crate reads its park context only through functions that
  are never inlined. Any other per-thread state that code reads or writes
  on both sides of a park must live in `Carrier` or go through such a read:
  4b moved `console_lock_held` into `Carrier` for this reason (on aarch64
  clang kept its address across the park). `errno` is the same hazard,
  since glibc's `__errno_location` is declared `const`: read it before
  anything that can park, never after one from an address taken before.
- No code keeps a pointer into the carrier across a scheduler request.
  `scheduler_request` reads the carrier again after it resumes and writes the
  saved fiber context into whichever carrier that is. `flush_channel_releases`
  parks in a loop, so it reads the shared queue again on every pass.
- Leaf code that never parks (the allocator, the collector, the copier) may
  read `carrier` once per call.
- The memory counters cannot simply become per-carrier fields. An allocation
  made on one carrier can be freed on another, so that carrier's unsigned
  `footprint_bytes` or `live_allocations` would wrap below zero, and its peak
  check would then record a value near the type's maximum. They need signed
  per-carrier deltas summed at report time, or shared atomics with an atomic
  maximum for the peaks. A sum of per-carrier peaks is not the process's
  peak either, so decide what the peak fields mean before step 3 changes the
  numbers.
- Run the carrier simulation build (`CHIMI_CARRIER_SIMULATION`, "Handle
  counts") over the suite before each step that moves state. Every fiber
  resumes on the other of two carriers from the one it parked on, each with
  its own scratch and caches. It is the only check that catches a field filed as carrier state
  that must in fact follow the fiber.
- The shared values (`shared_values`, the module constants) live in the
  static area. `decode_constants` creates all of them at startup, so nothing
  appends to the static area once carriers run.

## Handle counts

Every live Sender, Receiver, Completion and Task value counts as one reference
to its channel or task (`runtime.c`, "Channel release"). A handle copied on
one carrier can be freed on another, since a fiber migrates with its heap, so
only the total count means anything. Step 2 keeps one table for every carrier:

- `channel_endpoint_retain` and `channel_endpoint_drop` take the table's lock.
  A copy is counted when it is made, before any scheduler request can move it
  to another carrier, so a drop can never see a count that a retain elsewhere
  has not reached yet. The freers' "any handle at all" shortcut reads the count
  without the lock: a value being freed was counted before it got there.
- A key whose count reaches zero is queued on the carrier that dropped it,
  after the lock is released. The next request of a fiber on that carrier
  issues the release (`flush_channel_releases`, which reads `carrier` again on
  every pass because each pass parks).
- Step 4 may stripe the table by key hash. Striping by owner would miscount
  once a fiber migrates.

Per-carrier counts were rejected because only their sum is meaningful, and
summing needs a snapshot across carriers. Plus-or-minus-one messages to an
owner carrier were rejected because they can arrive out of order. A drop
applied before the matching retain shows a false zero and releases a live
channel. A drop that finds no entry is ignored, so the late retain then leaves
a count that never falls. Weighted reference counts avoid both problems, but
they put a weight in every handle value and need a path for when a weight can
no longer be split.

Step 3 must also drain a carrier's release queue from its own loop before it
idles and when it stops. Otherwise a zero reached in a release hook, on a
carrier where no fiber makes another request, strands the release. The
simulation build showed this: before its flush drained every carrier's queue,
a dropped channel's buffered messages outlived it and a fragment-peak test
failed. Until PR 3b a flush drained the running carrier's queue first and
then any other carrier's, in place of that loop. That stand-in
(`carrier_with_releases`) read another carrier's queue without a lock: two
threads popping one queue could lose a release or issue it twice. PR 3b
replaced it with the scheduler's `take_release` drain, and only the
single-threaded simulation still lets a fiber drain another carrier's queue.
That hides a missing per-carrier drain from the simulation, so the drain has
its own tests.
The unlocked `handles_live()` read also assumes that moving a value to
another carrier synchronises (the hand-off is a scheduler request), which
step 3 must keep true.

Native handles are counted in the same table (2026-11-20), and a handle
whose count reaches zero is closed at once, on the carrier that dropped it,
without a scheduler request. The binding handle table (`binding_handles`)
is behind `binding_handle_lock`, and one binding call at a time runs on a
handle (step 3 PR 5c-1), so a binding's own state needs no lock.

Two tests check this step:
- `tests/runtime_state.rs` `the_handle_table_counts_across_threads` hands
  references from one thread to another under ThreadSanitizer.
- The `CHIMI_CARRIER_SIMULATION` test build resumes every fiber on the other
  of two carriers from the one it parked on
  (`CHIMI_TEST_RUNTIME_DEFINES=-DCHIMI_CARRIER_SIMULATION` with a debug
  compiler). The whole `chimi` suite passes under it, except the test that
  checks the prebuilt runtime is used, which a test build never uses.

## Alternatives considered

- **One global lock around the scheduler.** It is simple, but every request
  would serialise on it, and most of a service's work is requests. It is
  acceptable only as a short step on the way to sharding.
- **A thread per process.** It does not scale to the many small processes the
  supervision model encourages, and it loses cheap parking.
- **A shared heap with a concurrent collector.** This was rejected for the
  single-thread design already, for reasons that only get stronger with
  threads.

## Ordering rules under several carriers

The spec promises some orders across owners. Before step 3, each was checked
against parallel runs. A rule that a lock or a single owner can keep survives.
A rule that says "before any other task runs" cannot, since another carrier
is running tasks at the same moment.

- **Kept by the one owner of a queue.** These rules hold under parallel runs
  as long as each queue changes only under its lock:
  - mailbox delivery is strict FIFO in acceptance order across senders
    (process-execution-and-mailboxes.md), and a monitor notice comes after
    every message already accepted (process-monitors-and-metrics.md);
  - parked channel receivers are served in the order they parked, a parked
    plain receive before a select, and parked publishes are accepted in
    order (task-and-channel-capabilities.md);
  - a select commits the first ready source in list order
    (time-and-outcomes.md);
  - a supervisor stops its members one at a time in reverse order
    (supervisor-policy-and-lifecycle.md);
  - a pool serves waiting statements in the order they asked
    (network-modules.md);
  - recorder values come in the order the records linearized (testing.md).
- **Kept by running supervisor decisions under the kernel lock.** A
  supervisor decision runs "between fiber resumes": a defect, its
  termination, the restart charge and the replacement's creation happen in
  one step (supervised-process-conformance.md, "Determinism obligations").
  With several carriers, that step must run under the kernel lock, with no
  process of that supervisor running in between. The note's risk list
  already places a supervisor's record on one carrier.
- **Does not survive, and must change in step 3.**
  - Select commit (task-and-channel-capabilities.md, "Select commit"). After
    a select answers with a ready receiver, the host's receive is "served
    before any other task runs" by putting the task at the front of the run
    queue. On another carrier, a second receiver of the same queue could take
    the value in between. Step 3 must make the commit atomic: the select's
    answer either takes the value, or reserves it for that task until its
    receive. The observable rule ("the other source keeps its value") stays
    the same. Done in step 3 PR 1: the select step takes the value into
    `Operation::ReceiveReady`, which the follow-up receive reads first.
  - IO wait order (host-capabilities-and-execution-properties.md, "Order").
    "An OK answer resumes the task before any other task runs" becomes "on
    its carrier". That is enough, because the wait is level-triggered and a
    host that finds the descriptor drained waits again, as it already does
    when another OS process drains it. "Tasks ready together wake in the
    order they began waiting" holds per carrier only.
- **Exact interleavings stay single-carrier.** The lab's determinism
  obligations ("One carrier thread. The ready queue is FIFO.") and every
  `--clock=virtual` run keep one carrier, so transcripts stay exact. A
  program under the real clock on several carriers is promised outcomes, not
  an interleaving. Real IO already makes real-clock interleavings depend on
  timing.

No rule needed a language decision. Each one either survives as written or
keeps its observable meaning through a different mechanism.

## Order of work

1. Move `runtime.c`'s per-fiber and per-carrier statics into one carrier
   struct, with no behavior change. Done (CHI-267); the list is under
   "Runtime state by owner".
2. Make the handle-count table safe for every carrier, with a release queue
   per carrier. Done: one locked table rather than per-carrier counts or
   messages ("Handle counts").
3. Split `Scheduler` into a carrier loop plus shared kernel state behind a
   lock, and run N carriers with work stealing turned off. Measure. This
   step also has to:
   - make the select commit atomic ("Ordering rules under several
     carriers");
   - have each carrier loop drain its own release queue, replacing
     `carrier_with_releases` (landed in PR 3b);
   - turn `carrier` into the accessor.

   Epochs are already process-wide and atomic, and the shared values are
   already created at startup.
4. Shard the kernel and the channel registry by owner, and add stealing
   ("Step 4 plan": stealing first, then the shards).
5. Publish multi-core baselines next to the single-thread ones (R2).

## Step 3 plan

Step 3 lands as six pull requests. Each one is green on its own, and the first
four keep one carrier.

1. **Atomic select commit (one carrier).** When `handle_select` answers
   Ready on a receiver, it takes the value under the same scheduler step and
   stores it in the task's `Operation::ReceiveReady` slot. The follow-up
   `REQUEST_CHANNEL_RECEIVE` already reads that slot first, so the ABI does
   not change. A stored value is released if its task dies first.
   `SelectCommitted` is still emitted before the take, so lab transcripts do
   not change.
2. **Split `Scheduler` into a `Kernel` and a carrier loop (one carrier, no
   lock).**
   - Adds a `Running` task phase, because today a running task stays
     `Runnable`, and cancel, force and defect paths need a rule for a task
     running elsewhere.
   - Adds a `PinnedFiber { home, fiber }` that asserts it is resumed and
     dropped on its home carrier.
   - Adds a per-carrier ready queue held in the kernel.
   - Adds a jobs path for the two places that resume another task's fiber
     inline: `begin_terminal`'s terminal resume, and `await_incarnation`
     from `force`, `begin_shutdown`, `shutdown_processes` and the shutdown
     deadline. Those jobs run on the fiber's home carrier, inline when that
     carrier is the current one.
   - As landed: the `Running` phase, `PinnedFiber` (its home is the thread of
     its first resume, checked on every resume and on destruction), and the
     per-carrier ready queues (`scheduler/ready.rs`, each task queued on its
     home carrier, a started task on its parent's). The two inline resumes
     assert, in debug builds, that they run on the fiber's home carrier. Turning them into
     jobs moves to PR 5, the first PR where another carrier exists to run
     them, so the job path is tested when it is written. The kernel lock and
     the split of the loop into its own type wait for PR 4, which is where
     they change behavior.
3. **C side.**
   - As landed (PR 3a): `carrier` is a thread-local pointer
     (`carrier_this_thread`, initial-exec except on Apple, where the default
     model is used), read directly. The plan's no-inline accessor was
     measured and dropped: a call per access cost 32% on an allocation-heavy loop
     (5.0 s against 6.8 s, median of 7 interleaved runs), while a direct
     thread-local read is within noise of main (5.42 s against 5.29 s,
     median of 11). A direct read is safe because a fiber never leaves the
     thread of its first resume (PR 2's `PinnedFiber`), so a thread-local
     address a compiler keeps across a scheduler request still names the
     right carrier. `carrier_bind` sets it; the main thread binds carrier 0
     at startup. (Superseded by step 4 PR 4b, which lets a fiber move: the
     read is now a fresh thread-pointer read, "Step 4 plan".)
   - A per-carrier drain hook issues queued releases before the carrier
     idles and when it stops. It replaces `carrier_with_releases`.
     As landed (PR 3b, fiber ABI 28): `ChimiSchedulerConfig.take_release`
     hands over one queued release at a time, and `Scheduler::drive` calls
     it whenever the carrier's ready queue is empty, applying each release
     with no task attributed (a trace event with no cause). It runs only
     between resumes, never inside a channel transition. A fiber's flush
     drains only its own carrier's queue. At stop nothing is drained: the
     scheduler's teardown releases every value it holds, and the next run
     clears the queues. The simulation build runs both carriers' loops as
     one loop that is never idle while a fiber runs, so there a fiber also
     drains the carrier it left, standing for that carrier's idle loop;
     without that, a fragment-peak test grows. The drain itself is tested
     outside the simulation: a release queued inside a release hook while
     the root sleeps is done before the clock moves (a scheduler test and
     a native test).
   - The host stack limit is stored per carrier (landed in PR 3a). Only the
     main thread computes it today; PR 5's carrier threads must run
     `carrier_bind` and then `guard_host_stack` before their first fiber,
     or their stack check is off. A thread that never binds aliases
     carrier 0, so PR 5 also adds a debug check that each carrier thread
     bound its own carrier.
   - The memory counters become signed per-carrier deltas. Moved to PR 5
     (decided 2026-11-21): with one carrier, and in the simulation, which
     carries one set of counters from carrier to carrier, no test can tell
     deltas from today's counters. A per-carrier peak of a delta means
     nothing, so PR 6b gives the report process-wide peaks. PR 5 has the
     first allocation freed on another thread, so the change is tested
     there.
   - New test: a count reaches zero inside a release hook, on a carrier
     where no fiber requests again.
4. **One `Mutex` around the kernel (one carrier).** The lock is held except
   while a fiber runs and while the carrier waits. This PR shows the lock
   discipline and measures its cost against the performance gates. Lock
   order is the kernel lock first, then `handle_table`, never the reverse.
   The kernel state and the carrier loop become separate types here, since
   the lock is what separates them.
   - Shape (advisor review, 2026-11-21): `Scheduler` stays the kernel type,
     holding nearly all state: tasks, ready queues, channels, processes,
     clock and timers, events and the trace sink, host hooks, reactor
     registrations, children, signals and `host_operations`. The carrier
     loop (`drive`, `run_until_idle`) is separate from it; its id is still
     the kernel's `current` until PR 5 gives each carrier thread its own. The loop is written once against
     kernel access that is either a plain `&mut Scheduler` (the existing
     methods and unit tests) or a `Mutex<Scheduler>` (`chimi_scheduler_run`,
     so the gates measure the lock).
   - The lock drops in three places: around a fiber's resume in the
     carrier's `run`, around the reactor's descriptor wait, and around a
     real-clock wait. Each is split into a locked prepare, the unlocked
     block, and a locked apply that looks the task up again by id. A
     `Running` record's fiber is never dropped or removed while it runs,
     so the resume uses the fiber's pointer without holding the record.
   - The two inline resumes (`begin_terminal`, `await_incarnation`) run
     deep inside kernel operations, so they keep the lock across the resume
     until PR 5 turns them into jobs. Hooks called under the lock (trace
     sink, release, copy, violation and defect reports) must never park or
     call into the scheduler.
   - Lock access cannot fail: a panic under the lock aborts inside the C
     entry point, so a poisoned lock is only taken over. A stalled run
     takes the scheduler back out of the mutex before forgetting it.
   - As landed: `scheduler/carrier.rs` holds the loop (`drive`,
     `run_until_idle`) over `KernelAccess`, and `chimi_scheduler_run` runs
     it against `Mutex<Scheduler>` (std's, uncontended). The kernel is taken
     once per resume, to finish one task's step and enter the next. A test
     checks that fibers the carrier resumes run without the lock. The
     stalled-root signal wait also runs without it. Measured on the
     performance lab, medians of 7 interleaved runs against main: PG01
     +0.8%, PG02 off -0.4%, PG03 +0.8%, PG04 start +0.6% and stop -0.9%.
     The verifier's own runs (7 interleaved, then 11 of PG04 alone) agree
     on PG01 to PG03 and PG04 start, but found PG04 stop 10 to 14% slower
     (about 1 µs, against a 30 µs gate). A stop spans many resumes, each
     taking the lock once, so this is read as the lock's real cost there.
     PG06 fails its list gate on main on this host too, and its dag figure
     is bimodal on both (2.8 or 3.4 µs).
   - Left for PR 5: a real-clock sleep or descriptor wait is computed
     under the lock, so a timer or ready task another carrier adds during
     the wait is seen only when it ends (at most `MAX_REAL_WAIT`). PR 5's
     per-carrier wake descriptor ends such a wait at once.
5. **N carrier threads behind a runner flag (`--carriers=N`, default 1).**
   Split into five PRs (advisor review, 2026-11-21), each merging with one
   carrier and behavior unchanged:
   - 5a, the C runtime safe for N threads. Carrier states are sized at
     startup. `binding_handles` and every other table fibers touch outside
     the kernel lock get a lock: open files, test mailboxes, process
     defects, profile rows, the `random` stream and the span counter. The
     memory and copy counters become signed per-carrier deltas, summed in
     the report. `carrier_thread_enter` binds a thread's carrier and
     computes its stack limit. Tested with pthread C fixtures under gcc's
     ThreadSanitizer and with the simulation suite.
     - As landed (5a-1): everything above except binding handles, which
       need per-slot use counts (5a-2). `fixtures/carrier-threads-stress.c`
       runs four carrier threads that hand fragments across carriers and
       share spans, telemetry, the profile, `random`, a test Recorder and a
       file closed during reads, under ThreadSanitizer; it found
       `next_heap_owner`, which was a plain counter. Telemetry takes a
       mutex rather than `flockfile`, which ThreadSanitizer cannot see.
       With several carriers a task started on a thread that never entered
       its carrier aborts. Peaks are recorded per carrier against its own
       delta (the simulation compares the sum over its two carriers), so
       with one carrier every number is as before. `carrier_thread_enter`
       also runs the host stack check, which only a program without the
       scheduler arms, so a carrier thread's is off as the main thread's
       is under the scheduler.
     - As landed (5a-2): binding handles, the console, standard input
       and the signal installers, as listed under "Runtime state by
       owner". The stress fixture now also opens, uses and closes binding
       handles from four threads (a release while a call uses the state
       fails it), reads standard input from a pipe, and writes console
       lines. Queued console output needs the scheduler, so the fixture
       covers only direct writes; 5e's stress programs cover the queue.
       With one carrier the console takes no lock: a print-only program
       measured about 25 ns a line slower with it.
     - Left for 5b: `chimi_binding_wait` checks that its handle is open
       and then registers the wait. With several carriers a close on
       another carrier can land between the two, and its withdraw then
       misses the wait, which parks until the descriptor is ready. The
       kernel should keep, per slot, the last generation withdrawn, and
       answer a wait on that generation or an older one as withdrawn.
     - Left for 5c: holding a handle keeps its state alive but does not
       serialize calls on it, so two carriers may run calls on one handle
       at once (a TLS session read on one and written on another, an exec
       command built on one while another starts it). Before real threads
       run, each binding type's state needs its own lock, or the runtime
       runs one call per handle at a time. The binding sources
       (`bindings/`) are not scanned by `runtime_state.rs`.
     - A close also wakes a child wait on the handle (its key is now in
       `CHILD_WAIT`'s `channel`), since a close while another task waits
       no longer kills the child until that wait's call returns. At
       program end every handle is released, including ones an
       abandoned call still counts as holding.
   - 5b, kernel pieces on one thread: the carrier id passed through
     `Locked`, jobs for the inline resumes, the wake descriptors and the
     global idle check. Tested on a two-carrier kernel driven from one
     thread, which also kills the ready-queue mutant PR 2 left.
     Split into six PRs (advisor review, 2026-11-21), each merging with
     one carrier and behavior unchanged:
     - 5b-1: `Locked` carries its carrier and enters it on every lock;
       every ready push goes through one `enqueue`; a resume asserts it is
       on the task's home carrier. A test-only two-carrier kernel, driven
       from one thread, kills the ready-queue mutant.
       - As landed: `Locked::new(kernel, carrier)`, `Scheduler::enter_carrier`,
         `Scheduler::with_carriers` (tests only). `enqueue(task, front)`
         reads the home from the task's record, so no caller picks a
         carrier. Debug assertions check that a resume (`prepare_run`) and
         the two inline resumes run on the task's home; 5b-3 relaxes the
         inline one for a task that never ran. Tests cover a wake, a root
         start and a child start across two carriers, and the lock
         entering its carrier. Mutants in `carrier_routing_mutations.py`,
         all six killed. The verifier measured PG01 and PG04 within noise.
     - 5b-2: the sticky withdraw. The reactor keeps, per binding slot, the
       last generation withdrawn (`channel` is slot << 32 | generation, as
       `binding_resource_key` builds it). An IO or child wait on that
       generation or an older one is answered as a withdrawn parked wait
       (pending, woken at once) without registering, so C's loop sees the
       handle closed. One table entry per slot ever withdrawn. The request
       layout is unchanged, and the key's form becomes part of the
       contract, which the native runtime, the only host, already follows,
       so the fiber ABI stays 28.
       - As landed: `Reactor.withdrawn` maps a slot to its newest
         withdrawn generation; `handle_io_wait` and `handle_child_wait`
         consult it for a nonzero key. With one carrier it changes nothing
         a program can see: the native runtime checks a handle open with no
         park before its wait, so no close lands in between. Mutants in
         `sticky_withdraw_mutations.py`, all eight killed.
     - 5b-3: jobs for `begin_terminal`'s inline resume. The inline path
       stays when the target's home is the current carrier or the target
       never ran (an unstarted fiber given Cancel returns without running,
       so nothing is pinned). A target running on another carrier records
       a pending end, applied by its own `complete_run`. Otherwise the part
       up to `CancellationRequested` runs where the cancel was decided, and
       a job on the home carrier runs the cleanup resume without the lock,
       then the rest of `begin_terminal` under it. While a job is pending,
       `finish_draining` does not publish the task's terminal.
       - As landed: `scheduler/jobs.rs` holds a FIFO of jobs per carrier
         (`Job::Unwind { task, forced }`) and `PendingEnd`. A carrier runs
         its jobs before its ready tasks; `drive` resumes a job's fiber
         without the lock like any other, and `Scheduler::complete`
         dispatches on `RunKind`. `begin_terminal` keeps its order up to
         `CancellationRequested`; its tail is now `end_after_unwind` (the
         unwind's result decides a defect, overflow or park) and
         `end_drained` (drop the fiber, cancel children and supervisors,
         publish), shared by the inline path and the job. A pending end
         applies in `complete_run` as if decided just after the step: the
         step's park and request are served first, as one carrier always
         serves them (a scope exit or a transfer completion must not be
         dropped), and then, if the task is still live, it is cancelled
         (`cancel_task`), or forced (`force_terminal`) when the end was.
         An end the step already reached is not overridden, as a cancel
         after the end is ignored with one carrier. With one carrier no job or pending end is ever
         made, so the inline path is the old code. A process shutdown off
         its home carrier (`await_incarnation`, `force`) is 5b-4's. Mutants in `unwind_jobs_mutations.py`,
         all seventeen killed.
     - 5b-4: `await_incarnation` off home. It runs the body inline only
       on its home carrier; otherwise the body stays queued there and the
       caller takes the "not yet" path it already has. A forced body arms
       no new deadline (the old one is spent, and re-arming it would fire
       at once forever). Process teardown, which must finish
       synchronously, enters the home carrier in 5b; 5c must run it on
       each carrier before the carriers exit.
       - As landed: `Scheduler::may_resume_here(task, command)` says
         whether a fiber may run on the current carrier (its home; never
         ran and given Cancel; or teardown). `await_incarnation` runs the
         body inline only then; otherwise the body waits on its home queue
         and the caller takes its "not yet" path. A forced body left
         waiting on its home arms no new deadline; a forced body that is
         already draining still arms one, even at once, since its firing
         forces the descendants still in their releases (the review found
         that a zero deadline otherwise deadlocked such a child). `Drop` sets `any_carrier`, runs every carrier's queued
         jobs, then tears processes down as before, all on the dropping
         thread; 5c replaces this with each carrier draining its own
         before it exits. Tests stop a supervisor homed on carrier 1 from
         carrier 0 (the transcript matches one carrier's), fire a shutdown
         deadline on carrier 0, and drop a kernel whose processes live on
         carrier 1, and drop a kernel with a job queued on another carrier.
         Mutants in `process_off_home_mutations.py`, all six killed (one by
         a timeout: the teardown otherwise spins).
       - Left for 5b-5 and 5b-6 (review): a queued job must wake its home
         carrier, and the idle check must count every carrier's jobs, or a
         home carrier asleep in its idle wait leaves the task draining with
         its fiber, and carrier 0 could report a deadlock. Left for 5c: each
         carrier must run its own jobs before it exits (5b-4's `Drop` runs
         them all on the dropping thread, which only a one-thread kernel
         may do).
     - 5b-5: wake descriptors. One non-blocking pipe per carrier, none with
       one carrier. The kernel marks carriers under the lock (work queued
       for another carrier; a reactor, child or signal wait registered off
       carrier 0; a new earliest timer; stop), and `Locked` writes the
       bytes after it unlocks.
       - As landed: `scheduler/wake.rs` holds the pipes (opened with
         `pipe2` on Linux, so a fork cannot inherit them) and the marks.
         `wake_carrier` marks a carrier other than the current one; it is
         called by `enqueue` (every ready push), by the job push, by a
         descriptor, child or signal wait registered off carrier 0, and by
         `insert_timer` (every timer) when the timer is the new earliest.
         `Locked::with` takes the marks under the lock and writes them
         after the guard drops; a test-only assertion checks that no wake
         is written while the lock is held, and a debug assertion that no
         mark is left from an access that bypassed `Locked`. The idle
         sleep, the descriptor wait and the stalled root's signal wait
         watch the carrier's own pipe and empty every pipe that woke them;
         only carrier 0 also watches the signal pipe. A task may not wait
         on a wake pipe. With one carrier no pipe exists, and the wait
         descriptors are held in a fixed array, so nothing allocates.
         The carrier loop's idle step is `carrier::wait_idle`, so a test
         can run one. Tests in `scheduler.rs` cover each mark, the write
         after unlock, each wait (the loop's sleep, a descriptor wait, a
         stalled root's) ending at its carrier's byte, and a wait emptying
         both the signal and the wake pipe. Mutants in
         `wake_pipe_mutations.py`, all 24 killed; the last five came from
         the verifier's own run against the whole suite.
       - Left for 5b-6 (review): `idle_step` still lets any carrier build a
         descriptor wait or a timed sleep; 5b-6 restricts both to carrier
         0, which the marks already assume. Left for 5c: the stop mark, and
         turning a failed pipe (for example EMFILE) into a scheduler error
         instead of a panic, since only tests open pipes today.
     - 5b-6: global idle. Every idle carrier fires due timers first. A
       carrier other than 0 with nothing to do parks on its descriptor;
       carrier 0 decides quiescence, deadlock or a virtual-time jump only
       when every other carrier is parked and every ready and job queue is
       empty. The last carrier to park wakes carrier 0. Tested with a
       round-robin driver on one thread that fails if every carrier parks
       with no descriptor readable and no verdict.
       - As landed: `idle_step` on a carrier other than 0 is `park_step`:
         fire due timers, run again if that gave the carrier work, else
         set its `parked` flag and park on its wake pipe (new
         `IdleStep::Park`), or under the real clock sleep only until the
         earliest timer, so a busy carrier 0 cannot hold that timer up.
         The last carrier to park wakes carrier 0. Carrier 0 keeps its old
         path, with one new check after the descriptor wait: unless
         `others_quiet` (every other carrier parked, no ready task or job
         on any carrier) it fires due timers and sleeps until the earliest
         timer (real clock) or parks, with no time jump and no verdict
         (`wait_for_others`). A carrier's flag clears only when it takes
         the kernel; work handed to it while parked is seen in its queues.
         With one carrier `others_quiet` is always true, so nothing
         changes. A root finishes only with no job queued. The test-only
         `carrier::round_robin` runs each carrier in turn and fails on a
         lost wake; the 5b-4 tests that needed several carriers to settle
         use it. Tests in `idle_tests.rs`; mutants in
         `global_idle_mutations.py`, all 19 killed; the last five came
         from the verifier's own run against the whole suite.
       - Left for 5c (review): when carrier 0 returns a verdict or the
         root's run ends, every parked carrier must be woken (the stop
         mark), or its thread waits forever. `Idle::AwaitingHost` is still
         decided before `others_quiet`, so carrier 0 returns to the host
         while other carriers may have work; `end_host_operation` should
         then wake carrier 0. The round-robin driver checks lost wakes only
         for `Park`; a carrier 0 sleep or descriptor wait it skips is not
         checked.
   - 5c, real threads behind a test-only API: sharing the kernel between
     carrier threads, spawning and joining, each carrier destroying its
     own fibers before it exits, and placement.
     Split into six PRs (advisor review, 2026-11-21), each merging with
     one carrier and behavior unchanged. 5c-1 and 5c-2 may land in any
     order, but before 5c-6; 5c-3 to 5c-6 land in order.
     - 5c-1: one binding call per handle at a time. A per-slot lock in
       the binding handle table, held while binding code runs and
       released across `chimi_binding_wait`, skipped with one carrier.
       Tested under gcc's ThreadSanitizer with two threads on one handle.
     - 5c-2: placement. `create_task` takes an explicit home: the root is
       on carrier 0, `Task.start` on its parent's carrier, and a process
       incarnation on its membership id modulo the carrier count, kept
       across restarts.
     - 5c-3: fallible carrier setup. A crate-private constructor builds a
       kernel with several carriers and reports a wake pipe that cannot be
       opened as `SchedulerError::CarrierSetupFailed`, instead of a panic.
     - 5c-4: the stop. `root_step` sets `stopping` in the same lock hold
       that ends the run and marks every carrier; a parked carrier sees it
       and exits its loop (`IdleStep::Exit`, `carrier::run_carrier`).
       `Idle::AwaitingHost` waits for the other carriers too, and
       `end_host_operation` wakes carrier 0.
     - 5c-5: per-carrier teardown. Carrier 0 shuts the processes down and
       keeps running until no incarnation or blocked plan is left; then
       each carrier runs its own jobs and destroys its own fibers
       (`retire_carrier`) before it exits. `Drop`'s `any_carrier` path
       stays for one-thread kernels only.
     - 5c-6: threads. A test-only `run_root_on_threads` shares the kernel
       behind its mutex with scoped threads, built and dropped on the main
       thread. `Mutex<Scheduler>` is `Sync` only if `Scheduler` is `Send`,
       which its raw pointers (hook contexts, the signal words, request
       pointers) and `Rc` process records prevent, so this needs an
       `unsafe impl Send` with a written argument: every pointer is reached
       only under the lock, and a started fiber is resumed only on its home
       (`Rc` becomes `Arc`). It spawns carriers 1 to N and joins them; an
       unstarted fiber needs no wrapper, since its first resume claims its
       home. Stress loops under a watchdog, with fixtures that check a
       fiber never changes thread.
     Also found for later (advisor): a C-runtime fiber cannot run on a
     carrier thread until the runtime exports a way to bind one (5d); the
     host hooks run on whichever carrier holds the lock, which the header
     should say (5d); `list_slice`'s in-place compaction of a shared
     constant (5d-1); `park_step` may park with this carrier's host releases
     undrained (to check in 5c-4).
     - 5c-3 as landed: `Scheduler::with_options_carriers(options, n)` and
       `install_carriers`, which opens the pipes before changing anything,
       so a failure leaves the kernel as it was and closes every pipe
       already opened. The test-only `with_carriers` and `set_carriers`
       use it. Tests inject a failure at the nth pipe and check that every
       pipe opened before it was closed and the kernel left as it was.
     - 5c-1 as landed: each handle slot names the call running on it
       (`runner`). A call takes all of its handles at once when it starts,
       waiting on the condition variable `binding_handle_free` while any of
       them is taken, so calls sharing handles in either order cannot
       deadlock. It lets them go around every scheduler request it makes,
       waits and immediate requests alike (`binding_request`), since a
       forced process body is abandoned at any park and on one carrier
       another task may run first; a call ends only with its handles taken
       back. It is not skipped with one carrier: the table lock is taken
       there already, and the wait never happens. Tested under gcc's
       ThreadSanitizer with four threads on two handles, alone, in pairs
       in both orders and named twice, and with a descriptor wait and a
       child wait that must let their handle go; and end to end with a
       forced process body whose binding asks the scheduler once more on
       its way out.
     - 5c-2 as landed: `create_task` takes the home from its caller.
       `start` homes a child on its parent's carrier and a root on the
       current carrier, which is carrier 0 wherever the runner starts one.
       `spawn` homes an incarnation on `membership_home`, its membership id
       modulo the carrier count, so a restart stays where its predecessor
       ran; a member's restart is decided on that same carrier, since it
       ends there. A restarted nested supervisor rebuilds its members under
       new memberships, so they may move. Tested with a three-carrier
       supervisor whose crashed member's next incarnation id would land
       elsewhere.
     - 5c-4 as landed: `root_step` calls `stop_carriers` in the hold where
       it returns Finished or Parked, never while work is left, and
       `begin_run` clears it when `run_root` starts. A carrier other than
       0 then leaves its loop (`IdleStep::Exit`, the test-only
       `carrier::run_carrier`) instead of parking; it does not count as
       parked afterwards, so a later run needs its carriers started again,
       which 5c-6 does by spawning and joining them per run. A host
       operation returns `AwaitingHost` only once the others are quiet,
       carrier 0 meanwhile waiting for them (its own timers still fire),
       and `end_host_operation` wakes carrier 0. Only tests begin host
       operations today; a fiber on another carrier that spun until one
       ended would hold carrier 0 from the host. Both idle waits now fire
       due timers and drain the carrier's host releases first
       (`refill_before_waiting`).
     - 5c-5 as landed: the run, not `Drop`, ends the processes. Once the
       root is terminal and no carrier has work, `root_step` gives a
       process still left its pending shutdown deadline, if a timer is
       queued; with none, nothing but forcing can move it, so
       `force_survivors` makes one pass and the run continues, ending
       (`RunEnd::Finished`) once nothing is left or a pass moved nothing.
       The pass forces the bodies stop plans wait on and resumes plans
       whose blocker is gone; only if that moved nothing does it shut
       down and force every incarnation left, cancelling the deadline
       `force` leaves queued. It never loops, since a body on another
       carrier ends only when its home runs it. No program reaches the
       pass today (a full suite run found no process left at the old end
       of a run): every incarnation left after `shutdown_processes` has a
       deadline. `stopping` became `run_end: Option<RunEnd>`. A carrier
       other than 0 leaving a finished run asserts it owes no job and
       that no task homed on it still holds its fiber (`retire_carrier`),
       which holds because everything on such a carrier belongs to a
       process body, so `Drop` on carrier 0's thread never touches one of
       its fibers. Destroying such fibers on their carrier was tried and
       dropped in review: taking a live task's fiber leaves a task that can
       never end. A parked run keeps its fibers on their carriers, so a
       parked kernel may be dropped or cancelled into only on one thread;
       5c-6 must keep that rule or retire a parked kernel's carriers, and
       must join carriers 1 and up before `run_root` returns, since the
       check assumes nothing reaches the kernel between the run's end and
       their exit. Found
       in review and left as before: `force` clears a deadline without
       cancelling its timer, and `Drop` destroys a parked fiber without
       setting a terminal resume context.
     - 5c-6 as landed: the test-only `process_abi::run_root_on_threads`
       moves the kernel into its mutex for one run, clears the last run's
       end before any carrier starts, runs carrier 0's `run_root` on the
       caller's thread and carriers 1 and up on scoped threads, joins them,
       and hands the kernel back. `Scheduler` is `Send` by the argument at
       its `unsafe impl`: what it holds is reached under the lock, a parked
       request (a pointer into a fiber's stack) is cleared in `prepare_run`
       before the fiber resumes, and a started fiber runs and is destroyed
       only on its home thread. The `Rc` process records stay `Rc`, since
       only kernel code clones them. A panic on any carrier aborts, since
       the others would wait for it forever, and a run that parks on
       several threads panics, since its fibers would stay homed on joined
       threads. Tests loop runs of hopping fibers on one, three and four
       threads, virtual and real clock, two runs per kernel, under a
       watchdog, checking one thread per carrier and that no fiber moves or
       runs holding the kernel, and a supervisor whose members on three
       carrier threads are stopped at home with the one-carrier transcript.
       Stress (from the review): hundreds of loops in debug and release,
       on all cores and pinned to one or two, with no failure. Left for 5d
       (review): carrier threads must enter the C runtime
       (`carrier_thread_enter`) before running a C fiber; host hooks run on
       whichever thread holds the lock, so test hooks that record into
       thread-locals miss events from carrier threads; carriers persist
       across runs (or a parked run is refused) so a parked kernel stays
       resumable; a new earliest timer's wake to carrier 0 is masked in
       these tests by the last parker's wake.
   - 5d, the runner flag and its config field (fiber ABI 29), and exit 64
     with `--clock=virtual`. Split in four (advisor):
     - 5d-1: shared constants. A module constant lives in the static area
       and every carrier may read it, so `list_slice` must not compact one
       in place while carriers run on threads: it copies the window
       instead. C only, no ABI change. Tested under ThreadSanitizer.
     - 5d-2: the threaded runner becomes production code
       (`run_on_carriers`), and `chimi_scheduler_run` uses it. A parked run
       is allowed there, because the ABI path forgets the kernel; a failed
       spawn ends the run as a scheduler defect.
     - 5d-3: fiber ABI 29. The config gains the carrier count (0 or 1 is
       one) and a hook to bind a carrier thread to the C runtime, appended
       after `take_release`. More than one carrier with the virtual clock
       is a defect. The header says which thread runs each hook.
     - 5d-4: `--carriers=<n>` in the C runner, exit 64 for a bad value or
       for more than one with `--clock=virtual`. The default stays 1.
     Also for later: non-atomic close-on-exec off Linux gets its own small
     PR before 5e; `exit()` from a carrier, blocking binding releases under
     the lock, and `strerror` go to 5e.
     - Close-on-exec as landed (before 5e): off Linux a socket, pipe or
       kqueue is marked close-on-exec by a second call, so a child started
       on another carrier between the two could inherit it. Rather than
       guard every such site with a lock, a child std/exec starts holds
       only its three standard streams: macOS spawns with
       `POSIX_SPAWN_CLOEXEC_DEFAULT` and names an inherited stream with
       `posix_spawn_file_actions_addinherit_np`; glibc (2.34 and later)
       adds `posix_spawn_file_actions_addclosefrom_np(3)` as the last
       action. std/exec is the only code that starts children. The rule
       also stops a child from holding a descriptor the program itself
       inherited. Test: std_exec.rs `a_child_holds_only_its_standard_streams`
       starts the program with descriptor 9 open and checks its children,
       inherited and captured, do not hold it.
     - 5d-1 as landed: `list_shared_across_threads` is true for a list in
       the static area when there is more than one carrier, outside the
       simulation build (whose carriers share one thread). `list_slice`
       then copies the window into the slicing carrier's heap, so nothing
       rewrites a constant or appends to the static area while carriers
       run. One carrier compacts a constant as before, so the memory
       report of a one-carrier run is unchanged. The stress fixture slices
       one constant from four threads and checks that the windows are
       copies, the constant keeps its vector, a long window of a whole
       constant shares its vector, and each carrier's own list still
       compacts; without the fix ThreadSanitizer reports the race. The
       review found the case latent: `decode_constant` builds every list
       constant whole, and a whole list is never compacted, so only a
       constant built as a window (the fixture's) reaches it. The guard
       stays so a later constant form cannot bring the race back. The
       fixture runs five carriers, so exactly two is untested here; 5d-4's
       flag tests and 5e cover it.
     - 5d-2 as landed: `process_abi::run_on_carriers(scheduler, root,
       code)` takes the kernel, runs the root with carriers 1 and up on
       scoped threads, and hands the kernel back with the outcome;
       `chimi_scheduler_run` uses it (one carrier until 5d-3, so no thread
       is spawned). Spawned carriers wait at a start gate until every one
       has a thread. If a spawn fails they leave without touching the
       kernel and the run defects with `SCHEDULER_DEFECT_CODE`, its roots
       unstarted and the kernel runnable again; if the spawning loop
       unwinds, a guard sends them away too. The run asserts the last
       run's end is cleared before any carrier starts (the review found it
       guarded only by a race). A parked run's kernel is forgotten on the
       ABI path, since its fibers may be homed on joined threads; the
       test-only `run_root_on_threads` refuses one instead. Tests: a stack
       overflow on a carrier thread ends only its task, and a failed spawn
       leaves the roots unstarted and the kernel able to run them all.
     - 5d-3 as landed (fiber ABI 29): `ChimiSchedulerConfig` gains
       `carriers` (0 or 1 is one), `carrier_enter` and `carrier_context`,
       appended after `take_release`. `chimi_scheduler_run` builds the
       kernel with that many carriers (`with_options_carriers`, so a wake
       pipe that cannot be opened defects) and calls the enter hook on
       each carrier thread before it runs a fiber. More than one carrier
       under the virtual clock defects before any task runs. The header
       says which thread runs each hook: the entry and tasks on their
       carrier's thread without the lock; every other hook on whichever
       carrier holds the lock, one call at a time; the deadlock report on
       the caller's thread. Tests run the supervisor fixture through the
       ABI on three carriers (the enter hook once per carrier thread, the
       same kinds of trace event as one carrier), refuse two carriers
       under virtual time, return a parked two-carrier run as parked, and
       check each carrier thread is entered before its first fiber.
       Found on the way: ES01's fixture settles a restart with a 1 ms
       sleep, which one carrier always honours (the restart is ready
       before the timer) but carriers on threads under the real clock need
       not, so its second crash can reach the dying incarnation and the
       run stalls. That is the program's own race, not a lost wake; the
       ABI test uses a fixture that crashes each member once instead. 5e's
       stress programs must not lean on such timing either.
     - 5d-4 as landed: the C runner takes `--carriers=<n>`, from 1 through
       256 (each carrier holds a wake pipe), and exits 64 for anything else,
       for more than one with `--clock=virtual`, and for more than one in
       the carrier simulation build. It calls `carriers_create(n)` before
       the program runs and passes the count and `enter_carrier_thread`
       (which calls `carrier_thread_enter`) in the scheduler config. The
       default stays one carrier. Tests (tests/carriers.rs): four workers
       on their own memberships answer 200 requests, each starting a task
       and building a list, with the same value on 1, 2, 4 and 8 carriers
       ten times each; four carriers show four threads in /proc while the
       program runs; bad counts and the virtual clock are refused. The
       count must start with a digit, since `strtoul` alone takes a sign
       or a space (`--carriers=-18446744073709551612` read as 4).
       The review found a scheduler bug the flag made reachable: a stop
       whose deadline forced a process body that had never run, from
       another carrier, ran its first resume there (`may_resume_here`
       allows it) while its record still named its old home, so debug
       builds aborted on "a task resumed off its home carrier", and a
       release build left a record whose home disagreed with its fiber's
       thread. A fiber's first resume now makes the current carrier its
       home (`prepare_run`), as `Home` already records. Tests: a scheduler
       test forces an unstarted body from carrier 0, and a program stops
       six busy processes with a 50 ms deadline on four carriers.
   - 5e, the looped stress programs at 2 to 8 carriers.
     - 5e as landed: tests/carrier_stress.rs builds seven programs whose
       value does not depend on interleaving or timing, and runs each on 1,
       2, 4 and 8 carriers (10 or 20 times each in the suite, two minutes
       at most per run): six processes in a ring passing four tokens 3000
       hops each; eight receivers of one queue fed by three producer
       processes, half in a two-arm `Select.first` and half in
       `Channel.receive`, taking 1 to 6000 with none lost or doubled; four
       registered workers that defect after every second answer, about a
       hundred restarts while main keeps asking; twenty rounds of six
       processes stopped with a 5 ms deadline while their
       `Resource.scoped` release sleeps, so each cancellation unwind parks
       at home and is forced; twenty-five rounds of eight processes with
       three sleeping tasks each, stopped with 2 ms and zero deadlines, so
       forced bodies' tasks end through unwind jobs; thirty rounds of
       eight processes stopped mid-spin with 1 ms and 3 ms deadlines, so
       deadlines fire away from the bodies they force; and four processes
       opening, echoing over and closing loopback sockets 25 times each.
       The review's verifier found a heap corruption the first programs
       missed, there before 5e and possible on one carrier: a zero shutdown
       deadline forces a body at once, and `begin_shutdown` (and `force`)
       wrote `RESPONSE_FORCED` into the request of a body that was
       Runnable, whose request had already been answered (a woken task
       keeps its request pointer until it resumes). A `Channel.send` whose
       value the receiver had taken then read as forced, and the C runtime
       freed the message the receiver owned. At 2 to 8 carriers the
       verifier's program crashed in about 70% of runs. A force now writes
       the status only into a parked body's open request; a woken body
       learns it was forced from its resume's terminal context, as before.
       Tests: process_tests.rs `a_force_leaves_the_answer_of_a_woken_bodys_send_alone`
       (deterministic, one carrier) and the zero-deadline stress program,
       both failing without the fix. Mutants of the cross-carrier paths
       are in scripts/mutation/carrier_stress_mutations.py.
       Hazards closed: a process body now makes the bound-carrier check a
       task makes (`require_bound_carrier`); the fixture tests it through
       the task entry, since the process entry is built only into the
       process kernel. A fatal runtime error (exit 70) now goes through
       `runtime_exit`, so only the first failing carrier calls `exit`
       (which drains queued console lines) and any other leaves with
       `_exit(70)`: concurrent `exit` calls are undefined by POSIX. glibc
       serializes them itself, so no test here tells the two apart.
       Left as known: a binding handle's release runs under the kernel
       lock, so a slow release (a socket close that lingers, a database
       close that writes) stalls every carrier for its length; PR 6's
       measurements say whether it matters. `strerror` is MT-Safe from
       glibc 2.32, which gives each thread its own buffer (strerror(3) and
       the 2.32 release notes); macOS's has not been checked, and an
       unknown code's message there may share one buffer.
   Design points: one wake descriptor per carrier, written after unlock
   when another carrier queues work for it or a new earliest timer is set.
   Only carrier 0 polls descriptors, children and the signal pipe, since
   the reactor is one table, and another carrier that registers a wait
   wakes carrier 0. Global idle is every carrier idle with every ready
   and job queue empty, no timer, no IO or child wait and no host
   operation; only carrier 0 acts on it. An inline resume whose fiber
   lives on another carrier becomes a job there. The job runs the resume
   without the lock, then the rest of `begin_terminal` under it, and the
   inline path stays when the home is the current carrier.
   ThreadSanitizer is available for C through gcc only. Rust's needs a
   nightly toolchain, which is not installed, and corosensei's stack
   switches are invisible to it anyway, so Rust thread tests rely on
   stress loops and assertions.
   - Each carrier has a wake descriptor. The carrier loop polls it, and
     carrier 0 also polls the signal pipe.
   - A fiber stays on the carrier of its first resume. Corosensei's
     coroutine is not `Send`, so an unstarted fiber is handed over through a
     narrow wrapper.
   - Placement: `Task.start` runs on the parent's carrier. A process
     incarnation runs on membership id modulo N.
   - Carrier 0 owns signals, stdin and the root.
   - Global idle means every carrier is idle, every ready queue is empty, no
     timer is due and no IO wait is pending, all checked under the lock.
     That check is what decides a deadlock.
   - Timers stay in the kernel. Any carrier fires due timers.
   - `--carriers` above 1 together with `--clock=virtual` exits 64, as
     `--signal` does.
   - Tests:
     - ThreadSanitizer on the Rust tests.
     - Looped stress programs at 2 to 8 carriers: process ping-pong; a select
       racing a plain receive on one queue, with no value lost or doubled; a
       restart storm with shutdown deadlines; handle churn.
6. **Measure, then choose the default.** Changing the default to one carrier
   per core changes the interleavings of real-clock programs and the seeded
   `random` stream, so that change is the project owner's decision, made with
   the measurements in hand. The meaning of the memory report's peak fields
   is decided in the same pull request.
   - 6a as landed: `chimi-process-lab performance --carriers <n>` measures
     PG04, PG05 and PG08 on n carriers (the other gates' workloads pace
     themselves with virtual sleeps, so they report `unsupported`), and
     `scripts/measure_carriers.py` adds parallel work
     (`scripts/carriers/app/fan_out.chimi`) and serial round trips
     (`ping_pong.chimi`). Numbers and method:
     `docs/performance/multi-carrier-baseline.md`. The first run found the
     shared mark-epoch counter capping a counting loop at 1.7 times on four
     carriers; taking epochs in blocks brought it to 3.7, what the host gives four unrelated processes. What the
     measurements left: a hand-off to another carrier costs a wake-pipe
     write and a sleeping thread's wake-up, about 10 to 20 µs on the cloud
     host, so a message round trip across carriers takes 22 to 39 µs
     against 3 µs on one, and PG04's stop, which ends processes on every
     carrier, misses its one-carrier limit. A carrier that spins briefly
     before it parks is the next change to measure (6c). The review found
     the same shared-counter pattern in span ids under traced modes (a
     traced counting loop ran 2.5 times slower on four carriers than on
     one); a carrier on a thread now takes span ids 4,096 at a time
     (`take_spans`), so they are unique but not dense across carriers.
     Kept at one carrier by default (decisions.md, 2026-11-21).
   - 6c as landed: an idle carrier spins up to 50 µs on a wake flag
     (`WakeFds::spin`, set by `Marked::write` after the byte) before it
     blocks, and only when every carrier can have a core. A spin that sees
     the flag empties the watched pipes, as a blocking wait would, so the
     pipe stays the authority and a flag or byte left over only ends one
     later wait early. A signal that lands during a spin is seen when the
     spin ends, about 50 µs late while the spinner holds its core. Carrier
     0's descriptor wait and a stalled root's signal wait do not spin. Cross-carrier round trips fell from 22-39
     µs to 8-10 µs; PG04's start and stop, which are kernel-lock bound, did
     not improve (stop got slower), so their fix is step 4's sharded kernel.
     Numbers: multi-carrier-baseline.md "Spinning before a carrier blocks".
   - 6b as landed: the memory report's `heap_chunk_bytes_peak` and
     `fragment_bytes_peak` mean the whole program's peak on several carriers
     too. Each carrier folds its change into process-wide atomic counters
     once it has moved 64 KiB either way since its last fold
     (`footprint_fold`), and a fold raises the process peak with a
     compare-and-swap. Young allocations, whose counts wait for their
     pass's end, are checked once per 16 KiB chunk. The report takes that
     peak, raised to the exact sum at exit, so on n carriers it is within
     n × 80 KiB of the true peak;
     one carrier and the simulation build still record it exactly, as
     before. A per-carrier peak would drift without bound, since an
     allocation made on one carrier can be freed on another. The other
     peaks (a scope's, an incarnation heap's, a task's) have one owner each
     and were already well defined. `fixtures/carrier-threads-stress.c`
     allocates on one carrier and frees on another 1,000 times, overlaps a
     pass's unflushed young values on one carrier with a rise on another,
     and checks the process peaks against the true one.

After step 5, `Task.start` fan-out still runs on one carrier. Only processes
spread out until step 4 of "Order of work" adds stealing.

## Step 4 plan

Step 4 was written as "shard the kernel and the channel registry by owner,
and add stealing". Measuring first (multi-carrier-baseline.md, "Where
start, stop and messaging spend their time") showed that the two halves
fix different things, so they land as two tracks, stealing first:

- Parallel messaging is bound by the kernel lock. Eight pairs that share
  nothing but the kernel run 2.8 times slower on four carriers than on
  one, and most thread samples wait for the lock. Sharding is the expected
  fix; 4e measures whether it is.
- Process stop is bound by hand-off latency. A supervisor stops its
  members one at a time, each on the next carrier, and that carrier is
  usually asleep. Stealing fixes it: an idle carrier runs the hand-off
  itself instead of waking the task's home. The lock is a small part of
  this cost.

Stealing is smaller, independent of the kernel's shape, and is the change
the gates measure, so it goes first.

1. **4a: measure (done with this plan).** `scripts/carriers/app/pairs.chimi` and
   its row in `scripts/measure_carriers.py`; the evidence above.
2. **4b: a started fiber may move to another carrier.** Today a fiber
   never leaves the thread of its first resume (`PinnedFiber`), because a
   compiler may keep a thread-local's address in a register across a call
   ("Runtime state by owner", rules). This PR makes that safe:
   - In `runtime.c` the carrier is read through an accessor that the
     compiler cannot keep across a call that parks.
   - The fiber crate's thread-locals get the same audit.
   - `PinnedFiber` gives way to a home the kernel may change while the
     fiber is parked.
   A fiber that never ran can already start on any carrier
   (`prepare_run`), so only started fibers need this. The asserts that a
   fiber resumes and is destroyed on its home (`PinnedFiber::enter`, its
   `Drop`, and `prepare_run`'s) become checks that the kernel moved it
   while it was parked. Programs move nothing yet; a test moves fibers
   between real threads, alongside the simulation build that already moves
   every resume. The accessor's cost on one carrier is measured with PG01 and
   PG06, and it must stay within their noise.
   - 4b as landed:
     - The fiber crate had a real bug for a moved fiber.
       `chimi_fiber_request` restored the fiber's park context through a
       reference it took before suspending. After a move it wrote the old
       thread's slot, and the next park on the new thread found none. Now
       the context is read and written only through two functions that
       are never inlined (`park_context`, `set_park_context`), after the
       suspend returns. `a_parked_fiber_moves_between_threads` failed
       before the change.
     - In `runtime.c`, `carrier` is `carrier_current()`. On Linux (x86-64
       and aarch64) it is one volatile read of the thread pointer plus the
       variable's offset from it. That offset is the same on every thread
       for an initial-exec variable, and is taken once before main. On any
       other target (macOS today) it is a call the compiler cannot merge.
       The writer's "holds the console lock" flag, the only other C
       thread-local, became a `Carrier` field.
     - A plain call cost 30% on message copying (PG06's list payload), so
       leaf code that never parks reads the carrier once
       (`Carrier *const here = carrier;`). With that, one carrier runs 1.9
       to 3.8% more instructions on PG06's payloads (callgrind); wall-time
       medians move by -13% to +31% between runs and builds on this host,
       and 12 interleaved runs of the text payload differ by 6%
       (multi-carrier-baseline.md, "One carrier after 4b").
     - What no test checks yet: a C fiber (the runtime's, not the test's
       Rust fibers) moving threads. The C side rests on the simulation
       build, which moves every resume between two carriers on one thread,
       and on reading the generated code (gcc and clang, x86-64 and
       aarch64). 4c adds C fibers that really move (see 4c as landed).
     - `Scheduler::rehome` moves a task that neither runs nor drains to
       another carrier, and lets its fiber's next resume claim that
       carrier's thread (`Home::release`). Only tests call it until 4c.
       `a_parked_task_moved_to_another_carrier_resumes_on_its_thread`
       moves six tasks to the next carrier each time they become ready, on
       three real threads under both clocks, and checks that every resume
       is on another thread.
3. **4c: an idle carrier steals.** Before it parks, an idle carrier takes
   the oldest ready task of a carrier that is parked asleep and makes
   itself that task's home, so a hand-off to a sleeping carrier runs at
   once. It never takes from a carrier that is running or spinning, and
   never takes the root or a task waiting on carrier 0's descriptors.
   - The steal pops the task from the victim's ready queue and rehomes it
     under one hold of the lock. `rehome` checks only that the task neither
     runs nor drains, so the queue, jobs and the victim's queued handle
     releases are the steal's to keep right. A stolen incarnation's
     restarts still go to its membership's home.
   - Goal: PG04's stop within 1.5 times its one-carrier value on four
     carriers.
   - Parallel counting must not get slower.
   - A stolen process keeps its membership's home for restarts.
   - 4c as landed:
     - `Scheduler::steal` runs in `next_runnable` once the carrier's own
       jobs, ready queue and host releases are empty. It takes the front
       of the first parked carrier's ready queue, trying carriers in turn
       from the next one, and `move_home` makes the thief its home. Carrier
       0 never sets its parked flag, so it is never a victim; it can steal.
     - Only a task that has run before is taken. A task that never ran is
       new work, and taking it would pull a fan-out onto the carrier that
       started it. A carrier whose front entry never ran is skipped, and
       the search goes on to the next one.
     - "Parked" is the flag `park_step` sets and the carrier's next kernel
       access clears. It covers the 50 µs spin before the block, so a
       spinning carrier can be stolen from; it then finds nothing and parks
       again. Telling the two apart needs a flag the carrier writes outside
       the lock, and the measurements did not call for it.
     - When the steal empties the victim's ready queue and jobs, its wake
       mark is withdrawn before the lock is released. A carrier other than
       0 is marked only for its queued work or the end of the run, so
       nothing is lost. Without this, stop on four carriers was 67 µs.
     - Jobs (unwinds) are not stolen. A graceful stop wakes the member,
       which ends itself, so the stop chain goes through the ready queue.
       A first try that unwound a parked process body on the deciding
       carrier left stop unchanged for that reason.
     - Results (multi-carrier-baseline.md, "Stealing"): stop on four
       carriers 85 to 19 µs, within PG04's limit at every count; the serial
       round trip 4 µs at every count (it was 42 µs on eight); fan-out,
       parallel messaging and start unchanged.
     - Runtime C fibers now really move: about 15,000 steals ran across
       the carriers and carrier_stress suites on real threads (counted with
       a temporary log), and both suites pass.
   - 4d-1, before the split: a carrier retries the kernel lock for up to
     20 µs before it blocks in it (`carrier::lock_spinning`). Parallel
     messaging on four carriers went from 1625 to 925 ms, level with one
     carrier (multi-carrier-baseline.md, "Spinning on the kernel lock").
     It does not make more carriers faster; the split does.
   - 4d-2, before the split: the task table is a hash map with a one-
     multiplication hash of the id (`scheduler/id_map.rs`), not a
     `BTreeMap`. A kernel step looks its task up many times, and each
     B-tree lookup walked the tree's levels. On pairs (one carrier, 2000
     rounds, callgrind) the kernel's instructions fell by a third and the
     whole program's by 12% (multi-carrier-baseline.md, "A hash map for
     the task table"). The table is unordered now, so what lists tasks
     sorts them by id, as the tree's order gave before: the deadlock report
     and its trace events, the roots a program termination forces, and the
     unrun contexts released at the end. A hash map does not shrink, so its memory stays at the run's
     peak task count.
4. **4d: the kernel split by owner, still under one lock.** Per carrier:
   the tasks homed on it, its ready queue, jobs and timers, the mailboxes
   of incarnations homed on it, and the channels created on it. Shared:
   supervisors, names, the membership-to-home table, ids and telemetry
   order. This has no behaviour change, as step 3's PR 2 did, and may take
   several PRs: today one `Scheduler` method can touch all of these.
   - 4d plan, after 4d-2. Stack samples of pairs on four carriers still
     put most carrier samples in the kernel lock, spinning or asleep. One
     pairs round trip on one carrier is five requests, each one hold of
     `drive`. Besides per-task, per-incarnation and per-channel records,
     every hold touches state that is global today:
     - `current`, `parked` and the wake marks: every carrier writes them on
       every hold, into shared vectors (false sharing even after a split).
     - `collect_user_signals` (an atomic swap) and `deliver_signal` (reads
       the root's record) on every carrier's every hold.
     - `withdraw_signal_wait` and `clear_child_wait`: a `retain` over the
       global waiter lists on every request.
     - `channels.parcels` read on each channel and mailbox operation.
     - `released_terminal`, taken on every hold, and `task_values`.
     - Config read everywhere: trace mode, stack size, hooks, clock mode.
   - Ids stay as they are: global counters, allocated on the path that
     takes every lock (creation already does). Tasks and incarnations move
     (4b, 4c), so a shard encoded in an id would go stale, and the reports
     sort by id to get creation order. A request finds its own records in
     its own shard; a miss means "not local".
   - PRs, each behaviour-preserving unless it says otherwise, run with
     the simulation build, and measured (pairs on 1, 2 and 4 carriers; pairs under callgrind; PG01
     to PG06 on one carrier):
     - 4d-3: signal collection and delivery on carrier 0 only, which owns
       the root and whose idle wait watches the signal pipe.
       - As landed: `between_resumes` reads the signal words only on
         carrier 0, so the other carriers' housekeeping no longer swaps
         the user signal word or reads the root's record. (A signal-wait
         request still collects user signals on the carrier that serves
         it, and a fired signal timer delivers on any carrier.) One
         carrier is unchanged.
       - The one behaviour change with several carriers: a signal is
         taken at carrier 0's next hold. Before, any carrier's hold could
         take it. If main runs a long step that makes no request, a
         SIGTERM's cancel and a std/signal wake wait for that step to end,
         as they always did on one carrier.
       - The waiter lists (`withdraw_signal_wait`, `clear_child_wait`)
         stayed: a `retain` over an empty list costs next to nothing.
         Under 4e it writes `Global`, so the local path checks that the
         list is empty first.
       - `std_signal.rs` `signals_reach_a_program_on_four_carriers`: three
         hangups for three waiters, and a terminate that drains a program
         sleeping on several carriers, on four carriers.
     - 4d-4: immutable configuration into a `KernelConfig`.
       - As landed: `Scheduler.config` holds the stack size, trace mode,
         trace sink and its context, the host's release hooks, the
         release take and the violation report. Field moves only: no behaviour
         change, so no mutants and no measurement. The trace sink is set
         once after construction (`set_trace_sink`), before any carrier
         runs. Channel config (`observe_*`, the copy hook) stays inside
         `ChannelRegistry` until 4d-5.
     - 4d-5: a channel operation moves into its task's record, so channel
       state is per channel and per task.
     - 4d-5 dropped: a channel operation is keyed by its task, so it can
       split by carrier as the task table does, without moving into the
       task's record.
     - 4d-6, in parts: a `Shard` per carrier holding its ready queue, jobs,
       parked flag and wake mark; then tasks (a move moves the record);
       then incarnations, which follow their task; then channels, in their
       creator's shard, and `released_terminal` and `task_values` per
       shard. Shared: a `Global` with homes, supervisors, names, parcels,
       counters, clock, timers, signals, reactor, children and trace.
       Carrier 0's housekeeping writes `Signals` (the latch, delivery,
       timers once the root ends), so it takes every lock when a signal
       word is set or the root has ended.
       - 4d-6a as landed: `scheduler/shard.rs`. `Shards` holds one `Shard`
         per carrier with its ready queue, jobs and parked flag, which were
         three vectors indexed by carrier (`ReadyQueues`, `Jobs`,
         `parked`). The wake marks stay in `Wakes` beside the pipes they
         write. No behaviour change.
       - 4d-6b, tasks: measured and changed. A task table with one map per
         carrier, each record filed under its home and looked up in the
         current carrier's map first, cost 5.6% more instructions on pairs
         with one carrier (callgrind, 2000 rounds: 659 to 696 million),
         even with the fast path inlined: every lookup goes through one
         more indirection and bounds check, and a step does dozens. That
         breaks the plan's rule that a split costs nothing measurable on
         one carrier. So the task table stays one map, and 4e protects it
         by rule rather than by partition: inserting or removing a record
         takes every lock (start and release already do), so the map's
         structure is read-only under a shard lock, and a record is
         written only under its home carrier's lock or every lock. The
         same rule fits the incarnation and channel tables, so 4d-6c is
         not needed either; 4e's guard types carry the rule.
5. **4e: a lock per shard.** A request whose state is all in its own
   carrier's shard takes only that shard's lock: a send to a local
   mailbox, a receive, a channel operation on a local channel, a yield or
   a sleep. Every other request takes every shard's lock in carrier order,
   as one lock does today. Goal: parallel messaging at four carriers
   faster than at one, and as close to the host ceiling as the
   measurement allows.
   - 4e plan. A `Kernel` holds the config, `Global` and one lock per
     shard. One shard lock allows reading `Global` and writing that shard;
     every lock, taken in carrier order, allows writing anything. A request
     is local when tracing is off, its task and the mailbox or channel it
     touches are in the carrier's shard, no select, IO, child or signal
     wait or scope deadline is involved, and no host hook would run.
     Anything else takes every lock and runs today's code. Steal takes the
     victim's lock with `try_lock`. (4e-1 replaced the per-shard locks with
     one reader-writer lock; see "4e-1 as built".)
     - 4e-1: the locks, every request on the all-locks path (cost of n
       locks measured); 4e-2: the local path for process send and
       receive, channel send and receive, wake and the run loop, with a
       per-shard count of escalations; 4e-3: widen it where the counts
       show it pays.
     - The access model, after 4d-6b. Rust forbids two live `&mut
       Scheduler`, even over disjoint fields, so the local path never makes
       one:
       - `Kernel` keeps the scheduler in an `UnsafeCell` beside the shard
         locks. The all-locks guard hands out `&mut Scheduler`, as the one
         lock does today. A shard guard hands out a `LocalKernel` view:
         shared references to the config and to the tables' structure, and
         mutable access only to its own shard and to records homed on it.
       - Records that a local request writes (a task, an incarnation's
         mailbox, a channel's queues) sit behind their own `UnsafeCell` in
         the table, so a shared reference to the table and a written record
         never alias. Inserting or removing a record, and changing a home,
         take every lock.
       - A test build checks the rule: each record carries its home, and a
         local write of a record homed elsewhere panics.
       - This is soundness work, so 4e-1 and 4e-2 get an Opus review with
         a verifier, a TSan run (gcc, as for the C side) of the carrier
         suites, and the simulation build.
     - 4e-1 first build: `carrier::Kernel` held one cache-line-padded lock
       per carrier and the scheduler in an `UnsafeCell`, and every access
       took every lock in carrier order. Pairs (20,000 rounds, against main
       at 062029f): one, two and four carriers within noise, eight carriers
       690 to 853 ms, the cost of eight locks per hold.
     - 4e-1 as built, the lock redesigned: one reader-writer lock instead
       of a lock per carrier. Only one thread ever reaches the kernel as a
       given carrier, so a carrier on the local path needs no lock of its
       own to keep another carrier off its records: the rule "write only
       what is homed on you" already does. The read side is the local
       path; the write side is the one kernel lock of before, and costs one
       acquisition however many carriers there are. Every access still
       takes the write side, with the 4d-1 spin. Pairs, two alternating
       rounds of five runs each against main at 062029f (medians): one
       carrier 840 and 844 against 872 and 859 ms, two 780 and 735 against
       784 and 807, four 741 and 688 against 843 and 761, eight 669 and
       616 against 728 and 733: no slower anywhere, and a little faster
       with several carriers (the lock now sits on its own cache line).
       The access model above holds with "a shard lock" read as "the read
       side, as the carrier named by `Locked`", and "every lock" as "the
       write side".
     - 4e-1 also files tasks in `scheduler/homed.rs`: each record in its
       own `UnsafeCell`, its home beside the cell (`Task.carrier` moved
       there). Until the local path exists every write goes through
       `&mut Homed`, so it behaves as the map it wraps.
     - 4e-2 as built. What the local path writes sits in cells: the
       shards (`Shards::own`), incarnations (`Homed`, homed with their body
       task: `move_home` and a first resume keep the two in step), channels
       (`Homed`, homed for good on the carrier that made them) and each
       task's channel operation (`Operations`, a slot per task made on the
       write side and removed when the task is cleaned up). The tables that
       iterate in order where order reaches output (teardown, shutdown
       deadlines, released values) sort by id. `scheduler/local.rs` runs
       a `drive` step on the read side: finishing a park whose request is
       a process send or receive or a channel send or receive, the
       housekeeping after a resume, and entering the next ready task. Each
       phase checks first and changes nothing when it cannot run locally,
       and the write side goes on from that phase (`Scheduler::step_from`).
       It accepts tracing off; no scope, select, descriptor, child or signal
       wait; no stop under way; no capability in flight anywhere; plain
       open queues with no select watching; and only tasks, mailboxes and
       channels homed on its carrier. `KernelAccess for Scheduler` runs the
       same path over its `&mut`, so every untraced unit test exercises it.
       Pairs (20,000 rounds, medians of five, two rounds, against 4e-1):
       one carrier 676 and 706 against 854 and 826 ms, two 449 and 451
       against 758 and 769, four 271 and 235 against 718 and 716, eight 226
       and 221 against 637 and 662. The host has four cores.
     - Risks: the C handle table's lock is taken by every Sender copy and
       drop, and may become the shared lock on the hot path; host hooks
       have only ever run under one lock, so a request that calls one
       escalates until the C side is audited.

6. **4f: cross-shard requests without the global order,** for example a
   send to another carrier's mailbox queued to that carrier's inbox. Only
   if 4e's measurements show that the all-locks path matters.

The ordering rules ("Ordering rules under several carriers") hold through
both tracks. Each mailbox and each channel queue stays in one shard and
changes only under that shard's lock. Supervisor decisions stay in the
shared part. `--clock=virtual` and the lab keep one carrier, where neither
stealing nor shards change the order of anything.

## Risks

- Signal delivery and the reactor need one owner per descriptor. The simplest
  rule is that carrier 0 owns signals and stdin, and other carriers ask it.
- `Supervisor` decisions must stay atomic across carriers. A supervisor's
  record can live on one carrier, with membership changes sent to it.
- Performance gates (PG01 to PG05) were measured on one thread. Each step
  must re-run them, so the single-carrier path does not slow down.
