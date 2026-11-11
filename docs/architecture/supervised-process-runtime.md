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

The supervision kernel now runs inside the `chimi-fiber` scheduler
(`runtime/chimi-fiber/src/scheduler/process.rs`; tickets `ts-3d5b38`,
`ts-b1a535`, and `ts-2e0451`). Where the scheduler departs from the
prototype, [The scheduler as built](#the-scheduler-as-built) records what it
does and which test and mutant prove it. The prototype sections remain the
evidence for the decision itself.

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
runs 27 tests. All pass, in release and debug builds. Each test guards one
decision. Every break listed below was applied on its own, and at least one
test failed under it (a crash or abort counts as a failure). Rows marked
"not run" have no recorded mutation check.

The breaks are mutants in `scripts/mutation/heap_mutations.py` and
`scripts/mutation/kernel_mutations.py`. The latest run, with its commit, is
[`MUTATION-RESULTS.txt`](../../prototypes/supervised-process-runtime/MUTATION-RESULTS.txt).
Reproduce it from the repository root with:

```sh
scripts/mutation/mutate.py scripts/mutation/heap_mutations.py scripts/mutation/kernel_mutations.py
```

| Decision | Test | Break used to confirm the test fails |
| --- | --- | --- |
| A pass does not re-walk older survivors | `pass_cost_does_not_grow_with_state_that_already_survived` (copied bytes and `scanned_values`) | Run a full collection on every pass; rescan every survivor on every pass |
| Memory depends on live state, not uptime | `a_million_passes_keep_peak_memory_flat` (value bytes and chunks) | Never run a full collection |
| Values from before the loop stay valid | `values_allocated_before_the_loop_survive_its_collections` | Not run |
| An inner loop's passes never move the enclosing region's values; exit hands survivors to the enclosing region | `inner_passes_never_move_values_of_the_enclosing_region_and_exit_hands_survivors_out` | Drop the region check in `evacuate`; tag spliced chunks as base; drop the inner survivors at exit |
| Adopted messages join the innermost region and are reclaimed by its passes | `adopted_fragments_join_the_innermost_region_and_its_passes_reclaim_them` | Adopt into the base area |
| A copy preserves sharing | `copying_preserves_sharing` (a graph with 2^40 paths and 41 values) | Not run |
| A pointer into another heap is reported, not followed | `a_pointer_into_another_heap_is_reported_not_followed` | Ignore the owner tag |
| Regions, adoption, and minor and full passes compose without moving outer values, changing adopted copies, or leaking chunks | `heap_survives_mixed_regions_fragments_and_collections` (`tests/stress.rs`, `c/stress.c`) | Run on its own: full collection on every pass; never run a full collection; drop the region check in `evacuate`; adopt into the base area; drop the inner survivors at exit |
| Teardown frees every chunk | `destroying_heaps_and_unadopted_fragments_frees_every_chunk` | Free an unadopted fragment without releasing its chunks |
| A defect restarts only its member, with the exact transcript | `a_defect_restarts_only_that_member_and_the_peer_keeps_its_state` | Keep the name after termination |
| Exceeding the limit stops peers in shutdown order and escalates from the root, with the exact transcript | `exceeding_the_restart_limit_stops_peers_in_shutdown_order_and_escalates_from_the_root` | Do not stop peers; charge the restart after it starts; stop declared members in forward order |
| The restart window rolls | `defects_further_apart_than_the_window_do_not_accumulate` | Never prune the window |
| A name follows the replacement; a direct address does not | `names_follow_the_replacement_and_direct_addresses_stay_unavailable` | Do not update the handle's incarnation on restart |
| A send never parks; FIFO delivery | `a_full_mailbox_returns_the_message_and_delivery_is_fifo`, `a_send_from_a_body_never_parks_and_reports_a_full_mailbox` | Ignore mailbox capacity; park a full send until the receiver drains |
| Shutdown order; unread messages are freed | `stop_runs_newest_dynamic_first_then_declared_in_reverse_and_frees_unread_messages` | Stop declared members in forward order |
| A missed deadline is forced | `cleanup_that_misses_its_deadline_is_forced` | Report a forced end as cancelled |
| Cleanup drains accepted messages, then receive returns `Cancelled` without parking | `cleanup_receives_messages_accepted_before_cancellation_then_cancelled_without_parking` | Cancel without delivering queued messages; park a receive after cancellation; force cleanup work inside the deadline |
| Removal cancels a process parked in `receive` and releases its name | `removing_a_process_parked_in_receive_cancels_it_and_releases_its_name` | Remove without shutting down |
| A direct send to a stopping incarnation is `Unavailable` during its cleanup | `a_send_to_a_stopping_incarnation_is_unavailable_during_its_cleanup` | Keep accepting during cleanup |
| A handle outlives a stop or escalation of its supervisor | `a_handle_outlives_a_stop_and_an_escalation_of_its_supervisor` | Look up the membership record unconditionally (the pre-fix code) |
| Membership bookkeeping is bounded by live slots and outstanding handles | `membership_bookkeeping_does_not_grow_with_ended_removed_or_rebuilt_members` | Keep ended records; keep removed records |
| Admission fails fast; removal releases the name | `admission_fails_fast_and_removal_releases_the_name` | Skip the capacity check |
| A failed start rolls back | `a_failed_root_start_stops_the_members_it_already_started` | Do not stop started members on failure |
| A nested escalation is restarted by its parent | `a_nested_supervisor_that_exceeds_its_limit_is_restarted_by_its_parent` | Treat a nested escalation as a root one |
| A panicking body is a contained defect | `a_panicking_body_is_a_contained_defect` | Treat a panic as a return |
| A long-lived process keeps flat memory | `a_process_handles_a_million_messages_in_flat_memory` (heap peak and chunks) | Adopt messages into the base area |

### Measurements

Command: `cargo run --release --example measure`. Machine: Apple M1, rustc
1.97.1, one thread. Byte counts are exact; times are from a single run. A
rerun after the review fixes, on the same machine under load from other work,
reproduced every byte count and collection count. Its times ranged from 0.7x
to 2.5x of the original run, so the times below are from the original run.

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
4.4 MiB, almost all of it the 64 queued fragments: each queued Int message
holds 65,536 chunk bytes for 16 value bytes (see risks).

**Resident memory.** The whole measure run peaks at 141 MiB RSS. Two items
dominate, and neither is per-process cost. The 1M-Int retained state of the
per-pass table is about 24 MB and exists twice during its first full
collection. The kernel's in-memory event transcript holds 1,000,001 events
(38 MiB) after the 1M-message run. That transcript is a test fixture. The
native runtime emits the same events to the trace ABI and keeps none. The
prototype does not measure RSS per process.

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

- **Forced termination unwinds from the pending kernel call.** A parked `chimi-fiber`
  fiber cannot simply be destroyed. Destruction resumes it with `Cancel`. If
  the body parks again, `corosensei` force-unwinds the coroutine through the
  `extern "C"` `chimi_fiber_request` frame, and the process aborts. At the
  deadline, the kernel therefore answers the body's pending kernel call with a
  *forced* reply, and the runtime side of that call unwinds the body to the
  fiber entry. In the native runtime this becomes a cancellation control value
  that every generated call propagates without running further user code.
- **Cleanup drains, then never parks.** After cancellation, the mailbox
  accepts no new messages. `receive` returns the messages accepted before the
  cancellation, oldest first, and then returns `Cancelled` immediately on
  every call. A body that drains and returns ends `Cancelled`. Because
  cleanup can no longer park, the prototype models slow cleanup with
  `ProcessContext::work(duration)`, which consumes virtual time and is forced
  at the deadline. Full drain semantics for the language come with G22. The
  scheduler replaces this model: cleanup may park, and time reaches the
  deadline only through the scheduler clock (see
  [Cleanup waits on the scheduler clock](#cleanup-waits-on-the-scheduler-clock)).
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
  `RestartAttempted`. The scheduler adds `supervisor_started` for every
  supervisor (see [Nested supervisors](#nested-supervisors)).
- **A `Full` send event names no incarnation.** The trace contract records the
  incarnation that *accepted* a message. `Full` and `Unavailable` record none.
- **One root per program.** A root escalation sets the program terminated and
  stops that root's subtree. The prototype assumes a single root supervisor,
  so it does not stop other roots. A runtime that allows several roots must
  stop all of them on a root escalation. The scheduler allows several roots,
  and a root escalation defects every root task and stops every running root
  (`terminate_program`). No test covers more than one root at escalation.
- **Bookkeeping follows the slot.** A membership record is freed when its slot
  is released: on removal, on an end without restart, and on stop or
  escalation. A `ManagedProcess` handle whose record is gone keeps only its
  last incarnation, so `address` returns a direct address that stays
  `Unavailable`, and `remove_process` of an ended membership returns without
  an event. In the native runtime the handle value should carry the last
  incarnation, so nothing outlives a collected handle. In the prototype, a
  handle that is dropped without `remove_process` leaves one entry behind.
- **The event transcript is test-only.** `Kernel::events` grows with every
  event, so it is unbounded by design. The native runtime emits each event to
  the trace ABI and keeps no transcript.

## The scheduler as built

The scheduler reproduces the prototype's kernel decisions. The tests are in
`runtime/chimi-fiber/src/scheduler/process_tests.rs`, and their names begin
with the conformance-lab scenario ID
([supervised-process conformance](../specification/supervised-process-conformance.md)).
`cargo test -p chimi-fiber` runs them. The mutants are in
`scripts/mutation/fiber_mutations.py`. The latest run, with its commit, is
[`runtime/chimi-fiber/MUTATION-RESULTS.txt`](../../runtime/chimi-fiber/MUTATION-RESULTS.txt):
28 mutants, all killed. Reproduce it from the repository root with:

```sh
scripts/mutation/mutate.py scripts/mutation/fiber_mutations.py
```

The subsections below describe where the scheduler differs from the
prototype. Each table row names the test that fails when the rule breaks and
the mutant that confirms it. "None" means no mutant targets that rule.

### Cleanup waits on the scheduler clock

The prototype consumed time inside cleanup with `work(duration)`. In the
scheduler, cleanup is ordinary code that may park. Each incarnation under
shutdown has a deadline of `now + shutdown_within`. When a stop or removal
must wait for the incarnation, the deadline becomes a timer in the
scheduler's timer queue (`TimerTarget::ShutdownDeadline`). Virtual time
moves only when nothing is runnable and no host operation is in flight, and
then it jumps to the earliest timer. The real clock sleeps until that timer.
Cleanup that finishes before its deadline never moves time. When the timer
fires, the body's pending call gets a forced reply and the body runs to its
end. A forced body that parks again is abandoned, not destroyed.

| Rule | Test | Mutant |
| --- | --- | --- |
| Cleanup that parks again is forced at `now + shutdown_within`, and time jumps there | `sd04_cleanup_that_parks_again_is_forced_at_its_deadline`, `sd04_the_c_entry_forces_cleanup_that_parks_again_and_never_resumes_it` | forced fiber destroyed |
| Cleanup that finishes in time does not move time | `sd03_cleanup_within_its_deadline_drains_everything_and_time_stays_put` | None |
| A zero deadline forces at once | `sd02_a_zero_deadline_forces_at_once_and_discards_unread_messages`, `sd02_a_zero_deadline_forces_a_sleeping_handler_without_running_its_cleanup` | zero deadline drains |
| Time moves only when nothing is runnable | `virtual_time_advances_only_when_nothing_is_runnable` | time moves while runnable |
| A host operation in flight holds virtual time | `a_host_operation_in_flight_holds_virtual_time` | None |

### Drain and cancellation rules

Shutdown of an incarnation follows these steps, in order:

1. Routing is removed for the whole subtree before any cleanup runs.
2. The body's child tasks are cancelled and drained.
3. Whatever the body's current park waits on is withdrawn: its sleep timer,
   the child it awaits, its pending channel operation, and its receive.
4. The body learns of the cancellation:
   - A body that has not run yet is cancelled before its first run and
     ends `cancelled`.
   - A body parked in `receive` gets the messages accepted before shutdown,
     oldest first, and then `Cancelled` on every call.
   - A body parked anywhere else is cancelled at that park.
   - A body that is running is cancelled at its next park that would block.

These follow coordinator decision 1 on `ts-3d5b38`. A draining handler's
sleep, `Task.result`, and channel operations are cancelled. Its supervisor
operations are served (RR06). After the body has been told it is
cancelled, every later blocking call except a receive waits for the
deadline. A child task that defects during shutdown makes the end
`defected`, inside the shutdown protocol.

| Rule | Test | Mutant |
| --- | --- | --- |
| Receive drains accepted messages, then cancels | `mb08_removal_drains_messages_accepted_before_it_then_cancels` | cleanup does not drain |
| Routing is removed for the subtree first | `sd06_routing_is_removed_for_the_whole_subtree_before_any_cleanup` | unroute member by member |
| Child tasks drain before the next member terminates, and before a replacement starts | `sd05_each_members_children_drain_before_the_next_member_terminates`, `ra07_child_tasks_drain_before_the_replacement_starts` | replacement before children drain |
| A sleeping handler is interrupted without moving time | `rr05_removal_interrupts_a_sleeping_handler_without_advancing_time` | None |
| A cancelled sleep leaves no timer | `cancelling_a_sleeper_removes_its_timer_and_time_never_reaches_it` | sleep timer kept on cancel |
| A pending channel receive is withdrawn | `shutdown_cancels_a_pending_channel_receive_so_a_later_send_does_not_reach_cleanup` | None |
| A running body is cancelled at its next park | `an_incarnation_runnable_when_shutdown_begins_is_cancelled_at_its_next_park` | None |
| A removal during a stop terminates its target once | `rr06_a_removal_during_stop_terminates_the_sibling_once_and_stop_skips_it` | None |
| A child defect during shutdown stays in the protocol | `a_child_defect_during_shutdown_stays_inside_the_shutdown_protocol` | None |
| Removal waits for termination | `rr01_rt05_removing_a_parked_process_cancels_it_and_frees_its_name` | remove does not wait |

### Deadlock reporting

The prototype had no deadlock rule. `run_until_idle` reports a deadlock
when no task is runnable, no timer is pending, and a task that is not a
process body is parked. A process parked in `receive` alone is quiescent,
not deadlocked. Under virtual time, `run_root` defects the root with
`CHIMI_SCHEDULER_DEFECT_DEADLOCK` (-5). It also returns each parked task
with the request kind it waits on, emits a `DeadlockWaiter` clock event for
each, and `chimi_scheduler_run` passes them to the config's
`deadlock_report` (coordinator decision 5). Under the real clock a host
event may still wake the root, so `run_root` returns `CHIMI_FIBER_PARKED`
and leaves the execution suspended.

| Rule | Test | Mutant |
| --- | --- | --- |
| Each parked task is named and the root defects | `a_virtual_deadlock_names_each_parked_task_and_defects_the_root`, `a_c_root_with_nothing_to_wake_it_defects_with_the_deadlock_code` | None |
| The real clock never reports deadlock | `a_real_clock_root_with_nothing_pending_waits_for_the_host_instead_of_defecting` | None |

### Root supervisors are owned by the task that starts them

A root started through `CHIMI_REQUEST_SUPERVISOR_START` belongs to the
calling task. This is coordinator decision 2 on `ts-3d5b38`. When the task
ends, the scheduler stops that root after the task's child tasks are
cancelled. The task stays draining until every root it owns has retired, and
that includes cleanup that lingers to its deadline. Its `CleanupCompleted`
and `Terminal` events therefore come after its members' terminal events. A
parent task likewise stays draining until its draining children are
terminal. A process body that starts a root is held the same way, and a
removal of that process waits for the root. A root that escalates instead
of stopping also releases its draining owner, so the owner's terminal and
any removal waiting on it still complete while the program terminates. A
root started by the host through `Scheduler::start_supervisor` has no owner.

| Rule | Test | Mutant |
| --- | --- | --- |
| The owner's terminal follows its root's stop, at 5.001 s, deterministically | `an_owner_task_stays_draining_until_the_supervisor_it_started_has_stopped` | owner terminal before its roots stop |
| A parent's terminal follows a child that is still draining | `a_parent_task_stays_draining_until_a_child_waiting_on_its_supervisor_is_terminal` | parent terminal before its draining child |
| A process body that owns a root is held until the root stops | `removing_a_process_that_started_a_root_waits_for_that_root_to_stop` | process owners not held |
| An owner that ends with nothing lingering stops its root before it is terminal | `a_root_supervisor_stops_when_the_task_that_started_it_ends` | None |
| A draining owner whose root escalates still finishes, also at teardown (`ts-bb32bf`) | `removing_an_owner_whose_root_escalates_while_it_drains_finishes`, `tearing_down_with_an_owner_draining_under_an_escalating_root_finishes_the_owner` | escalated root strands its draining owner; escalated root strands its draining owner at teardown |

### Nested supervisors

Declared nested supervisors are members of their parent's policy (SV09,
ES02 to ES04). Dynamic nested supervisors use
`Scheduler::start_nested_supervisor` or
`CHIMI_REQUEST_SUPERVISOR_START_SUPERVISOR`. Each one takes one slot of its
parent's combined capacity. The handle is the membership. It is exclusive,
and `remove_supervisor` consumes it. `nested_supervisor` names the
supervisor that currently runs for the handle. That supervisor changes when
the parent replaces it after an escalation. Removal is `Done` only when the
whole subtree has stopped. The trace then shows the members' terminations,
`supervisor_stopped`, and `membership_removed`. `stop` works on a nested
supervisor too. It ends the membership as a requested end, which frees the
slot and never restarts. A stop or removal that meets an escalation already
stopping the nested supervisor marks it `end_requested`. When the
escalation finishes, the membership then ends as a requested end, not as a
defect, so the parent starts no replacement (`ts-9c0516`).

A stop, removal, or escalation is a plan. Plans can now block on a
supervisor as well as on an incarnation. A plan that reaches a nested
supervisor whose members another plan has already taken waits for that
supervisor to retire. It does not stop the supervisor a second time.
Retiring a supervisor, or a nested escalation finishing, answers every
task in `Supervisor.wait` on it.

`supervisor_started` (trace kind 10) is emitted before a supervisor's
declared members start. It carries `supervisor`, and for a nested
supervisor also `parent_supervisor` and `membership`. Lab open question 5
recommended it for U9. Every transcript therefore begins with
`supervisor_started supervisor=S1 parent=root`. A root that is rolled back
after a failed member start emits `supervisor_stopped`.

| Rule | Test | Mutant |
| --- | --- | --- |
| A dynamic nested supervisor takes a parent slot, and admission is fail-fast | `sv10_a_dynamic_nested_supervisor_takes_a_parent_slot_and_removal_stops_its_subtree`, `sv10_the_request_abi_starts_a_nested_supervisor_and_removal_parks_until_its_subtree_stops` | nested supervisor outside parent capacity |
| Removal stops the subtree, then releases the slot; handles are checked by kind and consumed | same two tests | removal leaves the nested subtree running; handle kind unchecked |
| A handle whose member already ended is still checked by kind, and a wrong-kind attempt does not consume it | `sv10_a_retired_handle_is_still_kind_checked` | retired handle kind unchecked |
| `supervisor_started` names the parent and membership | `sv10_a_dynamic_nested_supervisor_takes_a_parent_slot_and_removal_stops_its_subtree` | supervisor_started without parent |
| A stopped nested supervisor is not restarted | `sv10_stopping_a_nested_supervisor_ends_its_membership_without_a_restart` | stopped nested supervisor restarts |
| A parent stop waits for a nested stop or escalation already in flight | `sv10_a_parent_stop_waits_for_a_nested_stop_already_in_flight_and_stops_it_once`, `es02_a_parent_stop_during_a_nested_escalation_waits_for_it_instead_of_stopping_it_again` | nested stop in flight expanded again; nested escalation strands waiting plans |
| A removal during a nested escalation waits for it, and no replacement starts once it is `Done` | `sv10_removing_a_nested_supervisor_during_its_escalation_starts_no_replacement` | removal in flight expanded again; removal during escalation not recorded; nested escalation ignores a requested end |
| A stop during a nested escalation ends the membership without a restart | `sv10_stopping_a_nested_supervisor_during_its_escalation_does_not_restart_it` | stop during escalation not recorded; nested escalation ignores a requested end |
| A dynamic nested supervisor escalates to its parent, which replaces it or escalates in turn | `sv10_a_dynamic_nested_supervisor_escalates_to_its_parent_which_replaces_it` | None |
| A wait on a nested supervisor returns when it retires | `sv10_a_wait_on_a_nested_supervisor_returns_when_its_parent_stops_it` | only root waiters answered |
| A wait on a nested supervisor returns when it escalates | `sv10_a_wait_on_a_nested_supervisor_returns_when_it_escalates` | nested escalation leaves waiters parked |

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
| `Task.result` returns the child's `ChimiControl *` | The child's result is copied into a fragment when the child ends, and the child's heap is destroyed in O(chunks). Each `Task.result` copies the fragment into the caller's current heap without consuming it (see the port requirements below). |
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
  shares mutable runtime state with the incarnation it replaces. That copy
  must leave the source fragment intact, which needs a new heap operation
  (below).

### Requirements for the `runtime.c` port

The prototype heap has three value shapes, each with an inline payload. The
values in `runtime.c` break several of the prototype's assumptions. The port
must meet these requirements, and each one needs a test that fails when the
requirement is broken, as the prototype tests do.

- **Out-of-line payloads.** A `runtime.c` `ChimiValue` is a fixed header
  whose payload points to separately allocated objects:
  - `ListNode` tries for `List`;
  - `MapNode` trees for `Map`, whose `key` points to text;
  - `RuntimeField` arrays for records and constructed values;
  - `ChimiCallable` records, which point to a `ChimiEnvironment` and an
    `inner` callable;
  - `ChimiEnvironment` slot arrays, which clones can share (for example,
    `clone_local_environment` shares `requirements`).

  Each of these must become a heap object that the Cheney scan and the
  fragment copy can parse (kind and size in a header) and trace. Each must be
  allocated in the owner heap of the value that points to it.
- **Pointers outside every heap.** Field names, type and variant names, and
  literal text point into static program data (`CHIMI_STRINGS`). The owner
  check masks a pointer to find its chunk header, which is undefined for a
  pointer that is not in a chunk. Static data therefore needs its own tag, or
  an address-range check that runs before the mask.
- **List windows.** Since commit 1621c42, a `List` is the window
  `[start, start + length)` over a persistent vector that other lists share.
  A fragment copy must rebuild a compact vector from the window's values. It
  must not copy the whole shared trie, or sending a 10-element slice of a
  1M-element list would copy 1M values. Inside a region, a window keeps its
  whole vector reachable, so a small slice that survives a pass retains the
  whole trie. That cost matches the current runtime, but it is now paid per
  process heap.
- **Environments are written after allocation.** `sequence` and pattern
  binding (`match_plain`) write `environment->locals[slot]` after the
  environment is cloned. `provision` writes `requirements` slots. `loop`
  rebinds its state slot before every pass. In each case an older object
  points to a newer value, which breaks the rule that the pass relies on.
  The port keeps this safe by splitting environments in two:
  - *Frame environments* are the ones these nodes clone and write. They are
    not heap values. They are allocated outside every region (a frame stack,
    or allocations freed at node exit) and are never moved. No heap value
    may point to one. At a pass boundary, the only frame slot that points
    into the region is the loop's state slot. The loop overwrites it before
    the next pass reads it, and it is never read after the loop exits.
  - *Captured environments* are the ones `capture_closure_environment` and
    `capture_requirements` build. They are filled at creation from values
    that already exist, and they are never written again. They are ordinary
    immutable heap values.

  A debug build should check that no traced heap value points to a frame
  environment.
- **`Task.result` adoption.** A task handle is a shareable integer, so
  `Task.result` can be called more than once, and it can be called inside a
  loop pass. A consuming adopt would work only once. It would also leave the
  result in the region of the first caller's pass, which a later pass can
  reclaim. The scheduler therefore keeps the child's result fragment until
  the task record is released, and every `Task.result` copies it into the
  caller's innermost region. The cost is O(result) per call.
- **Copy a fragment into a heap without consuming it.** Both the `Task.result`
  requirement and the provider snapshots above need
  `chimi_fragment_copy_into_current(const ChimiFragment *)`. It copies a
  fragment's graph into the current heap's innermost region, keeps sharing,
  and leaves the source intact. The prototype has only the consuming
  `chimi_fragment_adopt`.
- **Full-collection trigger.** The prototype triggers a full collection when
  the survivors reach twice their size at the last full collection, with a
  baseline of 0. The first pass that promotes anything therefore runs a full
  collection, and a small state that is replaced every pass runs one on every
  other pass (500,000 of 1,000,000 passes). The port must set the baseline
  at the first promotion and require a minimum growth, for example
  `bytes >= 2 * last && bytes - last >= CHUNK_BYTES`. A test must bound
  `full_collections` for a small replaced state.
- **Small messages share chunks** (`prog show ts-df67fb`). Each queued Int
  message holds a 64 KiB chunk (measured: 65,536 chunk bytes per queued
  message). The port carves small fragments from shared slabs, as decided on
  ts-a6b43b.
- **Scan work is part of the pass-cost contract.** `ChimiHeapStats` reports
  `scanned_values` as well as `copied_bytes`. A pass that rescans old
  survivors copies nothing, so only a scan counter exposes it. The port keeps
  both counters and both test bounds.

### Trace ABI additions

`ChimiSchedulerTraceEvent` has a process family,
`CHIMI_SCHEDULER_TRACE_PROCESS = 3`, with one kind for each prototype
`LifecycleEvent`: `process_started`, `admission_rejected`,
`process_terminated`, `restart_attempted`, `restart_limit_exceeded`,
`supervisor_escalated`, `membership_removed`, `supervisor_stopped`, and
`process_send`. The scheduler adds a tenth kind, `supervisor_started`. A
clock family, `CHIMI_SCHEDULER_TRACE_CLOCK = 4`, records time advances,
timers, and deadlock waiters. As built, the event has these fields:

- Dedicated `supervisor`, `membership`, `incarnation`, and
  `parent_supervisor` fields, separate from the task fields (OB07).
  `process_started` also sets `task` to the incarnation's task, so the
  parents of child tasks resolve in traces (coordinator decision 3).
- A terminal or admission `cause` in the existing `cause` field.
- For a send: `address_kind`, the name's label in `name`, the accepting
  incarnation or 0, and `outcome`.
- `attempts_in_window` and the event `time`, each in its own field.

These fields moved `CHIMI_FIBER_ABI_VERSION` to 4. Later request and trace
kinds are additive, so the version stays 4.
`ob07_the_process_abi_layouts_match_the_c_header` checks the layouts and the
nested-supervisor kind values against `chimi_fiber.h`.

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
  cache is not released when a thread exits, and `chimi_chunk_stats` does not
  report it. The port needs a thread-exit destructor (or an explicit drain)
  and cache counts in the stats, so leak tests can see the cache.
- **Graph copy constant.** About 13 ns per distinct value, dominated by
  forwarding-table work. A cheaper first pass could copy a tree without a
  table and fall back to the table on the first revisit. Large flat lists are
  also a case for a chunked representation (the runtime's list nodes) rather
  than one item array.
- **Full-collection cadence.** In the prototype, a state that is replaced
  each pass triggers a full collection every other pass. This is correct and
  cheap while the state is small, and the port's trigger requirement above
  removes it. A large state rebuilt each pass costs O(state) per pass, as it
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

The scheduler now covers the first three (RA07, SD05, SV10, and SV05).
Neither the prototype nor the scheduler runs on more than one thread.

The heap invariant (no pointer into another heap) already holds for tasks,
so child tasks should compose: each child gets its own heap, and its result
is a fragment.
