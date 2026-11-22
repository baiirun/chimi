"""Mutants of the editor finding a file's package (T5 PR 2, CHI-290).

Run: scripts/mutation/mutate.py scripts/mutation/packages_editor_mutations.py
"""

CRATE = "compiler/chimi-lsp"

W = "src/workspace.rs"

MUTATIONS = [
    dict(
        name="a package file is analyzed alone",
        file=W,
        edits=[(
            "            .or_else(|| self.package_project_for(path))",
            "            .or_else(|| None)",
        )],
        targets=["lib"],
        filter="package",
    ),
    dict(
        name="the editor lints dependencies",
        file="../chimi/src/compiler/session.rs",
        edits=[(
            "|origin| origin == crate::compiler::ModuleOrigin::Own",
            "|origin| origin != crate::compiler::ModuleOrigin::Standard",
        )],
        targets=["lib"],
        filter="package",
    ),
]
