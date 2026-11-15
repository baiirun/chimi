"""Mutants of build-time embedding (Phase 6a, CHI-211): the lowering checks in
src/executable.rs and the runtime natives in bytes_operations.rs and runtime.c.

Run: scripts/mutation/mutate.py scripts/mutation/embed_mutations.py
"""

CRATE = "compiler/chimi"

LOWERING = "src/executable.rs"
CORE = "src/execution/bytes_operations.rs"
RUNTIME = "runtime/native/runtime.c"


def checks(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter="embed")


def programs(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"],
                filter="standard_programs")


MUTATIONS = [
    checks("functions count as build time", LOWERING, [
        ("let at_build_time = std::mem::replace(&mut self.at_build_time, false);",
         "let at_build_time = self.at_build_time;")]),
    checks("every declaration counts as build time", LOWERING, [
        ("lowerer.at_build_time = kind == ModuleDeclarationKind::Constant;",
         "lowerer.at_build_time = true;")]),
    checks("Embed as a value is allowed", LOWERING, [
        ("        if !self.embed_callee\n", "        if false\n")]),
    checks("parent segments are allowed", LOWERING, [
        ('.any(|segment| matches!(segment, "" | "." | ".."))',
         '.any(|segment| matches!(segment, "" | "."))')]),
    checks("absolute paths are allowed", LOWERING, [
        ("        || path.starts_with('/')\n", "")]),
    checks("symbolic links may escape", LOWERING, [
        ("if !file.starts_with(directory.canonicalize().map_err(unreadable)?) {",
         "if false && !file.starts_with(directory.canonicalize().map_err(unreadable)?) {")]),
    checks("Embed.text skips the UTF-8 check", LOWERING, [
        ("StandardFunction::EmbedText => String::from_utf8(contents).map_err(|invalid| {",
         "StandardFunction::EmbedText => Ok::<String, std::string::FromUtf8Error>(String::from_utf8_lossy(&contents).into_owned()).map_err(|invalid| {")]),
    programs("Rust Embed.bytes drops the last byte", CORE, [
        ("RuntimeValue::Bytes(from_hex(hex).into())",
         "RuntimeValue::Bytes(from_hex(&hex[..hex.len().saturating_sub(2)]).into())")]),
    programs("C Embed.bytes returns the hex as text", RUNTIME, [
        ("return success(bytes_from_hex(arguments[0]->as.text.bytes, arguments[0]->as.text.length));",
         "return success(arguments[0]);")]),
]
