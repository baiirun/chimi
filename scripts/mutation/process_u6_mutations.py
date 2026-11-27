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
PROCESSES = "../../runtime/chimi-runtime/src/intrinsics/processes.rs"
FIXTURE = "src/process_lab/fixture.rs"
KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"
# The supervisor starts marshal their specs and policies in Rust since
# CHI-305 step 5, phase 7.
STARTS = "../../runtime/chimi-runtime/src/intrinsics/process_starts.rs"
# The scheduler trace writers are Rust since CHI-305 step 5, phase 7.
TELEMETRY = "../../runtime/chimi-runtime/src/telemetry.rs"
OB04_COUNT = "                (b\"discard_count\", event.discard_count),"
OB04_PID = "                (b\"discard_count\", event.discard_count + libc::getpid() as u64),"

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


MEMBER_RESTART = '                let restart = restart_of(field(member, c"restart"));'
LIMIT = "                restart_limit: limit as u32,"
WINDOW = "                restart_within: (*window).payload.integer,"
DYNAMIC_RESTART = "        request.restart = restart_of(arguments[2]);"
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
    lab("declared members always marshal as Restart.Never", STARTS, "RA01",
        [TRANSCRIPT, EXITED], [(MEMBER_RESTART, "                let restart = Scheduler::RESTART_NEVER;")]),
    lab("declared members always marshal as OnDefect", STARTS, "SV04",
        [TRANSCRIPT], [(MEMBER_RESTART, "                let restart = Scheduler::RESTART_ON_DEFECT;")]),
    lab("dynamic members always marshal as Restart.Never", STARTS, "RT01",
        [TRANSCRIPT, EXITED], [(DYNAMIC_RESTART, "        request.restart = Scheduler::RESTART_NEVER;")]),
    lab("the restart limit marshals as 0", STARTS, "RA01",
        [TRANSCRIPT, EXITED], [(LIMIT, "                restart_limit: 0,")]),
    lab("the restart window marshals as 1 ns", STARTS, "RA02",
        [TRANSCRIPT, EXITED], [(WINDOW, "                restart_within: 1,")]),
    # Restart accounting in the scheduler.
    lab("a restart exactly `within` later is inside the window", KERNEL, "RA04",
        [TRANSCRIPT, EXITED], [(
            WINDOW_PRUNE,
            "            .is_some_and(|at| at.saturating_add(restarts.within) < now)")]),
    lab("the window never rolls", KERNEL, "RA03",
        [TRANSCRIPT, EXITED], [(WINDOW_PRUNE, "            .is_some_and(|_| false)")]),
    lab("the window tumbles: every restart is forgotten once the oldest expires", KERNEL, "RA04",
        [TRANSCRIPT], [(
            """        while supervision
            .restart_times
            .front()
            .is_some_and(|at| at.saturating_add(restarts.within) <= now)
        {
            supervision.restart_times.pop_front();
        }""",
            """        if supervision
            .restart_times
            .front()
            .is_some_and(|at| at.saturating_add(restarts.within) <= now)
        {
            supervision.restart_times.clear();
        }""")]),
    lab("returned and failed members restart", KERNEL, "SV03",
        [TRANSCRIPT, OUTCOME], [(
            RESTART_RULE,
            "        if cause != TerminalCause::Cancelled && entry.member.restart == Restart::OnDefect && open {")]),
    lab("the restart is charged after the replacement starts", KERNEL, "RA01",
        [TRANSCRIPT], [(CHARGE, CHARGE_AFTER_START)]),
    # Both release points must go: the body's end (unroute) and its finish.
    lab("a name outlives its incarnation", KERNEL, "RT02",
        [TRANSCRIPT, OUTCOME, EXITED], [
            ("            self.kernel.names.remove(&name);\n        }\n        if let Some(timer) = record.stopping",
             "        }\n        if let Some(timer) = record.stopping"),
            ("        record.accepting = false;\n        if let Some(name) = record.spec.name\n            && self.kernel.names.get(&name) == Some(&incarnation)\n        {\n            self.kernel.names.remove(&name);\n        }",
             "        record.accepting = false;")]),
    # Escalation.
    lab("escalation stops declared members before dynamic ones", KERNEL, "ES05",
        [TRANSCRIPT], [(STOP_ORDER, "                    for membership in dynamic.into_iter().chain(declared) {")]),
    lab("a root escalation ends only parked root tasks", KERNEL, "RA05",
        [OUTCOME, EXITED], [(
            "                    && matches!(record.phase, TaskPhase::Runnable | TaskPhase::Parked)",
            "                    && matches!(record.phase, TaskPhase::Parked)")]),
    lab("a root escalation ends the program as a scheduler failure", RUNTIME, "RA02",
        [OUTCOME], [(ESCALATION_OUTCOME, "")]),
    # Range checks at the call that receives the value.
    lab("Process.spec accepts a computed zero capacity", PROCESSES, "MB10",
        [OUTCOME, EXITED], [(
            "                if integer(1) < 1 {\n                    return range_defect(b\"Process.spec: mailbox capacity",
            "                if integer(1) < 0 {\n                    return range_defect(b\"Process.spec: mailbox capacity")]),
    lab("Supervisor.policy accepts members over capacity", PROCESSES, "SV11",
        [OUTCOME, EXITED], [(
            "                if i64::from(members) > capacity {",
            "                if i64::from(members) > capacity + 1 {")]),
    # Each incarnation runs on its own copy of the body and its providers.
    lab("the first incarnation takes the spec's body instead of a copy", RUNTIME, "CB08",
        [OUTCOME, EXITED], [(
            "  fragment_copy_into_current(context->body, roots);",
            "  fragment_adopt(context->body, roots);")]),
    # Virtual time and determinism.
    lab("the runner ignores --clock=virtual", RUNTIME, "OB06",
        ["exceeded the 60 s run limit"], [(CLOCK_MODE, ".clock_mode = CHIMI_SCHEDULER_CLOCK_REAL,")]),
    lab("telemetry carries the process id", TELEMETRY, "OB04",
        ["telemetry of the second --trace=full run"], [(OB04_COUNT, OB04_PID)]),
    # The same mutant through `all`, where OB04 reads the result MB01's own
    # run recorded in the runtime kind instead of rebuilding it.
    dict(name="OB04 (recorded runs): telemetry carries the process id", file=TELEMETRY,
         edits=[(OB04_COUNT, OB04_PID)],
         command=["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
                  "all", "--through", "U6"],
         expect=["\"OB04\",\"status\":\"fail\",\"detail\":\"MB01: telemetry of the second"]),
    lab("event classes ignore their class fields", FIXTURE, "RA08",
        ["expected.events line"], [(
            'const CLASS_FIELDS: [&str; 4] = ["cause", "reason", "outcome", "attempts_in_window"];',
            'const CLASS_FIELDS: [&str; 4] = ["", "", "", ""];')]),
]
