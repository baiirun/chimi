"""Mutants of supervised-process stage U8 (ts-6ccbc9): `shutdown_within`
deadlines, forced termination, and structured child tasks inside processes, as
the scheduler applies them to Chimi bodies through runtime.c, and the SD04
harness in kernel_faults.c. Each mutant breaks one decision a U8 scenario
guards and must turn that scenario red with the failure it names.

Run: scripts/mutation/mutate.py scripts/mutation/process_u8_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

Every mutant runs one scenario through the lab binary with `--through U8`.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"
SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"

TRANSCRIPT = "expected.transcript line"
OUTCOME = "expected.outcome line"
EXITED = "exited with"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U8"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


# An incarnation's children are dropped from its child set instead of being
# cancelled, so they outlive it (the chimi-fiber mutant of the same name).
CHILDREN_LEFT_RUNNING = [(
    "        for child in children {\n            self.begin_terminal(child, TaskOutcome::Cancelled, false);\n        }",
    "        if self.incarnation_of(task).is_some() {\n            self.tasks.get_mut(&task).unwrap().children.clear();\n        } else {\n            for child in children {\n                self.begin_terminal(child, TaskOutcome::Cancelled, false);\n            }\n        }")]

SLEEP_LOOP = """          .kind = CHIMI_REQUEST_TIMER_SLEEP_UNTIL,
          .value = deadline,
        };
        if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();"""

REMOVAL_ENTRY = """            return Ok(self.execute(plan));
        }
        let entry = self
            .kernel
            .memberships
            .get(&membership)
            .ok_or(ProcessError::UnknownMembership(membership))?;"""

MUTATIONS = [
    # Structured child tasks.
    lab("a defected body's children outlive it into the replacement", SCHEDULER, "RA07",
        [TRANSCRIPT], CHILDREN_LEFT_RUNNING),
    lab("a stopped member's children outlive it", SCHEDULER, "SD05",
        [TRANSCRIPT], CHILDREN_LEFT_RUNNING),
    lab("a task result points into the child's destroyed heap", RUNTIME, "CB05",
        [OUTCOME, EXITED], [(
            "  ChimiFiberResult result = child_task_result(control);",
            "  ChimiFiberResult result = {.state = CHIMI_FIBER_SUCCEEDED, .value = (int64_t)(intptr_t)control};")]),
    # Cancellation interrupts every park.
    lab("Clock.sleep ignores cancellation", RUNTIME, "RR05",
        [TRANSCRIPT], [(
            SLEEP_LOOP,
            SLEEP_LOOP.replace("return cancelled();", "continue;"))]),
    lab("a removal during stop is ignored", KERNEL, "RR06",
        [TRANSCRIPT, OUTCOME], [(
            REMOVAL_ENTRY,
            REMOVAL_ENTRY.replace(
                "        let entry = self\n",
                "        if self.kernel.supervisors.get(&supervisor).is_some_and(|record| record.members_taken) {\n            return Ok(self.execute(Plan::from([Step::Reply(requester)])));\n        }\n        let entry = self\n",
                1))]),
    # Deadlines.
    lab("a zero deadline lets cleanup drain", KERNEL, "SD02",
        [TRANSCRIPT, OUTCOME], [(
            "        let forced = deadline <= now;", "        let forced = false && deadline <= now;")]),
    lab("a non-zero deadline forces at once", KERNEL, "SD03",
        [TRANSCRIPT, OUTCOME], [(
            "        let forced = deadline <= now;", "        let forced = deadline > now;")]),
    lab("a forced body that parks again is destroyed", KERNEL, "SD04",
        [EXITED], [(
            "        std::mem::forget(record.fiber.take());", "        drop(record.fiber.take());")]),
    lab("a forced body that parks again is kept waiting", SCHEDULER, "SD04",
        [EXITED], [(
            "            if process && self.is_forced(task) {", "            if false && process && self.is_forced(task) {")]),
    # Structured stop.
    lab("routing is removed member by member", KERNEL, "SD06",
        [TRANSCRIPT, OUTCOME], [(
            "                Some(Instance::Process(incarnation)) => self.unroute(incarnation),",
            "                Some(Instance::Process(_)) => {}")]),
    lab("a forced end is reported as a defect", KERNEL, "SD07",
        [TRANSCRIPT], [(
            "        Some(stopping) if stopping.forced => TerminalCause::Forced,",
            "        Some(stopping) if stopping.forced => TerminalCause::Defected,")]),
    lab("a cancelled shutdown end is reported as a defect", KERNEL, "SD07",
        [TRANSCRIPT], [(
            "        Some(_) => TerminalCause::Cancelled,", "        Some(_) => TerminalCause::Defected,")]),
]
