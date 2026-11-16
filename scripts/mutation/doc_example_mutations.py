"""Mutants of doc-comment examples (Phase 5, CHI-208, part 5b): which blocks
are examples, how each keeps the documented layout, and how the runner names
and reports them.

Run: scripts/mutation/mutate.py scripts/mutation/doc_example_mutations.py
"""

CRATE = "compiler/chimi"

D = "src/doc_examples.rs"
T = "src/testing.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("testing",), filter="doc_"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a fragment block runs", D, [(
        '.trim_end() == "```chimi" {',
        '.trim_end().starts_with("```chimi") {',
    )]),
    mutant("examples are numbered per file, not per item", D, [(
        "            let ordinal = match counts.iter_mut().find(|(name, _)| *name == item) {",
        "            let ordinal = match counts.iter_mut().next() {",
    )]),
    mutant("the code shifts one column", D, [(
        '    format!("{}{code}", " ".repeat(indent + 3 + space))',
        '    format!("{}{code}", " ".repeat(indent + 4 + space))',
    )]),
    mutant("examples report under their own module", T, [(
        "                case.module.clone_from(documented);",
        "                let _ = documented;",
    )]),
    mutant("chimi check skips examples", M, [(
        "    if let Err(error) = project.add_doc_examples(standard) {\n        eprintln!(\"chimi: {error}\");\n        return ExitCode::FAILURE;",
        "    if let Err(error) = Ok::<(), String>(()) {\n        eprintln!(\"chimi: {error}\");\n        return ExitCode::FAILURE;",
    )]),
    mutant("a doc run before a blank line documents the next item", D, [(
        "        let Some(item) = lines.get(index).and_then(|line| item_name(line)) else {",
        "        let Some(item) = lines[index..].iter().find(|line| !line.trim().is_empty()).and_then(|line| item_name(line)) else {",
    )], targets=("testing", "lib")),
    mutant("JSON keeps the example's own offsets", "src/project.rs", [(
        "            same_place(json.source.text(), range.start(), text),",
        "            range.start(),",
    )]),
    mutant("a bodiless example gets no body", D, [(
        'output[block.close].push_str(if body.is_some() { "}" } else { "Void }" });',
        'output[block.close].push_str("}");',
    )], targets=("lib",)),
    mutant("struct items document nothing", D, [(
        '["fn ", "struct ", "enum ",',
        '["fn ", "enum ",',
    )], targets=("lib",)),
]
