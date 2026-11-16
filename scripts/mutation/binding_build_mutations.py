"""Mutants of how binding packages are built (std/tls groundwork): the
archive key, packages with subdirectories and their own flags, and the
archives build.rs prebuilds.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/binding_build_mutations.py
Results: scripts/mutation/binding_build_results.txt.
"""

CRATE = "compiler/chimi"

KEY = "src/backend/runtime_archive_key.rs"
NATIVE = "src/backend/native.rs"
BUILD = "build.rs"

TESTS = dict(targets=["lib", "bindings"], filter="")


def mutant(name, file, old, new):
    return dict(name=name, file=file, edits=[(old, new)], **TESTS)


MUTATIONS = [
    mutant("the key ignores a package's flags", KEY,
           "    for flag in inputs.package_flags {\n        part(flag.as_bytes());\n    }\n",
           ""),
    mutant("two sources may share an object", KEY,
           "        if !seen.insert(folded) {",
           "        if !seen.insert(folded) && false {"),
    mutant("a package's flags are not passed to the compiler", KEY,
           "                        .args(binding_package_flags(package).iter().map(|flag| {",
           "                        .args([\"\"; 0].iter().map(|flag: &&str| {"),
    mutant("a package's include directories are not made absolute", KEY,
           "                                    format!(\"-I{}\", directory.join(relative).display())",
           "                                    format!(\"-I{relative}\")"),
    mutant("sources in subdirectories are not embedded", BUILD,
           "        if path.is_dir() {\n            collect_binding_files(&path, files);",
           "        if path.is_dir() {"),
    mutant("a prebuilt archive is used whatever its key", NATIVE,
           ".find(|(name, prebuilt_key, _)| *name == package && *prebuilt_key == key)",
           ".find(|(name, _, _)| *name == package)"),
    mutant("prebuilt binding archives are never used", NATIVE,
           "    if let Some(archive) = prebuilt_binding_archive(package.name, &key) {",
           "    if let Some(archive) = prebuilt_binding_archive(package.name, \"\") {"),
]
