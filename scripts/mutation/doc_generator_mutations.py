"""Mutants of `chimi doc` (Phase 9, T4, CHI-273): which items it lists, how a
function shows its derived contract, and which comment is an item's doc.

Run: scripts/mutation/mutate.py scripts/mutation/doc_generator_mutations.py
"""

CRATE = "compiler/chimi"

G = "src/doc.rs"
D = "src/doc_examples.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("doc",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("private items are listed", G, [(
        ".filter(|declaration| declaration.public && !declaration.test)",
        ".filter(|declaration| !declaration.test)",
    )]),
    mutant("test items are listed", G, [(
        ".filter(|declaration| declaration.public && !declaration.test)",
        ".filter(|declaration| declaration.public)",
    )]),
    mutant("a function shows its source header, not its contract", G, [(
        "        ModuleDeclarationKind::Function => typed",
        "        ModuleDeclarationKind::Struct => typed",
    )]),
    mutant("the contract drops its errors and needs", G, [(
        '                    typed.format_contract_result(contract)',
        '                    typed.format_type(contract.success)',
    )]),
    mutant("parameter names are dropped", G, [(
        '                        format!("{}: {}", parameter.name.text, typed.format_type(*input))',
        '                        { let _ = parameter; typed.format_type(*input) }',
    )]),
    mutant("a multi-line constant shows its initializer", G, [(
        "            Some(_) => declared",
        "            Some(_) if false => declared",
    )]),
    mutant("doc text is not indented", G, [(
        '                        output.push_str("    ");',
        '                        output.push_str("");',
    )]),
    mutant("the doc comment is taken from one line too far up", D, [(
        "    let lines = &before[..before.len().saturating_sub(1)];",
        "    let lines = &before[..before.len().saturating_sub(2)];",
    )]),
    mutant("a plain comment between doc and item is skipped", D, [(
        "        .rposition(|line| doc_content(line).is_none())",
        "        .rposition(|line| doc_content(line).is_none() && !line.trim_start().starts_with(\"//\"))",
    )]),
    mutant("a header run that documents an item is a summary", D, [(
        "    (end > start && follows.trim().is_empty())",
        "    (end > start)",
    )]),
    mutant("a module that does not type-check is documented", M, [(
        "    if !report.is_success() {\n        eprint!(\"{}\", project.render_diagnostics(report.diagnostics()));",
        "    if false {\n        eprint!(\"{}\", project.render_diagnostics(report.diagnostics()));",
    )]),
    mutant("json leaves out the summary", G, [(
        "        write_optional(&mut output, self.summary.as_deref());",
        "        write_optional(&mut output, None);",
    )]),
]
