# Parallel Scheduler (R4)

## Status

Design note. Ticket: CHI-267. It records what running Chimi on more than one
thread would take, so that later work does not close the door on it. Step 1
(the carrier struct) is done; programs still run on one thread.

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
4. **Handle counts.** Task and channel handle counts are plain integers, and
   their releases go through one queue that the current fiber flushes.

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
  not interleavings; before step 3, check every spec rule that names an order
  across owners and decide whether it survives parallel runs.

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
    move with a heap, so a per-carrier counter could repeat an epoch a stale
    header still holds and treat a dead allocation as marked. It becomes a
    per-heap counter, not a per-carrier one.
  - The handle-count table and its release queue (`channel_endpoints`,
    `released_channels`). A handle copied on one carrier can be freed on
    another, so splitting the table would miscount; step 2 replaces it with
    per-carrier deltas or owner-routed messages.
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
- Before step 3, add a test build in which every resume switches to a second
  carrier with fresh scratch and caches. It is the only check that would
  catch a field filed as carrier state that must in fact follow the fiber.
- The lazily created shared values (`shared_values`, the module constants) are
  a race once two carriers can create them; create them before the carriers
  start.

## Alternatives considered

- **One global lock around the scheduler.** It is simple, but every request
  would serialise on it, and most of a service's work is requests. It is
  acceptable only as a short step on the way to sharding.
- **A thread per process.** It does not scale to the many small processes the
  supervision model encourages, and it loses cheap parking.
- **A shared heap with a concurrent collector.** This was rejected for the
  single-thread design already, for reasons that only get stronger with
  threads.

## Order of work

1. Move `runtime.c`'s per-fiber and per-carrier statics into one carrier
   struct, with no behavior change. Done (CHI-267); the list is under
   "Runtime state by owner".
2. Make handle counts and release queues per carrier, with cross-carrier
   releases sent as messages.
3. Split `Scheduler` into a carrier loop plus shared kernel state behind a
   lock, and run N carriers with work stealing turned off. Measure.
4. Shard the kernel and the channel registry by owner, and add stealing.
5. Publish multi-core baselines next to the single-thread ones (R2).

## Risks

- Signal delivery and the reactor need one owner per descriptor. The simplest
  rule is that carrier 0 owns signals and stdin, and other carriers ask it.
- `Supervisor` decisions must stay atomic across carriers. A supervisor's
  record can live on one carrier, with membership changes sent to it.
- Performance gates (PG01 to PG05) were measured on one thread. Each step
  must re-run them, so the single-carrier path does not slow down.
