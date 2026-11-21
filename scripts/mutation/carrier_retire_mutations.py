"""Mutants of R4 step 3 PR 5c-5: a finished run forces the processes left
before it ends, and each carrier but 0 runs its own jobs and retires its
own fibers as it leaves a finished run.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/carrier_retire_mutations.py
Results: scripts/mutation/carrier_retire_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
P = "src/scheduler/process_abi.rs"
K = "src/scheduler/process.rs"
C = "src/scheduler/carrier.rs"


def m(name, file, old, new):
    # The whole library suite: the teardown paths are shared with `Drop`.
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter="")


MUTATIONS = [
    m("a leaving carrier skips its jobs", S,
      "        self.run_jobs_of(current);\n        self.tasks",
      "        self.tasks"),
    m("a leaving carrier retires no fiber", S,
      "            .filter(|record| record.carrier == current)\n            .filter_map(|record| record.fiber.take())",
      "            .filter(|_| false)\n            .filter_map(|record| record.fiber.take())"),
    m("a leaving carrier retires every carrier's fibers", S,
      "            .filter(|record| record.carrier == current)\n            .filter_map(|record| record.fiber.take())",
      "            .filter_map(|record| record.fiber.take())"),
    m("a parked run's carriers retire their fibers", S,
      "            Some(carrier::RunEnd::Parked) => return carrier::IdleStep::Exit(carrier::Retired::default()),",
      "            Some(carrier::RunEnd::Parked) => return carrier::IdleStep::Exit(self.retire_carrier()),"),
    m("a finished run's carriers keep their fibers", S,
      "            Some(carrier::RunEnd::Finished) => return carrier::IdleStep::Exit(self.retire_carrier()),",
      "            Some(carrier::RunEnd::Finished) => return carrier::IdleStep::Exit(carrier::Retired::default()),"),
    m("a parked root ends the run as finished", P,
      "                self.stop_carriers(super::carrier::RunEnd::Parked);",
      "                self.stop_carriers(super::carrier::RunEnd::Finished);"),
    m("a finished run forces before a pending deadline", P,
      "        if left && (self.timers.earliest().is_some() || self.force_survivors()) {",
      "        if left && self.force_survivors() {"),
    m("a finished run never forces what is left", P,
      "        if left && (self.timers.earliest().is_some() || self.force_survivors()) {",
      "        if left && self.timers.earliest().is_some() {"),
    m("a finished run ends with blocked plans left", P,
      "        let left = !self.kernel.incarnations.is_empty() || self.has_blocked_plans();",
      "        let left = !self.kernel.incarnations.is_empty();"),
    m("a finished run ends with incarnations left", P,
      "        let left = !self.kernel.incarnations.is_empty() || self.has_blocked_plans();",
      "        let left = self.has_blocked_plans();"),
    m("the run ends with work queued", P,
      "        if !self.ready.is_empty() || !self.jobs.is_empty() {\n            return RootStep::Continue;\n        }\n        // A process",
      "        if !self.jobs.is_empty() {\n            return RootStep::Continue;\n        }\n        // A process"),
    m("forcing reports no progress", K,
      "        (self.kernel.incarnations.len(), self.blocked_plan_count()) != before\n            || !self.ready.is_empty()\n            || !self.jobs.is_empty()\n            || self.timers.earliest().is_some()\n",
      "        let _ = before;\n        false\n"),
    m("forcing ignores work it queued", K,
      "            || !self.ready.is_empty()\n            || !self.jobs.is_empty()",
      "            || !self.jobs.is_empty()"),
    m("forcing ignores jobs it queued", K,
      "            || !self.ready.is_empty()\n            || !self.jobs.is_empty()",
      "            || !self.ready.is_empty()"),
    m("forcing ignores timers it armed", K,
      "            || self.timers.earliest().is_some()\n",
      "\n"),
    m("forcing always reports progress", K,
      "        (self.kernel.incarnations.len(), self.blocked_plan_count()) != before\n",
      "        true || (self.kernel.incarnations.len(), self.blocked_plan_count()) != before\n"),
    m("forcing skips the blocked plans", K,
      "            if self.kernel.blocked.contains_key(&blocker) && self.blocker_can_be_cleared(blocker) {",
      "            if false && self.blocker_can_be_cleared(blocker) {"),
    m("forcing skips the incarnations left", K,
      "        for incarnation in leftover {\n            self.end_leftover(incarnation);\n        }\n        (self.kernel",
      "        let _ = leftover;\n        (self.kernel"),
    m("a lingering draining body is forced", K,
      "                    !matches!(self.tasks[&record.task].phase, TaskPhase::Draining(_))",
      "                    true"),
    m("a leftover incarnation is never forced", K,
      "            if !self.await_incarnation(incarnation) {\n                self.force(incarnation);\n            }",
      "            let _ = self.await_incarnation(incarnation);"),
]
