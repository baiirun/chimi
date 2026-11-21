"""Mutants of R4 step 3 PR 5b-5: a wake pipe per carrier, marked under the
kernel lock and written once it is released.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/wake_pipe_mutations.py
Each mutant must turn its filtered test red. The verifier's own mutants,
run against the whole lib suite, found five gaps the first list missed (the
carrier loop's sleep, the stalled root's wait, a second readable pipe in a
sleep and in a descriptor wait, and the write end of a pipe); they are the
last five here.
Results: scripts/mutation/wake_pipe_results.txt.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
C = "src/scheduler/carrier.rs"
W = "src/scheduler/wake.rs"
R = "src/scheduler/reactor.rs"
K = "src/scheduler/child.rs"
G = "src/scheduler/signal.rs"
QUEUED = "a_task_queued_for_another_carrier_wakes_it_once_the_lock_is_released"
JOB = "a_job_for_another_carrier_wakes_it"
WAITS = "a_wait_registered_off_carrier_zero_wakes_carrier_zero"
IDLE = "an_idle_wait_ends_at_its_carriers_wake"
ROOT = "a_stalled_root_waits_on_its_carriers_wake_too"
A = "src/scheduler/process_abi.rs"


def m(name, file, old, new, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=["lib"], filter=test)


MUTATIONS = [
    m("a queued task wakes no one", S,
      "            self.ready.push_back(carrier, task);\n        }\n        self.wake_carrier(carrier);",
      "            self.ready.push_back(carrier, task);\n        }", QUEUED),
    m("a queued job wakes no one", S,
      "            self.jobs.push_back(carrier, jobs::Job::Unwind { task, forced });\n            self.wake_carrier(carrier);",
      "            self.jobs.push_back(carrier, jobs::Job::Unwind { task, forced });", JOB),
    m("the current carrier wakes itself", S,
      "        if carrier != self.current {\n            self.wakes.mark(carrier);",
      "        if true {\n            self.wakes.mark(carrier);", QUEUED),
    m("the lock writes before it unlocks", C,
      "            let result = operation(&mut kernel);\n            (result, kernel.take_wakes())\n        };\n        wakes.write();",
      "            let result = operation(&mut kernel);\n            kernel.take_wakes().write();\n            (result, kernel.take_wakes())\n        };\n        wakes.write();", QUEUED),
    m("the lock never writes", C,
      "        };\n        wakes.write();\n        result",
      "        };\n        drop(wakes);\n        result", QUEUED),
    m("a take keeps its marks", W,
      "            if std::mem::take(marked) {",
      "            if *marked {", "a_mark_wakes_only_its_carrier_once"),
    m("any later timer wakes carrier 0", S,
      "            .is_none_or(|earliest| deadline < earliest)",
      "            .is_none_or(|_earliest| true)", WAITS),
    m("a new earliest timer wakes no one", S,
      "            .is_none_or(|earliest| deadline < earliest)",
      "            .is_none_or(|_earliest| false)", WAITS),
    m("a descriptor wait wakes no one", R,
      "                self.wake_carrier(super::ready::Carrier(0));\n                self.tasks.get_mut(&owner).expect(\"waiting task\").io_wait = Some(id);",
      "                self.tasks.get_mut(&owner).expect(\"waiting task\").io_wait = Some(id);", WAITS),
    m("a child wait wakes no one", K,
      "                self.wake_carrier(super::ready::Carrier(0));\n                self.children.waiters.push(ChildWaiter {",
      "                self.children.waiters.push(ChildWaiter {", WAITS),
    m("a signal wait wakes no one", G,
      "            self.wake_carrier(super::ready::Carrier(0));\n            self.signals.waiters.push(SignalWaiter {",
      "            self.signals.waiters.push(SignalWaiter {", WAITS),
    m("an idle wait ignores the carrier's pipe", S,
      "        if let Some(fd) = self.wakes.read_fd(self.current) {\n            fds.push(fd);",
      "        if let Some(fd) = None {\n            fds.push(fd);", IDLE),
    m("an idle wait watches carrier 0's pipe", S,
      "        if let Some(fd) = self.wakes.read_fd(self.current) {",
      "        if let Some(fd) = self.wakes.read_fd(ready::Carrier(0)) {", IDLE),
    m("every carrier watches the signal pipe", S,
      "self.signals.wake_fd().filter(|_| self.current == ready::Carrier(0))",
      "self.signals.wake_fd()", IDLE),
    m("a woken wait leaves its pipe full", R,
      "            crate::scheduler::signal::drain(poll.fd);",
      "            let _ = poll.fd;", IDLE),
    m("a woken sleep leaves its pipe full", G,
      "        for poll in polls.iter().filter(|poll| poll.revents != 0) {\n            drain(poll.fd);",
      "        for poll in polls.iter().filter(|poll| poll.revents != 0) {\n            let _ = poll.fd;", IDLE),
    m("a sleep drains only the first pipe", G,
      "        for poll in polls.iter().filter(|poll| poll.revents != 0) {",
      "        for poll in polls.iter().take(1).filter(|poll| poll.revents != 0) {", IDLE),
    m("a task may wait on a wake pipe", R,
      "            || self.wakes.owns(fd)\n",
      "", IDLE),
    m("one carrier gets pipes", W,
      "        let pipes = if carriers > 1 {",
      "        let pipes = if carriers > 0 {", "one_carrier_has_no_wake_pipe"),
    m("the carrier loop's sleep ignores its wake pipes", "src/scheduler/carrier.rs",
      "            clock::block_real(wait, wake_fds.as_slice());",
      "            clock::block_real(wait, &wake_fds.as_slice()[..0]);", IDLE),
    m("a stalled root watches only the signal pipe", A,
      "                return RootStep::WaitForSignal(self.idle_wake_fds());",
      "                return RootStep::WaitForSignal({\n                    let mut fds = super::wake::WakeFds::default();\n                    if let Some(fd) = self.signals.wake_fd() {\n                        fds.push(fd);\n                    }\n                    fds\n                });", ROOT),
    m("a sleep empties only one readable pipe", G,
      "        for poll in polls.iter().filter(|poll| poll.revents != 0) {",
      "        for poll in polls.iter().filter(|poll| poll.revents != 0).take(1) {", IDLE),
    m("a descriptor wait empties only its last wake pipe", R,
      "        for poll in polls[watched.len()..].iter().filter(|poll| poll.revents != 0) {",
      "        for poll in polls[watched.len()..].iter().rev().take(1).filter(|poll| poll.revents != 0) {", IDLE),
    m("a wake pipe's write end is a task's to wait on", W,
      "            .any(|pipe| pipe.read == fd || pipe.write == fd)",
      "            .any(|pipe| pipe.read == fd)", "a_mark_wakes_only_its_carrier_once"),
]
