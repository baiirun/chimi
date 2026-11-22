"""Mutants of workspaces (T5 PR 3, CHI-290): a workspace directory loads
every member as its own, members are checked, and a member's build writes
under the workspace's target/.

Run: scripts/mutation/mutate.py scripts/mutation/workspaces_mutations.py
"""

CRATE = "compiler/chimi"

P = "src/project.rs"
G = "src/project/packages.rs"
M = "src/main.rs"


def unit(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter="project::")


def commands(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["packages"], filter="workspace")


MUTATIONS = [
    unit("only the first member loads", P, [(
        "                for package in graph.own() {\n                    let name = package.name.as_str();",
        "                for package in graph.own().take(1) {\n                    let name = package.name.as_str();",
    )]),
    unit("only the first member is the build's own", P, [(
        "            origin: if target.own {",
        "            origin: if target.name == graph.own().next().unwrap().name {",
    )]),
    unit("two members may share a name", G, [(
        "                    if let Some((prior, _, _)) =\n                        own.iter().find(|(_, _, prior)| prior.name == package.name)",
        "                    if let Some((prior, _, _)) =\n                        own.iter().find(|_| false)",
    )]),
    unit("a member may sit outside the workspace", G, [(
        "                    if !directory.starts_with(root) {",
        "                    if false {",
    )]),
    unit("a member may be listed twice", G, [(
        "                    if own.iter().any(|(listed, _, _)| *listed == directory) {",
        "                    if false {",
    )]),
    unit("--root takes a workspace", G, [(
        "            Manifest::Workspace(_) if !workspace_allowed =>",
        "            Manifest::Workspace(_) if false =>",
    )]),
    commands("artifacts stay under the current directory", M, [(
        "            chimi::project::workspace_root_of(root).unwrap_or_default()",
        "            PathBuf::new()",
    )]),
    commands("doc of a directory prints text", M, [(
        "        DocOutput::Text | DocOutput::Json if matches!(target, DocTarget::Directory { .. }) => {",
        "        DocOutput::Text | DocOutput::Json if false => {",
    )]),
]
