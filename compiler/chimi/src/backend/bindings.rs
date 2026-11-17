//! C binding packages (G20): which ones a program reaches, and how each is
//! compiled into its own archive. The sources live under
//! `runtime/native/bindings/<package>/`, at any depth; `build.rs` embeds every
//! `.c` and `.h` file there, so the compiler ships them as it ships
//! `runtime.c`, and prebuilds each package's archive.

use crate::operations::Binding;

include!(concat!(env!("OUT_DIR"), "/binding_sources.rs"));

/// `chimi_binding.h`, the only runtime header a binding may include.
pub(super) const BINDING_HEADER: &str = include_str!("../../runtime/native/chimi_binding.h");

/// Operating-system libraries a package links, which the build report lists
/// (Principle 15). Every package not named here links none.
const SYSTEM_LIBRARIES: &[(&str, &[&str])] = &[
    ("net", &["pthread"]),
    ("tls", &["pthread"]),
    ("sqlite", &["pthread", "m"]),
];

/// One binding package a program links.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct BindingPackage {
    pub(super) name: &'static str,
    /// `(path in the package, text)` of every source and header, in path order.
    pub(super) files: Vec<(&'static str, &'static str)>,
    pub(super) system_libraries: &'static [&'static str],
}

impl BindingPackage {
    /// The `.c` files, which compile one object each.
    #[cfg(test)]
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

    #[test]
    fn package_flags_are_only_relative_includes_and_definitions() {
        use crate::backend::runtime_archive_key::BINDING_PACKAGE_FLAGS;
        for (package, flags) in BINDING_PACKAGE_FLAGS {
            for flag in *flags {
                let include = flag.strip_prefix("-I").is_some_and(|path| {
                    !path.is_empty()
                        && !path.starts_with('/')
                        && path.split('/').all(|part| part != "..")
                });
                assert!(
                    include || (flag.starts_with("-D") && flag.len() > 2),
                    "package {package}: flag {flag:?} is neither -I<path inside the package> nor -D<definition>"
                );
                assert!(
                    !flag.contains(' '),
                    "package {package}: flag {flag:?} holds a space"
                );
            }
        }
    }

    #[test]
    fn two_sources_that_would_share_an_object_are_refused() {
        use crate::backend::runtime_archive_key::compile_binding_archive;
        let directory =
            std::env::temp_dir().join(format!("chimi-binding-collision-{}", std::process::id()));
        let refused = compile_binding_archive(
            "p",
            &[("a/b.c", ""), ("a__b.c", "")],
            &directory,
            &directory.join("package"),
            &directory.join("p.a"),
        );
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("two sources map to the object")),
            "{refused:?}"
        );
    }

    #[test]
    fn two_sources_that_differ_only_in_case_are_refused() {
        use crate::backend::runtime_archive_key::compile_binding_archive;
        let directory =
            std::env::temp_dir().join(format!("chimi-binding-case-{}", std::process::id()));
        let refused = compile_binding_archive(
            "p",
            &[("Include/x.h", ""), ("include/x.h", "")],
            &directory,
            &directory.join("package"),
            &directory.join("p.a"),
        );
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("differ only in case")),
            "{refused:?}"
        );
    }

    #[test]
    fn the_archive_key_changes_with_every_input() {
        use crate::backend::runtime_archive_key::{BindingArchiveInputs, binding_archive_key};
        let keyed = |package, flags: &[&str], header: &[u8], sources: &[(&str, &str)]| {
            binding_archive_key(&BindingArchiveInputs {
                package,
                package_flags: flags,
                target_triple: "t",
                compiler_identity: "cc",
                binding_header: header,
                sources,
            })
        };
        let key =
            |package, header: &[u8], sources: &[(&str, &str)]| keyed(package, &[], header, sources);
        let base = key("p", b"h", &[("a.c", "x")]);
        assert_ne!(
            keyed("p", &["-DA", "-DB"], b"h", &[]),
            keyed("p", &["-DA -DB"], b"h", &[])
        );
        assert_ne!(
            keyed("p", &["-DA"], b"h", &[]),
            keyed("p", &["-DB"], b"h", &[])
        );
        assert_eq!(base, key("p", b"h", &[("a.c", "x")]));
        for other in [
            key("q", b"h", &[("a.c", "x")]),
            key("p", b"g", &[("a.c", "x")]),
            key("p", b"h", &[("b.c", "x")]),
            key("p", b"h", &[("a.c", "y")]),
            key("p", b"h", &[("a.c", "x"), ("b.c", "")]),
            keyed("p", &["-Iinclude"], b"h", &[("a.c", "x")]),
            keyed("p", &["-I", "include"], b"h", &[("a.c", "x")]),
            keyed("p", &["-DA -DB"], b"h", &[("a.c", "x")]),
        ] {
            assert_ne!(base, other);
        }
    }
}
