"""Mutants of R4 step 3 PR 5b-6: global idle. A carrier other than 0 with
nothing to do parks on its wake pipe; carrier 0 moves virtual time or gives a
verdict only when every other carrier is parked and no carrier has a ready
task or a job.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/global_idle_mutations.py
Each mutant must turn its filtered test red; a mutant that loses a wake
makes the round-robin driver fail rather than hang. The verifier's own
mutants against the whole suite found five gaps (the last parker's wake
with a timer pending, carrier 0's own due timer while it waits, and the
order of the host and descriptor checks); they are the last five here.
Results: scripts/mutation/global_idle_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
TIME = "time_moves_only_once_every_other_carrier_has_parked"
DEADLOCK = "a_deadlock_is_reported_only_once_every_carrier_is_idle"
WORK = "carrier_zero_waits_while_another_carrier_has_work"
RUNNING = "carrier_zero_waits_for_a_carrier_running_a_fiber"
LAST = "the_last_carrier_to_park_wakes_carrier_zero"
DUE = "an_idle_carrier_fires_due_timers_and_leaves_descriptors_to_carrier_zero"
REAL = "carrier_zero_waiting_for_others_sleeps_until_the_earliest_timer"
LAST_TIMER = "the_last_carrier_to_park_wakes_carrier_zero_with_a_timer_pending"
OWN_TIMER = "carrier_zero_waiting_for_others_runs_its_own_due_timer"
ORDER = "carrier_zero_serves_the_host_and_descriptors_while_others_run"


def m(name, file, old, new, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("carrier 0 ignores the other carriers", S,
      "        if !self.others_quiet() {\n            return self.wait_for_others();",
      "        if false {\n            return self.wait_for_others();", DEADLOCK),
    m("parked carriers are not counted", S,
      "        self.parked[1..].iter().all(|parked| *parked)\n",
      "        true\n", RUNNING),
    m("ready queues are not counted", S,
      "            && self.ready.is_empty()\n            && self.jobs.is_empty()",
      "            && self.jobs.is_empty()", WORK),
    m("jobs are not counted", S,
      "            && self.ready.is_empty()\n            && self.jobs.is_empty()",
      "            && self.ready.is_empty()", WORK),
    m("the last carrier to park wakes no one", S,
      "        if self.others_quiet() {\n            self.wake_carrier(ready::Carrier(0));\n        }",
      "", LAST),
    m("every carrier that parks wakes carrier 0", S,
      "        if self.others_quiet() {\n            self.wake_carrier(ready::Carrier(0));\n        }",
      "        self.wake_carrier(ready::Carrier(0));", LAST),
    m("a parking carrier does not say so", S,
      "        self.parked[self.current.0 as usize] = true;\n",
      "", TIME),
    m("a carrier taking the kernel stays parked", S,
      "        self.current = carrier;\n        self.parked[carrier.0 as usize] = false;",
      "        self.current = carrier;", RUNNING),
    m("an idle carrier does not fire due timers", S,
      "    fn park_step(&mut self) -> carrier::IdleStep {\n        self.fire_due_timers();",
      "    fn park_step(&mut self) -> carrier::IdleStep {", DUE),
    m("an idle carrier parks with work", S,
      "        self.fire_due_timers();\n        if self.has_work(self.current) {\n            return carrier::IdleStep::Continue;\n        }\n        self.parked",
      "        self.fire_due_timers();\n        self.parked", DUE),
    m("any carrier waits on descriptors", S,
      "        if self.current != ready::Carrier(0) {\n            return self.park_step();\n        }",
      "        if self.current != ready::Carrier(0)\n            && self.reactor.is_empty()\n            && self.children.is_empty()\n        {\n            return self.park_step();\n        }", DUE),
    m("a parked carrier sleeps past a timer", S,
      "        match self.timers.earliest() {\n            Some(deadline) if self.clock.mode() == ClockMode::Real => carrier::IdleStep::Sleep(\n                clock::real_wait(self.clock.now(), deadline),\n                self.idle_wake_fds(),\n            ),\n            _ => carrier::IdleStep::Park(self.idle_wake_fds()),\n        }\n    }\n\n    /// Carrier 0's",
      "        carrier::IdleStep::Park(self.idle_wake_fds())\n    }\n\n    /// Carrier 0's", REAL),
    m("carrier 0 waiting for others parks past a timer", S,
      "            return carrier::IdleStep::Continue;\n        }\n        match self.timers.earliest() {\n            Some(deadline) if self.clock.mode() == ClockMode::Real => carrier::IdleStep::Sleep(",
      "            return carrier::IdleStep::Continue;\n        }\n        match self.timers.earliest() {\n            Some(deadline) if false => carrier::IdleStep::Sleep(", REAL),
    m("has_work looks only at jobs", S,
      "        !self.ready.is_empty_for(carrier) || !self.jobs.is_empty_for(carrier)",
      "        !self.jobs.is_empty_for(carrier)", DUE),
    m("the last carrier to park wakes carrier 0 only with no timer", S,
      "        if self.others_quiet() {\n            self.wake_carrier(ready::Carrier(0));",
      "        if self.others_quiet() && self.timers.earliest().is_none() {\n            self.wake_carrier(ready::Carrier(0));", LAST_TIMER),
    m("carrier 0 waiting for others parks with its own task ready", S,
      "    fn wait_for_others(&mut self) -> carrier::IdleStep {\n        self.fire_due_timers();\n        if self.has_work(self.current) {\n            return carrier::IdleStep::Continue;\n        }",
      "    fn wait_for_others(&mut self) -> carrier::IdleStep {\n        self.fire_due_timers();", OWN_TIMER),
    m("carrier 0 waiting for others holds its due timers", S,
      "    fn wait_for_others(&mut self) -> carrier::IdleStep {\n        self.fire_due_timers();",
      "    fn wait_for_others(&mut self) -> carrier::IdleStep {", OWN_TIMER),
    m("carrier 0 waits for others before descriptors", S,
      "        if !self.reactor.is_empty() || !self.children.is_empty() {\n            return carrier::IdleStep::Io(self.prepare_io_wait());\n        }\n        if !self.others_quiet() {\n            return self.wait_for_others();\n        }",
      "        if !self.others_quiet() {\n            return self.wait_for_others();\n        }\n        if !self.reactor.is_empty() || !self.children.is_empty() {\n            return carrier::IdleStep::Io(self.prepare_io_wait());\n        }", ORDER),
    m("carrier 0 waits for others before the host", S,
      "            return self.park_step();\n        }\n",
      "            return self.park_step();\n        }\n        if !self.others_quiet() {\n            return self.wait_for_others();\n        }\n", ORDER),
]
