"""Mutants of the std/cli follow-ups (CHI-286): subcommands, switches and
repeated options. The original parser's mutants are in cli_mutations.py.

Run: scripts/mutation/mutate.py scripts/mutation/cli_followup_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

MODULE = "std/cli.chimi"


def parsing(name, edits):
    return dict(name=name, file=MODULE, edits=edits, targets=["std_modules"],
                filter="standard_tests_pass_on_both_backends")


MUTATIONS = [
    parsing("a switch ignores its default", [
        ("        None -> default\n", "        None -> False\n")]),
    parsing("--no-long turns a switch on", [
        ('      Some(found) -> (found.spec, "false")', '      Some(found) -> (found.spec, "true")')]),
    parsing("a switch is on once given at all", [
        ('        Some(value) -> value == "true"\n', "        Some(_) -> True\n")]),
    parsing("any flag has a --no- form", [
        ("        Kind.Switch -> each.spec.name == switch\n        _ -> False\n",
         "        _ -> each.spec.name == switch\n")]),
    parsing("--no-long is not one of a switch's names", [
        ('    Kind.Switch -> ["--${spec.name}", "--no-${spec.name}"]\n', "")]),
    parsing("usage shows a switch as a flag", [
        ('    Kind.Switch -> "--[no-]${spec.name}"\n', "")]),
    parsing("usage shows a repeated option as an option", [
        ('    Kind.Repeated -> "${names} <${spec.value}>..."\n', "")]),
    parsing("a repeated option keeps only its last value", [
        ("pub fn repeated<T>(long: Text, short: Text, help: Text, parser: { name: Text, read: fn(Text) -> Option<T> }) -> Handle<List<T>> {\n  let spec = named(Kind.Repeated, long, short, help, parser.name)\n  Handle {\n    spec: spec,\n    read: fn(values) {\n      List.map(values, fn(value) { convert(spec, parser, value) })",
         "pub fn repeated<T>(long: Text, short: Text, help: Text, parser: { name: Text, read: fn(Text) -> Option<T> }) -> Handle<List<T>> {\n  let spec = named(Kind.Repeated, long, short, help, parser.name)\n  Handle {\n    spec: spec,\n    read: fn(values) {\n      List.map(List.drop(values, List.length(values) - 1), fn(value) { convert(spec, parser, value) })")]),
    parsing("a subcommand given no words is not chosen", [
        ("      values: Map.put(state.values, word, List.append([word], rest)),",
         "      values: Map.put(state.values, word, rest),")]),
    parsing("a subcommand's help or error is lost", [
        ("        Err(exit) -> Failure(Stop.Ended(exit))", "        Err(_) -> Failure(Stop.Wrong(CliError.MissingCommand))")]),
    parsing("a subcommand is not named after its parent", [
        ('        name: "${command.name} ${sub.name}",', "        name: sub.name,")]),
    parsing("no subcommand is fine", [
        ("    True -> Failure(Stop.Wrong(CliError.MissingCommand))\n    False -> Parsed { values: values }",
         "    _ -> Parsed { values: values }")]),
    parsing("-- hides the subcommand name", [
        ("      ([\"--\", word, ..rest], True) -> break enter(command, state, word, rest)\n", "")]),
    parsing("the parent looks for help past the subcommand", [
        ("    [word, ..rest] -> case Option.is_some(chosen(command, word)) {", "    [word, ..rest] -> case False {")]),
    parsing("positionals and subcommands may mix", [
        ("  let mixed = List.any(args,", "  let mixed = False and List.any(args,")]),
    parsing("usage lists no subcommands", [
        ('section("Commands", command_rows)', "[]")]),
    parsing("the synopsis has no command", [
        ('    True -> ["<command>", "[<args>...]"]', "    True -> []")]),
    parsing("an unknown subcommand is missing", [
        ("    None -> Failure(Stop.Wrong(CliError.UnknownCommand(word)))", "    None -> Failure(Stop.Wrong(CliError.MissingCommand))")]),
]

MUTATIONS += [
    parsing("the subcommand is parsed before the parent's values are checked", [
        ("    Some(sub) -> {\n      check(command, state.values)\n", "    Some(sub) -> {\n")]),
    parsing("an option's value is searched for help", [
        ("      (False, True) -> asked_to_show(command, List.drop(rest, 1))\n", "")]),
    parsing("a cluster's value option does not take the next word", [
        ("        False -> List.is_empty(more)\n", "        False -> False\n")]),
]
