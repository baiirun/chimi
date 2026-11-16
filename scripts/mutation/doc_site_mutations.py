"""Mutants of `chimi doc --html` (Phase 9, T4, CHI-273): which modules get
pages, where the pages go, how they link, and how doc text becomes HTML.

Run: scripts/mutation/mutate.py scripts/mutation/doc_site_mutations.py

Equivalent, so not listed: dropping the sort in `render_site`.
`TypeReport::documentation_of` already returns modules in path order, since
the compiler numbers modules that way.
"""

CRATE = "compiler/chimi"

M = "src/main.rs"
H = "src/doc_site.rs"


def mutant(name, file, edits, targets=("doc", "lib"), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a project's pages include std", M, [(
        "                if standard_target { in_std } else { !in_std }",
        "                if standard_target { in_std } else { true }",
    )]),
    mutant("std pages include nothing", M, [(
        "    let standard_target = matches!(target, DocTarget::Standard { .. });",
        "    let standard_target = false;",
    )]),
    mutant("nested pages are not given their directory", M, [(
        "            .map_or(Ok(()), std::fs::create_dir_all)",
        "            .map_or(Ok(()), |_| Ok(()))",
    )]),
    mutant("links ignore the page's depth", H, [(
        "    let depth = from.matches('/').count();",
        "    let depth = 0;",
    )]),
    mutant("no summary line in the index", H, [(
        "            body.push_str(&format!(\" <span>{}</span>\", inline(&first)));",
        "",
    )]),
    mutant("doc comments are dropped from pages", H, [(
        "        if let Some(doc) = item.doc() {\n            body.push_str(&markdown(doc));",
        "        if let Some(doc) = item.doc() {\n            let _ = doc;",
    )]),
    mutant("signatures are not escaped", H, [(
        "            signature = escape(item.signature()),",
        "            signature = item.signature(),",
    )]),
    mutant("quotes are not escaped", H, [(
        "            '\"' => escaped.push_str(\"&quot;\"),\n",
        "",
    )]),
    mutant("inline code is not escaped", H, [(
        "            escape(&rest[open + 1..open + 1 + close])",
        "            &rest[open + 1..open + 1 + close]",
    )]),
    mutant("list entries do not continue", H, [(
        "        } else if let Some(last) = list.last_mut().filter(|_| line.starts_with(' ')) {",
        "        } else if let Some(last) = list.last_mut().filter(|_| false) {",
    )]),
    mutant("a list does not end at a paragraph", H, [(
        "            if !list.is_empty() {\n                flush(&mut html, &mut paragraph, &mut list);\n            }\n            paragraph.push(line.trim());",
        "            paragraph.push(line.trim());",
    )]),
    mutant("an unclosed fence is dropped", H, [(
        "    if let Some((_, code)) = fence {",
        "    if let Some((_, code)) = fence.filter(|_| false) {",
    )]),
    mutant("paragraphs are not split at blank lines", H, [(
        "        } else if line.trim().is_empty() {\n            flush(&mut html, &mut paragraph, &mut list);\n",
        "        } else if line.trim().is_empty() {\n",
    )]),
    mutant("any fence closes a block", H, [(
        "                .is_some_and(|ticks| ticks >= *opening && line.trim().len() == ticks);",
        "                .is_some();",
    )]),
    mutant("the index shows only the first line", H, [(
        "        .take_while(|line| !line.trim().is_empty() && fence_length(line).is_none())",
        "        .take(1)",
    )]),
    mutant("one std module may name the whole tree", M, [(
        "        DocOutput::Html { .. } if names_one_std_module => Err(UsageError {",
        "        DocOutput::Html { .. } if false && names_one_std_module => Err(UsageError {",
    )], targets=("doc",)),
]
