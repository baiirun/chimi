"""Mutants of G22: drain before cancel, ordered shutdown, signals, and the exit
status (docs/specification/application-lifecycle.md).

Run: scripts/mutation/mutate.py scripts/mutation/g22_mutations.py
Results: scripts/mutation/g22_results.txt.
"""

CRATE = "compiler/chimi"

PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
PROCESS_ABI = "../../runtime/chimi-fiber/src/scheduler/process_abi.rs"
SIGNAL = "../../runtime/chimi-fiber/src/scheduler/signal.rs"
RUNTIME_C = "runtime/native/runtime.c"
STARTUP = "../../runtime/chimi-runtime/src/startup.rs"
SIGNALS = "../../runtime/chimi-runtime/src/intrinsics/signals.rs"


def lab(scenario):
    return dict(
        command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                 "--scenario", scenario],
        expect=[scenario],
    )


def fiber(test):
    return dict(
        command=["cargo", "test", "--quiet", "-p", "chimi-fiber", "--lib", "--", test],
        expect=[test],
    )


def signals(test):
    return dict(targets=["signals"], filter=test)


MUTATIONS = [
    dict(name="shutdown interrupts a sleeping handler", file=PROCESS,
         edits=[("            TaskPhase::Parked if forced => {",
                 "            TaskPhase::Parked if forced || !receiving => {")],
         **lab("RR05")),
    dict(name="shutdown cancels a running body", file=PROCESS,
         edits=[("            TaskPhase::Runnable if forced || (!task_record.ran && nothing_accepted) => {",
                 "            TaskPhase::Runnable => {")],
         **fiber("an_incarnation_running_when_shutdown_begins_is_not_cancelled_at_its_next_park")),
    dict(name="a body that never ran is left to run", file=PROCESS,
         edits=[("            TaskPhase::Runnable if forced || (!task_record.ran && nothing_accepted) => {",
                 "            TaskPhase::Runnable if forced => {")],
         **fiber("a_body_stopped_before_its_first_run_is_cancelled_without_running")),
    dict(name="a body that never ran drops the messages it accepted", file=PROCESS,
         edits=[("            TaskPhase::Runnable if forced || (!task_record.ran && nothing_accepted) => {",
                 "            TaskPhase::Runnable if forced || !task_record.ran => {")],
         **fiber("a_body_stopped_before_its_first_run_still_drains_the_messages_it_accepted")),
    dict(name="a self-stop is not interrupted", file=PROCESS,
         edits=[("            .filter(|waiting| *waiting != task || !forced)",
                 "            .filter(|_| false)")],
         **lab("SD08")),
    dict(name="a self-stop made during the drain waits for the deadline", file=PROCESS_ABI,
         edits=[("        if self.request_awaits_own_shutdown(owner, &input) {",
                 "        if false && self.request_awaits_own_shutdown(owner, &input) {")],
         **fiber("a_stop_of_its_own_root_made_during_the_drain_is_answered_with_cancellation")),
    dict(name="a draining body's receive is not woken", file=PROCESS,
         edits=[("            TaskPhase::Parked if receiving => {\n                self.disarm_wake_sources(task);\n                let _ = self.wake_front(task);\n            }\n",
                 "")],
         **lab("SD13")),
    dict(name="stop unroutes every member first", file=PROCESS,
         edits=[("            supervision.state = SupervisorState::Stopping;\n        }\n    }",
                 "            supervision.state = SupervisorState::Stopping;\n        }\n        for record in self.kernel.incarnations.values_mut() {\n            record.accepting = false;\n        }\n    }")],
         **lab("SD06")),
    dict(name="a drain silences timeout scopes", file=PROCESS,
         edits=[("self.stopping(task).is_some_and(|stopping| stopping.forced)",
                 "self.stopping(task).is_some()")],
         **lab("SD12")),
    dict(name="a signal does not cancel the root", file=SIGNAL,
         edits=[("        self.begin_terminal(root, TaskOutcome::Cancelled, false);",
                 "        let _ = root;")],
         **lab("SG02")),
    dict(name="a signal timer outlives the root", file=SIGNAL,
         edits=[("    pub(super) fn deliver_signal(&mut self) {\n        if self.retire_signals_after_root() {\n            return;\n        }\n",
                 "    pub(super) fn deliver_signal(&mut self) {\n")],
         **lab("SG03")),
    dict(name="an idle service reports a deadlock", file=SIGNAL,
         edits=[("            && self.has_running_supervisor()\n",
                 "            && false\n")],
         **signals("an_idle_service_waits_for_a_signal_instead_of_reporting_a_deadlock")),
    dict(name="an injected signal exits 130", file=STARTUP,
         edits=[("                    return 128 + received;", "                    return 130;")],
         **lab("SG01")),
    dict(name="a real signal exits instead of re-raising", file=STARTUP,
         edits=[("                if received != 0 {\n                    chimi_runtime_end_by_signal_v1(received);\n                }\n", "")],
         **signals("sigterm_lets_the_worker_drain_its_jobs_then_ends_the_program_by_sigterm")),
    dict(name="a second signal waits for the drain", file=SIGNALS,
         edits=[("        chimi_runtime_end_by_signal_v1(number);\n", "")],
         **signals("a_second_signal_ends_the_program_at_once")),
    dict(name="a blocking read restarts after a signal", file=SIGNALS,
         edits=[("if scheduled { libc::SA_RESTART } else { 0 }", "libc::SA_RESTART")],
         **signals("sigint_cancels_a_program_without_the_scheduler_while_it_reads_input")),
    dict(name="a stop signal without the scheduler does not cancel", file=SIGNALS,
         edits=[("        CANCEL_RUNTIME.store(runtime, Ordering::Relaxed);\n", "        let _ = runtime;\n")],
         **signals("sigint_cancels_a_program_without_the_scheduler_while_it_reads_input")),
]
