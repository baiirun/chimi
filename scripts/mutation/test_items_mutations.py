"""Mutants of `test "name" { }` items (Phase 5, CHI-208): parsing the contextual
word, the duplicate-name and visibility checks, the test body contract, and
dropping tests from builds.

Run: scripts/mutation/mutate.py scripts/mutation/test_items_mutations.py
"""

CRATE = "compiler/chimi"

P = "src/syntax.rs"
S = "src/semantic.rs"
Y = "src/typing.rs"
C = "src/compiler.rs"


def lib(name, file, edits, filter="test_items"):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


MUTATIONS = [
    lib("parser: `test` needs no text literal after it", P, [(
        "            && matches!(\n                self.kind_at(index + 1),\n                TokenKind::Text | TokenKind::TextStart\n            )",
        "",
    )]),
    lib("lowering: duplicate test names are allowed", S, [(
        ".find(|declaration| declaration.test && declaration.name.text == name.text)",
        ".find(|declaration| declaration.test && declaration.name.text == name.text && false)",
    )]),
    lib("lowering: `pub` on a test is ignored", S, [(
        "            .find(|token| token.kind() == TokenKind::Pub)\n        {\n            self.diagnostics.push(",
        "            .find(|token| token.kind() == TokenKind::Pub && false)\n        {\n            self.diagnostics.push(",
    )]),
    lib("typing: any success type passes T4402", Y, [(
        "                        && matches!(symbol.name.as_str(), \"Void\" | \"Never\")",
        "                        || true",
    )]),
    lib("typing: a test body may not end in Never", Y, [(
        "matches!(symbol.name.as_str(), \"Void\" | \"Never\")",
        "matches!(symbol.name.as_str(), \"Void\")",
    )]),
    lib("typing: the test plan closes every need", Y, [(
        ".filter(|need| !need.explained && !self.test_plan_closes(need))",
        ".filter(|need| !need.explained && !self.test_plan_closes(need) && false)",
    )]),
    lib("typing: the test plan closes nothing", Y, [(
        "            Some(StandardSymbol::Type(StandardType::Console)) => true,\n            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == \"clock\",\n            _ => false,\n        }\n    }\n\n    /// Whether the runner's",
        "            _ => false,\n        }\n    }\n\n    /// Whether the runner's",
    )]),
    dict(name="build: tests are kept in a program", file=C, edits=[(
        "    resolve_project(lower_project_sources(sources).without_tests())",
        "    resolve_project(lower_project_sources(sources))",
    )], targets=["cli"], filter="build_output_is_identical_with_and_without_test_items"),
    dict(name="lowering: tests lower in source order", file=S, edits=[(
        "                NodeKind::TestDeclaration => tests.push(node),",
        "                NodeKind::TestDeclaration => {\n                    if let Some(declaration) = lowerer.lower_test(&node, &declarations) {\n                        declarations.push(declaration);\n                    }\n                }",
    )], targets=["cli"], filter="build_output_is_identical_with_and_without_test_items"),
]
