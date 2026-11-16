"""Mutants of editor completion after `Alias.` (Phase 9, T2, CHI-272): which
text counts as an alias and a dot, and which items the module offers.

Run: scripts/mutation/mutate.py scripts/mutation/completion_mutations.py
"""

CRATE = "compiler/chimi-lsp"

D = "../chimi/src/doc.rs"
W = "src/workspace.rs"
S = "src/server.rs"


def mutant(name, file, edits, targets=("lib", "protocol"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("any import completes, not only the alias named", D, [(
        "        ImportBinding::Alias(name) if name.text == alias => Some(import.path.as_str()),",
        "        ImportBinding::Alias(_) => Some(import.path.as_str()),",
    )]),
    mutant("a partial name blocks completion", W, [(
        "    let partial_start = before.trim_end_matches(is_name).len();",
        "    let partial_start = before.len();",
    )]),
    mutant("no dot is needed", W, [(
        "    let dotted = before[..partial_start].strip_suffix('.')?;",
        "    let dotted = before[..partial_start].strip_suffix('.').unwrap_or(&before[..partial_start]);",
    )]),
    mutant("a digit can start an alias", W, [(
        "        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic());",
        "        .is_some_and(|c| c == '_' || c.is_ascii_alphanumeric());",
    )]),
    mutant("a longer path completes", W, [(
        "    (starts_name && !dotted[..alias_start].ends_with('.')).then_some(alias)",
        "    starts_name.then_some(alias)",
    )]),
    mutant("no doc comments in completion", W, [(
        "                    documentation: item.doc().map(str::to_owned),",
        "                    documentation: None,",
    )]),
    mutant("no '.' trigger", S, [(
        "            trigger_characters: Some(vec![\".\".to_owned()]),",
        "            trigger_characters: None,",
    )]),
    mutant("completion kinds are all classes", S, [(
        "        \"fn\" => CompletionItemKind::FUNCTION,",
        "        \"fn\" => CompletionItemKind::CLASS,",
    )]),
]
