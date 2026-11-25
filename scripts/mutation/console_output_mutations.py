"""CHI-266: console writes wait in the IO reactor instead of blocking every task.

The queue itself lives in runtime/chimi-runtime/src/console.rs since CHI-308;
rust_console_mutations.py holds the mutants its unit tests kill. These keep
the end-to-end ones.

Run: scripts/mutation/mutate.py scripts/mutation/console_output_mutations.py
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
CONSOLE = "../../runtime/chimi-runtime/src/console.rs"

MUTATIONS = [
    dict(
        name="every stream writes directly, blocking the scheduler",
        file=RUNTIME,
        edits=[("    ? CONSOLE_QUEUED : CONSOLE_DIRECT;", "    ? CONSOLE_DIRECT : CONSOLE_DIRECT;")],
        targets=["console_output"],
    ),
    dict(
        name="a write after poll takes the whole queue, not PIPE_BUF",
        file=RUNTIME,
        edits=[("!console_write_front(stream, PIPE_BUF)", "!console_write_front(stream, SIZE_MAX)")],
        targets=["console_output"],
    ),
    dict(
        name="a writer returns once its line is queued",
        file=CONSOLE,
        edits=[("            Some(line) if self.resolved < line.end => LineState::Pending,\n", "")],
        targets=["console_output"],
    ),
    dict(
        name="a writer does not wait for its line",
        file=RUNTIME,
        edits=[("  while ((outcome = chimi_console_line_state_v1(queue, line)) < 0)", "  while ((outcome = chimi_console_line_state_v1(queue, line)) < 0 && false)")],
        targets=["console_output"],
    ),
    dict(
        name="a written count does not advance the queue",
        file=RUNTIME,
        edits=[("  chimi_console_advance_v1(stream->queue, (size_t)count);\n", "")],
        targets=["console_output"],
    ),
    dict(
        name="the end of the program drops lines still queued",
        file=RUNTIME,
        edits=[
            ("static void console_drain_all(void) {\n", "static void console_drain_all(void) {\n  return;\n"),
        ],
        targets=["console_output"],
    ),
    dict(
        name="a writer waits for readable instead of writable",
        file=RUNTIME,
        edits=[(".value = stream->fd, .token = CHIMI_IO_WRITABLE}", ".value = stream->fd, .token = CHIMI_IO_READABLE}")],
        targets=["console_output"],
    ),
    dict(
        name="standard error keeps its own queue when it names standard output's file",
        file=RUNTIME,
        edits=[("      stream->queue = other_stream->queue;\n", "      (void)other_stream;\n")],
        targets=["console_output"],
    ),
    dict(
        name="a failure also fails lines already written",
        file=CONSOLE,
        edits=[("            if line.end > self.resolved {", "            if true {")],
        targets=["console_output"],
    ),
    dict(
        name="the streams are not listed as shared runtime state",
        file="tests/runtime_state.rs",
        edits=[('    "console_streams",\n', "")],
        targets=["runtime_state"],
    ),
]
