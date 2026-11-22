"""Mutants of the one-line import `import m as M.{T}` (Phase 7c, CHI-289):
the parser, its lowering to two bindings, the once-only unknown-module
report, and the formatter's join of split imports.

Run: scripts/mutation/mutate.py scripts/mutation/combined_import_mutations.py
"""

CRATE = "compiler/chimi"

S = "src/syntax.rs"
SEM = "src/semantic.rs"
R = "src/resolution.rs"
F = "src/formatter.rs"

FILTER = "combined"


def mutant(name, file, edits, targets=("language", "lint"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("the parser ignores members after an alias", S, [(
        "            if self.at(TokenKind::Dot) {\n                self.parse_import_members();\n            }\n        } else if",
        "        } else if",
    )], filter=FILTER),
    mutant("lowering keeps only the first binding", SEM, [(
        "        let bindings = if bindings.is_empty() {",
        "        let bindings: Vec<ImportBinding> = bindings.into_iter().take(1).collect();\n        let bindings = if bindings.is_empty() {",
    )], filter=FILTER),
    mutant("an unknown combined import is reported twice", R, [(
        "                if !unresolved_ranges.contains(&import.range) {",
        "                if true {",
    )], filter=FILTER),
    mutant("the formatter never joins", F, [(
        "    if edits.is_empty() {\n        return None;",
        "    if true {\n        return None;",
    )], filter="the_formatter_joins"),
    mutant("the formatter joins across a comment", F, [(
        "        .any(|token| token.is_missing() || token.kind() == TokenKind::Comment)",
        "        .any(|token| token.is_missing())",
    )], filter="the_formatter_joins"),
    mutant("the formatter joins a line with a trailing comment", F, [(
        " || !source[end..line_end].trim().is_empty() {",
        " {",
    )], filter="the_formatter_joins"),
    mutant("the formatter keeps the removed line", F, [(
        "            edits.push((other.lines.clone(), String::new()));",
        "            let _ = other;",
    )], filter="the_formatter_joins"),
    mutant("the formatter joins two aliases", F, [(
        "        let [alias_line] = aliases.as_slice() else {\n            continue;\n        };",
        "        let Some(alias_line) = aliases.first() else {\n            continue;\n        };",
    )], filter="the_formatter_joins"),
]
