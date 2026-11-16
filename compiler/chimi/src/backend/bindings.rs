//! C binding packages (G20): which ones a program reaches, and how each is
//! compiled into its own archive. The sources live under
//! `runtime/native/bindings/<package>/`; `build.rs` embeds every `.c` and `.h`
//! file there, so the compiler ships them as it ships `runtime.c`.

use crate::operations::Binding;

include!(concat!(env!("OUT_DIR"), "/binding_sources.rs"));

/// `chimi_binding.h`, the only runtime header a binding may include.
pub(super) const BINDING_HEADER: &str = include_str!("../../runtime/native/chimi_binding.h");

/// Operating-system libraries a package links, which the build report lists
/// (Principle 15). Every package not named here links none.
const SYSTEM_LIBRARIES: &[(&str, &[&str])] = &[];

/// One binding package a program links.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct BindingPackage {
    pub(super) name: &'static str,
    /// `(file name, text)` of every source and header, in name order.
    pub(super) files: Vec<(&'static str, &'static str)>,
    pub(super) system_libraries: &'static [&'static str],
}

impl BindingPackage {
    /// The `.c` files, which compile one object each.
    pub(super) fn sources(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.files
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| name.ends_with(".c"))
    }

    pub(super) fn archive_name(&self) -> String {
        format!("libchimi_bind_{}.a", self.name)
    }
}

/// The packages that define `bindings`, each once, in name order.
///
/// Requires: every binding names a package with embedded sources, which the
/// registry test `every_binding_package_has_sources` checks.
pub(super) fn packages_for(bindings: &[Binding]) -> Vec<BindingPackage> {
    let mut names = bindings
        .iter()
        .map(|binding| binding.package)
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| BindingPackage {
            name,
            files: BINDING_SOURCES
                .iter()
                .filter(|(package, _, _)| *package == name)
                .map(|(_, file, text)| (*file, *text))
                .collect(),
            system_libraries: SYSTEM_LIBRARIES
                .iter()
                .find(|(package, _)| *package == name)
                .map_or(&[], |(_, libraries)| libraries),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BINDING_SOURCES, packages_for};
    use crate::operations::OPERATIONS;

    #[test]
    fn every_binding_package_has_sources() {
        for binding in OPERATIONS.iter().filter_map(|operation| operation.binding) {
            let packages = packages_for(&[binding]);
            assert!(
                packages[0].sources().next().is_some(),
                "binding package {} has no C source",
                binding.package
            );
            let defined = packages[0]
                .files
                .iter()
                .any(|(_, text)| text.contains(&format!("*{}(ChimiBindingCall", binding.symbol)));
            assert!(
                defined,
                "{} is not defined in package {}",
                binding.symbol, binding.package
            );
        }
        assert!(
            BINDING_SOURCES
                .iter()
                .all(|(_, _, text)| !text.contains("runtime.h"))
        );
    }
}
