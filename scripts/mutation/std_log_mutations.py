"""Mutants of std/log (Phase 7c, CHI-289): the level functions, the runner's
provider in std source, the interpreter's host core and the native runtime's.

Run: scripts/mutation/mutate.py scripts/mutation/std_log_mutations.py
"""

CRATE = "compiler/chimi"

S = "std/log.chimi"
H = "src/execution/host_operations.rs"
E = "src/execution.rs"
C = "runtime/native/runtime.c"


def mutant(name, file, edits, filter="", targets=("std_log",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("warn builds an info entry", S, [(
        "  log.write(Entry(Level.Warn, message, fields))",
        "  log.write(Entry(Level.Info, message, fields))",
    )]),
    mutant("fields come back reversed", S, [(
        "  List.map(entry.fields, fn(field) { (field.key, field.value) })",
        "  List.reverse(List.map(entry.fields, fn(field) { (field.key, field.value) }))",
    )]),
    mutant("the provider keeps every level", S, [(
        "      case level >= minimum {",
        "      case True {",
    )]),
    mutant("the provider keeps only levels above the minimum", S, [(
        "      case level >= minimum {",
        "      case level > minimum {",
    )]),
    mutant("a float field is written as text", S, [(
        "    Success(number) -> number\n",
        "    Success(_) -> Json.text(Float.to_text(value))\n",
    )]),
    mutant("the interpreter ignores the log file", H, [(
        "                match &self.config.log_file {",
        "                match &None::<String> {",
    )]),
    mutant("the interpreter truncates the log file", H, [(
        "                            .append(true)",
        "                            .write(true)",
    )]),
    mutant("the interpreter's info level is debug", E, [(
        '            "info" => 1,',
        '            "info" => 0,',
    )]),
    mutant("the native runner ignores --log-level", C, [(
        '      if (strcmp(level, "debug") == 0) runtime.log_level = 0;',
        '      if (strcmp(level, "debug") == 0) runtime.log_level = 1;',
    )]),
    mutant("the native log file is truncated", C, [(
        '    runtime.log_file = open_report(log_path, "a");',
        '    runtime.log_file = open_report(log_path, "w");',
    )]),
    mutant("the native log file may be the outcome file", C, [(
        "                   (outcome_path && strcmp(log_path, outcome_path) == 0) ||",
        "                   false ||",
    )]),
    mutant("the native runner writes log lines to stdout", C, [(
        "    written = console_write_line(runtime, &console_streams[1], line, length);",
        "    written = console_write_line(runtime, &console_streams[0], line, length);",
    )]),
]
