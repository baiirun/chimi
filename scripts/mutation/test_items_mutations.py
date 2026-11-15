"""Mutants of `test "name" { }` items (Phase 5, CHI-208): parsing the contextual
word, the duplicate-name and visibility checks, the test body contract, and
dropping tests from builds.

Run: scripts/mutation/mutate.py scripts/mutation/test_items_mutations.py

Not listed: reading `test` as a keyword without the text literal after it is
equivalent on valid programs, since no other module item starts with an
identifier named `test`; it only changes the syntax error for broken input.
"""

CRATE = "compiler/chimi"

P = "src/syntax.rs"
S = "src/semantic.rs"
Y = "src/typing.rs"
C = "src/compiler.rs"


def lib(name, file, edits, filter="test_items"):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


MUTATIONS = [
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
    lib("typing: the test plan closes a clock under any name", Y, [(
        "            Some(StandardSymbol::Type(StandardType::Clock)) => need.lexical_name == \"clock\",\n            _ => false,\n        }\n    }\n\n    /// Whether the runner's",
        "            Some(StandardSymbol::Type(StandardType::Clock)) => true,\n            _ => false,\n        }\n    }\n\n    /// Whether the runner's",
    )]),
    dict(name="build: tests are kept in a program", file=C, edits=[(
        "    resolve_project(lower_project_sources(sources).without_tests())",
        "    resolve_project(lower_project_sources(sources))",
    )], targets=["cli"], filter="build_output_is_identical_with_and_without_test_items"),
]
