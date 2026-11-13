"""Mutants of supervised-process stage U9 (ts-6ccbc9): declared and dynamic
nested supervisors and `supervisor_started`, as runtime.c marshals and reports
them and the scheduler runs them, and the lab's projection of
`supervisor_started`. Each mutant breaks one decision a U9 scenario guards and
must turn that scenario red with the failure it names.

Run: scripts/mutation/mutate.py scripts/mutation/process_u9_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

Every mutant runs one scenario through the lab binary with `--through U9`.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"

TRANSCRIPT = "expected.transcript line"
OUTCOME = "expected.outcome line"
EXITED = "exited with"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U9"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


NESTED_MEMBER = "      array[index].supervisor = child;\n"

MUTATIONS = [
    # Declared nested supervisors.
    lab("a stopped nested supervisor reports no stop", KERNEL, "SV09",
        [TRANSCRIPT], [(
            "                            plan.steps.push_front(Step::Emit(ProcessEventKind::Stopped {\n                                supervisor: child,\n                            }));\n",
            "")]),
    lab("supervisor_started names no parent", RUNTIME, "SV09",
        ["root `supervisor_started` has membership"], [(
            'json_identity(output, "parent_supervisor_id", event->parent_supervisor);',
            'json_identity(output, "parent_supervisor_id", 0);')]),
    # Dynamic nested supervisors.
    lab("a nested supervisor takes no parent slot", KERNEL, "SV10",
        [TRANSCRIPT, OUTCOME], [(
            "        if members >= supervision.policy.capacity.get() as usize {",
            "        if members >= supervision.policy.capacity.get() as usize && matches!(member.kind, MemberKind::Process(_)) {")]),
    lab("remove_supervisor leaves the subtree running", KERNEL, "SV10",
        [TRANSCRIPT, OUTCOME], [(
            "            Some(Instance::Supervisor(child)) => Plan::from([\n                Step::StopMembers(child),\n                Step::Emit(ProcessEventKind::Stopped { supervisor: child }),\n                Step::Retire(child),\n                finish,\n            ]),",
            "            Some(Instance::Supervisor(_)) => Plan::from([finish]),")]),
    lab("the ManagedSupervisor handle holds the nested supervisor, not its membership", RUNTIME, "SV10",
        [OUTCOME, EXITED], [(
            "managed_value(VALUE_MANAGED_SUPERVISOR, arguments[0]->as.managed.supervisor,\n                                              request.membership)",
            "managed_value(VALUE_MANAGED_SUPERVISOR, arguments[0]->as.managed.supervisor,\n                                              request.supervisor)")]),
    # Nested escalation.
    # Never restarted, `counter` stays unrouted, so main's `Report` send
    # fails and main ends with that failure (exit 1).
    lab("a declared nested supervisor is marshalled as Never", RUNTIME, "ES02",
        [TRANSCRIPT, OUTCOME, EXITED], [(
            NESTED_MEMBER, NESTED_MEMBER + "      array[index].restart = CHIMI_RESTART_NEVER;\n")]),
    lab("a declared nested supervisor is marshalled as OnDefect", RUNTIME, "ES03",
        [TRANSCRIPT, OUTCOME], [(
            NESTED_MEMBER, NESTED_MEMBER + "      array[index].restart = CHIMI_RESTART_ON_DEFECT;\n")]),
    lab("a nested escalation ends its membership as a requested end", KERNEL, "ES04",
        [TRANSCRIPT, OUTCOME], [(
            "                let cause = if record.end_requested {",
            "                let cause = if true || record.end_requested {")]),
]
