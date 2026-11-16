"""Mutants of `chimi test <directory>` (Phase 5, CHI-208, part 5a): every
module under the directory, in module path order, skipping hidden
directories and `target`.

Run: scripts/mutation/mutate.py scripts/mutation/test_directory_mutations.py
"""

CRATE = "compiler/chimi"

P = "src/project.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("testing",), filter="directory"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("target is walked", P, [(
        "!name.starts_with('.') && name != \"target\"",
        "!name.starts_with('.')",
    )]),
    mutant("hidden directories are walked", P, [(
        "!name.starts_with('.') && name != \"target\"",
        "name != \"target\"",
    )]),
    mutant("subdirectories are never walked", P, [(
        "                    .is_some_and(|name| descend(&name.to_string_lossy()))",
        "                    .is_some_and(|_| false)",
    )]),
    mutant("only the first module loads", P, [(
        "        for module_path in &module_paths {\n            loader.load_module(module_path)?;",
        "        for module_path in module_paths.iter().take(1) {\n            loader.load_module(module_path)?;",
    )]),
    mutant("a directory is read as a file", M, [(
        "            input = Some(if path.is_dir() {",
        "            input = Some(if false {",
    )]),
    mutant("symbolic links are followed", P, [(
        "            if kind.is_dir() {",
        "            let kind = std::fs::metadata(&path).map_err(failed)?.file_type();\n            if kind.is_dir() {",
    )]),
    mutant("hidden files load", P, [(
        "                && !name.starts_with('.')\n",
        "",
    )]),
    mutant("an empty directory runs nothing", P, [(
        "        if module_paths.is_empty() {",
        "        if false {",
    )]),
]
