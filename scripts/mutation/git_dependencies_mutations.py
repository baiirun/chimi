"""Mutants of git dependencies (T5 PR 4, CHI-290): builds take commits
only from chimi.lock and never write it, `chimi deps update` moves only
what changed or what it is told to, and --offline never fetches.

Run: scripts/mutation/mutate.py scripts/mutation/git_dependencies_mutations.py
"""

CRATE = "compiler/chimi"

G = "src/project/packages.rs"
C = "src/project/git.rs"
P = "src/project.rs"
L = "src/manifest/lock.rs"
M = "src/main.rs"


def commands(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["packages"], filter="")


def lock(name, edits):
    return dict(name=name, file=L, edits=edits, targets=["lib"], filter="manifest::lock")


MUTATIONS = [
    commands("a build ignores the lock's source", G, [(
        "                .filter(|locked| locked.source == *source)\n                .map(|locked| locked.commit.clone())",
        "                .map(|locked| locked.commit.clone())",
    )]),
    commands("an update keeps a named package's pin", G, [(
        "                        locked.source == *source\n                            && !moving.iter().any(|moved| moved == name.as_str())",
        "                        locked.source == *source",
    )]),
    commands("an update re-resolves every pin", G, [(
        "                    .filter(|locked| {\n                        locked.source == *source\n",
        "                    .filter(|locked| {\n                        false && locked.source == *source\n",
    )]),
    commands("an update accepts any package name", G, [(
        "            return Err(PackageError::NotAGitDependency { name });",
        "            let _ = name;",
    )]),
    commands("offline fetches anyway", C, [(
        "            if self.network == Network::Offline {",
        "            if false {",
    )]),
    commands("a git directory may leave its repository", G, [(
        "                        if !directory.starts_with(&checkout) {",
        "                        if false {",
    )]),
    commands("two git sources for one name are accepted", G, [(
        "                            if known.git.as_ref().map(|locked| &locked.source) == Some(source) {",
        "                            if known.git.is_some() {",
    )]),
    commands("a member's lock is written beside it", G, [(
        "            Manifest::Package(_) => workspace_root(root)?,",
        "            Manifest::Package(_) => root.to_owned(),",
    )]),
    commands("the cache checks out the branch tip, not the commit", C, [(
        '            .args(["checkout", "--quiet", "--force", commit, "--", "."])',
        '            .args(["checkout", "--quiet", "--force", "main", "--", "."])',
    )]),
    commands("an unchanged lock is never rewritten and changes are not printed", M, [(
        "            for change in changes {\n                eprintln!(\"chimi: {change}\");\n            }",
        "            let _ = changes;",
    )]),
    lock("the lock skips the directory", [(
        "                        directory: fields.get(\"directory\").map(|text| (*text).to_owned()),",
        "                        directory: None,",
    )]),
    dict(name="the lock accepts a short commit", file=C, targets=["lib"], filter="manifest::lock", edits=[(
        "    text.len() == 40\n",
        "    !text.is_empty()\n",
    )]),
    lock("quotes are written raw", [(
        "            '\"' => quoted.push_str(\"\\\\\\\"\"),",
        "            '\"' => quoted.push('\"'),",
    )]),
]
