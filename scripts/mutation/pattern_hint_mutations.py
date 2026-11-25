"""Mutants of the CHI-291 follow-up fixes: nested and bare struct patterns, and
a struct literal as a case subject without parentheses.

Run: scripts/mutation/mutate.py scripts/mutation/pattern_hint_mutations.py
"""

CRATE = "compiler/chimi"

SYNTAX = "src/syntax.rs"
TYPING = "src/typing.rs"
LITERALS = "src/typing/struct_literals.rs"
FILTER = "struct_pattern_and_case_subject_fixes_apply_cleanly"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=FILTER)


MUTATIONS = [
    mutant("inner patterns keep their own fixes", TYPING, [(
        "                    context.rewritten_by_outer_fix |= rejected.fixed;",
        "",
    )]),
    mutant("the outer fix keeps inner text as written", LITERALS, [(
        "                    format!(\"{field}: {}\", self.rewritten_sub_pattern(module_id, argument))",
        "                    format!(\"{field}: {}\", { let r = argument.range(); &self.resolved.semantic.modules[module_id.raw() as usize].source.text()[r.start() as usize..r.end() as usize] })",
    )]),
    mutant("a bare pattern gets no fix", LITERALS, [(
        "            Some(format!(\"{written} {{ .. }}\"))",
        "            None",
    )]),
    mutant("a bare pattern keeps its field-count error", TYPING, [(
        "                    if !rejected.arity_matches {",
        "                    if false {",
    )]),
    mutant("a bracketless subject literal is not detected", SYNTAX, [(
        "            } else if names_a_type && self.at_bracketless_case_subject_literal() {",
        "            } else if false && self.at_bracketless_case_subject_literal() {",
    )]),
    mutant("an arm list that starts with a typed pattern counts as a literal", SYNTAX, [
        (
            "                TokenKind::Arrow if depth == 1 => return false,",
            "",
        ),
        (
            "        self.kind_at(position) == TokenKind::LeftBrace\n    }\n\n    /// Reports the struct literal",
            "        true\n    }\n\n    /// Reports the struct literal",
        ),
    ]),
    mutant("the subject fix adds no parentheses", SYNTAX, [(
        "        .with_fix(range, format!(\"({text})\"));",
        "        .with_fix(range, text.to_owned());",
    )]),
]
