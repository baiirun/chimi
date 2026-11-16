"""Mutants of lint findings in the editor (Phase 9, T3, CHI-274): when the
language server publishes them and how.

Run: scripts/mutation/mutate.py scripts/mutation/editor_lint_mutations.py
"""

CRATE = "compiler/chimi-lsp"

SESSION = "../chimi/src/compiler/session.rs"
W = "src/workspace.rs"
S = "src/server.rs"


def mutant(name, file, edits, targets=("lib", "protocol"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("lint findings beside errors", SESSION, [(
        "        if !self.diagnostics().is_empty() {\n            return findings;",
        "        if false {\n            return findings;",
    )]),
    mutant("configured projects publish no lint", W, [(
        "        let lint = report.lint_by_source();\n        let mut modules = HashMap::new();",
        "        let lint = HashMap::<&std::path::Path, Vec<Diagnostic>>::new();\n        let mut modules = HashMap::new();",
    )]),
    mutant("lint findings are errors in the editor", S, [(
        "            Severity::Warning => DiagnosticSeverity::WARNING,",
        "            Severity::Warning => DiagnosticSeverity::ERROR,",
    )]),
]
