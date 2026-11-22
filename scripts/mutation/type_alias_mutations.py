"""Mutants for type aliases (CHI-279): each reintroduces a plausible bug.

Dropped as equivalent: removing the diagnostics truncation in `alias_type`. With
it removed, an alias type holding an error still reported that error once (tried
with uses above and below the declaration), so no test can observe it.
"""

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
        name="type arguments on an alias are ignored",
        file="src/typing/aliases.rs",
        edits=[("        if !arguments.is_empty() {\n            self.type_error(", "        if false {\n            self.type_error(")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="a cycle through a needs row is not followed",
        file="src/typing/aliases.rs",
        edits=[("                    RequirementTermUse::Field { interface, .. } => Some(interface),", "                    RequirementTermUse::Field { .. } => None,")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="a type alias keeps its line breaks when formatted",
        file="src/formatter.rs",
        edits=[("            || token_has_ancestor(token, &[NodeKind::TypeAliasDeclaration])\n", "")],
        targets=["language"],
        filter="alias",
    ),
    dict(
        name="doc keeps comments inside a written type",
        file="src/doc.rs",
        edits=[("                .any(|token| token.kind() == TokenKind::Comment)\n            {\n                return None;", "                .any(|token| token.kind() == TokenKind::Comment && false)\n            {\n                return None;")],
        targets=["doc"],
        filter="written_type",
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
