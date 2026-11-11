"""Mutants of the chimi-fiber process scheduler (src/scheduler.rs
and src/scheduler/).

Run: scripts/mutation/mutate.py scripts/mutation/fiber_mutations.py
Each mutant runs the named unit test of the chimi-fiber library; it must turn
it red.

At 15cebc9 three mutants survived. "zero deadline drains" (sd02) only
reached receives, where the receive itself checks the deadline; sd02 now also
removes a process sleeping in a handler. "time moves while runnable"
(virtual_time_advances) needed a timer pending while a task is runnable,
which the test now arranges. "replacement before children drain" (ra07)
cleared the incarnation's child set but still cancelled every child, so it
did not change behaviour; it now leaves an incarnation's children running.
"""

CRATE = "runtime/chimi-fiber"

MUTATIONS = [
    dict(
        name='send reply resumes caller at tail',
        file='src/scheduler/process_abi.rs',
        edits=[('            self.write(waiter, status, fill);\n            let _ = self.wake_front(waiter.task);', '            self.write(waiter, status, fill);\n            let _ = self.wake(waiter.task);')],
        targets=["lib"],
        filter='mb03',
    ),
    dict(
        name='unread messages not released',
        file='src/scheduler/process.rs',
        edits=[('            self.release_message(message);\n', '            let _ = message;\n')],
        targets=["lib"],
        filter='mb05',
    ),
    dict(
        name='window edge inside',
        file='src/scheduler/process.rs',
        edits=[('.is_some_and(|at| at.saturating_add(restarts.within) <= now)', '.is_some_and(|at| at.saturating_add(restarts.within) < now)')],
        targets=["lib"],
        filter='ra04',
    ),
    dict(
        name='failed start uncharged',
        file='src/scheduler/process.rs',
        edits=[('            self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });\n            self.member_ended(membership, TerminalCause::Defected);', '            self.emit_process(ProcessEventKind::AdmissionRejected { supervisor, error });\n            self.kernel.supervisors.get_mut(&supervisor).unwrap().restart_times.pop_back();\n            self.member_ended(membership, TerminalCause::Defected);')],
        targets=["lib"],
        filter='ra06',
    ),
    dict(
        name='stop order forward',
        file='src/scheduler/process.rs',
        edits=[('for membership in declared.into_iter().chain(dynamic) {', 'for membership in declared.into_iter().chain(dynamic).rev() {')],
        targets=["lib"],
        filter='sd01',
    ),
    dict(
        name='unroute member by member',
        file='src/scheduler/process.rs',
        edits=[('                Some(Current::Process(incarnation)) => self.unroute(incarnation),', '                Some(Current::Process(_)) => {}')],
        targets=["lib"],
        filter='sd06',
    ),
    dict(
        name='cleanup does not drain',
        file='src/scheduler/process.rs',
        edits=[('                stopping.pending = false;\n                match record.mailbox.pop_front() {', '                stopping.pending = false;\n                match None::<u64> {')],
        targets=["lib"],
        filter='mb08',
    ),
    dict(
        name='zero deadline drains',
        file='src/scheduler/process.rs',
        edits=[('        let forced = deadline <= now;', '        let forced = false && deadline <= now;')],
        targets=["lib"],
        filter='sd02',
    ),
    dict(
        name='remove does not wait',
        file='src/scheduler/process.rs',
        edits=[('            Current::Process(incarnation) => Plan::from([Step::Await(incarnation), finish]),', '            Current::Process(_) => Plan::from([finish]),')],
        targets=["lib"],
        filter='rr01',
    ),
    dict(
        name='returned removal silent',
        file='src/scheduler/process.rs',
        edits=[('            self.forget(membership, true);', '            self.forget(membership, false);')],
        targets=["lib"],
        filter='rr04',
    ),
    dict(
        name='sleep timer kept on cancel',
        file='src/scheduler.rs',
        edits=[('        if let Some(timer) = record.timer.take() {\n            self.cancel_timer(timer);\n        }\n\n        let channel_cleanup', '        let _ = record.timer;\n\n        let channel_cleanup')],
        targets=["lib"],
        filter='cancelling_a_sleeper',
    ),
    dict(
        name='replacement before children drain',
        file='src/scheduler.rs',
        edits=[('        for child in children {\n            self.begin_terminal(child, TaskOutcome::Cancelled, false);\n        }', '        if self.incarnation_of(task).is_some() {\n            self.tasks.get_mut(&task).unwrap().children.clear();\n        } else {\n            for child in children {\n                self.begin_terminal(child, TaskOutcome::Cancelled, false);\n            }\n        }')],
        targets=["lib"],
        filter='ra07',
    ),
    dict(
        name='time moves while runnable',
        file='src/scheduler.rs',
        edits=[('        loop {\n            self.drive();\n            if self.host_operations > 0 {', '        loop {\n            if let Some(d) = self.timers.earliest() { self.clock.wait_until(d); self.fire_due_timers(); }\n            self.drive();\n            if self.host_operations > 0 {')],
        targets=["lib"],
        filter='virtual_time_advances',
    ),
    dict(
        name='forced fiber destroyed',
        file='src/scheduler/process.rs',
        edits=[('        std::mem::forget(record.fiber.take());', '        drop(record.fiber.take());')],
        targets=["lib"],
        filter='sd04_the_c_entry',
    ),
    dict(
        name='name registered late',
        file='src/scheduler/process.rs',
        edits=[('        if let Some(name) = spec.name {\n            self.kernel.names.insert(name, incarnation);\n        }', '')],
        targets=["lib"],
        filter='mb04',
    ),
    dict(
        name='restart limit per member',
        file='src/scheduler/process.rs',
        edits=[('        let attempts = supervision.restart_times.len() as u32 + 1;', '        let attempts = 1;')],
        targets=["lib"],
        filter='ra02',
    ),
    dict(
        name='rollback skipped',
        file='src/scheduler/process.rs',
        edits=[('                let rolled_back = self.execute(Plan::from([Step::StopMembers(supervisor)]));\n                debug_assert_eq!(rolled_back, Progress::Done);', '')],
        targets=["lib"],
        filter='rt06',
    ),
]
