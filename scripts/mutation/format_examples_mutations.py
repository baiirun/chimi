"""Mutants of formatting doc-comment example code (Phase 5, CHI-208).

Run: scripts/mutation/mutate.py scripts/mutation/format_examples_mutations.py
"""

CRATE = "compiler/chimi"

D = "src/doc_examples.rs"
F = "src/formatter.rs"


def mutant(name, file, edits, targets=("lib",), filter="formatter::tests::doc_comment_examples"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("examples are not formatted", F, [(
        "            parsed.diagnostics().is_empty().then(|| format_parse(&parsed))",
        "            parsed.diagnostics().is_empty().then(|| code.to_owned())",
    )]),
    mutant("an unparsable example is formatted anyway", F, [(
        "            parsed.diagnostics().is_empty().then(|| format_parse(&parsed))",
        "            Some(format_parse(&parsed))",
    )]),
    mutant("no blank line after the imports", D, [(
        "    if !lines.is_empty() && !statements.is_empty() {\n        lines.push(String::new());\n    }",
        "",
    )]),
    mutant("the body keeps the test indentation", D, [(
        "        .map(|line| line.strip_prefix(\"  \").unwrap_or(line).to_owned())",
        "        .map(|line| (*line).to_owned())",
    )]),
    mutant("empty lines keep a trailing space", D, [(
        "                if line.is_empty() {\n                    format!(\"{indent}///\")",
        "                if false {\n                    format!(\"{indent}///\")",
    )]),
    mutant("fragment blocks are formatted", D, [(
        "                if doc_content(lines[fence]).unwrap_or_default().trim_end() == \"```chimi\" {",
        "                if doc_content(lines[fence]).unwrap_or_default().starts_with(\"```chimi\") {",
    )]),
]
