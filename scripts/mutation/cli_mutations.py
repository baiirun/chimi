"""Mutants of std/cli (CHI-252): how arguments are parsed, the declaration
checks, and how the runner applies `Exit` on both backends.

Run: scripts/mutation/mutate.py scripts/mutation/cli_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

MODULE = "std/cli.chimi"
RUNTIME = "runtime/native/runtime.c"
ORACLE = "src/execution.rs"
DRIVER = "src/main.rs"
LOWERING = "src/executable.rs"


def parsing(name, edits):
    return dict(name=name, file=MODULE, edits=edits, targets=["std_modules"],
                filter="standard_tests_pass_on_both_backends")


def runner(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_cli"])


MUTATIONS = [
    parsing("the first repeat wins", [
        ("    case List.last(values) {", "    case List.first(values) {")]),
    parsing("-- is an ordinary word", [
        ('      ["--", ..rest] -> break Scan([], state.values, List.append(state.positionals, rest))\n', "")]),
    parsing("help after an error is ignored", [
        ('      ["--help", .._] -> Failure(Stop.Shown(usage(command)))\n', "")]),
    parsing("a negative number is an option", [
        ('    ["-", second, .._] -> not List.contains(["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "."], second)',
         '    ["-", second, .._] -> True')]),
    parsing("a flag takes a value", [
        ('    (Kind.Flag, Some(value), _) -> Failure(Stop.Wrong(CliError.UnexpectedValue("--${name}", value)))\n', "")]),
    parsing("a cluster stops at its first flag", [
        ('(Kind.Flag, _, _) -> scan_cluster(command, Scan(rest, add(state.values, spec, "true"), state.positionals), more, rest)',
         '(Kind.Flag, _, _) -> Scan(rest, add(state.values, spec, "true"), state.positionals)')]),
    parsing("an attached short value loses its first letter", [
        ("(_, _, _) -> Scan(rest, add(state.values, spec, Text.concat(more)), state.positionals)",
         "(_, _, _) -> Scan(rest, add(state.values, spec, Text.concat(List.drop(more, 1))), state.positionals)")]),
    parsing("extra positionals are dropped", [
        ("    [extra, .._] -> Failure(Stop.Wrong(CliError.UnexpectedArgument(extra)))", "    [_, .._] -> placed")]),
    parsing("rest takes only one", [
        ("      (Kind.Rest, _) -> (Map.put(placed, display(each.spec), left), [])",
         "      (Kind.Rest, [first, ..others]) -> (Map.put(placed, display(each.spec), [first]), others)\n      (Kind.Rest, []) -> state")]),
    parsing("values are keyed by bare name", [
        ("fn display(spec: Spec) -> Text {\n  case is_positional(spec) {\n    True -> \"<${spec.name}>\"\n    False -> \"--${spec.name}\"\n  }",
         "fn display(spec: Spec) -> Text {\n  case is_positional(spec) {\n    True -> spec.name\n    False -> spec.name\n  }")]),
    parsing("a rest may come first", [
        ("    (True, False) -> Debug.crash(\"Cli.command: ${name} declares an argument after its rest\")\n    (True, True) -> Command(name, summary, args, None)",
         "    (True, _) -> Command(name, summary, args, None)")]),
    parsing("help may be declared", [
        ("  case (unique and not declares_help, rest_is_last(args)) {", "  case (unique, rest_is_last(args)) {")]),
    parsing("version may be declared twice", [
        ('List.contains(option_names(each.spec), "--version")', "False")]),
    parsing("help after an error is a usage error", [
        ("    Err(Stop.Wrong(_)) if asks_help(arguments) -> Failure(Exit.Help(usage(command)))\n", "")]),
    parsing("h in a cluster is unknown", [
        ('          "h" -> Failure(Stop.Shown(usage(command)))\n', "")]),
    parsing("help after -- is shown", [
        ('    ["--", .._] -> False\n', "")]),
    parsing("the built-in --version shadows a declared one", [
        ('      ["--help", .._] -> Failure(Stop.Shown(usage(command)))\n',
         '      ["--help", .._] -> Failure(Stop.Shown(usage(command)))\n      ["--version", .._] -> Failure(Stop.Wrong(CliError.UnknownOption("--version")))\n')]),
    parsing("a digit is a short name", [
        ("[byte] -> (byte >= 0x41 and byte <= 0x5A) or (byte >= 0x61 and byte <= 0x7A)", "[_] -> True")]),
    parsing("usage leaves out the pointer to --help", [
        ("\\nTry '${command.name} --help' for more information.", "")]),
    parsing("an unparsable value is accepted as absent", [
        ("      Some(value) -> Some(convert(spec, parser, value))", "      Some(value) -> parser.read(value)")]),
    runner("native Help goes to standard error", RUNTIME, [
        ("  FILE *stream = help ? stdout : stderr;", "  FILE *stream = stderr;")]),
    runner("native Usage exits 1", RUNTIME, [
        ("  *status = written ? (help ? 0 : 2) : 74;", "  *status = written ? (help ? 0 : 1) : 74;")]),
    runner("native codes wrap", RUNTIME, [
        ("    if (field->as.integer < 0 || field->as.integer > 255) {", "    if (0) {")]),
    runner("native Exit is ignored on failure", RUNTIME, [
        ("  if (control->kind != CONTROL_SUCCESS && control->kind != CONTROL_FAILURE) return false;",
         "  if (control->kind != CONTROL_SUCCESS) return false;")]),
    runner("native Exit is ignored", RUNTIME, [
        ("  int exit_status;\n  if (program_exit(control, &exit_status)) return exit_status;\n", "")]),
    runner("interpreted Exit is ignored", DRIVER, [
        ("        None => outcome_exit_code(&report.outcome),\n", "        _ => outcome_exit_code(&report.outcome),\n")]),
    runner("interpreted Usage goes to standard output", DRIVER, [
        ("Some(ProgramExit::Usage(message)) => shown(&mut std::io::stderr(), message, 2),",
         "Some(ProgramExit::Usage(message)) => shown(&mut std::io::stdout(), message, 2),")]),
    runner("interpreted Exit is ignored on failure", ORACLE, [
        ("        let (Control::Success(value) | Control::Failure(value, _)) = control else {",
         "        let Control::Success(value) = control else {")]),
    runner("any type named Exit ends the program", LOWERING, [
        ('        program_exit: declaration.name.text == "Exit"\n            && typed.resolved.semantic.modules[module_id.raw() as usize].path == "std/cli",',
         '        program_exit: declaration.name.text == "Exit",')]),
]
