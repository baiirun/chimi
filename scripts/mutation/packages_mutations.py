"""Mutants of packages (T5 PR 2, CHI-290): manifests, the package graph,
imports across packages, module origins, and the editor finding a file's
package. Each one reintroduces a boundary or check that packages.md requires.

Run: scripts/mutation/mutate.py scripts/mutation/packages_mutations.py
"""

CRATE = "compiler/chimi"

P = "src/project.rs"
G = "src/project/packages.rs"
M = "src/manifest.rs"
T = "src/manifest/toml.rs"
S = "src/semantic.rs"
C = "src/compiler.rs"


def unit(name, file, edits, filter="project::"):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=filter)


def commands(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["packages"], filter="")


MUTATIONS = [
    unit("any package of the build is importable", P, [(
        "        } else if graph.find_str(first).is_some() {\n            return Err(ProjectLoadError::NotADependency {",
        "        } else if let Some(package) = graph.find_str(first) {\n            package\n        } else if false {\n            return Err(ProjectLoadError::NotADependency {",
    )]),
    unit("internal modules are public", P, [(
        "if target.name != importing.name && rest.starts_with(\"internal/\")",
        "if false && rest.starts_with(\"internal/\")",
    )]),
    unit("a package cannot import its own internal modules", P, [(
        "if target.name != importing.name && rest.starts_with(\"internal/\")",
        "if rest.starts_with(\"internal/\")",
    )]),
    unit("a bare package name is a module", P, [(
        "        if rest.is_empty() {\n            return Err(edge_error(",
        "        if false {\n            return Err(edge_error(",
    )]),
    unit("a dependency's modules count as the entry's", P, [(
        "            origin: if target.name == entry.name {",
        "            origin: if true {",
    )]),
    unit("nested packages are read as part of the enclosing one", P, [(
        "        if self.packages.is_some()\n            && let Some(manifest) = nested_manifest(&root, &path)",
        "        if self.packages.is_none()\n            && let Some(manifest) = nested_manifest(&root, &path)",
    )]),
    unit("a package directory keeps nested packages' files", P, [(
        "                .filter(|path| nested_manifest(&root, &root.join(path)).is_none())",
        "                .filter(|_| true)",
    )]),
    unit("the entry module may be outside the entry package", P, [(
        "            if first != entry.name.as_str() || rest.is_empty() {",
        "            if rest.is_empty() {",
    )]),
    unit("path dependencies may leave the workspace", G, [(
        "                if !directory.starts_with(&workspace) {",
        "                if false {",
    )]),
    unit("two sources for one name are accepted", G, [(
        "                    if known.root != directory {",
        "                    if false {",
    )]),
    unit("package cycles are accepted", G, [(
        "        graph.reject_cycles()?;",
        "",
    )]),
    unit("a dependency's own name is not checked", G, [(
        "                if package.name != dependency.name {",
        "                if false {",
    )]),
    unit("an unlisted member is accepted", G, [(
        "        if !listed {",
        "        if false {",
    )]),
    unit("a workspace root is loaded as a package", G, [(
        "            Manifest::Workspace(_) => {\n                return Err(PackageError::RootIsWorkspace {\n                    manifest: manifest_path,\n                });\n            }",
        "            Manifest::Workspace(_) => return Ok(None),",
    )]),
    unit("unknown dependency keys are ignored", M, [(
        "        .find(|(key, _)| !matches!(key.as_str(), \"path\" | \"git\" | \"rev\" | \"directory\"))",
        "        .find(|_| false)",
    )], filter="manifest::"),
    unit("std is a package name", M, [(
        "            && text != \"std\";",
        ";",
    )], filter="manifest::"),
    unit("a git dependency needs no rev", M, [(
        "            let rev = field(\"rev\").ok_or_else(|| {",
        "            let rev = field(\"rev\").or(Some(String::new())).ok_or_else(|| {",
    )], filter="manifest::"),
    unit("duplicate keys are accepted", T, [(
        "                if table.entries.iter().any(|prior| prior.key == entry.key) {",
        "                if false {",
    )], filter="manifest::"),
    unit("text after a value is ignored", T, [(
        "            Some(_) => Err(self.error(\"expected the end of the line\")),",
        "            Some(_) => Ok(()),",
    )], filter="manifest::"),
    commands("a dependency's tests run", S, [(
        "    if origin == ModuleOrigin::Dependency {\n        module.declarations.retain(|declaration| !declaration.test);",
        "    if false {\n        module.declarations.retain(|declaration| !declaration.test);",
    )]),
    commands("lint covers dependencies", C, [(
        "            origin == ModuleOrigin::Own || (include_standard && origin == ModuleOrigin::Standard)",
        "            origin != ModuleOrigin::Standard || include_standard",
    )]),
]
