"""Mutants of the Phase 6c system modules, part 1 (CHI-246, CHI-247): the
runner-provided capability mechanism, std/path, and std/env on both backends.

Run: scripts/mutation/mutate.py scripts/mutation/system_modules_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

LOWERING = "src/executable.rs"
TYPING = "src/typing.rs"
OPERATIONS = "src/operations.rs"
HOST = "src/execution/host_operations.rs"
RUNTIME = "runtime/native/runtime.c"
PATH = "std/path.chimi"
ENV = "std/env.chimi"


def host(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["host_modules"])


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


MUTATIONS = [
    # The mechanism.
    host("the runner closes any need name", OPERATIONS, [
        (".find(|provider| provider.interface == interface && provider.need == need)",
         ".find(|provider| provider.interface == interface)")]),
    host("main's body is not given the provider", LOWERING, [
        ("            body = lowerer.provide_runner_capabilities(body, root.entry);\n", "")]),
    host("the test plan closes the environment", TYPING, [
        ("            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == \"clock\",\n            _ => false,\n        }\n    }\n\n    /// Whether the runner's",
         "            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == \"clock\",\n            Some(StandardSymbol::Type(StandardType::Environment)) => true,\n            _ => false,\n        }\n    }\n\n    /// Whether the runner's")]),
    # std/env on each backend.
    host("the interpreter keeps non-UTF-8 pairs as empty", HOST, [
        ("Some((name.into_string().ok()?, value.into_string().ok()?))",
         "Some((name.into_string().ok()?, value.into_string().unwrap_or_default()))")]),
    host("native variables keep non-UTF-8 values", RUNTIME, [
        ("            !valid_utf8((const unsigned char *)equals + 1, value_length))",
         "            false)")]),
    host("native names split at a leading =", RUNTIME, [
        ("const char *equals = **entry ? strchr(*entry + 1, '=') : NULL;",
         "const char *equals = strchr(*entry, '=');")]),
    host("native variables keep the separator", RUNTIME, [
        ("(RuntimeField){NULL, text_from_bytes(equals + 1, value_length)};",
         "(RuntimeField){NULL, text_from_bytes(equals, value_length + 1)};")]),
    host("native working directory is never known", RUNTIME, [
        ("        if (getcwd(buffer, capacity)) {", "        if (false && getcwd(buffer, capacity)) {")]),
    host("the last value of a name wins", ENV, [
        ("List.reduce(List.reverse(environment.variables()), Map.empty(), fn(variables, variable) {",
         "List.reduce(environment.variables(), Map.empty(), fn(variables, variable) {")]),
    host("get compares names ignoring case", ENV, [
        ("    case key == name {", "    case Text.lowercase(key) == Text.lowercase(name) {"),
        ("import std/list as List\n", "import std/list as List\nimport std/text as Text\n")]),
    # std/path.
    programs("join lets an absolute part replace the base", PATH, [
        ("  let tail = trim_leading_separators(part)\n  case (base, tail) {\n    (\"\", _) -> from(tail)",
         "  let tail = trim_leading_separators(part)\n  case (base, tail) {\n    _ if Text.starts_with(part, \"/\") -> from(part)\n    (\"\", _) -> from(tail)")]),
    programs("join from an empty base keeps an absolute part", PATH, [
        ('    ("", _) -> from(tail)', '    ("", _) -> from(part)')]),
    programs("replace_extension keeps trailing dot components", PATH, [
        ('      None -> case Text.strip_suffix(rest, "/.") {\n        Some(shorter) -> shorter\n        None -> break rest\n      }',
         '      None -> break rest')]),
    programs("join doubles the separator", PATH, [
        ("      True -> from(Text.append(base, tail))", "      True -> from(Text.concat([base, \"/\", tail]))")]),
    programs("name keeps ..", PATH, [
        ("    Some(\"..\") -> None\n    other -> other", "    other -> other")]),
    programs("a hidden file has an extension", PATH, [
        ("    [\"\", _] -> None\n", "")]),
    programs("an empty extension is Some", PATH, [
        ("      Some((_, \"\")) -> None\n", "")]),
    programs("the parent of a single name is None", PATH, [
        ("    ([_], False) -> Some(from(\".\"))", "    ([_], False) -> None")]),
    programs("normalize keeps .. under the root", PATH, [
        ("        (None, True) -> (rest, kept)", "        (None, True) -> (rest, List.push(kept, \"..\"))")]),
    programs("normalize cancels a leading ..", PATH, [
        ("        (Some(\"..\"), _) -> (rest, List.push(kept, \"..\"))\n", "")]),
    programs("normalize of the empty path is empty", PATH, [
        ("    (False, \"\") -> from(\".\")\n", "")]),
    programs("replace_extension drops the directory", PATH, [
        ("      from(Text.append(directory, renamed))", "      from(renamed)")]),
]
