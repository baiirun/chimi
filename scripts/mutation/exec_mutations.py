"""Mutants of std/exec's child-exit wait in the scheduler (CHI-250, part 1).

Not listed, as equivalent: dropping any one of the redundant withdrawals
(`handle_request`, a task's end, `disarm_wake_sources`): each path that leaves
a wait passes through at least two of them, so one alone changes nothing a
test can see. Also equivalent: the guard that answers ERROR for a pid of 0 or
less without calling `waitid` (`waitid(P_PID, 0 or -1)` fails the same way on
Linux and macOS), and the recheck backoff's growth and reset (they only change
how often an idle wait rechecks, never whether it does).

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
    lab("a busy run never rechecks", SCHEDULER, [
        ("                    self.poll_io_now();\n                }\n                self.recheck_children();\n",
         "                    self.poll_io_now();\n                }\n")]),
    lab("the SIGCHLD pipe byte does not end an idle wait", REACTOR, [
        ("let woken = self.reactor.wait(timeout, self.signals.wake_fd());",
         "let woken = self.reactor.wait(timeout, None);")]),
    lab("an idle wait never rechecks", REACTOR, [
        ("        self.wake_io(woken);\n        self.recheck_children_after_idle();\n", "        self.wake_io(woken);\n")]),
    lab("a child wait does not hold virtual time", SCHEDULER, [
        ("if !self.reactor.is_empty() || !self.children.is_empty() {", "if !self.reactor.is_empty() {")]),
    lab("a child wait is not a cancellation point", SCOPE, [
        ("            | super::REQUEST_SIGNAL_WAIT\n            | super::REQUEST_CHILD_WAIT\n            | super::REQUEST_SCOPE_ENTER",
         "            | super::REQUEST_SIGNAL_WAIT\n            | super::REQUEST_SCOPE_ENTER")]),
    lab("a firing scope does not withdraw a child wait", SCOPE, [
        ("            | super::REQUEST_SIGNAL_WAIT\n            | super::REQUEST_CHILD_WAIT\n    )",
         "            | super::REQUEST_SIGNAL_WAIT\n    )")]),
    lab("the recheck backstop waits forever", CHILD, [
        ("(!self.children.is_empty()).then_some(self.children.recheck)", "None")]),
]
