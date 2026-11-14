//! The standard library as Chimi source, embedded in the compiler at build time.
//!
//! `build.rs` lists every file of `compiler/chimi/std/`; a build loads only the
//! standard modules it imports, directly or transitively. Each loaded module's
//! source path is `chimi-std:///std/<path>.chimi`, which diagnostics and editors
//! show, and only modules loaded from here may declare `native` items.

use crate::compiler::ProjectSource;
use crate::semantic::{self, ImportBinding};
use crate::source::SourceSnapshot;
use std::collections::HashSet;

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
#[allow(dead_code)]
pub(crate) fn embedded_module_paths() -> impl Iterator<Item = &'static str> {
    EMBEDDED_MODULES.iter().map(|(path, _)| *path)
}

/// The embedded module `module_path` as a parsed project source.
pub(crate) fn standard_source(module_path: &str) -> Option<ProjectSource> {
    let text = embedded_module(module_path)?;
    let source =
        SourceSnapshot::from_text(format!("{SOURCE_PREFIX}{module_path}.chimi"), text);
    Some(
        ProjectSource::parsed(module_path, source)
            .expect("embedded standard module paths are valid"),
    )
}

/// Whether `source` is the embedded text of `module_path`, loaded from this tree:
/// only such a module may declare `native` items or take a runtime identity.
pub(crate) fn is_embedded(module_path: &str, source: &SourceSnapshot) -> bool {
    embedded_module(module_path).is_some_and(|text| {
        source.text() == text
            && source.path().to_str() == Some(&format!("{SOURCE_PREFIX}{module_path}.chimi"))
    })
}

/// Adds every embedded module that `sources` import, directly or through
/// another standard module, and that is not already present.
///
/// Ensures: idempotent; workspace modules are untouched, so a workspace file
/// named `std/...` stays for resolution to reject (`N3102`).
pub(crate) fn add_imported_modules(sources: &mut Vec<ProjectSource>) {
    let mut present = sources
        .iter()
        .map(|source| source.module_path().to_owned())
        .collect::<HashSet<_>>();
    let mut pending = sources.iter().flat_map(imports).collect::<Vec<_>>();
    while let Some(path) = pending.pop() {
        if present.contains(&path) {
            continue;
        }
        let Some(module) = standard_source(&path) else {
            continue;
        };
        present.insert(path);
        pending.extend(imports(&module));
        sources.push(module);
    }
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
        .collect()
}
