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

The nested-supervisor and structured-stop mutants (from "nested supervisor
outside parent capacity" on) guard the decisions of ts-b1a535 and ts-2e0451.

The mutants from "removal in flight expanded again" on guard the review
fixes ts-9c0516 (a stop or removal that meets a nested escalation ends the
membership without a restart) and ts-bb32bf (an escalated root releases its
draining owner), and the guarantees that review found untested: the retired
handle kind check and nested escalation answering `Supervisor.wait`.
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
        edits=[('                Some(Instance::Process(incarnation)) => self.unroute(incarnation),', '                Some(Instance::Process(_)) => {}')],
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
        edits=[('            Some(Instance::Process(incarnation)) => Plan::from([Step::Await(incarnation), finish]),', '            Some(Instance::Process(_)) => Plan::from([finish]),')],
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
        edits=[('        if let Some(timer) = record.timer.take() {\n            self.cancel_timer(timer);\n        }\n        self.unroute_task(task);\n', '        let _ = record.timer;\n        self.unroute_task(task);\n')],
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
        edits=[('                let rolled_back = self.execute(Plan::from([\n                    Step::StopMembers(supervisor),\n                    Step::Emit(ProcessEventKind::Stopped { supervisor }),\n                ]));\n                debug_assert_eq!(rolled_back, Progress::Done);', '')],
        targets=["lib"],
        filter='rt06',
    ),
    dict(
        name='nested supervisor outside parent capacity',
        file='src/scheduler/process.rs',
        edits=[('        if members >= supervision.policy.capacity.get() as usize {', '        if members >= supervision.policy.capacity.get() as usize && matches!(member.kind, MemberKind::Process(_)) {')],
        targets=["lib"],
        filter='sv10',
    ),
    dict(
        name='removal leaves the nested subtree running',
        file='src/scheduler/process.rs',
        edits=[('            Some(Instance::Supervisor(child)) => Plan::from([\n                Step::StopMembers(child),\n                Step::Emit(ProcessEventKind::Stopped { supervisor: child }),\n                Step::Retire(child),\n                finish,\n            ]),', '            Some(Instance::Supervisor(_)) => Plan::from([finish]),')],
        targets=["lib"],
        filter='sv10_a_dynamic_nested',
    ),
    dict(
        name='handle kind unchecked',
        file='src/scheduler/process.rs',
        edits=[('        if !kind_matches {', '        if false && !kind_matches {')],
        targets=["lib"],
        filter='sv10_a_dynamic_nested',
    ),
    dict(
        name='supervisor_started without parent',
        file='src/scheduler/process.rs',
        edits=[('            parent: parent.map(|(parent, _)| parent),', '            parent: None,')],
        targets=["lib"],
        filter='sv10_a_dynamic_nested',
    ),
    dict(
        name='stopped nested supervisor restarts',
        file='src/scheduler/process.rs',
        edits=[('            self.member_ended(membership, TerminalCause::Cancelled);', '            self.member_ended(membership, TerminalCause::Defected);')],
        targets=["lib"],
        filter='sv10_stopping',
    ),
    dict(
        name='nested stop in flight expanded again',
        file='src/scheduler/process.rs',
        edits=[('                        Some(Instance::Supervisor(child)) if self.stop_in_flight(child) => {\n                            plan.steps.push_front(Step::AwaitSupervisor(child));', '                        Some(Instance::Supervisor(child)) if false && self.stop_in_flight(child) => {\n                            plan.steps.push_front(Step::AwaitSupervisor(child));')],
        targets=["lib"],
        filter='a_parent_stop',
    ),
    dict(
        name='nested escalation strands waiting plans',
        file='src/scheduler/process.rs',
        edits=[('                self.member_ended(membership, TerminalCause::Defected);\n                self.resume_blocked(Blocker::Supervisor(supervisor));', '                self.member_ended(membership, TerminalCause::Defected);')],
        targets=["lib"],
        filter='es02_a_parent_stop',
    ),
    dict(
        name='only root waiters answered',
        file='src/scheduler/process.rs',
        edits=[('        for waiter in record.waiters {\n            self.reply(waiter, super::RESPONSE_OK, front);\n        }', '        if record.parent.is_none() {\n            for waiter in record.waiters {\n                self.reply(waiter, super::RESPONSE_OK, front);\n            }\n        }')],
        targets=["lib"],
        filter='sv10_a_wait',
    ),
    dict(
        name='owner terminal before its roots stop',
        file='src/scheduler.rs',
        edits=[('        if !record.children.is_empty() || self.owns_live_roots(task) {', '        if !record.children.is_empty() {')],
        targets=["lib"],
        filter='an_owner_task_stays',
    ),
    dict(
        name='parent terminal before its draining child',
        file='src/scheduler.rs',
        edits=[('        if !record.children.is_empty() || self.owns_live_roots(task) {', '        if self.owns_live_roots(task) {')],
        targets=["lib"],
        filter='a_parent_task_stays',
    ),
    dict(
        name='process owners not held',
        file='src/scheduler.rs',
        edits=[('        if !record.children.is_empty() || self.owns_live_roots(task) {', '        if !record.children.is_empty() || (record.process.is_none() && self.owns_live_roots(task)) {')],
        targets=["lib"],
        filter='removing_a_process_that_started',
    ),
    dict(
        name='removal in flight expanded again',
        file='src/scheduler/process.rs',
        edits=[('            Some(Instance::Supervisor(child)) if self.stop_in_flight(child) => {\n                self.request_end(child);', '            Some(Instance::Supervisor(child)) if false && self.stop_in_flight(child) => {\n                self.request_end(child);')],
        targets=["lib"],
        filter='sv10_removing_a_nested_supervisor_during_its_escalation',
    ),
    dict(
        name='removal during escalation not recorded',
        file='src/scheduler/process.rs',
        edits=[('                self.request_end(child);\n', '')],
        targets=["lib"],
        filter='sv10_removing_a_nested_supervisor_during_its_escalation',
    ),
    dict(
        name='stop during escalation not recorded',
        file='src/scheduler/process.rs',
        edits=[('                record.end_requested = true;\n', '')],
        targets=["lib"],
        filter='sv10_stopping_a_nested_supervisor_during_its_escalation',
    ),
    dict(
        name='nested escalation ignores a requested end',
        file='src/scheduler/process.rs',
        edits=[('                let cause = if record.end_requested {', '                let cause = if false && record.end_requested {')],
        targets=["lib"],
        filter='during_its_escalation',
    ),
    dict(
        name='retired handle kind unchecked',
        file='src/scheduler/process.rs',
        edits=[('                _ => return Err(ProcessError::WrongMemberKind(membership)),', '                _ => Step::Reply(None),')],
        targets=["lib"],
        filter='sv10_a_retired_handle_is_still_kind_checked',
    ),
    dict(
        name='nested escalation leaves waiters parked',
        file='src/scheduler/process.rs',
        edits=[('                for waiter in record.waiters {\n                    self.reply(waiter, super::RESPONSE_OK, false);\n                }', '                let _ = record.waiters;')],
        targets=["lib"],
        filter='sv10_a_wait_on_a_nested_supervisor_returns_when_it_escalates',
    ),
    dict(
        name='escalated root strands its draining owner',
        file='src/scheduler/process.rs',
        edits=[('                if let Some(owner) = owner {\n                    self.finish_draining(owner);\n                }', '                let _ = owner;')],
        targets=["lib"],
        filter='removing_an_owner_whose_root_escalates',
    ),
    dict(
        name='escalated root strands its draining owner at teardown',
        file='src/scheduler/process.rs',
        edits=[('                if let Some(owner) = owner {\n                    self.finish_draining(owner);\n                }', '                let _ = owner;')],
        targets=["lib"],
        filter='tearing_down_with_an_owner_draining',
    ),
]
