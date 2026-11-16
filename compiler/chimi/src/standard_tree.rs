//! The standard library as Chimi source, embedded in the compiler at build time.
//!
//! `build.rs` lists every file of `compiler/chimi/std/`; a build loads only the
//! standard modules it imports, directly or transitively. Each loaded module's
//! source path is `chimi-std:///std/<path>.chimi`, which diagnostics and editors
//! show, and only modules loaded from here may declare `native` items. A
//! compiler developer may read the tree from a directory instead (`--std-dir`,
//! [`crate::project::StandardTree`]); its modules take the same names.

use crate::compiler::ProjectSource;
use crate::semantic::{self, ImportBinding};
use crate::source::SourceSnapshot;
use std::collections::HashSet;
use std::convert::Infallible;

include!(concat!(env!("OUT_DIR"), "/standard_tree.rs"));

/// The scheme of an embedded standard module's source path.
pub(crate) const SOURCE_PREFIX: &str = "chimi-std:///";

/// The embedded text of a standard module, such as `std/list`.
pub(crate) fn embedded_module(module_path: &str) -> Option<&'static str> {
    EMBEDDED_MODULES
        .iter()
        .find(|(path, _)| *path == module_path)
        .map(|(_, text)| *text)
}

/// Every embedded module path, in ascending order.
pub(crate) fn embedded_module_paths() -> impl Iterator<Item = &'static str> {
    EMBEDDED_MODULES.iter().map(|(path, _)| *path)
}

/// The embedded module `module_path` as a parsed project source.
pub(crate) fn standard_source(module_path: &str) -> Option<ProjectSource> {
    embedded_module(module_path).map(|text| standard_module(module_path, text))
}

/// Standard module `module_path` with `text`, named `chimi-std:///<path>.chimi`.
///
/// Requires: `module_path` is a valid module path under `std/`.
pub(crate) fn standard_module(
    module_path: &str,
    text: impl Into<std::sync::Arc<str>>,
) -> ProjectSource {
    let source = SourceSnapshot::from_text(source_path(module_path), text);
    ProjectSource::parsed(module_path, source).expect("standard module paths are valid")
}

fn source_path(module_path: &str) -> String {
    format!("{SOURCE_PREFIX}{module_path}.chimi")
}

/// Whether `source` was loaded from the standard tree, embedded or read from a
/// `--std-dir` override: only such a module may declare `native` items or take
/// a runtime identity. The project loader names workspace files by their
/// canonical path on disk, and a single file may be named anything, so the
/// module path must also be under `std/`, which no workspace module may use
/// (`N3102`).
pub(crate) fn is_standard(module_path: &str, source: &SourceSnapshot) -> bool {
    module_path.starts_with("std/") && source.path().to_str() == Some(&source_path(module_path))
}

/// The alias a program conventionally imports `module_path` under, as
/// `std/list` is `List`; diagnostics name standard functions with it.
pub(crate) fn conventional_alias(module_path: &str) -> String {
    let name = module_path.rsplit('/').next().unwrap_or(module_path);
    let mut characters = name.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_ascii_uppercase().to_string() + characters.as_str()
    })
}

/// Adds every embedded module that `sources` import, directly or through
/// another standard module, and that is not already present.
///
/// Ensures: idempotent; workspace modules are untouched, so a workspace file
/// named `std/...` stays for resolution to reject (`N3102`).
pub(crate) fn add_imported_modules(sources: &mut Vec<ProjectSource>) {
    let Ok(()) =
        add_imported_modules_from(sources, |path| Ok::<_, Infallible>(standard_source(path)));
}

/// [`add_imported_modules`] with modules from `read`, which returns `None` for a
/// module the tree does not have; stops at `read`'s first error.
pub(crate) fn add_imported_modules_from<E>(
    sources: &mut Vec<ProjectSource>,
    mut read: impl FnMut(&str) -> Result<Option<ProjectSource>, E>,
) -> Result<(), E> {
    let mut present = sources
        .iter()
        .map(|source| source.module_path().to_owned())
        .collect::<HashSet<_>>();
    let mut pending = sources.iter().flat_map(imports).collect::<Vec<_>>();
    while let Some(path) = pending.pop() {
        if present.contains(&path) {
            continue;
        }
        let Some(module) = read(&path)? else {
            continue;
        };
        present.insert(path);
        pending.extend(imports(&module));
        sources.push(module);
    }
    Ok(())
}

fn imports(source: &ProjectSource) -> Vec<String> {
    let parsed;
    let syntax = match source.syntax() {
        Some(syntax) => syntax.as_ref(),
        None => {
            parsed = crate::syntax::parse(source.source());
            &parsed
        }
    };
    semantic::discover_imports(syntax)
        .into_iter()
        .filter(|import| !matches!(import.binding, ImportBinding::Missing))
        .map(|import| import.path)
        .filter(|path| path.starts_with("std/"))
        .flat_map(|path| {
            // The built-in `std/task` returns `Result` values (`Task.results`),
            // so it loads the source module that declares them.
            let implied = (path == "std/task").then(|| "std/result".to_owned());
            std::iter::once(path).chain(implied)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A single file the user names `chimi-std:///main.chimi` is not standard source.
    #[test]
    fn a_standard_looking_path_outside_std_is_not_standard() {
        let crafted = SourceSnapshot::from_text(source_path("main"), "");
        assert!(!is_standard("main", &crafted));
        let embedded = SourceSnapshot::from_text(source_path("std/list"), "");
        assert!(is_standard("std/list", &embedded));
    }
}
