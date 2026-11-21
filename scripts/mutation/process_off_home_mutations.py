"""Mutants of R4 step 3 PR 5b-4: process shutdown decided off the body's
home carrier.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/process_off_home_mutations.py
Each mutant must turn its filtered test red. The two that make a carrier spin
(a re-armed spent deadline, a teardown that cannot run a fiber) end in
TIMEOUT, which mutate.py counts as killed.
The review's zero-deadline case (a forced draining body must still arm its
deadline) is covered by compiler/chimi/tests/resources.rs
a_zero_deadline_forces_the_child_of_a_body_that_already_returned, outside this
crate; with the earlier guard (`&& !stopping.forced`) it fails with a
virtual-time deadlock.
Results: scripts/mutation/process_off_home_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
P = "src/scheduler/process.rs"
STOP = "a_supervisor_stopped_from_another_carrier_stops_its_members_at_home_in_reverse_order"
DEADLINE = "a_deadline_fired_on_another_carrier_forces_the_body_at_its_home"
TEARDOWN = "a_kernel_with_processes_on_another_carrier_tears_down"


def m(name, file, old, new, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("a body is run inline off its home", P,
      "            if !self.may_resume_here(task, self.tasks[&task].command) {\n                break;",
      "            if false {\n                break;", STOP),
    m("a forced body's spent deadline is re-armed", P,
      "        if stopping.timer.is_none() && !stopping.forced {",
      "        if stopping.timer.is_none() {", DEADLINE),
    m("a teardown keeps carriers apart", S,
      "        self.any_carrier = true;\n        self.run_queued_jobs();",
      "        self.run_queued_jobs();", TEARDOWN),
    m("any fiber may be resumed anywhere", S,
      "        self.any_carrier\n            || record.carrier == self.current",
      "        true\n            || record.carrier == self.current", STOP),
    m("a dropped kernel leaves its jobs queued", S,
      "        self.any_carrier = true;\n        self.run_queued_jobs();",
      "        self.any_carrier = true;", "a_kernel_dropped_with_a_queued_job_runs_it"),
    m("a dropped kernel runs only the current carrier's jobs", S,
      "        for carrier in 0..self.ready.carriers() {\n            while let Some(job) = self.jobs.pop_front(ready::Carrier(carrier)) {",
      "        for _carrier in 0..1 {\n            while let Some(job) = self.jobs.pop_front(self.current) {",
      "a_kernel_dropped_with_a_queued_job_runs_it"),
]
