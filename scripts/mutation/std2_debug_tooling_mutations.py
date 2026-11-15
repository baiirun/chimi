"""Mutants of the std/debug port and the standard-tree tooling (f3bdadf): the
`--std-dir` override (project.rs), the build report's standard_library entry,
the `is_standard` test, `chimi std check` / operations::verify_tree, and the
rendering and crash behaviour of std/debug.

Run: scripts/mutation/mutate.py --revision f3bdadf scripts/mutation/std2_debug_tooling_mutations.py
Each mutant must turn the unit tests (`--lib`), tests/std_modules.rs, or the
`std_` tests of tests/cli.rs red.
"""

CRATE = "compiler/chimi"

OPS = "src/operations.rs"
PROJECT = "src/project.rs"
TREE = "src/standard_tree.rs"
NATIVE = "src/backend/native.rs"
MAIN = "src/main.rs"
DEBUG = "src/execution/debug.rs"
CORE = "src/execution/core_operations.rs"
RUNTIME = "runtime/native/runtime.c"
RESOLVE = "src/resolution/standard_source.rs"

LIB = dict(targets=["lib"], filter="")
CLI = dict(targets=["cli"], filter="std_")
STD = dict(targets=["std_modules"], filter="")
PROGRAMS = dict(targets=["std_modules"], filter="standard_programs")


def mutant(name, file, edits, **selection):
    return dict(name=name, file=file, edits=edits, **selection)


MUTATIONS = [
    mutant("std check accepts an operation declared twice", OPS, [(
        "            1 => {}\n",
        "            1.. => {}\n")], **LIB),
    mutant("std check accepts an operation with no native fn", OPS, [(
        "match declarations.get(operation.id).copied().unwrap_or(0) {",
        "match declarations.get(operation.id).copied().unwrap_or(1) {")], **CLI),
    mutant("native fn arity is not checked against the registry", RESOLVE, [(
        "signature.parameters.len() != operation.arity",
        "false")], targets=["lib", "std_modules", "cli"], filter="std_"),
    mutant("std check ignores a runtime-known declaration missing from source", OPS, [(
        "        if !declared {\n",
        "        if false {\n")], **CLI),
    mutant("std check drops the type-check diagnostics", OPS, [(
        "        diagnostics: typed.diagnostics,\n",
        "        diagnostics: Vec::new(),\n")], **CLI),
    mutant("std check exits successfully on a mismatch", MAIN, [(
        "    if !verification.matches() {\n",
        "    if false {\n")], **CLI),
    mutant("override falls back to the embedded module when the directory lacks it", PROJECT, [(
        "                && crate::standard_tree::embedded_module(module_path).is_none() =>\n"
        "        {\n"
        "            Ok(None)\n",
        "                && crate::standard_tree::embedded_module(module_path).is_none() =>\n"
        "        {\n"
        "            Ok(None)\n"
        "        }\n"
        "        Err(error) if error.kind() == io::ErrorKind::NotFound => {\n"
        "            Ok(crate::standard_tree::standard_source(module_path))\n")], **CLI),
    mutant("a --root project ignores the --std-dir override", PROJECT, [(
        "        let mut modules = loader.modules;\n        standard.add_imported_modules(&mut modules)?;",
        "        let mut modules = loader.modules;\n        crate::standard_tree::add_imported_modules(&mut modules);")], **CLI),
    mutant("build report omits the override directory", NATIVE, [(
        "match &self.standard_override {",
        "match &None::<PathBuf> {")], **CLI),
    mutant("is_standard accepts any workspace module under std/", TREE, [(
        "    source.path().to_str() == Some(&source_path(module_path))\n",
        "    let _ = source;\n    module_path.starts_with(\"std/\")\n")],
        targets=["std_modules"], filter="rejected"),
    mutant("interpreter Debug.crash returns the message instead of defecting", CORE, [(
        "            (StandardFunction::DebugCrash, [RuntimeValue::Text(message)]) => {\n"
        "                defect(message.clone())",
        "            (StandardFunction::DebugCrash, [RuntimeValue::Text(message)]) => {\n"
        "                Control::Success(RuntimeValue::Text(message.clone()))")], **PROGRAMS),
    mutant("interpreter render_indented drops the trailing comma", DEBUG, [(
        "            if self.indented {\n                self.output.push(',');\n",
        "            if false {\n                self.output.push(',');\n")], **PROGRAMS),
]
