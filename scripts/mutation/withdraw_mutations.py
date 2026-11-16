"""Mutants of CHI-259: a descriptor wait names its resource, a withdraw wakes
the waits on it, closing a binding handle withdraws before its release, and a
failed poll fails the waits in flight. A mutant that makes a wait hang shows
as TIMEOUT, which counts as caught.

Run: scripts/mutation/mutate.py --timeout 120 scripts/mutation/withdraw_mutations.py
Results: scripts/mutation/withdraw_results.txt.
"""

CRATE = "compiler/chimi"

REACTOR = "../../runtime/chimi-fiber/src/scheduler/reactor.rs"
SCHEDULER = "../../runtime/chimi-fiber/src/scheduler.rs"
RUNTIME_C = "runtime/native/runtime.c"


def fiber(test):
    return dict(
        command=["cargo", "test", "--quiet", "-p", "chimi-fiber", "--lib", "--", test],
        expect=[test],
    )


WITHDRAW = "a_withdraw_wakes_only_the_waits_on_its_resource"
FAILED_POLL = "a_failed_poll_fails_the_waits_in_flight_instead_of_spinning"


def mutant(name, file, old, new, **test):
    return dict(name=name, file=file, edits=[(old, new)], **test)


MUTATIONS = [
    mutant("a withdraw wakes nobody", REACTOR,
           "            for task in waiting {\n                self.clear_io_wait(task);\n                let _ = self.wake(task);\n            }",
           "            let _ = waiting;",
           **fiber(WITHDRAW)),
    mutant("a withdraw ignores the resource", REACTOR,
           "            .filter(|wait| wait.resource == resource)",
           "            .filter(|_| resource != 0)",
           **fiber(WITHDRAW)),
    mutant("a wait forgets its resource", REACTOR,
           "self.reactor.register(owner, fd, interest, request.channel)",
           "self.reactor.register(owner, fd, interest, 0)",
           **fiber(WITHDRAW)),
    mutant("a failed poll leaves the wait pending", REACTOR,
           "            if woken.failed\n",
           "            if false && woken.failed\n",
           **fiber(FAILED_POLL)),
    mutant("a stdin wait reads after the reactor fails", "runtime/native/runtime.c",
           "      if (waited == INPUT_WAIT_FAILED) return INPUT_FAILED;",
           "",
           targets=["cli"], filter=""),
    mutant("a failed poll reads as an empty wait", REACTOR,
           "            return Woken {\n                tasks: self.waits.values().map(|wait| wait.task).collect(),\n                failed: true,\n            };",
           "            return Woken {\n                tasks: Vec::new(),\n                failed: false,\n            };",
           **fiber(FAILED_POLL)),
    mutant("a dispatch drops the withdraw", SCHEDULER,
           "            REQUEST_IO_WITHDRAW => self.handle_io_withdraw(owner, request),\n",
           "",
           targets=["bindings"], filter=""),
    mutant("closing a handle does not withdraw", RUNTIME_C,
           "release_binding_slot(entry, value->as.handle.slot, true);",
           "release_binding_slot(entry, value->as.handle.slot, false);",
           targets=["bindings"], filter=""),
    mutant("a binding wait names no resource", RUNTIME_C,
           "      resource = binding_resource_key(value->as.handle.slot, value->as.handle.generation);",
           "      resource = 0;",
           targets=["bindings"], filter=""),
    mutant("the withdraw key is the new generation", RUNTIME_C,
           ".channel = binding_resource_key(slot, entry->generation)};",
           ".channel = binding_resource_key(slot, entry->generation + 1)};",
           targets=["bindings"], filter=""),
]
