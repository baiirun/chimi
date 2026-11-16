"""Mutants of std/exec's child-exit wait in the scheduler (CHI-250, part 1).

Not listed, as equivalent: dropping any one of the redundant withdrawals
(`handle_request`, a task's end, `disarm_wake_sources`): each path that leaves
a wait passes through at least two of them, so one alone changes nothing a
test can see. Also equivalent: the recheck every `IO_POLL_INTERVAL` resumes
of a busy scheduler (a busy program reaches an idle wait or the SIGCHLD bit
soon enough; it only shortens a wake's latency).

Run: scripts/mutation/mutate.py scripts/mutation/exec_mutations.py
"""

CRATE = "runtime/chimi-fiber"

CHILD = "src/scheduler/child.rs"
SIGNAL = "src/scheduler/signal.rs"
SCHEDULER = "src/scheduler.rs"
REACTOR = "src/scheduler/reactor.rs"
SCOPE = "src/scheduler/scope.rs"


def lab(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="child")


MUTATIONS = [
    lab("a check reaps the child", CHILD, [
        ("libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,\n                )\n            };\n            if result",
         "libc::WEXITED | libc::WNOHANG,\n                )\n            };\n            if result")]),
    lab("a running child answers at once", CHILD, [
        ("            return if unsafe { info.si_pid() } == 0 {\n                Check::Running",
         "            return if unsafe { info.si_pid() } == -1 {\n                Check::Running")]),
    lab("a non-child parks instead of failing", CHILD, [
        ("                return Check::Unknown;\n            }\n            // With WNOHANG",
         "                return Check::Running;\n            }\n            // With WNOHANG")]),
    lab("pid 0 is checked as a pid", CHILD, [
        ("let check = if pid > 0 { sys::check(pid) } else { Check::Unknown };",
         "let check = sys::check(pid);")]),
    lab("the SIGCHLD bit is ignored", SIGNAL, [
        ("        if bits & super::CHILD_EXITED_SIGNAL_BIT != 0 {\n            self.recheck_children();\n        }\n", "")]),
    lab("an idle wait never rechecks", REACTOR, [
        ("        self.wake_io(woken);\n        self.recheck_children();\n", "        self.wake_io(woken);\n")]),
    lab("a child wait does not hold virtual time", SCHEDULER, [
        ("if !self.reactor.is_empty() || !self.children.is_empty() {", "if !self.reactor.is_empty() {")]),
    lab("a child wait is not a cancellation point", SCOPE, [
        ("            | super::REQUEST_SIGNAL_WAIT\n            | super::REQUEST_CHILD_WAIT\n            | super::REQUEST_SCOPE_ENTER",
         "            | super::REQUEST_SIGNAL_WAIT\n            | super::REQUEST_SCOPE_ENTER")]),
    lab("a firing scope does not withdraw a child wait", SCOPE, [
        ("            | super::REQUEST_SIGNAL_WAIT\n            | super::REQUEST_CHILD_WAIT\n    )",
         "            | super::REQUEST_SIGNAL_WAIT\n    )")]),
    lab("the recheck backstop waits forever", CHILD, [
        ("        } else {\n            Some(UNSIGNALED_RECHECK)\n        }", "        } else {\n            None\n        }")]),
    lab("with a SIGCHLD source the backstop is the fast one", CHILD, [
        ("            Some(super::clock::MAX_REAL_WAIT)\n", "            Some(UNSIGNALED_RECHECK)\n")]),
]
