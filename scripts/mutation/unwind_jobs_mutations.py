"""Mutants of R4 step 3 PR 5b-3: a cancel decided on another carrier unwinds
the fiber on its home carrier.

Run: scripts/mutation/mutate.py scripts/mutation/unwind_jobs_mutations.py
Each mutant must turn its filtered test red.
Results: scripts/mutation/unwind_jobs_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
JOBS = (
    "            while let Some(job) = self.jobs.pop_front(self.current) {\n"
    "                if let Some(runnable) = self.prepare_job(job) {\n"
    "                    return Some(runnable);\n                }\n            }\n")


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
    m("a job's unwind is not terminal",
      "                let fiber = record.fiber.as_mut()?.enter(scope::ResumeContext::TERMINAL);",
      "                let fiber = record.fiber.as_mut()?.enter(scope::ResumeContext::default());",
      "a_jobs_unwind_is_terminal_and_its_result_decides_the_outcome"),
    m("a job's unwind result is ignored",
      "                self.end_after_unwind(runnable.task, outcome, &event);\n",
      "                let _ = (outcome, &event);\n                self.end_drained(runnable.task, false);\n",
      "a_jobs_unwind_is_terminal_and_its_result_decides_the_outcome"),
    m("a forced job is not forced",
      "                self.forcing += u32::from(forced);\n                self.end_after_unwind",
      "                self.forcing += 0;\n                self.end_after_unwind",
      "a_forced_job_forces_the_tasks_children"),
    dict(name="ready tasks run before jobs", file=S, edits=[
        (JOBS, ""),
        ("            if !self.drain_host_releases() {\n", JOBS + "            if !self.drain_host_releases() {\n")],
        targets=["lib"], filter="a_carrier_runs_its_jobs_before_its_ready_tasks"),
    m("a later cancel replaces a pending defect",
      "            Some(recorded) if recorded.outcome.is_defect() || !outcome.is_defect() => jobs::PendingEnd {",
      "            Some(recorded) if false => jobs::PendingEnd {",
      "pending_ends_merge_with_a_defect_and_a_force_winning"),
    m("a later defect does not replace a pending cancel",
      "            Some(recorded) if recorded.outcome.is_defect() || !outcome.is_defect() => jobs::PendingEnd {",
      "            Some(recorded) if true => jobs::PendingEnd {",
      "pending_ends_merge_with_a_defect_and_a_force_winning"),
    m("a later force is forgotten",
      "                outcome: recorded.outcome,\n                forced: recorded.forced || forced,",
      "                outcome: recorded.outcome,\n                forced: recorded.forced,",
      "pending_ends_merge_with_a_defect_and_a_force_winning"),
    m("a pending forced end applies as a cancel",
      "            if end.forced {\n                self.force_terminal(task, end.outcome, false);",
      "            if false {\n                self.force_terminal(task, end.outcome, false);",
      "a_pending_forced_end_does_not_wait_for_releases"),
]
