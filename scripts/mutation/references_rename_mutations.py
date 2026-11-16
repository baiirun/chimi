"""Mutants of find-references and rename (Phase 9, T2, CHI-272): which names
count as one symbol's occurrences, and which renames the editor refuses.

Run: scripts/mutation/mutate.py scripts/mutation/references_rename_mutations.py
"""

CRATE = "compiler/chimi-lsp"

O = "../chimi/src/occurrences.rs"
W = "src/workspace.rs"


def mutant(name, file, edits, targets=("lib", "protocol"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a struct's type and constructor are separate symbols", O, [(
        "        (Some(module_id), Some(semantic_id)) => resolved",
        "        (Some(module_id), Some(semantic_id)) if false => resolved",
    )]),
    mutant("a constructor call does not name its struct", O, [(
        "                    .then(|| struct_in_scope(resolved, module, &reference.text))",
        "                    .then(|| None)",
    )]),
    mutant("only the first clause head is the function's name", O, [(
        "                        ranges.push(head);",
        "                        let _ = head;",
    )]),
    mutant("a member import does not name the item", O, [(
        "                    occurrences.push(occurrence(module, member.range, false));",
        "",
    )]),
    mutant("unresolved constructor calls are not the struct's", O, [(
        "        if struct_constructor && in_scope {",
        "        if false && struct_constructor && in_scope {",
    )]),
    mutant("standard symbols are renamable", O, [(
        "            .any(|occurrence| occurrence.module_path.starts_with(\"std/\"))",
        "            .any(|_| false)",
    )]),
    mutant("module aliases are renamable", O, [(
        "    let renamable = source_backed\n",
        "    let renamable = true\n",
    )]),
    mutant("references always include the declaration", W, [(
        "                .filter(|occurrence| include_declaration || !occurrence.is_declaration())",
        "                .filter(|_| true)",
    )]),
    mutant("a rename may change a name's case", W, [(
        "            || starts_uppercase(new_name) != starts_uppercase(occurrences.name())",
        "            || false",
    )]),
    mutant("a rename may collide", W, [(
        "        if renamed.diagnostics().len() > self.report.diagnostics().len() {",
        "        if false {",
    )]),
    mutant("a rename may shadow if it type-checks", W, [(
        "        found == expected\n",
        "        let _ = (found, expected);\n        true\n",
    )]),
    mutant("later spans in a file do not shift", W, [(
        "                shift += i64::from(length) - i64::from(end - start);",
        "                let _ = end;",
    )]),
    mutant("a local named like an item is imported", O, [(
        "            | ProjectSymbolKind::LocalBinding\n            | ProjectSymbolKind::PatternBinding\n    ) {",
        "            | ProjectSymbolKind::LocalBinding\n            | ProjectSymbolKind::PatternBinding\n    ) && false {",
    )]),
]
