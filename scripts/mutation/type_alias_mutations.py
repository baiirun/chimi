"""Mutants for type aliases (CHI-279): each reintroduces a plausible bug."""

CRATE = "compiler/chimi"

MUTATIONS = [
    dict(
        name="an alias lowers to Poison instead of its type",
        file="src/typing.rs",
        edits=[("                        self.alias_type(symbol)\n", "                        TypeId::new(0)\n")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="an alias after errors is never a failure set",
        file="src/typing/failure_sets.rs",
        edits=[("            return self.failure_set_of(aliased);", "            return None;")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="a self-reference is not a cycle",
        file="src/typing/aliases.rs",
        edits=[("                    if symbol == target {\n                        return Some(Vec::new());", "                    if symbol == target && false {\n                        return Some(Vec::new());")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="a cycle through another alias is not followed",
        file="src/typing/aliases.rs",
        edits=[("                    if visited.insert(symbol)\n", "                    if false && visited.insert(symbol)\n")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="an alias declaration does not report its own type's errors",
        file="src/typing/aliases.rs",
        edits=[("        self.lower_value_type(module_id, annotation, &HashMap::new());\n        let Some(alias)", "        let Some(alias)")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="a use of an alias repeats its type's errors",
        file="src/typing/aliases.rs",
        edits=[("        self.diagnostics.truncate(diagnostics);\n", "")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="generic aliases are accepted",
        file="src/semantic.rs",
        edits=[('                "L2130",', '                "L2131",')],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="doc prints the inferred type instead of the written one",
        file="src/doc.rs",
        edits=[("                                .cloned()\n                                .flatten()\n", "                                .cloned()\n                                .flatten()\n                                .filter(|_| false)\n")],
        targets=["doc"],
        filter="alias",
    ),
    dict(
        name="doc prints the inferred success type",
        file="src/doc.rs",
        edits=[("                let result = match &written.success {", "                let result = match &None::<String> {")],
        targets=["doc"],
        filter="alias",
    ),
    dict(
        name="pub on an alias is ignored",
        file="src/semantic.rs",
        edits=[("            kind: ModuleDeclarationKind::TypeAlias,\n            public: node", "            kind: ModuleDeclarationKind::TypeAlias,\n            public: false && node")],
        targets=["language"],
        filter="alias",
    ),
]
