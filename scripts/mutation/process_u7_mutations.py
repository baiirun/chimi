"""Mutants of supervised-process stage U7 (ts-6ccbc9): admission, names,
removal, `Supervisor.wait`, and drain on cancellation, as runtime.c reports
them and the scheduler applies them, and the RA06 fault-injection harness. Each
mutant breaks one decision a U7 scenario guards and must turn that scenario red
with the failure it names.

Run: scripts/mutation/mutate.py scripts/mutation/process_u7_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

Every mutant runs one scenario through the lab binary with `--through U7`.
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

RUNTIME = "runtime/native/runtime.c"
KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"

TRANSCRIPT = "expected.transcript line"
OUTCOME = "expected.outcome line"
EXITED = "exited with"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U7"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


ROLLBACK = """                let rolled_back = self.execute(Plan::from([
                    Step::StopMembers(supervisor),
                    Step::Emit(ProcessEventKind::Stopped { supervisor }),
                ]));
                debug_assert_eq!(rolled_back, Progress::Done);
"""
FAILED_REPLACEMENT = """        if let Err(error) = self.start_incarnation(membership) {
            self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });"""
DRAIN = """                stopping.pending = false;
                match record.mailbox.pop_front() {"""

MUTATIONS = [
    # Admission.
    lab("admission lets a start past capacity", KERNEL, "SV02",
        [TRANSCRIPT, OUTCOME], [(
            "        if members >= supervision.policy.capacity.get() as usize {",
            "        if members > supervision.policy.capacity.get() as usize {")]),
    lab("Stopped is reported as CapacityReached", RUNTIME, "SV06",
        [OUTCOME], [(
            "case CHIMI_RESPONSE_STOPPED: return failure(standard_value(CHIMI_CONSTRUCTOR_START_STOPPED, NULL, 0));",
            "case CHIMI_RESPONSE_STOPPED: return failure(standard_value(CHIMI_CONSTRUCTOR_START_CAPACITY_REACHED, NULL, 0));")]),
    # Names.
    lab("NameTaken carries a text other than the trace label", RUNTIME, "RT03",
        [OUTCOME], [('{text_value(label ? label : "")}', '{text_value("counter")}')]),
    lab("every process name is one scheduler name", EVAL, "RT04",
        [TRANSCRIPT, OUTCOME, EXITED], [(
            "                (*value).payload.address.target = u64::from(symbol_index) + 1;",
            "                (*value).payload.address.target = 1;")]),
    lab("a root whose member start fails is left running", KERNEL, "RT06",
        [TRANSCRIPT, OUTCOME, EXITED], [(ROLLBACK, "")]),
    lab("a second register replaces the name", RUNTIME, "RT07",
        [OUTCOME, EXITED], [(
            "      if (name->as.constructed.length != 0)\n        return defect(\"Process.register: spec already has a name\", call_node);",
            "      if (false)\n        return defect(\"Process.register: spec already has a name\", call_node);")]),
    # Removal.
    # Removal answers only after its target has terminated. (A runtime.c
    # mutant that took the scheduler's PENDING reply as done survives at U7:
    # every U7 cleanup finishes inline, so removal never replies PENDING.)
    lab("removal finishes before its target terminates", KERNEL, "RR01",
        [TRANSCRIPT, OUTCOME, EXITED], [(
            "            Some(Instance::Process(incarnation)) => Plan::from([Step::Await(incarnation), finish]),",
            "            Some(Instance::Process(incarnation)) => Plan::from([finish, Step::Await(incarnation)]),")]),
    lab("the foreign-owner defect names one supervisor", RUNTIME, "SV12",
        [OUTCOME], [(
            "function, request.owner, arguments[0]->as.managed.supervisor),",
            "function, request.owner, request.owner),")]),
    # Restart accounting with a failed replacement (C harness).
    lab("a failed replacement is refunded", KERNEL, "RA06",
        ["attempts_in_window"], [(
            FAILED_REPLACEMENT,
            """        if let Err(error) = self.start_incarnation(membership) {
            if let Some(record) = self.kernel.supervisors.get_mut(&supervisor) {
                record.restart_times.pop_back();
            }
            self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });""")]),
    # Drain on cancellation.
    lab("cleanup receives nothing", KERNEL, "MB08",
        [OUTCOME, TRANSCRIPT, EXITED], [(
            DRAIN, "                stopping.pending = false;\n                match None::<u64> {")]),
    lab("routing is kept during cleanup", KERNEL, "MB09",
        [OUTCOME, TRANSCRIPT, EXITED], [(
            "        let task = record.task;\n        self.unroute(incarnation);",
            "        let task = record.task;")]),
]
