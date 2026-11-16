"""Mutants of `chimi lint` (Phase 9, T3, CHI-274): what each rule counts as a
use, which names it exempts, and when the command reports.

Run: scripts/mutation/mutate.py scripts/mutation/lint_mutations.py
"""

CRATE = "compiler/chimi"

L = "src/lint.rs"
R = "src/resolution.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("lint",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a qualified type does not use its alias", L, [(
        "                        || reference.text.starts_with(&qualified)\n                });",
        "                });",
    )]),
    mutant("Name.Variant does not use the imported enum", L, [(
        "                                || reference.text.starts_with(&qualified))",
        "                                || false)",
    )]),
    mutant("a module alias counts as a member use", L, [(
        "                        reference.namespace != Namespace::Module\n",
        "                        true\n",
    )]),
    mutant("underscore names are reported", L, [(
        "    name.starts_with('_') || is_generated_name(name)",
        "    is_generated_name(name)",
    )]),
    mutant("public items are reported", L, [(
        "            !declaration.public\n                && !declaration.test",
        "            !declaration.test",
    )]),
    mutant("runner providers are reported", L, [(
        "                && !is_runner_provider(&module.path, &declaration.name.text)\n",
        "",
    )]),
    mutant("parameters are reported", L, [(
        "                    ProjectSymbolKind::LocalBinding | ProjectSymbolKind::PatternBinding",
        "                    ProjectSymbolKind::LocalBinding | ProjectSymbolKind::PatternBinding | ProjectSymbolKind::Parameter",
    )]),
    mutant("pattern bindings are never reported", L, [(
        "                    ProjectSymbolKind::LocalBinding | ProjectSymbolKind::PatternBinding",
        "                    ProjectSymbolKind::LocalBinding",
    )]),
    mutant("std modules are linted", "src/compiler.rs", [(
        "            include_standard || !module_path.starts_with(\"std/\")",
        "            include_standard || true",
    )]),
    mutant("binding ranges are not recorded", R, [(
        "        self.symbols[id.raw() as usize].name_range = Some(name.range);\n",
        "",
    )]),
    mutant("findings exit 0", M, [(
        "    if findings.is_empty() {\n        ExitCode::SUCCESS",
        "    if true {\n        ExitCode::SUCCESS",
    )]),
    mutant("lint runs on code that does not type-check", M, [(
        "    let findings = if report.is_success() {\n        report.lint(false)",
        "    let findings = if true {\n        report.lint(false)",
    )]),
]
