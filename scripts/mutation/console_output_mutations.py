"""CHI-266: console writes wait in the IO reactor instead of blocking every task.

The queue itself lives in runtime/chimi-runtime/src/console.rs since CHI-308;
rust_console_mutations.py holds the mutants its unit tests kill. These keep
the end-to-end ones.

Run: scripts/mutation/mutate.py scripts/mutation/console_output_mutations.py
"""

CRATE = "compiler/chimi"

IO = "../../runtime/chimi-runtime/src/intrinsics/console_io.rs"
CONSOLE = "../../runtime/chimi-runtime/src/console.rs"

MUTATIONS = [
    dict(
        name="every stream writes directly, blocking the scheduler",
        file=IO,
        edits=[("    stream.mode = if queued { Mode::Queued } else { Mode::Direct };", "    stream.mode = Mode::Direct;")],
        targets=["console_output"],
    ),
    dict(
        name="a write after poll takes the whole queue, not PIPE_BUF",
        file=IO,
        edits=[("!write_front(console, index, libc::PIPE_BUF)", "!write_front(console, index, usize::MAX)")],
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
        file=IO,
        edits=[("            LineState::Pending => {\n                if let Waited::Cancelled", "            LineState::Pending => {\n                break 0;\n                #[allow(unreachable_code)]\n                if let Waited::Cancelled")],
        targets=["console_output"],
    ),
    dict(
        name="a written count does not advance the queue",
        file=IO,
        edits=[("            queue.advance(count);\n", "            let _ = count;\n")],
        targets=["console_output"],
    ),
    dict(
        name="the end of the program drops lines still queued",
        file=IO,
        edits=[("pub extern \"C\" fn chimi_runtime_console_drain_v1() {\n", "pub extern \"C\" fn chimi_runtime_console_drain_v1() {\n    if true {\n        return;\n    }\n")],
        targets=["console_output"],
    ),
    dict(
        name="a writer waits for readable instead of writable",
        file=IO,
        edits=[("    request.token = Scheduler::IO_WRITABLE;", "    request.token = 1;")],
        targets=["console_output"],
    ),
    dict(
        name="standard error keeps its own queue when it names standard output's file",
        file=IO,
        edits=[("                console.streams[index].queue = console.streams[earlier].queue;\n", "")],
        targets=["console_output"],
    ),
    dict(
        name="a failure also fails lines already written",
        file=CONSOLE,
        edits=[("            if line.end > self.resolved {", "            if true {")],
        targets=["console_output"],
    ),
    dict(
        name="a direct line loses its newline",
        file=IO,
        edits=[("    let (mut body, mut tail): (&[u8], &[u8]) = (text, &newline);", "    let (mut body, mut tail): (&[u8], &[u8]) = (text, &newline[..0]);")],
        targets=["console_output"],
    ),
    dict(
        name="a failed direct write reports success",
        file=IO,
        edits=[("                error = if failed == 0 { libc::EIO } else { failed };", "                error = 0;")],
        targets=["console_output"],
    ),
    dict(
        name="a stop signal does not end a write a full pipe holds",
        file=IO,
        edits=[("        if (!body.is_empty() || !tail.is_empty()) && stopped() {", "        if (!body.is_empty() || !tail.is_empty()) && false {")],
        targets=["console_output"],
    ),
]
