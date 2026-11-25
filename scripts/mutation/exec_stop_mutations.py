"""Mutants of std/exec part 4 (CHI-250): stopping a child still running when
its `Exec.start` ends (SIGTERM, `stop_within`, then SIGKILL), and the exit
backstop.

Not covered by a test: the kqueue ticker (macOS; CI runs the same tests
there); a ticker that cannot be opened (the stop then sends SIGKILL only);
a negative `stop_within` reaching native code (`Exec.stop_within` rejects it
first); and a SIGTERM sent before an immediate SIGKILL when `stop_within` is
zero, which no test can see, since the SIGKILL lands before any handler;
a stop whose wait is cancelled (since PR 97 a stop signal cancels main
like any task, so only a task ended outright, such as a process body forced
at its shutdown deadline, gets there; the binding contract's rule is kept);
and the SIGKILL phase's parked wait (without it the release's blocking reap
waits the moment a killed child takes to go).

Run: scripts/mutation/mutate.py scripts/mutation/exec_stop_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

EXEC = "../../runtime/chimi-runtime/src/binding/exec.rs"
MODULE = "std/exec.chimi"


def exec_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_exec"])


MUTATIONS = [
    exec_test('no SIGTERM first', EXEC, [
        ('            unsafe { libc::kill(pid, libc::SIGTERM) };\n            waited = await_exit_until',
         '            waited = await_exit_until')]),
    exec_test('no SIGKILL after the time', EXEC, [
        ('            unsafe { libc::kill(pid, libc::SIGKILL) };\n            if waited == StopWait::Cancelled {',
         '            if waited == StopWait::Cancelled {')]),
    exec_test('the deadline is ignored', EXEC, [
        ('        if monotonic_ms() >= deadline {\n            return StopWait::Deadline;\n        }\n',
         '')]),
    exec_test('the stop blocks the scheduler', EXEC, [
        ('        if call.wait(None, ticker, Interest::Readable) != Waited::Ready {\n            return StopWait::Cancelled;\n        }',
         '        let _ = &call;\n        unsafe { libc::usleep((STOP_TICK_MS * 1000) as _) };')]),
    exec_test("the start does not stop its child", MODULE, [
        ("      native_stop(process, Duration.to_milliseconds(command.stop_within))\n", "")]),
    exec_test("the pipes close after the stop", MODULE, [
        ("      close_pipe(input_pipe)\n      close_pipe(output_pipe)\n      close_pipe(errors_pipe)\n      native_stop(process, Duration.to_milliseconds(command.stop_within))\n      native_release(process)\n",
         "      native_stop(process, Duration.to_milliseconds(command.stop_within))\n      native_release(process)\n      close_pipe(input_pipe)\n      close_pipe(output_pipe)\n      close_pipe(errors_pipe)\n")]),
    exec_test("stop_within is not kept", MODULE, [
        ("    stop_within: duration,\n", "    stop_within: command.stop_within,\n")]),
    exec_test("a negative stop_within is kept", MODULE, [
        ("  case duration < Duration.zero {", "  case duration < Duration.milliseconds(-1000000) {")]),
    exec_test('the backstop leaves the child', EXEC, [
        ('            unsafe { libc::kill(self.pid, libc::SIGKILL) };\n            while',
         '            while')]),
]
