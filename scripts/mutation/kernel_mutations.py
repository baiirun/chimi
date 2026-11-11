"""Mutants of the prototype's supervision kernel (src/kernel.rs) and of the
heap behaviour the kernel tests depend on.

Run: scripts/mutation/mutate.py scripts/mutation/kernel_mutations.py
Each mutant runs tests/kernel.rs; it must turn one red.
"""

CRATE = "prototypes/supervised-process-runtime"
CARGO_TEST_ARGS = ["--release"]

K = "src/kernel.rs"
H = "c/heap.c"


def m(name, file, edits, target="kernel", filter=""):
    return dict(name=name, file=file, edits=edits, targets=[target], filter=filter)


MUTATIONS = [
 m("keep the name after termination", K, [("""        let incarnation = self.incarnations.remove(&id).expect("live incarnation");
        if let Some(name) = incarnation.spec.name
            && self.names.get(&name) == Some(&id)
        {
            self.names.remove(&name);
        }""", """        let incarnation = self.incarnations.remove(&id).expect("live incarnation");""")]),
 m("do not stop peers on escalation", K, [("""        self.events.push(LifecycleEvent::RestartLimitExceeded { supervisor });
        self.stop_members(supervisor);""", """        self.events.push(LifecycleEvent::RestartLimitExceeded { supervisor });""")]),
 m("charge the restart after it starts", K, [("""        if attempts > supervision.policy.restarts.limit {
            self.escalate(supervisor);
            return;
        }
        supervision.restart_times.push_back(now);""", """        let limit = supervision.policy.restarts.limit;
        supervision.restart_times.push_back(now);"""), ("""            self.member_ended(membership, TerminalCause::Defected);
        }
    }

    /// The supervisor""", """            self.member_ended(membership, TerminalCause::Defected);
        }
        if attempts > limit { self.escalate(supervisor); }
    }

    /// The supervisor""")]),
 m("never prune the restart window", K, [("""        while supervision.restart_times.front().is_some_and(|at| *at + within <= now) {
            supervision.restart_times.pop_front();
        }""", "let _ = within;")]),
 m("do not update the handle's incarnation on restart", K, [("""                entry.last_incarnation = Some(incarnation);""", """                entry.last_incarnation = entry.last_incarnation.or(Some(incarnation));""")]),
 m("ignore mailbox capacity", K, [("""                if incarnation.mailbox.len() >= incarnation.spec.mailbox_capacity.get() as usize =>""", """                if false =>""")]),
 m("stop declared members in forward order", K, [("""        order.extend(std::mem::take(&mut supervision.declared).into_iter().rev());""", """        order.extend(std::mem::take(&mut supervision.declared));""")]),
 m("report a forced end as cancelled", K, [("""        (END_FORCED, _) => TerminalCause::Forced,""", """        (END_FORCED, _) => TerminalCause::Cancelled,""")]),
 m("skip the admission capacity check", K, [("""        if members >= supervision.policy.capacity.get() as usize {""", """        if members >= usize::MAX {""")]),
 m("do not roll back a failed start", K, [("""                self.events.push(LifecycleEvent::AdmissionRejected { supervisor, error });
                self.stop_members(supervisor);""", """                self.events.push(LifecycleEvent::AdmissionRejected { supervisor, error });""")]),
 m("treat a nested escalation as a root one", K, [("""            Some((_, membership)) => {
                self.supervisors.remove(&supervisor);""", """            Some((_, membership)) => {
                self.program_terminated = true;
                self.supervisors.remove(&supervisor);""")]),
 m("treat a body panic as a return", K, [("""        Err(_) => END_DEFECTED,""", """        Err(_) => END_RETURNED,""")]),
 m("adopt messages into the base area", H, [("""  Area *area = allocation_area(heap, &region, &role);
  size_t bytes = fragment->area.bytes;""", """  Area *area = allocation_area(heap, &region, &role);
  area = &heap->base; region = NULL; role = ROLE_BASE;
  size_t bytes = fragment->area.bytes;""")]),
 m("cancel without delivering queued messages", K, [("""            let reply = incarnation.mailbox.pop_front().map_or(Reply::Cancelled, Reply::Message);""", """            let reply = Reply::Cancelled;""")]),
 m("park a receive after cancellation (old behaviour)", K, [("""                    None if cancelling => incarnation.exchange.reply = Some(Reply::Cancelled),""", """                    None if cancelling => { incarnation.wait = Wait::Receiving; return; }""")]),
 m("force all cleanup work", K, [("""                        Some(deadline) if finish > deadline => {""", """                        Some(deadline) if finish >= deadline - deadline => {""")]),
 m("remove without shutting down", K, [("""        if let Current::Process(incarnation) = entry.current {
            self.shut_down(incarnation);
        }
        self.release_slot(membership);
        self.memberships.remove(&membership);""", """        self.release_slot(membership);
        self.memberships.remove(&membership);""")]),
 m("keep accepting during cleanup", K, [("""        incarnation.accepting = false;
        incarnation.deadline""", """        incarnation.deadline""")]),
 m("handle lookups require the record (pre-fix)", K, [("""        let incarnation = match self.memberships.get(&managed.0) {
            Some(entry) => entry.last_incarnation,
            None => self.retired.get(&managed.0).copied(),
        };""", """        let incarnation = self.memberships[&managed.0].last_incarnation;""")]),
 m("keep ended membership records", K, [("""            self.release_slot(membership);
            self.forget(membership);""", """            self.release_slot(membership);""")]),
 m("keep removed membership records", K, [("""        self.release_slot(membership);
        self.memberships.remove(&membership);""", """        self.release_slot(membership);""")]),
]
