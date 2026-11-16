# Parallel Scheduler (R4)

## Status

Design note, not started. Ticket: CHI-267. It records what running Chimi on
more than one thread would take, so that later work does not close the door
on it. Nothing here changes behavior yet.

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

1. **Process-wide globals in `runtime.c`.** About 60 file-scope statics hold
   per-fiber or per-carrier state. Examples are `current_heap`,
   `allocation_target`, the copy and mark stacks, the channel and task handle
   count table (`channel_endpoints`, `released_channels`), and the heap
   totals. `scheduler_request` saves and restores `current_heap` and
   `allocation_target` around every park, which is correct for one carrier
   only.
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
   struct, with no behavior change. This is the largest step, and it is
   mechanical once the list is fixed.
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
