"""Mutants of G22: drain before cancel, ordered shutdown, signals, and the exit
status (docs/specification/application-lifecycle.md).

Run: scripts/mutation/mutate.py scripts/mutation/g22_mutations.py
Results: scripts/mutation/g22_results.txt.
"""

CRATE = "compiler/chimi"

PROCESS = "../../runtime/chimi-fiber/src/scheduler/process.rs"
SCOPE = "../../runtime/chimi-fiber/src/scheduler/scope.rs"
SIGNAL = "../../runtime/chimi-fiber/src/scheduler/signal.rs"
PROCESS_ABI = "../../runtime/chimi-fiber/src/scheduler/process_abi.rs"
RUNTIME_C = "runtime/native/runtime.c"


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
         edits=[("            TaskPhase::Runnable if !task_record.ran || forced => {",
                 "            TaskPhase::Runnable => {")],
         **fiber("an_incarnation_running_when_shutdown_begins_is_not_cancelled_at_its_next_park")),
    dict(name="a body that never ran is left to run", file=PROCESS,
         edits=[("            TaskPhase::Runnable if !task_record.ran || forced => {",
                 "            TaskPhase::Runnable if forced => {")],
         **lab("RR02")),
    dict(name="a self-stop is not interrupted", file=PROCESS,
         edits=[("            .filter(|waiting| *waiting != task || !forced)",
                 "            .filter(|_| false)")],
         **lab("SD08")),
    dict(name="a draining body's receive is not woken", file=PROCESS,
         edits=[("            TaskPhase::Parked if receiving => {\n                self.disarm_wake_sources(task);\n                let _ = self.wake_front(task);\n            }\n",
                 "")],
         **lab("MB08")),
    dict(name="stop unroutes every member first", file=PROCESS,
         edits=[("            supervision.state = SupervisorState::Stopping;\n        }\n    }",
                 "            supervision.state = SupervisorState::Stopping;\n        }\n        for record in self.kernel.incarnations.values_mut() {\n            record.accepting = false;\n        }\n    }")],
         **lab("SD06")),
    dict(name="a drain silences timeout scopes", file=SCOPE,
         edits=[("        let forced = self.is_forced(task);\n        let record = self.tasks.get_mut(&task).expect(\"live task\");",
                 "        let forced = self.is_forced(task) || self.stopping(task).is_some();\n        let record = self.tasks.get_mut(&task).expect(\"live task\");")],
         **lab("SD12")),
    dict(name="a signal does not cancel the root", file=SIGNAL,
         edits=[("            self.begin_terminal(root, TaskOutcome::Cancelled, false);",
                 "            let _ = root;")],
         **lab("SG02")),
    dict(name="a signal timer outlives the root", file=PROCESS_ABI,
         edits=[("            self.retire_signal_timer();\n", "")],
         **lab("SG03")),
    dict(name="an idle service reports a deadlock", file=SIGNAL,
         edits=[("            && self.has_running_supervisor()\n",
                 "            && false\n")],
         **signals("an_idle_service_waits_for_a_signal_instead_of_reporting_a_deadlock")),
    dict(name="an injected signal exits 130", file=RUNTIME_C,
         edits=[("return 128 + received_signal;", "return 130;")],
         **lab("SG01")),
    dict(name="a real signal exits instead of re-raising", file=RUNTIME_C,
         edits=[("      if (received_signal != 0) end_by_signal(received_signal);\n", "")],
         **signals("sigterm_lets_the_worker_drain_its_jobs_then_ends_the_program_by_sigterm")),
    dict(name="a second signal waits for the drain", file=RUNTIME_C,
         edits=[("  if (received_signal != 0) end_by_signal(number);\n", "")],
         **signals("a_second_signal_ends_the_program_at_once")),
    dict(name="a blocking read restarts after a signal", file=RUNTIME_C,
         edits=[("  action.sa_flags = scheduled ? SA_RESTART : 0;", "  action.sa_flags = SA_RESTART;")],
         **signals("sigint_cancels_a_program_without_the_scheduler_while_it_reads_input")),
]
