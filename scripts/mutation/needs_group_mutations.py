"""Mutants of needs groups (Phase 7c, CHI-289, G24): `pub needs Name { ... }`
in a needs clause, alone or with `+`, across modules.

Run: scripts/mutation/mutate.py scripts/mutation/needs_group_mutations.py
"""

CRATE = "compiler/chimi"

R = "src/resolution.rs"
T = "src/typing.rs"
F = "src/formatter.rs"


def mutant(name, file, edits, filter="needs_group", targets=("language",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a group's entries are not bound in the body", R, [(
        "        for name in self.needs_group_entries(group) {",
        "        for name in Vec::<Name>::new() {",
    )]),
    mutant("a non-group type stands as a group", R, [(
        "                if self.symbols[symbol.raw() as usize].kind != ProjectSymbolKind::NeedsGroup =>",
        "                if false =>",
    )]),
    mutant("a group adds no needs to the contract", T, [(
        "            self.add_need(module_id, entries, requirement, name, interface);\n        }\n    }",
        "            let _ = (requirement, interface);\n        }\n    }",
    )]),
    mutant("a group's entry interface is read in the using module", T, [(
        "            let interface = self.lower_value_type(group_module, interface, &no_generics);",
        "            let interface = self.lower_value_type(module_id, interface, &no_generics);",
    )]),
    mutant("group entries do not count as written", T, [(
        "                if symbol.kind == ProjectSymbolKind::NeedsGroup {\n                return crate::resolution",
        "                if false {\n                return crate::resolution",
    )]),
    mutant("a shared need is bound twice", R, [(
        "        if !bound {\n            self.define_local(",
        "        if true {\n            self.define_local(",
    )]),
    mutant("a second term's entry skips the interface check", T, [(
        "                        .or_else(|| {\n                            entries\n                                .iter()\n                                .find(|entry| entry.lexical_name == name.text)\n                                .map(|entry| entry.requirement)\n                        })",
        "",
    )]),
    mutant("a group item joins the line above", F, [(
        " && next.parent().kind() != NodeKind::NeedsGroupDeclaration",
        "",
    )]),
]
