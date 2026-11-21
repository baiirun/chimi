"""Mutants of R4 step 3 PR 5b-3: a cancel decided on another carrier unwinds
the fiber on its home carrier.

Run: scripts/mutation/mutate.py scripts/mutation/unwind_jobs_mutations.py
Each mutant must turn its filtered test red.
Results: scripts/mutation/unwind_jobs_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"


def m(name, old, new, test, file=S):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("an unwind always runs inline",
      "        if record.carrier != self.current && record.ran {",
      "        if false {",
      "a_cancel_from_another_carrier_unwinds_on_the_home_carrier"),
    m("an unstarted task also waits for a job",
      "        if record.carrier != self.current && record.ran {",
      "        if record.carrier != self.current {",
      "a_cancel_from_another_carrier_ends_an_unstarted_task_at_once"),
    m("a draining task with its fiber can be published",
      "        if record.fiber.is_some() || !record.children.is_empty() || self.owns_live_roots(task) {",
      "        if !record.children.is_empty() || self.owns_live_roots(task) {",
      "a_task_waiting_for_its_unwind_outlives_its_last_child"),
    m("a job forgets that its end was forced",
      "            let forced = self.forcing > 0;\n            let carrier = record.carrier;",
      "            let forced = false;\n            let carrier = record.carrier;",
      "a_forced_end_from_another_carrier_queues_a_forced_job"),
    m("a running task is drained at once",
      "        if !fiber_is_terminal && phase == Some(TaskPhase::Running) {",
      "        if false {",
      "a_cancel_while_the_fiber_runs_elsewhere_applies_when_it_parks"),
    m("a pending end is never applied",
      "        let end = self.tasks.get_mut(&task).and_then(|record| record.pending_end.take());",
      "        let end = self.tasks.get_mut(&task).and_then(|_record| None::<jobs::PendingEnd>);",
      "a_cancel_while_the_fiber_runs_elsewhere_applies_when_it_parks"),
    m("a pending end drops the request its step parked on",
      "        self.apply_step(task, event);\n        // An end decided",
      "        if end.is_none() {\n            self.apply_step(task, event);\n        } else {\n"
      "            let record = self.tasks.get_mut(&task).expect(\"running task\");\n"
      "            record.phase = TaskPhase::Parked;\n            record.park += 1;\n        }\n        // An end decided",
      "a_pending_end_serves_the_request_its_step_parked_on"),
    m("a carrier runs no jobs",
      "            while let Some(job) = self.jobs.pop_front(self.current) {",
      "            while let Some(job) = None::<jobs::Job> {",
      "a_cancel_from_another_carrier_unwinds_on_the_home_carrier"),
    m("a job's unwind does not end the task",
      "                self.end_after_unwind(runnable.task, outcome, &event);\n                self.forcing -= u32::from(forced);",
      "                let _ = (outcome, &event);\n                self.forcing -= u32::from(forced);",
      "a_cancel_from_another_carrier_unwinds_on_the_home_carrier"),
]
