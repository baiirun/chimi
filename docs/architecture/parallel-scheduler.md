# Parallel Scheduler (R4)

## Status

Design note. Ticket: CHI-267. It records what running Chimi on more than one
thread would take, so that later work does not close the door on it. Steps 1
(the carrier struct) and 2 (the shared handle table) are done; programs still
run on one thread.

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
  another; the report sums them and takes the largest peak (PR 5a).
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
    That is one relaxed fetch-add per collection pass.
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

- `carrier` is a thread-local, which is enough only because a fiber never
  leaves the thread of its first resume: a compiler may keep a
  thread-local's address in a register across a call, so a fiber that
  migrated would use the old thread's state. Stealing (step 4) must either
  keep fibers pinned or make `carrier` an accessor that is read again after
  every park.
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
has no lock yet. Step 3 must add one before a second real carrier runs,
because a close on one carrier can race a binding call or a new handle on
another.

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
4. Shard the kernel and the channel registry by owner, and add stealing.
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
     at startup.
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
     deltas from today's counters. And a per-carrier peak of a delta means
     nothing until PR 6 decides what the report's peaks mean. PR 5 has the
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
         `Scheduler::with_carriers` (tests only). Mutants in
         `carrier_routing_mutations.py`, all four killed.
     - 5b-2: the sticky withdraw. The reactor keeps, per binding slot, the
       last generation withdrawn (`channel` is slot << 32 | generation, as
       `binding_resource_key` builds it). An IO or child wait on that
       generation or an older one is answered as a withdrawn parked wait
       (pending, woken at once) without registering, so C's loop sees the
       handle closed. 4 bytes per slot ever used; no ABI change.
     - 5b-3: jobs for `begin_terminal`'s inline resume. The inline path
       stays when the target's home is the current carrier or the target
       never ran (an unstarted fiber given Cancel returns without running,
       so nothing is pinned). A target running on another carrier records
       a pending end, applied by its own `complete_run`. Otherwise the part
       up to `CancellationRequested` runs where the cancel was decided, and
       a job on the home carrier runs the cleanup resume without the lock,
       then the rest of `begin_terminal` under it. While a job is pending,
       `finish_draining` does not publish the task's terminal.
     - 5b-4: `await_incarnation` off home. It runs the body inline only
       on its home carrier; otherwise the body stays queued there and the
       caller takes the "not yet" path it already has. A forced body arms
       no new deadline (the old one is spent, and re-arming it would fire
       at once forever). Process teardown, which must finish
       synchronously, enters the home carrier in 5b; 5c must run it on
       each carrier before the carriers exit.
     - 5b-5: wake descriptors. One non-blocking pipe per carrier, none with
       one carrier. The kernel marks carriers under the lock (work queued
       for another carrier; a reactor, child or signal wait registered off
       carrier 0; a new earliest timer; stop), and `Locked` writes the
       bytes after it unlocks.
     - 5b-6: global idle. Every idle carrier fires due timers first. A
       carrier other than 0 with nothing to do parks on its descriptor;
       carrier 0 decides quiescence, deadlock or a virtual-time jump only
       when every other carrier is parked and every ready and job queue is
       empty. The last carrier to park wakes carrier 0. Tested with a
       round-robin driver on one thread that fails if every carrier parks
       with no descriptor readable and no verdict.
   - 5c, real threads behind a test-only API: a `Send` wrapper for the
     kernel and unstarted fibers, spawning and joining, each carrier
     destroying its own fibers before it exits, and placement.
   - 5d, the runner flag and its config field (fiber ABI 29), and exit 64
     with `--clock=virtual`.
   - 5e, the looped stress programs at 2 to 8 carriers.
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

After step 5, `Task.start` fan-out still runs on one carrier. Only processes
spread out until step 4 of "Order of work" adds stealing.

## Risks

- Signal delivery and the reactor need one owner per descriptor. The simplest
  rule is that carrier 0 owns signals and stdin, and other carriers ask it.
- `Supervisor` decisions must stay atomic across carriers. A supervisor's
  record can live on one carrier, with membership changes sent to it.
- Performance gates (PG01 to PG05) were measured on one thread. Each step
  must re-run them, so the single-carrier path does not slow down.
