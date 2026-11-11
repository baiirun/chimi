# Supervised Process Runtime

## Status

Architecture decision for `prog show ts-a6b43b`. Evidence comes from the
disposable prototype in
[`prototypes/supervised-process-runtime`](../../prototypes/supervised-process-runtime/README.md).
The behavior it implements is specified in
[supervisor policy and lifecycle](../specification/supervisor-policy-and-lifecycle.md),
[process execution and mailboxes](../specification/process-execution-and-mailboxes.md),
and [process identity and routing](../specification/process-identity-and-routing.md).
This document chooses a representation. It does not change any of those
contracts.

## Decision

Native supervised processes use four mechanisms:

1. **A heap per owner.** Each process incarnation, and each task that runs
   Chimi code, owns a heap made of 64 KiB-aligned chunks. Every chunk header
   records the owner tag, the region, and the role of its values. The runtime
   switches the current heap around every fiber resume.
2. **Copy on transfer.** A value that crosses an owner boundary is copied once,
   at the moment it is accepted, into a detached fragment. The copy uses a
   forwarding table, so shared subvalues are copied once. The receiver adopts
   the fragment by splicing its chunks into its own heap. Adoption does not
   copy again.
3. **Regions for loops.** Entering a `loop` opens a region. Each pass runs a
   Cheney copy of the pass's young values that the loop state reaches, into
   the region's survivor area, and then resets the young chunks. Values are
   immutable, so older values never point to newer ones. That fact replaces
   stack maps and write barriers: values from before the loop, and survivors
   of earlier passes, are never moved or walked by a minor pass. When the
   survivor area has doubled since the last full collection, a pass collects
   the whole region instead. Leaving the loop splices the region into the
   enclosing one without copying.
4. **Supervisors as kernel data.** Supervisors run no user code, so they are
   records in the scheduler, not fibers. Admission, restart accounting,
   escalation, routing, and shutdown order run between fiber resumes on the
   scheduler's own stack.

The compiler/runtime boundary stays narrow. Generated code calls value
constructors, `region_enter`/`region_pass`/`region_exit` at loop boundaries, and
scheduler requests through `chimi_fiber_request`. It never sees chunks, owners,
fragments, or supervisor records.

## Alternatives rejected

- **One shared heap with a tracing collector across fibers.** A parked fiber's
  frames are opaque to the C backend. Exact roots would need either stack maps
  or a conservative scan of every fiber stack, and a pause would grow with the
  total live data of all processes, not with one process's state. A defect
  could not release its memory in O(chunks).
- **Reference counting with copy-free sharing.** Restart and shutdown would
  have to walk every value. A cycle-free immutable graph would avoid leaks,
  but every send to another owner would still need atomic counts once the
  scheduler runs on more than one thread.
- **Supervisors as fibers.** A supervisor would need a stack, a mailbox for
  member exits, and its own scheduling slot. Its decisions would then
  interleave with member code, so the event transcript would depend on
  scheduling order. As kernel data, a supervisor costs no stack, and every
  decision is ordered by the kernel.

## Evidence

The prototype runs every supervision path on real `chimi-fiber` fibers
(`chimi_fiber_create`, `chimi_fiber_resume`, `chimi_fiber_request`,
`chimi_fiber_destroy`). The heap is C; the kernel is Rust.

### Tests

`cargo test --release --manifest-path prototypes/supervised-process-runtime/Cargo.toml`
runs 19 tests. All pass, in release and debug builds. Each test guards one
decision. Before keeping each test, the guarded mechanism was broken on
purpose and the test was confirmed to fail.

| Decision | Test | Break used to confirm the test fails |
| --- | --- | --- |
| A pass does not re-walk older survivors | `pass_cost_does_not_grow_with_state_that_already_survived` | Run a full collection on every pass |
| Memory depends on live state, not uptime | `a_million_passes_keep_peak_memory_flat` (value bytes and chunks) | Never run a full collection |
| Values from before the loop stay valid | `values_allocated_before_the_loop_survive_its_collections` | |
| A copy preserves sharing | `copying_preserves_sharing` (a graph with 2^40 paths and 41 values) | |
| A pointer into another heap is reported, not followed | `a_pointer_into_another_heap_is_reported_not_followed` | Ignore the owner tag |
| Teardown frees every chunk | `destroying_heaps_and_unadopted_fragments_frees_every_chunk` | |
| A defect restarts only its member, with the exact transcript | `a_defect_restarts_only_that_member_and_the_peer_keeps_its_state` | Keep the name after termination |
| Exceeding the limit stops peers and escalates from the root | `exceeding_the_restart_limit_stops_peers_and_escalates_from_the_root` | Do not stop peers; charge the restart after it starts |
| The restart window rolls | `defects_further_apart_than_the_window_do_not_accumulate` | Never prune the window |
| A name follows the replacement; a direct address does not | `names_follow_the_replacement_and_direct_addresses_stay_unavailable` | |
| A send never parks; FIFO delivery | `a_full_mailbox_returns_the_message_and_delivery_is_fifo`, `a_send_from_a_body_never_parks_and_reports_a_full_mailbox` | Ignore mailbox capacity |
| Shutdown order; unread messages are freed | `stop_runs_newest_dynamic_first_then_declared_in_reverse_and_frees_unread_messages` | Stop declared members in forward order |
| A missed deadline is forced | `cleanup_that_misses_its_deadline_is_forced` | |
| Admission fails fast; removal releases the name | `admission_fails_fast_and_removal_releases_the_name` | |
| A failed start rolls back | `a_failed_root_start_stops_the_members_it_already_started` | |
| A nested escalation is restarted by its parent | `a_nested_supervisor_that_exceeds_its_limit_is_restarted_by_its_parent` | |
| A panicking body is a contained defect | `a_panicking_body_is_a_contained_defect` | |
| A long-lived process keeps flat memory | `a_process_handles_a_million_messages_in_flat_memory` (heap peak and chunks) | |

### Measurements

Command: `cargo run --release --example measure`. Machine: Apple M1, rustc
1.97.1, one thread. Byte counts are exact; times are from a single run.

**Per-pass cost against retained state.** Each pass allocates three garbage
values and keeps a list of N Ints as its state.

| Retained Ints | First two passes (promote + first full) | Steady state, ns/pass | Bytes copied in 100,002 passes | Full collections |
| ---: | ---: | ---: | ---: | ---: |
| 10 | 6.6 µs | 83 | 512 | 1 |
| 1,000 | 56 µs | 69 | 48,032 | 1 |
| 100,000 | 4.6 ms | 59 | 3,200,000 | 1 |
| 1,000,000 | 32 ms | 39 | 32,000,000 | 1 |

Steady-state cost does not depend on retained size. A retained value is copied
twice in total: once when it is promoted and once by the first full
collection.

**Peak memory over 1M passes.** A loop that replaces an Int state 1,000,000
times runs at 55 ns/pass. Peak value bytes are 128 and peak chunk memory is
192 KiB (base, young, and two survivor chunks during a full collection). Both
are reached in the first few passes and never exceeded. Because the state is
replaced on every pass, survivors double on every other pass: 500,000 minor
and 500,000 full collections, each copying 16 bytes.

**Peak memory over 1M messages.** A counter process receives 1,000,000 Int
messages in batches of 64 at 316 ns per message. That time covers the host's
copy, mailbox acceptance, a fiber switch, adoption, and one pass. The process
heap peaks at 80 value bytes and stays there. Thread chunk memory peaks at
4.4 MiB, almost all of it the 64 queued fragments of 64 KiB each (see risks).

**Copy cost per message.** Each row is a copy into a fragment, adoption, and
one receiver pass.

| Message | Value bytes | ns/message | ns/KiB |
| --- | ---: | ---: | ---: |
| Int | 16 | 138 | — |
| Text, 64 B | 88 | 128 | — |
| Text, 4 KiB | 4,120 | 190 | 47 |
| List of 16 Ints | 400 | 281 | 721 |
| List of 1,000 Ints | 24,016 | 13,351 | 569 |
| List of 100,000 Ints | 2,400,016 | 1,604,335 | 685 |
| Shared DAG, 2^20 paths, 21 values | 656 | 402 | — |

Flat byte payloads copy at memcpy speed. A graph costs about 13 ns per
distinct value, most of it in forwarding-table probes and growth.

**Restart and teardown.**

- A defect, termination, restart, and the replacement's first park take
  3.9 µs per cycle.
- Starting a process and running it to its first park takes 4.0 µs with 100
  processes and 5.1 µs with 1,000. Stopping takes 1.6 µs and 2.8 µs per
  process.
- Destroying a heap of 1,000,000 values (246 chunks) takes 66 µs. The cost is
  one release per chunk, not one per value.

## Lifecycle semantics the prototype had to settle

These choices refine the contracts without changing them.

- **Forced termination unwinds from the parked call.** A parked `chimi-fiber`
  fiber cannot simply be destroyed. Destruction resumes it with `Cancel`. If
  the body parks again, `corosensei` force-unwinds the coroutine through the
  `extern "C"` `chimi_fiber_request` frame, and the process aborts. At the
  deadline, the kernel therefore resumes the fiber with a *forced* reply, and
  the runtime side of the parked call unwinds the body to the fiber entry. In
  the native runtime this becomes a cancellation control value that every
  generated call propagates without running further user code.
- **Cleanup never receives.** A `receive` made after cancellation parks until
  the deadline and never delivers a message. The routing contract says
  cleanup never processes the mailbox, so this keeps the two consistent.
- **The first body run is scheduled.** A new incarnation is registered and
  routable at creation, and its body first runs from the ready queue. An early
  message waits in the mailbox.
- **Escalation stops members before defecting.** On an exceeded limit, the
  trace is `RestartLimitExceeded`, then a `Terminated` event for each
  remaining member in shutdown order, then `Escalated`. No
  `RestartAttempted` is recorded for the refused attempt.
- **Nested supervisors produce no `Started` or `Terminated` events.** Those
  events carry an incarnation identity, which a supervisor does not have. A
  nested supervisor appears through `Escalated`, `Stopped`, and its parent's
  `RestartAttempted`.
- **A `Full` send event names no incarnation.** The trace contract records the
  incarnation that *accepted* a message. `Full` and `Unavailable` record none.

## Mapping onto the native runtime

`compiler/chimi/runtime/native/runtime.c` today allocates every value with a
malloc header. Allocation is linked into one global `current_allocation_scope`.
A `loop` reclaims per pass by marking from the new state, but only when
`!CHIMI_REQUIRES_SCHEDULER`. Once a program uses tasks or channels, loop
reclamation is switched off, because fibers share the one current scope and
`Channel.send` hands the scheduler a pointer the scope does not know about
(`prog show ts-da83af`). This design removes both causes.

| Today | Under this design |
| --- | --- |
| `allocate()` links a malloc header into `current_allocation_scope` | Bump allocation in the current heap's innermost region. The value layout keeps its header-free inline payloads. |
| `allocation_scope_begin` / `_collect` / `_end` in the `loop` node | `chimi_region_enter` / `chimi_region_pass(&state)` / `chimi_region_exit`. The pass cost moves from O(values allocated in the scope) mark-and-sweep to O(young survivors). |
| `reclaims = !CHIMI_REQUIRES_SCHEDULER` | Always reclaim. The scheduler switches the current heap on every resume, so a pass collects only its own owner's values. |
| `Task.start` transfers captures by walking and marking exclusive capabilities (`transfer_captured_value`) | The same walk plus a fragment copy of the captured environment into the child's new heap. Shareable capabilities (addresses, `Running`) are plain integers and copy as-is. |
| `Task.result` returns the child's `ChimiControl *` | The child's result is copied into a fragment when the child ends. The parent adopts it on `Task.result`. The child's heap is then destroyed in O(chunks). |
| `Channel.send` passes `(intptr_t)value` in `ChimiSchedulerRequest.value` | The sender copies into a fragment before the request. The request carries the fragment, and receive adopts it. `Closed` and `Aborted` hand the fragment back, and the sender keeps its original value. |
| No processes | `Process.send` and `Process.receive` are new request kinds, alongside the `CHIMI_REQUEST_TASK_*` and `CHIMI_REQUEST_CHANNEL_*` ones. `Supervisor.*` operations are request kinds handled by the scheduler's supervisor records. |

The design keeps these properties:

- Values reachable only from a parked fiber's C frames stay valid, because a
  pass moves only values reachable from the loop state, and those frames sit
  outside the region. No stack map is needed. `ChimiRootFrame` stays for
  tracing and backtraces, not for collection.
- `ChimiControl` objects must be heap values of the current owner, so a
  `break` value leaving a loop is spliced, not copied.
- A dependency provider snapshotted by `Process.spec` is held by the
  supervisor record as a detached fragment, because supervisors have no heap.
  It is copied into each incarnation's heap at start, so a restart never
  shares mutable runtime state with the incarnation it replaces.

### Trace ABI additions

`ChimiSchedulerTraceEvent` gains a third family,
`CHIMI_SCHEDULER_TRACE_PROCESS = 3`, with one kind for each prototype
`LifecycleEvent`: `process_started`, `admission_rejected`,
`process_terminated`, `restart_attempted`, `restart_limit_exceeded`,
`supervisor_escalated`, `membership_removed`, `supervisor_stopped`, and
`process_send`. The event needs these fields:

- `supervisor`, `membership`, and `incarnation` identities. `task` and
  `parent` can carry the incarnation and the supervisor.
- A terminal or admission `cause`. The existing `cause` field can carry an
  enum value.
- For a send: the address kind (named or direct), the name's label as a
  string-table index, the accepting incarnation or 0, and the outcome. These
  are new fields, so `CHIMI_FIBER_ABI_VERSION` moves to 4.
- `attempts_in_window`. `queue_depth` can carry it, but a dedicated field is
  clearer.

`emit_scheduler_event` in `runtime.c` gains the matching name table. The
prototype's event order is the reference transcript.

## Pay-for-play linking

`compiler/chimi/src/backend/native.rs` already materializes and links
`libchimi_fiber.a` only when `requires_fiber_runtime` finds a task or channel
operation, and `program_data.rs` emits `CHIMI_REQUIRES_SCHEDULER` from the same
kind of check. Processes extend this in three tiers:

- **No concurrency.** One heap and regions, with no scheduler and no fiber
  archive. The region code replaces the current allocation scope, so programs
  with no concurrency gain loop reclamation rather than paying for anything
  new.
- **Tasks and channels.** `libchimi_fiber.a` with the task and channel
  scheduler, plus one heap per task, fragments, and the heap switch on resume.
- **Processes.** Adds the process kernel (supervisor records, the name table,
  and mailboxes) as a separate archive or object. A new
  `requires_process_runtime` check selects it when the core program references
  `Process.*` or `Supervisor.*`, together with a `CHIMI_REQUIRES_PROCESSES`
  constant.

Fragment copy and owner checks live in the heap object that every tier links.
The supervisor code is the only addition that processes bring.

## Risks

### Performance

- **Fragment density.** Every fragment reserves a full 64 KiB chunk, so 64
  queued Int messages hold 4 MiB. On macOS and Linux, untouched pages stay
  virtual, so resident memory is one or two pages per fragment, but address
  space and the thread chunk cache scale with queue depth. Mitigation: carve
  small fragments from a shared per-thread fragment chunk and give each
  fragment a sub-chunk owner slot. Owner lookup would then need a second
  level, or would treat 0 as "detached".
- **Per-thread chunk cache.** Up to 64 empty chunks (4 MiB) per thread stay
  out of the allocator to keep per-message cost near 300 ns. Without the
  cache, each message pays an aligned 64 KiB allocation and release. The
  cache is not released when a thread exits.
- **Graph copy constant.** About 13 ns per distinct value, dominated by
  forwarding-table work. A cheaper first pass could copy a tree without a
  table and fall back to the table on the first revisit. Large flat lists are
  also a case for a chunked representation (the runtime's list nodes) rather
  than one item array.
- **Full-collection cadence.** A state that is replaced each pass triggers a
  full collection every other pass. This is correct and cheap while the state
  is small. A large state rebuilt each pass costs O(state) per pass, as it
  must. A small change to a large persistent structure promotes only the new
  path.
- **No incremental full collection.** A full collection of a large region is
  a pause proportional to its survivors (32 ms for 1M values here). It is
  bounded per process and never stops other processes in a multi-threaded
  scheduler, but it is not incremental.
- **Message-heavy workloads copy every message.** This is the price of O(1)
  per-chunk teardown and isolation. Large immutable payloads that need
  zero-copy sharing would need a separate shared, reference-counted area,
  which is out of scope.

### Portability

- **Aligned chunks.** The design depends on `posix_memalign` (or
  `aligned_alloc`/`mmap`) at 64 KiB alignment. Windows would need
  `_aligned_malloc` or `VirtualAlloc` with 64 KiB granularity, which happens
  to be the native allocation granularity there.
- **Thread-local current heap.** `_Thread_local` is required. A future
  work-stealing scheduler that migrates fibers between threads must move the
  current-heap switch with the fiber. It already happens at every resume, so
  no other change is needed.
- **Forced termination needs unwinding.** The prototype forces termination
  with a Rust unwind on the fiber stack. In generated C, termination has to be
  a propagated control value, because C has no unwinding. A body that loops
  without ever calling into the runtime cannot be forced. The same limitation
  already applies to task cancellation.
- **`chimi-fiber` platform matrix.** Stack-overflow detection is gated to
  macOS and Linux on aarch64 and x86_64. Other targets would run processes
  without overflow-to-defect conversion.

### Semantics not yet exercised

The prototype does not model:

- structured child tasks inside a process (cancel and drain before restart);
- `Supervisor.start_supervisor` and `remove_supervisor` for dynamic nested
  supervisors;
- `Supervisor.wait`;
- a scheduler on more than one thread.

The heap invariant (no pointer into another heap) already holds for tasks,
so child tasks should compose: each child gets its own heap, and its result
is a fragment.
