"""Mutants of std/exec part 2 (CHI-250): the exec binding package (in Rust since CHI-309), the
runtime's child-wait API, and the Chimi wrapper.

Not listed, as equivalent: the NUL checks on program, argument and variable text (the defect message is
the only difference a test can see, and Chimi text rarely holds U+0000), and
the non-scheduler branch of `chimi_binding_wait_child` (a binding call always
forces the scheduler).

Not covered by a test: a directory without search permission (`access` in
the spawn's directory check; tests run as root in CI, where it always
passes), a child stopped when no handle can be made for it (needs 2^20
open handles), and the report files closed in children (FD_CLOEXEC in
`open_report`; a child holding one open changes nothing a test can see
before the runner exits).

Run: scripts/mutation/mutate.py scripts/mutation/exec_binding_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

EXEC = "../../runtime/chimi-runtime/src/binding/exec.rs"
RUNTIME = "runtime/native/runtime.c"
MODULE = "std/exec.chimi"


def exec_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_exec"])


MUTATIONS = [
    exec_test('a released child is not killed', EXEC, [
        ('            unsafe { libc::kill(self.pid, libc::SIGKILL) };\n            while',
         '            while')]),
    exec_test('a released child is not reaped', EXEC, [
        ('            while unsafe { libc::waitpid(self.pid, &mut status, 0) } < 0 && errno() == libc::EINTR {}\n',
         '            let _ = &mut status;\n')]),
    exec_test('a second wait asks the kernel again', EXEC, [
        ('        if child.reaped.get() {\n            let status = child.status.get();\n            return status_result(call, status);\n        }\n',
         '')]),
    exec_test('a signal death reads as an exit', EXEC, [
        ('if libc::WIFSIGNALED(status) {',
         'if false {')]),
    exec_test('a name is not looked up on PATH', EXEC, [
        ("    if name.contains(&b'/') {",
         '    if true {')]),
    exec_test('a directory found on PATH counts as the program', EXEC, [
        ('    if info.st_mode & libc::S_IFMT == libc::S_IFDIR\n        || unsafe',
         '    if unsafe')]),
    exec_test("a replaced environment keeps the parent's", EXEC, [
        ('    if !builder.clear_environment {',
         '    if true {')]),
    exec_test('the first value of a variable wins', EXEC, [
        ('        if !later {\n',
         '        if !later || later {\n')]),
    exec_test('the working directory is ignored', EXEC, [
        ('    if error == 0\n        && let Some(directory) = &builder.directory\n',
         '    if false\n        && let Some(directory) = &builder.directory\n')]),
    exec_test('a bad directory is found only by the child', EXEC, [
        ('        && !CStr::from_bytes_with_nul(directory).is_ok_and(enterable)',
         '        && false')]),
    exec_test('Stdio.Null is inherited', EXEC, [
        ('            unsafe { libc::posix_spawn_file_actions_addopen(actions, fd, c"/dev/null".as_ptr(), flags, 0) == 0 }',
         '            let _ = flags;\n            true')]),
    exec_test('a relative program stays relative', EXEC, [
        ("    if path.first() == Some(&b'/') {",
         '    if true {')]),
    exec_test('a file the kernel cannot run is a permission', EXEC, [
        ('        libc::EACCES => code::PERMISSION_DENIED,\n        _ => code::FAILED,',
         '        libc::EACCES | libc::ENOEXEC => code::PERMISSION_DENIED,\n        _ => code::FAILED,')]),
    exec_test('terminate sends SIGKILL', EXEC, [
        ('chimi_exec_terminate => |call| signal(call, libc::SIGTERM)',
         'chimi_exec_terminate => |call| signal(call, libc::SIGKILL)')]),
    exec_test('a cancelled wait is not cancelled', EXEC, [
        ('            Waited::Cancelled => return call.cancelled(),\n            Waited::Closed => return child_closed(call),',
         '            Waited::Cancelled => {}\n            Waited::Closed => return child_closed(call),')]),
    exec_test("a child wait does not wait", RUNTIME, [
        ("    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_CHILD_WAIT, .value = pid, .channel = resource};\n    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return CHIMI_BINDING_WAIT_CANCELLED;\n",
         "    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_CHILD_WAIT, .value = pid, .channel = resource, .status = CHIMI_RESPONSE_OK};\n")]),
    exec_test("the child's signal mask is inherited", EXEC, [
        ('            0 => libc::posix_spawnattr_setflags(&mut spawn.attributes, SPAWN_FLAGS as _),',
         '            0 => 0,')]),
    exec_test('Stdio.Null stdin is opened for writing', EXEC, [
        ('let flags = if fd == libc::STDIN_FILENO { libc::O_RDONLY } else { libc::O_WRONLY };',
         'let flags = libc::O_WRONLY;')]),
    exec_test("stdin is never connected", MODULE, [
        ("stdio_code(command.stdin), stdio_code(command.stdout)", "stdio_code(Stdio.Inherit), stdio_code(command.stdout)")]),
    exec_test('a second wait after a cancelled one asks the dead pid', EXEC, [
        ('        self.reaped.set(true);\n',
         '')]),
    exec_test("no SIGCHLD handler", RUNTIME, [
        ("  action.sa_handler = on_child_signal;\n  sigemptyset(&action.sa_mask);\n  action.sa_flags = SA_RESTART | SA_NOCLDSTOP;\n  sigaction(SIGCHLD, &action, NULL);\n", "")]),
    exec_test("start does not release the child", MODULE, [
        ("fn(child) { child.release() }", "fn(_child) { Void }")]),
    exec_test("stdout and stderr swap", MODULE, [
        ("stdio_code(command.stdout), stdio_code(command.stderr)", "stdio_code(command.stderr), stdio_code(command.stdout)")]),
    exec_test("the working directory buffer never grows", EXEC, [
        ('let wanted = directory.capacity().saturating_mul(2);',
         'let wanted = directory.capacity();')]),
]
