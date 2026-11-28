"""Mutants of the cross-carrier scheduler paths, run against the R4 step 3
PR 5e stress programs (tests/carrier_stress.rs). M1 to M10, C1 and C2 come
from the PR's verifier; F1 undoes the PR's fix for forcing a woken body.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_stress_mutations.py
Results: scripts/mutation/carrier_stress_results.txt.
"""

CRATE = "compiler/chimi"
S = "../../runtime/chimi-fiber/src/scheduler.rs"
P = "../../runtime/chimi-fiber/src/scheduler/process.rs"
C = "runtime/native/runtime.c"
TASK_ENTRY = "../../runtime/chimi-runtime/src/intrinsics/task_entry.rs"
T = ["carrier_stress"]

MUTATIONS = [
    dict(name="M1 enqueue on another carrier writes no wake", file=S, targets=T, edits=[(
        "            self.ready.push_back(carrier, task);\n        }\n        self.wake_carrier(carrier);\n",
        "            self.ready.push_back(carrier, task);\n        }\n",
    )]),
    dict(name="M2 unwind job queued at home without waking home", file=S, targets=T, edits=[(
        "            self.jobs.push_back(carrier, jobs::Job::Unwind { task, forced });\n            self.wake_carrier(carrier);\n",
        "            self.jobs.push_back(carrier, jobs::Job::Unwind { task, forced });\n",
    )]),
    dict(name="M3 unwind job routed to the deciding carrier, not home", file=S, targets=T, edits=[(
        "            self.jobs.push_back(carrier, jobs::Job::Unwind { task, forced });",
        "            self.jobs.push_back(self.current, jobs::Job::Unwind { task, forced });",
    )]),
    dict(name="M4 last carrier to park does not wake carrier 0", file=S, targets=T, edits=[(
        "        if self.others_quiet() {\n            self.wake_carrier(ready::Carrier(0));\n        }",
        "        if self.others_quiet() {\n        }",
    )]),
    dict(name="M5 global idle ignores queued jobs", file=S, targets=T, edits=[(
        "            && self.ready.is_empty()\n            && self.jobs.is_empty()",
        "            && self.ready.is_empty()",
    )]),
    dict(name="M6 every membership placed on carrier 0", file=P, targets=T, edits=[(
        "let home = u32::try_from(membership.get() % carriers).expect(\"below the carrier count\");",
        "let home = u32::try_from(membership.get() % carriers * 0).expect(\"below the carrier count\");",
    )]),
    dict(name="M7 forced body waiting on home re-arms its deadline", file=P, targets=T, edits=[(
        "if stopping.timer.is_none() && !(stopping.forced && waits_on_home) {",
        "if stopping.timer.is_none() && !(stopping.forced && waits_on_home && false) {",
    )]),
    dict(name="M8 an end decided while running elsewhere is dropped", file=S, targets=T, edits=[(
        "        let end = self.tasks.get_mut(&task).and_then(|record| record.pending_end.take());",
        "        let _ = self.tasks.get_mut(&task).and_then(|record| record.pending_end.take());\n        let end: Option<jobs::PendingEnd> = None;",
    )]),
    dict(name="M9 first resume elsewhere keeps the old home", file=S, targets=["carrier_stress", "carriers"], edits=[(
        "        if !record.ran {\n            record.carrier = self.current;\n        }\n",
        "",
    )]),
    dict(name="M10 a new earliest timer does not wake carrier 0", file=S, targets=T, edits=[(
        "            self.wake_carrier(ready::Carrier(0));\n        }\n        self.timers.insert(deadline, target)",
        "        }\n        self.timers.insert(deadline, target)",
    )]),
    dict(name="C1 carrier threads never bind their carrier", file=C, targets=T, edits=[(
        "  carrier_bind(carrier_at(index));\n  if (!CHIMI_REQUIRES_SCHEDULER) guard_host_stack();",
        "  (void)index;\n  if (!CHIMI_REQUIRES_SCHEDULER) guard_host_stack();",
    )]),
    dict(name="C2 process entry skips the bound-carrier check", file=TASK_ENTRY, targets=["runtime_state"],
         filter="the_runtime_state_holds_across_carrier_threads", edits=[(
        "    require_bound_carrier(UNBOUND_PROCESS);\n",
        "",
    )]),
    dict(name="F1 a zero deadline rewrites a woken body's answered request", file=P, targets=T, edits=[(
        "                task_record.command = ChimiFiberCommand::Cancel;\n            }\n            TaskPhase::Parked if forced => {",
        "                task_record.command = ChimiFiberCommand::Cancel;\n                set_status(task_record.request, RESPONSE_FORCED, forced);\n            }\n            TaskPhase::Parked if forced => {",
    )]),
]
