"""Mutants of the Clock.sleep slice (ts-6ccbc9 decision 6): the root `clock`
need, `Clock.sleep` on the native runtime, the runner's `--clock` flag, virtual
time, virtual deadlock, and the clock trace events.

Run: scripts/mutation/mutate.py scripts/mutation/clock_mutations.py
Each mutant must turn the native clock tests (tests/cli.rs, `native_virtual`,
`native_settle`) or the root-closure language test red. The scheduler mutants
edit runtime/chimi-fiber, which the native build links, so they are judged by
what a compiled Chimi program does. Results: scripts/mutation/clock_results.txt.
"""

CRATE = "compiler/chimi"

SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
TIMERS = "../../runtime/chimi-fiber/src/scheduler/clock.rs"
RUNTIME = "runtime/native/runtime.c"
TYPING = "src/typing.rs"


def native(name, file, edits, filter="native_"):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=filter)


def language(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"],
                filter="the_runner_closes_only_a_root_clock_need")


MUTATIONS = [
    # Virtual time advances only when nothing is runnable.
    native("virtual clock advances while runnable work exists", SCHEDULER, [(
        "        loop {\n            self.drive();\n            if self.host_operations > 0 {",
        "        loop {\n            if let Some(task) = self.runnable.pop_front() {\n"
        "                self.run(task);\n            }\n            if self.host_operations > 0 {")]),
    # Timers fire in deadline order, not registration order.
    native("timers fire in registration order", TIMERS, [
        ("    by_deadline: BTreeMap<(Instant, TimerId), TimerTarget>,",
         "    by_deadline: BTreeMap<(TimerId, Instant), TimerTarget>,"),
        ("        self.by_deadline.insert((deadline, id), target);",
         "        self.by_deadline.insert((id, deadline), target);"),
        ("        self.by_deadline.remove(&(deadline, id))\n    }",
         "        self.by_deadline.remove(&(id, deadline))\n    }"),
        ("        self.by_deadline.keys().next().map(|(deadline, _)| *deadline)",
         "        self.by_deadline.keys().next().map(|(_, deadline)| *deadline)"),
        ("        let (&(deadline, id), _) = self.by_deadline.first_key_value()?;",
         "        let (&(id, deadline), _) = self.by_deadline.first_key_value()?;"),
        ("        let target = self.by_deadline.remove(&(deadline, id))?;",
         "        let target = self.by_deadline.remove(&(id, deadline))?;"),
    ]),
    # Deadlock detection under virtual time.
    native("virtual deadlock detection off", SCHEDULER, [(
        "        match self.clock.mode() {",
        "        match ClockMode::Real {")]),
    native("deadlock waiters dropped from the root's cause", RUNTIME, [(
        "  runtime->deadlock_waiters = message;",
        "  free(message);")]),
    # Root closure.
    language("root closure ignores the fixed need name", TYPING, [(
        '            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == "clock",',
        "            Some(StandardSymbol::Type(StandardType::Clock)) => true,")]),
    language("root closure stays off for clock", TYPING, [(
        '            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == "clock",',
        "            Some(StandardSymbol::Type(StandardType::Clock)) => false,")]),
    # Sleep and the runner.
    native("a negative duration sleeps its magnitude", RUNTIME, [(
        "      if (__builtin_add_overflow(now.value, arguments[0]->as.integer, &deadline)) {",
        "      if (__builtin_add_overflow(now.value, llabs(arguments[0]->as.integer), &deadline)) {")],
        filter="native_virtual_sleep_advances"),
    native("Instant overflow unchecked", RUNTIME, [(
        "      if (__builtin_add_overflow(now.value, arguments[0]->as.integer, &deadline)) {",
        "      if ((deadline = (int64_t)((uint64_t)now.value + (uint64_t)arguments[0]->as.integer), false)) {")],
        filter="native_sleep_past_the_instant_range"),
    native("sleep ignores cancellation and parks again", RUNTIME, [(
        "        if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();\n"
        "        if (request.status == CHIMI_RESPONSE_OK) return success(void_value());\n"
        "        if (request.status != CHIMI_RESPONSE_PENDING)\n"
        '          return defect("Clock.sleep scheduler request failed"',
        "        (void)scheduler_request(&request);\n"
        "        if (request.status == CHIMI_RESPONSE_OK) return success(void_value());\n"
        "        if (request.status != CHIMI_RESPONSE_PENDING)\n"
        '          return defect("Clock.sleep scheduler request failed"')],
        filter="native_cancelling_a_sleeping_task"),
    native("--clock=virtual ignored", RUNTIME, [(
        '    else if (strcmp(argv[index], "--clock=virtual") == 0) runtime.virtual_clock = true;',
        '    else if (strcmp(argv[index], "--clock=virtual") == 0) runtime.virtual_clock = false;')],
        filter="native_virtual_sleepers_wake"),
    # Telemetry.
    native("timer events are boundary events", SCHEDULER, [(
        "        if self.trace_mode != TraceMode::Full {\n            return;\n        }\n        self.emit_clock(",
        "        if self.trace_mode == TraceMode::Off {\n            return;\n        }\n        self.emit_clock(")],
        filter="native_virtual_sleep_advances"),
    native("clock request kind written as a cause task", RUNTIME, [(
        '  if (clock || event->cause == 0) fputs("null", output);',
        '  if (event->cause == 0) fputs("null", output);')],
        filter="native_virtual_deadlock"),
]
