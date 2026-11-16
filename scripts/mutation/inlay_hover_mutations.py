"""Mutants of inlay types and doc comments in hover (Phase 9, T2, CHI-272):
which bindings get a hint, which types are shown, and when hover adds a doc.

Run: scripts/mutation/mutate.py scripts/mutation/inlay_hover_mutations.py
"""

CRATE = "compiler/chimi-lsp"

H = "../chimi/src/hints.rs"
D = "../chimi/src/doc.rs"
W = "src/workspace.rs"
S = "src/server.rs"


def mutant(name, file, edits, targets=("lib", "protocol"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("an annotated let gets a hint", H, [(
        "            if after.trim_start().starts_with(':') {",
        "            if false {",
    )]),
    mutant("an unsettled type gets a hint", H, [(
        "            is_settled(typed, fact.success, 0).then(|| TypeHint {",
        "            true.then(|| TypeHint {",
    )]),
    mutant("a poisoned argument is settled", H, [(
        "        CanonicalType::Named { arguments, .. } => arguments.iter().all(settled),",
        "        CanonicalType::Named { .. } => true,",
    )]),
    mutant("a hint goes before the name", H, [(
        "                offset: name.end(),",
        "                offset: name.start(),",
    )]),
    mutant("hints ignore the requested lines", W, [(
        "                .filter(|hint| (first_line..=last_line).contains(&hint.position.line))",
        "                .filter(|_| true)",
    )]),
    mutant("hover drops the doc comment", W, [(
        "            (Some(markdown), Some(doc)) => format!(\"{markdown}\\n\\n---\\n\\n{doc}\"),",
        "            (Some(markdown), Some(_)) => markdown,",
    )]),
    mutant("no doc comment for an item in another module", D, [(
        "    let declaring = &resolved.semantic.modules[symbol.module_id?.raw() as usize];",
        "    let declaring = module;",
    )]),
    mutant("no inlay hint capability", S, [(
        "        inlay_hint_provider: Some(lsp_types::OneOf::Left(true)),",
        "        inlay_hint_provider: None,",
    )]),
]
