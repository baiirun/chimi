"""Mutants of G21, the IO reactor (host-capabilities-and-execution-properties.md,
"Waiting for descriptors"). A mutant that makes a wait hang shows as TIMEOUT,
which counts as caught.

Run: scripts/mutation/mutate.py --timeout 240 scripts/mutation/g21_mutations.py
Results: scripts/mutation/g21_results.txt.
"""

CRATE = "compiler/chimi"

SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
REACTOR = "../../runtime/chimi-fiber/src/scheduler/reactor.rs"
SCOPE = "../../runtime/chimi-fiber/src/scheduler/scope.rs"
RUNTIME_C = "runtime/native/runtime.c"


def fiber(test):
    return dict(
        command=["cargo", "test", "--quiet", "-p", "chimi-fiber", "--lib", "--", test],
        expect=[test],
    )


def signals(test):
    return dict(targets=["signals"], filter=test)


MUTATIONS = [
    dict(name="virtual time jumps past a descriptor wait", file=SCHEDULER,
         edits=[("            if !self.reactor.is_empty() {\n                self.wait_for_io();",
                 "            if !self.reactor.is_empty() && self.clock.mode() == ClockMode::Real {\n                self.wait_for_io();")],
         **fiber("a_descriptor_wait_holds_virtual_time")),
    dict(name="a busy scheduler never polls its waits", file=SCHEDULER,
         edits=[("                self.poll_io_now();", "")],
         **fiber("a_ready_descriptor_wakes_its_waiter_even_when_other_tasks_never_idle")),
    dict(name="the real-clock idle wait ignores timers", file=REACTOR,
         edits=[("            super::ClockMode::Real => self.timers.earliest().map(|deadline| {",
                 "            super::ClockMode::Real => self.timers.earliest().filter(|_| false).map(|deadline| {")],
         **fiber("a_parked_wait_lets_other_tasks_run_and_wakes_when_the_descriptor_turns_ready")),
    dict(name="the idle wait leaves out the signal pipe", file=REACTOR,
         edits=[("        for task in self.reactor.wait(timeout, self.signals.wake_fd()) {",
                 "        for task in self.reactor.wait(timeout, None) {")],
         **fiber("the_first_signal_cancels_a_root_parked_in_a_descriptor_wait")),
    dict(name="a terminal task keeps its registration", file=SCHEDULER,
         edits=[("        self.clear_select(task);\n        self.clear_io_wait(task);\n        self.clear_scopes(task);",
                 "        self.clear_select(task);\n        self.clear_scopes(task);")],
         **fiber("the_first_signal_cancels_a_root_parked_in_a_descriptor_wait")),
    dict(name="a timeout scope cannot withdraw a descriptor wait", file=SCOPE,
         edits=[("            | super::REQUEST_SELECT\n            | super::REQUEST_IO_WAIT\n    )",
                 "            | super::REQUEST_SELECT\n    )")],
         **fiber("a_timeout_scope_withdraws_a_parked_wait")),
    dict(name="any interest bits are accepted", file=REACTOR,
         edits=[("            || interest & !INTEREST != 0\n", "")],
         **fiber("a_ready_descriptor_answers_at_once")),
    dict(name="a closed descriptor counts as ready", file=REACTOR,
         edits=[("                Check::Invalid\n            };", "                Check::Ready(interest)\n            };")],
         **fiber("a_ready_descriptor_answers_at_once")),
    dict(name="a scheduled read blocks the scheduler", file=RUNTIME_C,
         edits=[("    if (CHIMI_REQUIRES_SCHEDULER && !wait_readable(input->fd)) return INPUT_CANCELLED;\n", "")],
         **signals("a_task_keeps_running_while_main_waits_for_standard_input")),
    dict(name="a cancelled wait reads as a failure", file=RUNTIME_C,
         edits=[("    if (CHIMI_REQUIRES_SCHEDULER && !wait_readable(input->fd)) return INPUT_CANCELLED;",
                 "    if (CHIMI_REQUIRES_SCHEDULER && !wait_readable(input->fd)) return INPUT_FAILED;")],
         **signals("the_first_sigint_cancels_a_scheduled_program_while_it_reads_input")),
]
