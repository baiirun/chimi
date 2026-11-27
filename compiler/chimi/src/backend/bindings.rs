//! Binding packages (G20): which ones a program reaches, and where each one's
//! code comes from. A C package's sources live under
//! `runtime/native/bindings/<package>/`, at any depth; `build.rs` embeds every
//! `.c` and `.h` file there, so the compiler ships them as it ships
//! `runtime.c`, and prebuilds each package's archive. A Rust package (one
//! with no C library under it) is compiled into the Rust runtime archive
//! every program links, and the link keeps only the functions a program
//! reaches.

use crate::operations::Binding;

include!(concat!(env!("OUT_DIR"), "/binding_sources.rs"));

/// `chimi_binding.h`, the only runtime header a binding may include.
pub(super) const BINDING_HEADER: &str = include_str!("../../runtime/native/chimi_binding.h");

/// The packages written in Rust, in `runtime/chimi-runtime/src/binding/`,
/// with their source, which the registry test reads.
pub(super) const RUST_PACKAGES: &[(&str, &str)] = &[
    (
        "exec",
        include_str!("../../../../runtime/chimi-runtime/src/binding/exec.rs"),
    ),
    (
        "net",
        include_str!("../../../../runtime/chimi-runtime/src/binding/net.rs"),
    ),
    (
        "terminal",
        include_str!("../../../../runtime/chimi-runtime/src/binding/terminal.rs"),
    ),
];

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
    pub(super) code: PackageCode,
    pub(super) system_libraries: &'static [&'static str],
}

/// Where a package's functions come from.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum PackageCode {
    /// `(path in the package, text)` of every source and header, in path
    /// order, compiled into the package's own archive.
    C(Vec<(&'static str, &'static str)>),
    /// Compiled into the Rust runtime archive; nothing to build.
    Rust,
}

impl BindingPackage {
    /// The `.c` files, which compile one object each.
    #[cfg(test)]
    pub(super) fn sources(&self) -> impl Iterator<Item = &'static str> + '_ {
        let files: &[(&str, &str)] = match &self.code {
            PackageCode::C(files) => files,
            PackageCode::Rust => &[],
        };
        files
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
/// Requires: every binding names a package with embedded sources or a Rust
/// package, which the registry test `every_binding_package_has_sources`
/// checks.
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
            code: if RUST_PACKAGES.iter().any(|(package, _)| *package == name) {
                PackageCode::Rust
            } else {
                PackageCode::C(
                    BINDING_SOURCES
                        .iter()
                        .filter(|(package, _, _)| *package == name)
                        .map(|(_, file, text)| (*file, *text))
                        .collect(),
                )
            },
            system_libraries: SYSTEM_LIBRARIES
                .iter()
                .find(|(package, _)| *package == name)
                .map_or(&[], |(_, libraries)| libraries),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BINDING_SOURCES, PackageCode, RUST_PACKAGES, packages_for};
    use crate::operations::OPERATIONS;

    #[test]
    fn every_binding_package_has_sources() {
        for binding in OPERATIONS.iter().filter_map(|operation| operation.binding) {
            let packages = packages_for(&[binding]);
            let defined = match &packages[0].code {
                PackageCode::C(files) => {
                    assert!(
                        packages[0].sources().next().is_some(),
                        "binding package {} has no C source",
                        binding.package
                    );
                    files
                        .iter()
                        .any(|(_, text)| text.contains(&format!("*{}(ChimiBindingCall", binding.symbol)))
                }
                PackageCode::Rust => RUST_PACKAGES
                    .iter()
                    .filter(|(package, _)| *package == binding.package)
                    .any(|(_, text)| {
                        text.contains(&format!(
                            "#[unsafe(no_mangle)]\npub unsafe extern \"C\" fn {}(",
                            binding.symbol
                        )) || (text.contains("macro_rules! binding")
                            && text.contains(&format!("\n    {} => ", binding.symbol)))
                    }),
            };
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
        for (package, _) in RUST_PACKAGES {
            assert!(
                BINDING_SOURCES.iter().all(|(c_package, _, _)| c_package != package),
                "package {package} has both C sources and Rust code"
            );
        }
    }

    /// The Rust view of the binding API is written by hand, so check each
    /// import against `chimi_binding.h`: same name, same number of
    /// parameters.
    #[test]
    fn the_rust_binding_api_matches_the_header() {
        const API: &str = include_str!("../../../../runtime/chimi-runtime/src/binding/api.rs");
        let header = super::BINDING_HEADER;
        let parameters = |text: &str, name: &str| {
            let start = text.find(&format!("{name}(")).map(|at| at + name.len() + 1)?;
            let end = start + text[start..].find(')')?;
            let list = text[start..end].trim();
            Some(if list == "void" {
                0
            } else {
                list.split(',').filter(|part| !part.trim().is_empty()).count()
            })
        };
        let imports = API.split("    fn ").skip(1).filter_map(|rest| {
            let name = rest.split('(').next()?;
            name.starts_with("chimi_binding_").then_some(name)
        });
        let mut checked = 0;
        for name in imports {
            assert_eq!(
                parameters(API, name),
                parameters(header, name),
                "{name}: the Rust import and chimi_binding.h disagree"
            );
            checked += 1;
        }
        assert!(checked > 0, "found no imports in api.rs");
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
            &[],
            &crate::backend::runtime_archive_key::CTools::host(),
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
            &[],
            &crate::backend::runtime_archive_key::CTools::host(),
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
