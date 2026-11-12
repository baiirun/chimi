"""Mutants of supervised-process stage U6 (ts-6ccbc9): restart accounting and
root escalation, as runtime.c marshals them into the scheduler and the
scheduler applies them, and the lab's U6 runners (RA08's event counts and
OB04's determinism check). Each mutant breaks one decision a U6 scenario guards
and must turn that scenario red with the failure it names.

Run: scripts/mutation/mutate.py scripts/mutation/process_u6_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

Every mutant runs one scenario through the lab binary with `--through U6` and
is scored on that scenario's own failure text; tests/process_lab.rs runs the
same scenarios in the default suite.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
FIXTURE = "src/process_lab/fixture.rs"
KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"

TRANSCRIPT = "expected.transcript line"
OUTCOME = "expected.outcome line"
EXITED = "exited with"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U6"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


MEMBER_RESTART = '    array[index].restart = marshal_restart(process_field(member, "restart"));'
LIMIT = "    .restart_limit = (uint32_t)limit->as.integer,"
WINDOW = "    .restart_within = window->as.integer,"
DYNAMIC_RESTART = "        .restart = marshal_restart(arguments[2]),"
WINDOW_PRUNE = "            .is_some_and(|at| at.saturating_add(restarts.within) <= now)"
RESTART_RULE = "        if cause == TerminalCause::Defected && entry.member.restart == Restart::OnDefect && open {"
CHARGE = """        supervision.restart_times.push_back(now);
        self.emit_process(ProcessEventKind::RestartAttempted {
            supervisor,
            membership,
            attempts_in_window: attempts,
        });
        if let Err(error) = self.start_incarnation(membership) {"""
CHARGE_AFTER_START = """        supervision.restart_times.push_back(now);
        let started = self.start_incarnation(membership);
        self.emit_process(ProcessEventKind::RestartAttempted {
            supervisor,
            membership,
            attempts_in_window: attempts,
        });
        if let Err(error) = started {"""
STOP_ORDER = "                    for membership in declared.into_iter().chain(dynamic) {"
ESCALATION_OUTCOME = """  if (outcome.state == CHIMI_FIBER_DEFECTED &&
      outcome.defect_code == CHIMI_SCHEDULER_DEFECT_RESTART_LIMIT_EXCEEDED)
    return defect("Supervisor.RestartLimitExceeded", -1);
"""
CLOCK_MODE = ".clock_mode = runtime->virtual_clock ? CHIMI_SCHEDULER_CLOCK_VIRTUAL : CHIMI_SCHEDULER_CLOCK_REAL,"

MUTATIONS = [
    # Restart policy marshalling (runtime.c -> ChimiSupervisorPolicy).
    lab("declared members always marshal as Restart.Never", RUNTIME, "RA01",
        [TRANSCRIPT], [(MEMBER_RESTART, "    array[index].restart = CHIMI_RESTART_NEVER;")]),
    lab("declared members always marshal as OnDefect", RUNTIME, "SV04",
        [TRANSCRIPT], [(MEMBER_RESTART, "    array[index].restart = CHIMI_RESTART_ON_DEFECT;")]),
    lab("dynamic members always marshal as Restart.Never", RUNTIME, "RT01",
        [TRANSCRIPT], [(DYNAMIC_RESTART, "        .restart = CHIMI_RESTART_NEVER,")]),
    lab("the restart limit marshals as 0", RUNTIME, "RA01",
        [TRANSCRIPT, EXITED], [(LIMIT, "    .restart_limit = 0,")]),
    lab("the restart window marshals as 1 ns", RUNTIME, "RA02",
        [TRANSCRIPT, EXITED], [(WINDOW, "    .restart_within = 1,")]),
    # Restart accounting in the scheduler.
    lab("a restart exactly `within` later is inside the window", KERNEL, "RA04",
        [TRANSCRIPT, EXITED], [(
            WINDOW_PRUNE,
            "            .is_some_and(|at| at.saturating_add(restarts.within) < now)")]),
    lab("the window never rolls", KERNEL, "RA03",
        [TRANSCRIPT, EXITED], [(WINDOW_PRUNE, "            .is_some_and(|_| false)")]),
    lab("returned and failed members restart", KERNEL, "SV03",
        [TRANSCRIPT, OUTCOME], [(
            RESTART_RULE,
            "        if cause != TerminalCause::Cancelled && entry.member.restart == Restart::OnDefect && open {")]),
    lab("the restart is charged after the replacement starts", KERNEL, "RA01",
        [TRANSCRIPT], [(CHARGE, CHARGE_AFTER_START)]),
    lab("a name outlives its incarnation", KERNEL, "RT02",
        [TRANSCRIPT, OUTCOME, EXITED], [(
            "            self.kernel.names.remove(&name);\n        }\n        if let Some(timer) = record.stopping",
            "        }\n        if let Some(timer) = record.stopping")]),
    # Escalation.
    lab("escalation stops declared members before dynamic ones", KERNEL, "ES05",
        [TRANSCRIPT], [(STOP_ORDER, "                    for membership in dynamic.into_iter().chain(declared) {")]),
    lab("a root escalation ends the program as a scheduler failure", RUNTIME, "RA02",
        [OUTCOME], [(ESCALATION_OUTCOME, "")]),
    # Range checks at the call that receives the value.
    lab("Process.spec accepts a computed zero capacity", RUNTIME, "MB10",
        [OUTCOME, EXITED], [(
            "      if (arguments[1]->as.integer < 1)\n        return process_range_defect(\"Process.spec: mailbox capacity",
            "      if (arguments[1]->as.integer < 0)\n        return process_range_defect(\"Process.spec: mailbox capacity")]),
    lab("Supervisor.policy accepts members over capacity", RUNTIME, "SV11",
        [OUTCOME, EXITED], [(
            "      if ((int64_t)list_length(arguments[0]) > capacity)",
            "      if ((int64_t)list_length(arguments[0]) > capacity + 1)")]),
    # Each incarnation runs on its own copy of the body and its providers.
    lab("the first incarnation takes the spec's body instead of a copy", RUNTIME, "CB08",
        [OUTCOME, EXITED], [(
            "  fragment_copy_into_current(context->body, roots);",
            "  fragment_adopt(context->body, roots);")]),
    # Virtual time and determinism.
    lab("the runner ignores --clock=virtual", RUNTIME, "OB06",
        ["exceeded the 60 s run limit"], [(CLOCK_MODE, ".clock_mode = CHIMI_SCHEDULER_CLOCK_REAL,")]),
    lab("telemetry carries the process id", RUNTIME, "OB04",
        ["telemetry of the second --trace=full run"], [(
            "          event->waiter_depth, event->discard_count);",
            "          event->waiter_depth, event->discard_count + (uint64_t)getpid());")]),
    lab("event classes ignore their class fields", FIXTURE, "RA08",
        ["expected.events line"], [(
            'const CLASS_FIELDS: [&str; 4] = ["cause", "reason", "outcome", "attempts_in_window"];',
            'const CLASS_FIELDS: [&str; 4] = ["", "", "", ""];')]),
]
