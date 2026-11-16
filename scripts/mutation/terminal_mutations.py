"""Mutants of std/terminal (CHI-251): the escape codes `style` writes and
`strip` removes, the `uses_color` rule, and the runner's provider.

Not covered by a test: the code -> descriptor mapping's out-of-range defect
(`stream_code` gives only 0 to 2).

Run: scripts/mutation/mutate.py scripts/mutation/terminal_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

MODULE = "std/terminal.chimi"
BINDING = "runtime/native/bindings/terminal/terminal.c"


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
        ("chimi_binding_make_integer(call, size.ws_col),\n                                 chimi_binding_make_integer(call, size.ws_row)",
         "chimi_binding_make_integer(call, size.ws_row),\n                                 chimi_binding_make_integer(call, size.ws_col)")]),
    terminal_test("a pipe has a size", BINDING, [
        ("  if (!isatty(fd) || ioctl(fd, TIOCGWINSZ, &size) != 0 || size.ws_col == 0 || size.ws_row == 0)\n    return chimi_binding_success(call, chimi_binding_none(call));",
         "  if (ioctl(fd, TIOCGWINSZ, &size) != 0) { size.ws_col = 80; size.ws_row = 24; }")]),
    terminal_test("zero columns is a size", BINDING, [
        (" || size.ws_col == 0 || size.ws_row == 0)", " || size.ws_row == 0)")]),
    terminal_test("zero rows is a size", BINDING, [
        (" || size.ws_col == 0 || size.ws_row == 0)", " || size.ws_col == 0)")]),
    terminal_test("every stream is a terminal", BINDING, [
        ("chimi_binding_make_boolean(call, isatty(fd) == 1)", "chimi_binding_make_boolean(call, 1)")]),
]
