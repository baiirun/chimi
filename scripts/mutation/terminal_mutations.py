"""Mutants of std/terminal (CHI-251): the escape codes `style` writes and
`strip` removes, the `uses_color` rule, and the runner's provider.

Not covered by a test: the code -> descriptor mapping's out-of-range defect
(`stream_code` gives only 0 to 2).

Run: scripts/mutation/mutate.py scripts/mutation/terminal_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

MODULE = "std/terminal.chimi"
BINDING = "../../runtime/chimi-runtime/src/binding/terminal.rs"
NATIVE = "src/backend/native.rs"


def terminal_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_terminal"])


MUTATIONS = [
    terminal_test("a blanket reset closes", MODULE, [
        ('"${sequence(opens)}${text}${sequence(closes)}"', '"${sequence(opens)}${text}\\u{1b}[0m"')]),
    terminal_test("shared closes repeat", MODULE, [
        ("List.map(List.unique(List.map(styles, close_code)), Int.to_text)", "List.map(List.map(styles, close_code), Int.to_text)")]),
    terminal_test("empty text is styled", MODULE, [
        ("case List.is_empty(styles) or Text.is_empty(text) {", "case List.is_empty(styles) {")]),
    terminal_test("bright colors use the normal range", MODULE, [
        ("Style.Foreground(color) -> color_code(color, 30, 90)", "Style.Foreground(color) -> color_code(color, 30, 30)")]),
    terminal_test("background uses the foreground range", MODULE, [
        ("Style.Background(color) -> color_code(color, 40, 100)", "Style.Background(color) -> color_code(color, 30, 90)")]),
    terminal_test("inverse has the wrong code", MODULE, [
        ("    Style.Inverse -> 7\n", "    Style.Inverse -> 8\n")]),
    terminal_test("strip keeps the final byte", MODULE, [
        ("Text.byte_slice(part, at + 1..Text.byte_length(part))", "Text.byte_slice(part, at..Text.byte_length(part))")]),
    terminal_test("strip drops an incomplete escape", MODULE, [
        ('    _ -> "\\u{1b}[${part}"\n', "    _ -> part\n")]),
    terminal_test("strip takes any byte as final", MODULE, [
        ("    Some(byte) if byte >= 0x40 and byte <= 0x7E -> Text.byte_slice", "    Some(byte) -> Text.byte_slice")]),
    terminal_test("NO_COLOR is ignored", MODULE, [
        ("  Text.is_empty(no_color) and Env.get", "  True and Env.get")]),
    terminal_test("an empty NO_COLOR disables", MODULE, [
        ('let no_color = Option.unwrap_or(Env.get("NO_COLOR"), "")\n  Text.is_empty(no_color)', 'let no_color = Env.get("NO_COLOR")\n  no_color == None')]),
    terminal_test("TERM=dumb is ignored", MODULE, [
        (' and Env.get("TERM") != Some("dumb")', "")]),
    terminal_test("piped output is colored", MODULE, [
        (" and terminal.is_terminal(stream)\n", "\n")]),
    terminal_test("the provider asks stdout for every stream", MODULE, [
        ("    Standard.Error -> 2\n", "    Standard.Error -> 1\n")]),
    terminal_test("columns and rows swap", BINDING, [
        ("call.make_integer(i64::from(columns)),\n                call.make_integer(i64::from(rows)),",
         "call.make_integer(i64::from(rows)),\n                call.make_integer(i64::from(columns)),")]),
    terminal_test("a pipe has a size", BINDING, [
        ("    if !is_terminal(fd) {\n        return None;\n    }\n", "")]),
    terminal_test("zero columns is a size", BINDING, [
        ("(size.ws_col != 0 && size.ws_row != 0)", "(size.ws_row != 0)")]),
    terminal_test("zero rows is a size", BINDING, [
        ("(size.ws_col != 0 && size.ws_row != 0)", "(size.ws_col != 0)")]),
    terminal_test("every stream is a terminal", BINDING, [
        ("call.make_boolean(is_terminal(fd))", "call.make_boolean(true)")]),
    terminal_test("the stream code is off by one", BINDING, [
        (".map(|code| code as c_int)", ".map(|code| ((code + 1) % 3) as c_int)")]),
    terminal_test("the link keeps every Rust binding", NATIVE, [
        ('        "-Wl,-dead_strip"\n    } else {\n        "-Wl,--gc-sections"\n    });', '        "-Wl,-v"\n    } else {\n        "-Wl,-v"\n    });')]),
]
