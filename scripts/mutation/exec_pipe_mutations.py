"""Mutants of std/exec part 3 (CHI-250): pipes (the binding in Rust since CHI-309) to a child's standard
streams, `Exec.capture`, and fakes with `Exec.piped`.

Not covered by a test: the macOS F_SETNOSIGPIPE branch (CI's Linux runs take
the blocked-signal branch; macOS runs the same tests on the other one), and a
pipe handle that cannot be made (needs 2^20 open handles).

Run: scripts/mutation/mutate.py scripts/mutation/exec_pipe_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

EXEC = "../../runtime/chimi-runtime/src/binding/exec.rs"
MODULE = "std/exec.chimi"


def exec_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_exec"])


MUTATIONS = [
    exec_test('pipe ends reach other children', EXEC, [
        ('libc::fcntl(fd.0, libc::F_DUPFD_CLOEXEC, libc::STDERR_FILENO + 1)',
         'libc::fcntl(fd.0, libc::F_DUPFD, libc::STDERR_FILENO + 1)')]),
    exec_test("the parent keeps the child's ends", EXEC, [
        ('        drop(child);\n        parent',
         '        core::mem::forget(child);\n        parent')]),
    exec_test("the parent's end blocks", EXEC, [
        ('flags | libc::O_NONBLOCK) } != 0',
         'flags) } != 0')]),
    exec_test('a write to a gone reader raises SIGPIPE', EXEC, [
        ('libc::pthread_sigmask(libc::SIG_BLOCK, &pipe_signal, &mut previous);',
         'libc::pthread_sigmask(libc::SIG_BLOCK, core::ptr::null(), &mut previous);')]),
    exec_test('a broken pipe is Failed', EXEC, [
        ('return os_failure(call, code::BROKEN_PIPE, libc::EPIPE);',
         'return os_failure(call, code::FAILED, libc::EPIPE);')]),
    exec_test('the end of output is a failure', EXEC, [
        ('            if count == 0 {\n                let none = call.none();\n                return call.ok(none);\n            }',
         '            if count == 0 {\n                return call.failure(code::FAILED, c"eof");\n            }')]),
    exec_test('stdin and stdout pipes swap ends', EXEC, [
        ('    let ends = if target == libc::STDIN_FILENO {',
         '    let ends = if false {')]),
    exec_test("a fake read after a stop is Failed", MODULE, [
        ("    StreamError.Aborted -> ExecError.Closed(program)\n", "")]),
    exec_test("release leaves the pipes open", MODULE, [
        ("      close_pipe(input_pipe)\n      close_pipe(output_pipe)\n      close_pipe(errors_pipe)\n", "")]),
    exec_test("capture inherits stdin", MODULE, [
        ("stdin(command, Stdio.Null)", "command")]),
    exec_test("capture swaps its streams", MODULE, [
        ("output: Task.result(written), error_output: Task.result(failed)", "output: Task.result(failed), error_output: Task.result(written)")]),
    exec_test("capture reads output to its end first", MODULE, [
        ("    let written = read_in_task(output(child))\n", "    let written_now = everything(output(child))\n    let written = Task.start(fn() { written_now })\n")]),
    exec_test("a fake input's Closed reads as Closed", MODULE, [
        ("Failure(fake_failure(error, program, ExecError.BrokenPipe(program)))", "Failure(fake_failure(error, program, ExecError.Closed(program)))")]),
    exec_test("a fake's error output is its output", MODULE, [
        ("    error_output: Some(fake_output(error_output)),", "    error_output: Some(fake_output(output)),")]),
]
