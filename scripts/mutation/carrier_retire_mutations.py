"""Mutants of R4 step 3 PR 5c-5: a finished run forces the processes left
before it ends, and each carrier but 0 checks, as it leaves a finished
run, that it holds no job and no fiber.

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
    m("a leaving carrier checks no job", S,
      "        assert!(self.jobs.is_empty_for(current), \"a finished run left a job on carrier {}\", current.0);\n",
      ""),
    m("a leaving carrier checks no fiber", S,
      "            .find(|(_, record)| record.carrier == current && record.fiber.is_some());",
      "            .find(|_| false);"),
    m("a leaving carrier checks every carrier's fibers", S,
      "            .find(|(_, record)| record.carrier == current && record.fiber.is_some());",
      "            .find(|(_, record)| record.fiber.is_some());"),
    m("a parked run's carriers check for fibers", S,
      "            if end == carrier::RunEnd::Finished {\n                self.retire_carrier();\n            }",
      "            let _ = end;\n            self.retire_carrier();"),
    m("a finished run's carriers check nothing", S,
      "            if end == carrier::RunEnd::Finished {\n                self.retire_carrier();\n            }",
      "            let _ = end;"),
    m("a parked root ends the run as finished", P,
      "                self.stop_carriers(super::carrier::RunEnd::Parked);",
      "                self.stop_carriers(super::carrier::RunEnd::Finished);"),
    m("a finished run forces before a pending deadline", P,
      "        if left && (self.timers.earliest().is_some() || self.force_survivors()) {",
      "        if left && self.force_survivors() {"),
    m("a finished run never forces what is left", P,
      "        if left && (self.timers.earliest().is_some() || self.force_survivors()) {",
      "        if left && self.timers.earliest().is_some() {"),
    m("the run ends with work queued", P,
      "        if !self.ready.is_empty() || !self.jobs.is_empty() {\n            return RootStep::Continue;\n        }\n        // A process",
      "        if !self.jobs.is_empty() {\n            return RootStep::Continue;\n        }\n        // A process"),
    m("forcing reports no progress", K,
      "        self.survivor_state() != before\n    }",
      "        let _ = before;\n        false\n    }"),
    m("forcing always reports progress", K,
      "        self.survivor_state() != before\n    }",
      "        let _ = before;\n        true\n    }"),
    m("forcing ignores work it queued", K,
      "            !self.ready.is_empty() || !self.jobs.is_empty() || self.timers.earliest().is_some(),",
      "            !self.jobs.is_empty() || self.timers.earliest().is_some(),"),
    m("forcing ignores incarnations ended", K,
      "        (\n            self.kernel.incarnations.len(),\n            self.blocked_plan_count(),",
      "        (\n            0,\n            self.blocked_plan_count(),"),
    m("forcing skips the blocked plans", K,
      "            if self.kernel.blocked.contains_key(&blocker) && self.blocker_can_be_cleared(blocker) {",
      "            if false && self.blocker_can_be_cleared(blocker) {"),
    m("forcing goes on to the leftovers after clearing a blocker", K,
      "        if self.survivor_state() != before {\n            return true;\n        }",
      ""),
    m("forcing skips the incarnations left", K,
      "        let leftover: Vec<_> = self.kernel.incarnations.keys().copied().collect();\n        for incarnation in leftover {\n            if !self.kernel",
      "        let leftover: Vec<IncarnationId> = Vec::new();\n        for incarnation in leftover {\n            if !self.kernel"),
    m("a leftover incarnation is never forced", K,
      "                self.force(incarnation);\n                // `force` leaves",
      "                // `force` leaves"),
    m("the force's stale deadline stays queued", K,
      "                if let Some(timer) = armed {\n                    self.cancel_timer(timer);\n                }",
      "                let _ = armed;"),
    m("a lingering draining body is forced", K,
      "                    !matches!(self.tasks[&record.task].phase, TaskPhase::Draining(_))",
      "                    true"),
    m("a body is forced off its home", K,
      "    pub(super) fn force_survivors(&mut self) -> bool {\n",
      "    pub(super) fn force_survivors(&mut self) -> bool {\n        self.any_carrier = true;\n"),
]
