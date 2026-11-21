"""Mutants of R4 step 3 PR 5c-4: the end of a run stops every carrier, the
host is served only once the other carriers are idle, and a carrier drains
its host releases before it waits.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/carrier_stop_mutations.py
Results: scripts/mutation/carrier_stop_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
P = "src/scheduler/process_abi.rs"
C = "src/scheduler/carrier.rs"
IDLE = "idle_tests"


def m(name, file, old, new, test=IDLE):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("a carrier parks after the run ended", S,
      "        if self.stopping {\n            return carrier::IdleStep::Exit;\n        }\n",
      ""),
    m("the stop sets nothing", S,
      "        self.stopping = true;\n        for carrier",
      "        for carrier"),
    m("the stop wakes no carrier", S,
      "        for carrier in 1..self.ready.carriers() {",
      "        for carrier in 1..1 {"),
    m("the stop skips carrier 1", S,
      "        for carrier in 1..self.ready.carriers() {",
      "        for carrier in 2..self.ready.carriers() {"),
    m("a new run keeps the old stop", S,
      "        self.stopping = false;\n        self.set_signal_root(root);",
      "        self.set_signal_root(root);"),
    m("a finished root leaves the carriers running", P,
      "            self.stop_carriers();\n            RootStep::Finished",
      "            RootStep::Finished"),
    m("a parked root leaves the carriers running", P,
      "                self.stop_carriers();\n                return RootStep::Parked;",
      "                return RootStep::Parked;"),
    m("the host is served while others run", S,
      "        if self.host_operations > 0 {\n            if !self.others_quiet() {\n                return self.wait_for_others();\n            }",
      "        if self.host_operations > 0 {"),
    m("an ended host operation wakes nobody", S,
      "            .expect(\"end_host_operation without a matching begin\");\n        self.wake_carrier(ready::Carrier(0));",
      "            .expect(\"end_host_operation without a matching begin\");"),
    m("no carrier drains before it waits", S,
      "        let drained = self.drain_host_releases();",
      "        let drained = false;"),
    m("carrier 0 waiting for others skips the drain", S,
      "    fn wait_for_others(&mut self) -> carrier::IdleStep {\n        if self.refill_before_waiting() {",
      "    fn wait_for_others(&mut self) -> carrier::IdleStep {\n        self.fire_due_timers();\n        if self.has_work(self.current) {"),
    m("a carrier about to park skips the drain", S,
      "            return carrier::IdleStep::Exit;\n        }\n        if self.refill_before_waiting() {",
      "            return carrier::IdleStep::Exit;\n        }\n        self.fire_due_timers();\n        if self.has_work(self.current) {"),
    m("a leaving carrier skips its last drive", C,
      "    loop {\n        drive(kernel);\n        match wait_idle(kernel) {\n            Waited::Again => {}\n            Waited::Exit => return,",
      "    loop {\n        match wait_idle(kernel) {\n            Waited::Again => {}\n            Waited::Exit => return,"),
    # The verifier's survivors.
    m("the run ends whenever the root is terminal", P,
      "        self.shutdown_processes();\n        if !self.has_blocked_plans()",
      "        self.stop_carriers();\n        self.shutdown_processes();\n        if !self.has_blocked_plans()"),
    m("the host is served once the others are parked, work queued or not", S,
      "        if self.host_operations > 0 {\n            if !self.others_quiet() {",
      "        if self.host_operations > 0 {\n            if !self.parked[1..].iter().all(|parked| *parked) {"),
    m("carrier 0 waiting on the host parks with its own due timer", S,
      "            if !self.others_quiet() {\n                return self.wait_for_others();\n            }\n            return carrier::IdleStep::Done(Idle::AwaitingHost);",
      "            if !self.others_quiet() {\n                return carrier::IdleStep::Park(self.idle_wake_fds());\n            }\n            return carrier::IdleStep::Done(Idle::AwaitingHost);"),
]
