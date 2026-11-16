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
  `process_heap_peak_value_bytes`). Today `carrier` is a constant pointer to
  one static, so every access compiles to the fixed address the old static
  had.
- **Shared** (file-scope globals): state every carrier sees.
  - Heaps and constants: `main_heap`, `static_area`, `next_heap_owner`,
    `shared_values`, `constant_values`, `host_stack_limit`.
  - `next_mark_epoch`. Mark epochs are stamped into allocation headers that
    move with a heap, so a per-carrier counter could repeat an epoch that a
    stale header still holds, and treat a dead allocation as marked. It is one
    process-wide atomic counter, so every pass anywhere gets a fresh epoch.
    That is one relaxed fetch-add per collection pass.
  - The handle-count table (`handle_table`, "Handle counts" below). Its
    release queue is carrier state (`released_handles`).
  - Tables whose ids travel inside values: binding handles, open files, test
    mailboxes, process defects.
  - The operation profile's rows (`profile_rows`), allocated once at startup
    and updated after nodes that may park. Each carrier will need its own
    rows, allocated before the carriers start and summed in the report.
  - The `random` provider's one seeded stream, which is deterministic only on
    one carrier.
  - Standard input and every signal field. Signal handlers write these, so
    they must never sit behind a carrier or thread-local pointer.

Rules for the steps that run more than one carrier:

- `carrier` becomes a call to an accessor that the compiler cannot inline or
  treat as pure, so it is read again after every park. A thread-local variable
  is not enough: a compiler may keep a thread-local's address in a register
  across a call, and a fiber resumed on another thread would then use the old
  thread's state.
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
failed. Today a flush drains the running carrier's queue first and then any
other carrier's, in place of that loop. That stand-in
(`carrier_with_releases`) reads another carrier's queue without a lock, so
step 3 must replace it, not keep it: two threads popping one queue could lose
a release or issue it twice. It also hides a missing per-carrier drain, so
the simulation cannot catch that gap; step 3 needs its own test for it.
The unlocked `handles_live()` read also assumes that moving a value to
another carrier synchronises (the hand-off is a scheduler request), which
step 3 must keep true.

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
    the same.
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
     `carrier_with_releases`;
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
3. **C side.**
   - `carrier` becomes a no-inline accessor.
   - A per-carrier drain hook issues queued releases before the carrier
     idles and when it stops. It replaces `carrier_with_releases`.
   - The host stack limit is kept per carrier.
   - The memory counters become signed per-carrier deltas.
   - New test: a count reaches zero inside a release hook, on a carrier
     where no fiber requests again.
4. **One `Mutex` around the kernel (one carrier).** The lock is held except
   while a fiber runs and while the carrier waits. This PR shows the lock
   discipline and measures its cost against the performance gates. Lock
   order is the kernel lock first, then `handle_table`, never the reverse.
5. **N carrier threads behind a runner flag (`--carriers=N`, default 1).**
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
